//! The crossterm driver: raw mode on the controlling terminal, an alternate
//! screen on stderr, and bracketed paste, all restored on every exit path,
//! including Ctrl-C (a key in raw mode) and panics.
use super::{Input, flow::Effect, screen::Installer};
use crate::installer::{Answers, Plan, PlanFailure, Settings};
use crossterm::{
    cursor::Show,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Stderr, stderr},
    panic::PanicHookInfo,
    sync::Arc,
};

type Hook = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

fn restore() {
    let _ = execute!(stderr(), DisableBracketedPaste, LeaveAlternateScreen, Show);
    let _ = terminal::disable_raw_mode();
}

/// An active terminal session; dropping it restores the terminal and the
/// previous panic hook.
pub struct Session {
    terminal: Terminal<CrosstermBackend<Stderr>>,
    previous_hook: Option<Hook>,
}

impl Session {
    /// Take over the terminal. Fails, leaving it unchanged, when it cannot
    /// be drawn on; callers then fall back to line mode.
    pub fn start() -> io::Result<Self> {
        let (columns, rows) = terminal::size()?;
        if columns == 0 || rows == 0 {
            return Err(io::Error::other("the terminal reports no size"));
        }
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(stderr(), EnterAlternateScreen, EnableBracketedPaste) {
            restore();
            return Err(error);
        }
        let previous: Hook = Arc::from(std::panic::take_hook());
        let chained = Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |info| {
            restore();
            chained(info);
        }));
        let session = Session {
            terminal: match Terminal::new(CrosstermBackend::new(stderr())) {
                Ok(terminal) => terminal,
                Err(error) => {
                    restore();
                    return Err(error);
                }
            },
            previous_hook: Some(previous),
        };
        Ok(session)
    }

    /// Run the UI until it confirms, cancels, interrupts, or fails.
    pub fn run(
        &mut self,
        installer: &mut Installer,
        mut plan: impl FnMut(&Settings, &Answers) -> Result<Plan, PlanFailure>,
    ) -> io::Result<Effect> {
        loop {
            self.terminal.draw(|frame| installer.render(frame))?;
            let input = match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => Input::Key(key),
                Event::Paste(text) => Input::Paste(text),
                _ => continue,
            };
            let mut effect = installer.update(input);
            while effect == Effect::Plan {
                self.terminal.draw(|frame| installer.render(frame))?;
                effect = installer.planned(plan(installer.settings(), installer.answers()));
            }
            if effect != Effect::None {
                return Ok(effect);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        restore();
        if let Some(previous) = self.previous_hook.take() {
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}
