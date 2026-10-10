//! The inline crossterm driver. Raw mode and bracketed paste stay enabled
//! throughout the dialog and are restored on every exit, including panics.
//! Widgets are cleared before their transcript is printed; no alternate screen
//! is used and the existing scrollback stays above the dialog.
use super::{Input, flow::Effect, screen::Installer};
use crate::installer::{Answers, Plan, PlanFailure, Settings, Style};
use crossterm::{
    cursor::{MoveTo, MoveToColumn, Show},
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{self, Clear, ClearType},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::{Backend, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};
use std::{
    io::{self, Stderr, Write, stderr},
    panic::PanicHookInfo,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

type Hook = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;
const NO_VIEWPORT: u32 = u32::MAX;

fn cursor_position() -> io::Result<(u16, u16)> {
    struct RestoreStdout(libc::c_int);

    impl RestoreStdout {
        fn restore(&mut self) -> io::Result<()> {
            if self.0 < 0 {
                return Ok(());
            }
            // SAFETY: the guard owns this duplicate of stdout; fd 1 is the destination.
            if unsafe { libc::dup2(self.0, libc::STDOUT_FILENO) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let saved = std::mem::replace(&mut self.0, -1);
            // SAFETY: this descriptor is owned by the guard and is closed exactly once.
            if unsafe { libc::close(saved) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for RestoreStdout {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }

    let mut stdout = io::stdout().lock();
    stdout.flush()?;
    // SAFETY: dup takes an fd number and returns a new descriptor owned by the guard.
    let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut restore = RestoreStdout(saved);
    // crossterm writes the query to stdout, which may be the JSON file.
    // SAFETY: dup2 duplicates stderr onto stdout, retaining the original in the guard.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        let error = io::Error::last_os_error();
        restore.restore()?;
        return Err(error);
    }
    let position = crossterm::cursor::position();
    restore.restore()?;
    position
}

struct InlineBackend(CrosstermBackend<Stderr>);

impl Backend for InlineBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        cursor_position().map(|(x, y)| Position { x, y })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.0)
    }
}

fn restore(origin: &AtomicU32) {
    let row = origin.swap(NO_VIEWPORT, Ordering::Relaxed);
    if row != NO_VIEWPORT {
        let _ = execute!(
            stderr(),
            MoveTo(0, row as u16),
            Clear(ClearType::FromCursorDown)
        );
    }
    let _ = execute!(stderr(), MoveToColumn(0), DisableBracketedPaste, Show);
    let _ = terminal::disable_raw_mode();
}

/// An active terminal session; dropping it restores the terminal and the
/// previous panic hook.
pub struct Session {
    terminal: Option<Terminal<InlineBackend>>,
    size: (u16, u16),
    serial: u64,
    origin: Arc<AtomicU32>,
    previous_hook: Option<Hook>,
}

impl Session {
    /// Enable inline drawing. Fails, restoring terminal modes, when the
    /// terminal cannot be queried; callers then fall back to line mode.
    pub fn start() -> io::Result<Self> {
        let size = terminal::size()?;
        if size.0 == 0 || size.1 < 2 {
            return Err(io::Error::other(
                "the terminal reports no room for a widget",
            ));
        }
        let origin = Arc::new(AtomicU32::new(NO_VIEWPORT));
        terminal::enable_raw_mode()?;
        if let Err(error) =
            execute!(stderr(), EnableBracketedPaste).and_then(|()| cursor_position().map(|_| ()))
        {
            restore(&origin);
            return Err(error);
        }
        let previous: Hook = Arc::from(std::panic::take_hook());
        let chained = Arc::clone(&previous);
        let cleanup_origin = Arc::clone(&origin);
        std::panic::set_hook(Box::new(move |info| {
            restore(&cleanup_origin);
            chained(info);
        }));
        Ok(Self {
            terminal: None,
            size,
            serial: u64::MAX,
            origin,
            previous_hook: Some(previous),
        })
    }

    /// Remove the active widget, returning the cursor to its first column
    /// and row. Cleared rows are reused by the transcript or next widget.
    fn clear_viewport(&mut self) -> io::Result<()> {
        if let Some(terminal) = self.terminal.take() {
            // Drop before printing so its cursor restoration cannot affect output.
            drop(terminal);
            let row = self.origin.load(Ordering::Relaxed);
            if row != NO_VIEWPORT {
                execute!(
                    stderr(),
                    MoveTo(0, row as u16),
                    Clear(ClearType::FromCursorDown)
                )?;
                self.origin.store(NO_VIEWPORT, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn print_pending(&mut self, installer: &mut Installer) -> io::Result<()> {
        let style = if installer.color() {
            Style::COLOR
        } else {
            Style::PLAIN
        };
        let text = installer.take_transcript(style);
        if !text.is_empty() {
            self.clear_viewport()?;
            let mut out = stderr().lock();
            // Raw mode does not translate LF to CRLF.
            out.write_all(text.replace('\n', "\r\n").as_bytes())?;
            if !text.ends_with('\n') {
                out.write_all(b"\r\n")?;
            }
            out.flush()?;
        }
        Ok(())
    }

    fn present(&mut self, installer: &mut Installer) -> io::Result<()> {
        self.print_pending(installer)?;
        let size = terminal::size()?;
        let serial = installer.flow().serial();
        let height = installer.height(size.0).min(size.1.saturating_sub(1));
        let previous_height = self.terminal.as_mut().map(|t| t.get_frame().area().height);
        if self.size != size || self.serial != serial || previous_height != Some(height) {
            self.clear_viewport()?;
        }
        self.size = size;
        self.serial = serial;
        if height == 0 {
            return Ok(());
        }
        if self.terminal.is_none() {
            let mut terminal = Terminal::with_options(
                InlineBackend(CrosstermBackend::new(stderr())),
                TerminalOptions {
                    viewport: Viewport::Inline(height),
                },
            )?;
            self.origin
                .store(u32::from(terminal.get_frame().area().y), Ordering::Relaxed);
            self.terminal = Some(terminal);
        }
        if let Some(terminal) = &mut self.terminal {
            let frame = terminal.draw(|frame| installer.render(frame))?;
            self.origin
                .store(u32::from(frame.area.y), Ordering::Relaxed);
        }
        Ok(())
    }

    /// Run the UI until it confirms, cancels, interrupts, or fails.
    pub fn run(
        &mut self,
        installer: &mut Installer,
        mut plan: impl FnMut(&Settings, &Answers) -> Result<Plan, PlanFailure>,
    ) -> io::Result<Effect> {
        loop {
            self.present(installer)?;
            let input = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => Input::Key(key),
                Event::Paste(text) => Input::Paste(text),
                _ => continue,
            };
            let mut effect = installer.update(input);
            while effect == Effect::Plan {
                self.present(installer)?;
                effect = installer.planned(plan(installer.settings(), installer.answers()));
            }
            if effect != Effect::None {
                self.print_pending(installer)?;
                self.clear_viewport()?;
                return Ok(effect);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The panic hook may already have cleared the viewport and printed
        // panic details below it; do not clear that output during unwinding.
        if !std::thread::panicking() {
            let _ = self.clear_viewport();
        }
        self.terminal.take();
        restore(&self.origin);
        if let Some(previous) = self.previous_hook.take()
            && !std::thread::panicking()
        {
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}
