//! Responsive prompts with an optional examples pane; all rendering uses stderr.
use anyhow::Result;
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::BTreeMap,
    io::{Stderr, stderr},
};
use tui_input::{Input, InputRequest, backend::crossterm::EventHandler};

#[derive(Debug)]
pub(super) struct Canceled;
impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("setup canceled")
    }
}
impl std::error::Error for Canceled {}

#[derive(Debug)]
pub(super) struct Back;
impl std::fmt::Display for Back {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("previous screen")
    }
}
impl std::error::Error for Back {}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Navigation {
    #[default]
    Forward,
    Topics,
    Setting,
    Review,
    Dialog,
}

impl Navigation {
    fn has_previous(self) -> bool {
        matches!(self, Self::Topics | Self::Setting | Self::Review)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Focus {
    #[default]
    Content,
    Previous,
    Next,
}

#[derive(PartialEq, Eq, Debug)]
enum ButtonAction {
    Previous,
    Next,
    Consumed,
}

impl Focus {
    fn handle(&mut self, key: KeyEvent, navigation: Navigation) -> Option<ButtonAction> {
        if navigation == Navigation::Dialog {
            return None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let order = if navigation.has_previous() {
                    &[Self::Content, Self::Previous, Self::Next][..]
                } else {
                    &[Self::Content, Self::Next][..]
                };
                let index = order.iter().position(|item| item == self).unwrap_or(0);
                let reverse =
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                *self = order[(index + if reverse { order.len() - 1 } else { 1 }) % order.len()];
                Some(ButtonAction::Consumed)
            }
            KeyCode::Enter | KeyCode::Char(' ') if *self != Self::Content => {
                Some(if *self == Self::Previous {
                    ButtonAction::Previous
                } else {
                    ButtonAction::Next
                })
            }
            KeyCode::Left if *self != Self::Content => {
                if navigation.has_previous() {
                    *self = Self::Previous;
                }
                Some(ButtonAction::Consumed)
            }
            KeyCode::Right if *self != Self::Content => {
                *self = Self::Next;
                Some(ButtonAction::Consumed)
            }
            KeyCode::Up | KeyCode::Down if *self != Self::Content => {
                *self = Self::Content;
                Some(ButtonAction::Consumed)
            }
            _ => None,
        }
    }
}

#[derive(Clone)]
struct MenuState {
    selected: Vec<bool>,
    cursor: usize,
}

#[derive(Clone, Copy)]
enum Controls {
    Menu {
        multi: bool,
        scroll: u16,
        navigation: Navigation,
    },
    Text,
}

#[derive(Clone)]
struct TextField {
    input: Input,
    replace_on_input: bool,
}

impl TextField {
    fn new(value: &str) -> Self {
        Self {
            input: Input::new(value.to_owned()),
            replace_on_input: !value.is_empty(),
        }
    }

    fn handle(&mut self, event: &Event) {
        match event {
            Event::Paste(text) => {
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                if !text.is_empty() {
                    self.clear_default();
                    for c in text.chars() {
                        self.input.handle(InputRequest::InsertChar(c));
                    }
                }
            }
            Event::Key(key) => {
                let typing = matches!(key.code, KeyCode::Char(_))
                    && !key.modifiers.contains(KeyModifiers::CONTROL);
                let deleting = matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
                    || matches!(key.code, KeyCode::Char('u' | 'w' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL));
                if typing || deleting {
                    self.clear_default();
                }
                if matches!(
                    key.code,
                    KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                ) || matches!(key.code, KeyCode::Char('a' | 'b' | 'e' | 'f') if key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    self.replace_on_input = false;
                }
                self.input.handle_event(event);
            }
            _ => {}
        }
    }

    fn clear_default(&mut self) {
        if self.replace_on_input {
            self.input = Input::default();
            self.replace_on_input = false;
        }
    }
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = execute!(stderr(), DisableBracketedPaste, LeaveAlternateScreen, Show);
        let _ = terminal::disable_raw_mode();
    }
}

pub(super) struct Ui {
    terminal: Terminal<CrosstermBackend<Stderr>>,
    _restore: Restore,
    pub context: String,
    color: bool,
    confirming_cancel: bool,
    pub navigation: Navigation,
    menus: BTreeMap<String, MenuState>,
    fields: BTreeMap<String, TextField>,
}

impl Ui {
    pub fn new() -> Result<Self> {
        let restore = Restore;
        terminal::enable_raw_mode()?;
        execute!(stderr(), EnterAlternateScreen, EnableBracketedPaste)?;
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stderr()))?,
            _restore: restore,
            context: "Choose project settings or user defaults for all projects.".into(),
            color: std::env::var_os("NO_COLOR").is_none_or(|s| s.is_empty()),
            confirming_cancel: false,
            navigation: Navigation::Forward,
            menus: BTreeMap::new(),
            fields: BTreeMap::new(),
        })
    }

    pub fn select(
        &mut self,
        label: &str,
        options: &[&str],
        default: usize,
        help: &str,
        example: Option<&str>,
    ) -> Result<usize> {
        Ok(self.menu(label, options, &[default], false, help, example)?[0])
    }

    pub fn multi(
        &mut self,
        label: &str,
        options: &[&str],
        defaults: &[usize],
        help: &str,
        example: Option<&str>,
    ) -> Result<Vec<usize>> {
        self.menu(label, options, defaults, true, help, example)
    }

    fn menu(
        &mut self,
        label: &str,
        options: &[&str],
        defaults: &[usize],
        multi: bool,
        help: &str,
        example: Option<&str>,
    ) -> Result<Vec<usize>> {
        let cache_key = format!("{}\0{label}", self.context);
        let state = if matches!(self.navigation, Navigation::Setting | Navigation::Topics)
            && !self.confirming_cancel
        {
            self.menus.get(&cache_key).cloned()
        } else {
            None
        };
        let mut state = state.unwrap_or_else(|| MenuState {
            selected: (0..options.len()).map(|i| defaults.contains(&i)).collect(),
            cursor: if multi { 0 } else { defaults[0] },
        });
        let mut help_scroll = 0u16;
        let mut focus = Focus::Content;
        let navigation = if self.confirming_cancel {
            Navigation::Dialog
        } else {
            self.navigation
        };
        loop {
            self.terminal.draw(|frame| {
                let area = page(
                    frame,
                    &self.context,
                    label,
                    help,
                    example,
                    self.color,
                    Controls::Menu {
                        multi,
                        scroll: help_scroll,
                        navigation: if self.confirming_cancel {
                            Navigation::Dialog
                        } else {
                            self.navigation
                        },
                    },
                );
                let area = buttons(frame, area, navigation, focus, self.color);
                let items = options.iter().enumerate().map(|(i, label)| {
                    ListItem::new(if multi {
                        format!("[{}] {label}", if state.selected[i] { 'x' } else { ' ' })
                    } else {
                        (*label).to_owned()
                    })
                });
                frame.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("> ")
                        .highlight_style(if focus == Focus::Content {
                            highlight(self.color)
                        } else {
                            Style::default().add_modifier(Modifier::BOLD)
                        })
                        .scroll_padding(1),
                    area,
                    &mut ListState::default().with_selected(Some(state.cursor)),
                );
            })?;
            let event = self.read_event()?;
            let Event::Key(key) = event else { continue };
            match focus.handle(key, navigation) {
                Some(ButtonAction::Consumed) => continue,
                Some(ButtonAction::Previous) => {
                    if matches!(self.navigation, Navigation::Setting | Navigation::Topics) {
                        self.menus.insert(cache_key, state);
                    }
                    return Err(Back.into());
                }
                Some(ButtonAction::Next) => return Ok(self.menu_answer(label, state, multi)),
                None => {}
            }
            if focus != Focus::Content {
                continue;
            }
            match key.code {
                KeyCode::Up => {
                    state.cursor = if state.cursor == 0 {
                        options.len() - 1
                    } else {
                        state.cursor - 1
                    }
                }
                KeyCode::Down => state.cursor = (state.cursor + 1) % options.len(),
                KeyCode::Char(' ') if multi => {
                    state.selected[state.cursor] = !state.selected[state.cursor]
                }
                KeyCode::Char('a') if multi && key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.selected.fill(true)
                }
                KeyCode::Char('d') if multi && key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.selected.fill(false)
                }
                KeyCode::PageUp => help_scroll = help_scroll.saturating_sub(5),
                KeyCode::PageDown => {
                    help_scroll = help_scroll
                        .saturating_add(5)
                        .min(help.chars().count().min(u16::MAX as usize) as u16)
                }
                KeyCode::Enter => return Ok(self.menu_answer(label, state, multi)),
                KeyCode::Esc if self.confirming_cancel => return Ok(vec![0]),
                KeyCode::Char(c)
                    if !c.is_whitespace() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if let Some(index) = options.iter().position(|option| {
                        option
                            .to_lowercase()
                            .starts_with(&c.to_lowercase().to_string())
                    }) {
                        state.cursor = index;
                    }
                }
                _ => {}
            }
        }
    }

    fn menu_answer(&mut self, label: &str, state: MenuState, multi: bool) -> Vec<usize> {
        let answer = if multi {
            (0..state.selected.len())
                .filter(|&i| state.selected[i])
                .collect()
        } else {
            vec![state.cursor]
        };
        if matches!(self.navigation, Navigation::Setting | Navigation::Topics)
            && !self.confirming_cancel
        {
            self.menus
                .insert(format!("{}\0{label}", self.context), state);
        }
        answer
    }

    fn read_event(&mut self) -> Result<Event> {
        let event = next_event()?;
        if matches!(&event, Event::Key(key) if key.code == KeyCode::Esc) && !self.confirming_cancel
        {
            self.confirming_cancel = true;
            let answer = self.select(
                "Cancel setup?",
                &["Keep editing", "Discard changes and exit"],
                0,
                "Your configuration has not been saved. Discard the draft and exit?",
                None,
            );
            self.confirming_cancel = false;
            if answer? == 1 {
                return Err(Canceled.into());
            }
            return Ok(Event::FocusGained);
        }
        Ok(event)
    }

    pub fn text(
        &mut self,
        label: &str,
        initial: &str,
        default: Option<&str>,
        help: &str,
        example: Option<&str>,
        validate: impl Fn(&str) -> Result<()>,
    ) -> Result<String> {
        let cache_key = format!("{}\0{label}", self.context);
        let mut field = self.fields.get(&cache_key).cloned().unwrap_or_else(|| {
            TextField::new(if initial.is_empty() {
                default.unwrap_or("")
            } else {
                initial
            })
        });
        field.replace_on_input = !field.input.value().is_empty();
        let mut error = String::new();
        let mut focus = Focus::Content;
        loop {
            self.terminal.draw(|frame| {
                let area = page(
                    frame,
                    &self.context,
                    label,
                    help,
                    example,
                    self.color,
                    Controls::Text,
                );
                let area = buttons(frame, area, self.navigation, focus, self.color);
                let [input_area, message] =
                    Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
                let inner = Block::bordered().inner(input_area);
                let scroll = field
                    .input
                    .visual_scroll(inner.width.saturating_sub(1).max(1) as usize);
                frame.render_widget(
                    Paragraph::new(field.input.value())
                        .style(if field.replace_on_input && focus == Focus::Content {
                            Style::default().add_modifier(Modifier::REVERSED)
                        } else {
                            Style::default()
                        })
                        .scroll((0, scroll as u16))
                        .block(Block::bordered()),
                    input_area,
                );
                frame.render_widget(
                    Paragraph::new(error.as_str()).wrap(Wrap { trim: false }),
                    message,
                );
                if focus == Focus::Content && inner.width > 0 && inner.height > 0 {
                    let x = inner.x
                        + field
                            .input
                            .visual_cursor()
                            .saturating_sub(scroll)
                            .min(inner.width.saturating_sub(1) as usize)
                            as u16;
                    frame.set_cursor_position((x, inner.y));
                }
            })?;
            let event = self.read_event()?;
            let action = if let Event::Key(key) = &event {
                focus.handle(*key, self.navigation)
            } else {
                None
            };
            match action {
                Some(ButtonAction::Consumed) => continue,
                Some(ButtonAction::Previous) => {
                    self.fields.insert(cache_key, field);
                    return Err(Back.into());
                }
                Some(ButtonAction::Next) => {}
                None if focus != Focus::Content => continue,
                None => {}
            }
            if action == Some(ButtonAction::Next)
                || matches!(&event, Event::Key(key) if key.code == KeyCode::Enter)
            {
                let value = field.input.value();
                match validate(value) {
                    Ok(()) => {
                        let value = value.to_owned();
                        self.fields.insert(cache_key, field);
                        return Ok(value);
                    }
                    Err(problem) => {
                        error = problem.to_string();
                        focus = Focus::Content;
                    }
                }
            } else {
                field.handle(&event);
            }
        }
    }
}

fn next_event() -> Result<Event> {
    loop {
        let event = event::read()?;
        if let Event::Key(key) = &event {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Err(Canceled.into());
            }
        }
        return Ok(event);
    }
}

fn highlight(color: bool) -> Style {
    let style = Style::default().add_modifier(Modifier::REVERSED);
    if color { style.fg(Color::Cyan) } else { style }
}

fn buttons(
    frame: &mut Frame,
    area: Rect,
    navigation: Navigation,
    focus: Focus,
    color: bool,
) -> Rect {
    if navigation == Navigation::Dialog {
        return area;
    }
    let [content, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);
    let [previous, next, _] = Layout::horizontal([
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Min(0),
    ])
    .spacing(2)
    .areas(bar);
    for (area, name, button, enabled) in [
        (
            previous,
            "Previous",
            Focus::Previous,
            navigation.has_previous(),
        ),
        (
            next,
            if navigation == Navigation::Review {
                "Confirm"
            } else {
                "Next"
            },
            Focus::Next,
            true,
        ),
    ] {
        let focused = focus == button && enabled;
        let label = if focused {
            format!("> {name} <")
        } else {
            name.to_owned()
        };
        let style = if !enabled {
            Style::default().add_modifier(Modifier::DIM)
        } else if focused {
            highlight(color)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(style)
                .block(Block::bordered()),
            area,
        );
    }
    content
}

fn columns(area: Rect, examples: bool) -> (Rect, Option<Rect>) {
    if examples && area.width >= 120 && area.height >= 18 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
                .spacing(2)
                .areas(area);
        (left, Some(right))
    } else {
        (area, None)
    }
}

fn page(
    frame: &mut Frame,
    context: &str,
    label: &str,
    help: &str,
    example: Option<&str>,
    color: bool,
    controls: Controls,
) -> Rect {
    let [title, context_area, content, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .margin(1)
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new("leanck configuration setup")
            .style(Style::default().add_modifier(Modifier::BOLD)),
        title,
    );
    frame.render_widget(
        Paragraph::new(context).wrap(Wrap { trim: false }),
        context_area,
    );
    let (left, right) = columns(content, example.is_some());
    if let (Some(area), Some(text)) = (right, example) {
        frame.render_widget(
            Paragraph::new(example_text(text, color))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" Examples ")),
            area,
        );
    }
    let help_rows = help
        .lines()
        .map(|line| (line.chars().count().max(1) as u16).div_ceil(left.width.max(1)))
        .sum::<u16>();
    let [question, hint, body] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(help_rows.min(left.height.saturating_sub(9)).max(2)),
        Constraint::Min(1),
    ])
    .areas(left);
    frame.render_widget(
        Paragraph::new(label)
            .wrap(Wrap { trim: false })
            .style(if color {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            })
            .add_modifier(Modifier::BOLD),
        question,
    );
    frame.render_widget(
        Paragraph::new(help).wrap(Wrap { trim: false }).scroll((
            match controls {
                Controls::Menu { scroll, .. } => scroll,
                Controls::Text => 0,
            },
            0,
        )),
        hint,
    );
    let keys = match controls {
        Controls::Menu {
            navigation: Navigation::Dialog,
            ..
        } => "Up/Down: choose  Enter: confirm  Esc: keep editing  Ctrl-C: exit",
        Controls::Menu { multi: true, .. } => {
            "Up/Down: choose  Space: toggle  Tab/Shift-Tab: focus buttons\nCtrl-A: select all  Ctrl-D: clear selections  Enter: continue  Esc: cancel"
        }
        Controls::Menu {
            navigation: Navigation::Review,
            ..
        } => {
            "Up/Down: choose  Tab/Shift-Tab: focus buttons  Enter: confirm\nPgUp/PgDn: scroll review  Esc: cancel  Ctrl-C: exit"
        }
        Controls::Menu {
            navigation: Navigation::Forward,
            ..
        } => "Up/Down: choose  Tab: focus Next  Enter: continue  Esc: cancel",
        Controls::Menu { .. } => {
            "Up/Down: choose  Tab/Shift-Tab: focus buttons  Enter: continue\nEsc: cancel  Ctrl-C: exit"
        }
        Controls::Text => {
            "Left/Right: move cursor  Tab/Shift-Tab: focus buttons  Enter: next\nType to replace; Home/End: jump  Ctrl-U: clear  Esc: cancel"
        }
    };
    frame.render_widget(Paragraph::new(keys).wrap(Wrap { trim: false }), footer);
    body
}

fn example_text(text: &str, color: bool) -> Text<'_> {
    let mut language = None;
    let mut block_comment = false;
    let lines = text
        .lines()
        .filter_map(|line| {
            match line {
                "```lean" => {
                    language = Some("lean");
                    return None;
                }
                "```text" => {
                    language = Some("text");
                    return None;
                }
                "```" => {
                    language = None;
                    block_comment = false;
                    return None;
                }
                _ => {}
            }
            Some(match language {
                Some("lean") => lean_line(line, color, &mut block_comment),
                Some("text") => Line::raw(line),
                _ => Line::styled(
                    line,
                    if color {
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM)
                    } else {
                        Style::default()
                    },
                ),
            })
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

/// Just the Lean tokens used in our hand-written examples, with no parser dependency.
fn lean_line<'a>(line: &'a str, color: bool, block_comment: &mut bool) -> Line<'a> {
    if !color {
        return Line::raw(line);
    }
    if *block_comment || line.trim_start().starts_with("/-") {
        *block_comment = !line.contains("-/");
        return Line::styled(line, Style::default().fg(Color::Green));
    }
    let mut spans = Vec::new();
    let mut offset = 0;
    while offset < line.len() {
        let rest = &line[offset..];
        let first = rest.chars().next().unwrap();
        let length = if first.is_alphanumeric() || first == '_' {
            rest.char_indices()
                .find(|(_, c)| !c.is_alphanumeric() && !matches!(c, '_' | '.'))
                .map_or(rest.len(), |(i, _)| i)
        } else {
            first.len_utf8()
        };
        let token = &rest[..length];
        let foreground = match token {
            "import" | "theorem" | "def" | "by" => Color::LightMagenta,
            "True" | "False" | "Nat" => Color::Yellow,
            "trivial" => Color::LightBlue,
            "simp" | "@" | "[" | "]" => Color::LightYellow,
            ":" | "=" => Color::Cyan,
            token if token.starts_with("Mathlib.") || token == "demo" => Color::LightCyan,
            _ => Color::Reset,
        };
        spans.push(Span::styled(token, Style::default().fg(foreground)));
        offset += length;
    }
    Line::from(spans)
}

pub(super) fn examples(key: &str) -> Option<&'static str> {
    Some(match key {
        "normalize.width" => {
            "Comment reflow (illustration)\n\nBefore\n```lean\n/-- A long comment about a proof that is easier to read when wrapped. -/\n```\n\nWith a narrower width\n```lean\n/-- A long comment about a proof\nthat is easier to read when wrapped. -/\n```\n\nLean code and inline math stay intact."
        }
        "normalize.spelling" => {
            "American English\n```lean\n/-- The color of the center. -/\n```\n\nBritish English\n```lean\n/-- The colour of the centre. -/\n```\n\nOff\nKeep the original spelling.\n\nCode and identifiers stay intact."
        }
        "normalize.header" => {
            "Generated header (illustration)\n\n```lean\n/-\nCopyright (c) 2026 Example Author.\nAuthors: Example Author\n-/\n```\n\nAttribution comes from the file's Git history. Unavailable history leaves a missing header absent."
        }
        "normalize.rules.frontmatter.license_text" => {
            "Example license text\n\n```text\nReleased under Apache 2.0 license as described in the file LICENSE.\n```\n\nThe {license} placeholder inserts this text in generated headers. The built-in default is empty. Existing headers are retained unless replacement is configured."
        }
        "normalize.rules.frontmatter.header_template" => {
            "Template (line breaks shown as \\n in the field)\n```lean\n/-\n{copyright}\n{license}\nAuthors: {authors}\n-/\n```\n\nExample with license text configured\n```lean\n/-\nCopyright (c) 2026 Example Author.\nReleased under Apache 2.0 license as described in the file LICENSE.\nAuthors: Example Author\n-/\n```\n\nNames and years come from Git history. Empty placeholder lines disappear."
        }
        "normalize.sort_imports" => {
            "Before\n```lean\nimport Mathlib.Data.Nat.Basic\nimport Mathlib.Algebra.Group.Basic\n```\n\nSorted\n```lean\nimport Mathlib.Algebra.Group.Basic\nimport Mathlib.Data.Nat.Basic\n```\n\nBlank-line-separated groups are preserved by default."
        }
        "normalize.headings" => {
            "Before\n```lean\n/-! ## a useful lemma -/\n```\n\nWith default heading rules\n```lean\n/-! ## A Useful Lemma -/\n```\n\nExisting heading levels are preserved."
        }
        "normalize.rules.attributes.placement" => {
            "Same line\n```lean\n@[simp] theorem demo : True := by\n  trivial\n```\n\nAbove\n```lean\n@[simp]\ntheorem demo : True := by\n  trivial\n```\n\nOff keeps the original placement."
        }
        "diagnostics.limit" | "diagnostics.severity" => {
            "Errors only\n```text\nerror: unsolved goals\n```\n\nErrors and warnings\n```text\nerror: unsolved goals\nwarning: declaration uses 'sorry'\n```\n\nA page size of 5 returns up to five matching diagnostics."
        }
        "build.warnings" | "build.strict" => {
            "Warnings included\nRecognized Lake warnings appear alongside errors.\n\nStrict builds\nA recognized warning makes the build fail, even when Lake succeeds.\n\nStrict mode includes warnings automatically."
        }
        "build.timeout_ms" => {
            "Examples\n\n```text\n0.5 minutes = 30 seconds\n30 minutes = 1,800 seconds\n```\n\nThe budget includes time waiting for another leanck build to finish."
        }
        "loogle.endpoints" => {
            "Local only\n```text\nhttp://127.0.0.1:8118/loogle/json\n```\n\nPublic only\n```text\nhttps://loogle.lean-lang.org/json\n```\n\nEnter multiple URLs separated by commas for fallback, or clear the field to disable search."
        }
        "loogle.limit" => {
            "A limit of 10 returns at most ten matching declarations.\n\nA limit of 20 allows more results.\n\nA limit of 0 returns no hits."
        }
        "daemon.idle_timeout_ms" => {
            "60 minutes\nShut down after an idle hour.\n\n10 minutes\nFree idle resources sooner.\n\n0\nKeep daemons running until stopped.\n\nBusy work is not interrupted."
        }
        "daemon.max_inflight" => {
            "4 concurrent requests\nLess simultaneous elaboration.\n\n8 concurrent requests\nThe built-in default.\n\nThe limit applies to each daemon, shared by agents using its worktree."
        }
        "cache.enabled" => {
            "Enabled\nTry the project's Mathlib cache fetch before Lean startup.\n\nDisabled\nSkip that fetch.\n\nFetching is best effort; the project's own Lake and toolchain are used."
        }
        "instances.limit" | "instances.on_limit" => {
            "Limit: 3\nAt most three worktree daemons under the default policy.\n\nStop idle: make room safely.\nRefuse: leave existing daemons.\nWarn: allow another daemon.\n\nA limit of 0 disables the cap."
        }
        "updates.check" | "updates.automatic" => {
            "Release checks\nLook for published leanck releases in the foreground.\n\nAutomatic updates\nInstall for installer-managed releases when no sessions are using the installation.\n\nSource builds keep their source update workflow."
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> Event {
        Event::Key(event::KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn focus_cycles_buttons_and_keeps_text_arrows_local() {
        let mut focus = Focus::Content;
        let press = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            focus.handle(press(KeyCode::Left), Navigation::Setting),
            None
        );
        assert_eq!(focus, Focus::Content);
        assert_eq!(
            focus.handle(press(KeyCode::Tab), Navigation::Setting),
            Some(ButtonAction::Consumed)
        );
        assert_eq!(focus, Focus::Previous);
        assert_eq!(
            focus.handle(press(KeyCode::Enter), Navigation::Setting),
            Some(ButtonAction::Previous)
        );
        focus.handle(press(KeyCode::Right), Navigation::Setting);
        assert_eq!(focus, Focus::Next);
        assert_eq!(
            focus.handle(press(KeyCode::Enter), Navigation::Setting),
            Some(ButtonAction::Next)
        );
        focus.handle(press(KeyCode::Tab), Navigation::Setting);
        assert_eq!(focus, Focus::Content);
        focus.handle(press(KeyCode::BackTab), Navigation::Setting);
        assert_eq!(focus, Focus::Next);

        let mut focus = Focus::Content;
        focus.handle(press(KeyCode::Tab), Navigation::Forward);
        assert_eq!(focus, Focus::Next);
        focus.handle(press(KeyCode::Left), Navigation::Forward);
        assert_eq!(focus, Focus::Next, "disabled Previous is skipped");
    }

    #[test]
    fn text_defaults_can_be_kept_replaced_edited_and_pasted() {
        let mut field = TextField::new("100");
        assert_eq!(field.input.value(), "100");
        field.handle(&key(KeyCode::Char('7')));
        field.handle(&key(KeyCode::Char('2')));
        assert_eq!(field.input.value(), "72");

        let mut field = TextField::new("100");
        field.handle(&key(KeyCode::Left));
        field.handle(&key(KeyCode::Backspace));
        assert_eq!(field.input.value(), "10");
        let mut field = TextField::new("100");
        field.handle(&key(KeyCode::Backspace));
        assert_eq!(field.input.value(), "");
        let mut field = TextField::new("old URL");
        field.handle(&Event::Paste("https://example.test/json\r\n".into()));
        assert_eq!(field.input.value(), "https://example.test/json");
    }

    #[test]
    fn examples_distinguish_explanations_results_and_lean_tokens() {
        let source =
            "Explanation\n```lean\ntheorem demo : True := by\n  trivial\n```\n```text\nresult\n```";
        let text = example_text(source, true);
        assert_eq!(text.lines[0].style.fg, Some(Color::Gray));
        assert!(text.lines[0].style.add_modifier.contains(Modifier::DIM));
        let code = &text.lines[1].spans;
        assert_eq!(
            code.iter()
                .find(|span| span.content == "theorem")
                .unwrap()
                .style
                .fg,
            Some(Color::LightMagenta)
        );
        assert_eq!(
            code.iter()
                .find(|span| span.content == "True")
                .unwrap()
                .style
                .fg,
            Some(Color::Yellow)
        );
        assert_eq!(text.lines[3].style, Style::default());
        let plain = example_text(source, false);
        for line in plain.lines {
            assert_eq!(line.style, Style::default());
            assert!(line.spans.iter().all(|span| span.style == Style::default()));
        }
    }

    #[test]
    fn examples_use_a_separate_column_only_when_they_fit() {
        for (width, height, expected) in [(140, 40, true), (80, 24, false), (140, 20, false)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let example = examples("normalize.spelling").unwrap();
            terminal
                .draw(|frame| {
                    page(
                        frame,
                        "Project: leanck.toml",
                        "Spelling convention",
                        "Choose a convention.",
                        Some(example),
                        false,
                        Controls::Menu {
                            multi: false,
                            scroll: 0,
                            navigation: Navigation::Setting,
                        },
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let screen = buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert_eq!(screen.contains("Examples"), expected);
            if expected {
                let (left, right) = columns(Rect::new(1, 6, width - 2, height - 9), true);
                assert!(right.unwrap().x > left.right());
                assert!(screen.contains("British English"));
            }
        }
    }
}
