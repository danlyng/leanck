//! The advanced-config form: one row per setting, edited in place.
//!
//! Navigation moves between rows; Left and Right cycle choices; Enter edits
//! a path, cycles a choice, or runs an action. Saving returns the draft
//! settings; cancelling a changed draft first asks before discarding it.
use super::{
    Input,
    flow::{Group, selection, skill_description},
};
use crate::{
    installer::{CompletionShell, Hosts, Mode, Settings, ensure_path_shell},
    paths::{absolute, display, valid_text},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::{cell::Cell, path::PathBuf};
use tui_input::{Input as Buffer, InputRequest};

/// One form row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    DataDir,
    BinDir,
    Cli,
    SkillsBundle,
    SkillHosts,
    Skill(usize),
    ClaudeDir,
    CodexSkills,
    Extra(Group, usize),
    AddExtra(Group),
    AgentsBundle,
    AgentHosts,
    CodexAgentsDir,
    Completions,
    CompletionsDir,
    AddToPath,
    Shell,
    PathFile,
    Mcp,
    PluginHosts,
    CodexDir,
    Save,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Path,
    Choice,
    Info,
    Action,
}

impl Field {
    fn kind(self) -> Kind {
        match self {
            Field::Cli
            | Field::SkillHosts
            | Field::Skill(_)
            | Field::AgentHosts
            | Field::Completions
            | Field::AddToPath
            | Field::Shell
            | Field::Mcp
            | Field::PluginHosts => Kind::Choice,
            Field::SkillsBundle | Field::AgentsBundle => Kind::Info,
            Field::AddExtra(_) | Field::Save | Field::Cancel => Kind::Action,
            _ => Kind::Path,
        }
    }
}

/// What closing the form produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormEffect {
    None,
    /// Saved settings, or `None` when cancelled or discarded.
    Close(Option<Box<Settings>>),
    /// Ctrl-C or Ctrl-D.
    Interrupted,
}

#[derive(Clone, Debug)]
struct Editing {
    buffer: Buffer,
    /// A location row added by an "Add ... location..." action.
    new_location: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Message {
    text: String,
    error: bool,
}

/// The advanced-config form state.
#[derive(Clone, Debug)]
pub struct Form {
    initial: Settings,
    draft: Settings,
    skills: Vec<String>,
    cwd: PathBuf,
    fields: Vec<Field>,
    selected: usize,
    offset: Cell<usize>,
    editing: Option<Editing>,
    message: Option<Message>,
    /// The discard dialog's cursor: 0 keeps editing, 1 discards.
    discard: Option<usize>,
}

const KEEP_EDITING: &str = "Keep editing";
const DISCARD: &str = "Discard changes";

/// Whether a key stops the installer: Ctrl-C or Ctrl-D.
pub fn is_interrupt(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c' | 'd' | 'C' | 'D'))
}

/// Apply a line-editing key to `buffer`; returns whether the key was used.
pub fn edit(buffer: &mut Buffer, key: &KeyEvent) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let request = match key.code {
        KeyCode::Char('a') if control => InputRequest::GoToStart,
        KeyCode::Char('e') if control => InputRequest::GoToEnd,
        KeyCode::Char('u') if control => InputRequest::DeleteLine,
        KeyCode::Char('k') if control => InputRequest::DeleteTillEnd,
        KeyCode::Char('w') if control => InputRequest::DeletePrevWord,
        KeyCode::Char(_) if control => return false,
        KeyCode::Char(c) if !c.is_control() => InputRequest::InsertChar(c),
        KeyCode::Backspace => InputRequest::DeletePrevChar,
        KeyCode::Delete => InputRequest::DeleteNextChar,
        KeyCode::Left => InputRequest::GoToPrevChar,
        KeyCode::Right => InputRequest::GoToNextChar,
        KeyCode::Home => InputRequest::GoToStart,
        KeyCode::End => InputRequest::GoToEnd,
        _ => return false,
    };
    buffer.handle(request);
    true
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.into()
}

const HOSTS: [Hosts; 4] = [
    Hosts::NONE,
    Hosts {
        claude: true,
        codex: false,
    },
    Hosts {
        claude: false,
        codex: true,
    },
    Hosts {
        claude: true,
        codex: true,
    },
];
const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

/// The next (or previous) value after `current` in `values`. An unknown
/// value goes to the first value forward or the last value backward.
fn cycle<T: PartialEq + Copy>(values: &[T], current: T, forward: bool) -> T {
    let n = values.len();
    match values.iter().position(|v| *v == current) {
        Some(i) if forward => values[(i + 1) % n],
        Some(i) => values[(i + n - 1) % n],
        None if forward => values[0],
        None => values[n - 1],
    }
}

impl Form {
    pub fn new(settings: Settings, skills: Vec<String>, cwd: PathBuf) -> Self {
        let mut form = Self {
            initial: settings.clone(),
            draft: settings,
            skills,
            cwd,
            fields: vec![],
            selected: 0,
            offset: Cell::new(0),
            editing: None,
            message: None,
            discard: None,
        };
        form.rebuild(None);
        form
    }

    /// The settings as edited so far.
    pub fn draft(&self) -> &Settings {
        &self.draft
    }
    /// Whether the draft differs from the settings the form opened with.
    pub fn dirty(&self) -> bool {
        self.draft != self.initial
    }
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }
    pub fn selected(&self) -> Field {
        self.fields[self.selected]
    }
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
    /// The discard dialog's highlighted option, while it is open.
    pub fn discard_selection(&self) -> Option<&'static str> {
        self.discard.map(|i| [KEEP_EDITING, DISCARD][i])
    }
    /// The message on the hint line.
    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|m| m.text.as_str())
    }
    /// The edit buffer, while editing.
    pub fn buffer(&self) -> Option<&str> {
        self.editing.as_ref().map(|e| e.buffer.value())
    }

    pub fn label(&self, field: Field) -> String {
        match field {
            Field::DataDir => "Plugin data directory".into(),
            Field::BinDir => "Executable directory".into(),
            Field::Cli => "Install executable".into(),
            Field::SkillsBundle | Field::SkillHosts => "Install skills".into(),
            Field::Skill(i) => format!("  {}", self.skills[i]),
            Field::ClaudeDir => "  Claude configuration".into(),
            Field::CodexSkills => "  Codex skills directory".into(),
            Field::Extra(Group::Skills, _) => "  Additional location".into(),
            Field::Extra(Group::ClaudeAgents, _) => "  Claude location".into(),
            Field::Extra(Group::CodexAgents, _) => "  Codex location".into(),
            Field::AddExtra(Group::Skills) => "  Add skill location...".into(),
            Field::AddExtra(Group::ClaudeAgents) => "  Add Claude location...".into(),
            Field::AddExtra(Group::CodexAgents) => "  Add Codex location...".into(),
            Field::AgentsBundle | Field::AgentHosts => "Install agents".into(),
            Field::CodexAgentsDir => "  Codex agents directory".into(),
            Field::Completions => "Shell completions".into(),
            Field::CompletionsDir => "  Completions directory".into(),
            Field::AddToPath => "Add leanck to PATH".into(),
            Field::Shell => "  Shell".into(),
            Field::PathFile => "  Shell profile".into(),
            Field::Mcp => "Enable MCP".into(),
            Field::PluginHosts => "Register plugins".into(),
            Field::CodexDir => "  Codex configuration".into(),
            Field::Save => "Save changes".into(),
            Field::Cancel => "Cancel".into(),
        }
    }

    /// A row's value as displayed, with `HOME` as `~`.
    pub fn value(&self, field: Field) -> String {
        let s = &self.draft;
        let path = |p: &std::path::Path| {
            if p.as_os_str().is_empty() {
                String::new()
            } else {
                display(p, &s.home)
            }
        };
        match field {
            Field::DataDir => path(&s.data_dir),
            Field::BinDir => path(&s.bin_dir),
            Field::Cli => yes_no(s.cli),
            Field::SkillsBundle => "[bundled with plugin]".into(),
            Field::SkillHosts => s.skill_hosts.label().into(),
            Field::Skill(i) => yes_no(s.selected_skills.includes(&self.skills[i])),
            Field::ClaudeDir => path(&s.claude_dir),
            Field::CodexSkills => path(&s.codex_skills),
            Field::Extra(group, i) => group.list(s).get(i).map(|p| path(p)).unwrap_or_default(),
            Field::AgentsBundle => match (s.plugin_hosts.claude, s.plugin_hosts.codex) {
                (true, _) => "Claude [bundled with plugin]".into(),
                (false, true) => "Codex".into(),
                (false, false) => "[bundled with plugin]".into(),
            },
            Field::AgentHosts => s.agent_hosts.label().into(),
            Field::CodexAgentsDir => path(&s.codex_agent_dir()),
            Field::Completions => s.completions.as_str().into(),
            Field::CompletionsDir => s.completion_dir().map(|d| path(&d)).unwrap_or_default(),
            Field::AddToPath => yes_no(s.add_to_path),
            Field::Shell => s.shell.clone(),
            Field::PathFile => path(&s.path_file),
            Field::Mcp => yes_no(s.mcp_enabled),
            Field::PluginHosts => s.plugin_hosts.label().into(),
            Field::CodexDir => path(&s.codex_dir),
            Field::AddExtra(_) | Field::Save | Field::Cancel => String::new(),
        }
    }

    fn build_fields(&self) -> Vec<Field> {
        let s = &self.draft;
        let mut fields = vec![];
        let extras = |fields: &mut Vec<Field>, group: Group| {
            for i in 0..group.list(s).len() {
                fields.push(Field::Extra(group, i));
            }
        };
        match s.mode {
            Mode::Plugin => {
                fields.extend([Field::DataDir, Field::BinDir, Field::SkillsBundle]);
                extras(&mut fields, Group::Skills);
                fields.extend([Field::AddExtra(Group::Skills), Field::AgentsBundle]);
                if s.plugin_hosts.codex {
                    fields.push(Field::CodexAgentsDir);
                }
            }
            Mode::Standalone => {
                fields.extend([Field::Cli, Field::BinDir, Field::SkillHosts]);
                fields.extend((0..self.skills.len()).map(Field::Skill));
                if s.skill_hosts.claude {
                    fields.push(Field::ClaudeDir);
                }
                if s.skill_hosts.codex {
                    fields.push(Field::CodexSkills);
                }
                extras(&mut fields, Group::Skills);
                fields.extend([Field::AddExtra(Group::Skills), Field::AgentHosts]);
                if s.agent_hosts.claude && !s.skill_hosts.claude {
                    fields.push(Field::ClaudeDir);
                }
                if s.agent_hosts.codex {
                    fields.push(Field::CodexAgentsDir);
                }
            }
        }
        extras(&mut fields, Group::ClaudeAgents);
        extras(&mut fields, Group::CodexAgents);
        fields.extend([
            Field::AddExtra(Group::ClaudeAgents),
            Field::AddExtra(Group::CodexAgents),
            Field::Completions,
        ]);
        if s.completions != CompletionShell::None {
            fields.push(Field::CompletionsDir);
        }
        fields.push(Field::AddToPath);
        if s.add_to_path {
            fields.extend([Field::Shell, Field::PathFile]);
        }
        if s.mode == Mode::Plugin {
            fields.extend([Field::Mcp, Field::PluginHosts]);
            if s.plugin_hosts.claude {
                fields.push(Field::ClaudeDir);
            }
            if s.plugin_hosts.codex {
                fields.push(Field::CodexDir);
            }
        }
        fields.extend([Field::Save, Field::Cancel]);
        fields
    }

    /// Rebuild the rows, keeping `keep` (or the current field) selected.
    fn rebuild(&mut self, keep: Option<Field>) {
        let keep = keep.or_else(|| self.fields.get(self.selected).copied());
        self.fields = self.build_fields();
        self.selected = keep
            .and_then(|field| self.fields.iter().position(|f| *f == field))
            .unwrap_or_else(|| self.selected.min(self.fields.len() - 1));
    }

    fn say(&mut self, text: &str, error: bool) {
        self.message = Some(Message {
            text: text.into(),
            error,
        });
    }

    pub fn update(&mut self, input: Input) -> FormEffect {
        if let Input::Key(key) = &input
            && is_interrupt(key)
        {
            return FormEffect::Interrupted;
        }
        if let Some(cursor) = self.discard {
            return self.update_discard(cursor, input);
        }
        if self.editing.is_some() {
            self.update_editing(input);
            return FormEffect::None;
        }
        let Input::Key(key) = input else {
            // A paste while navigating is swallowed.
            return FormEffect::None;
        };
        let n = self.fields.len();
        match key.code {
            KeyCode::Up => {
                self.selected = (self.selected + n - 1) % n;
                self.message = None;
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1) % n;
                self.message = None;
            }
            KeyCode::Left | KeyCode::Right => {
                let field = self.selected();
                if field.kind() == Kind::Choice {
                    self.cycle(field, key.code == KeyCode::Right);
                }
            }
            KeyCode::Esc => return self.request_cancel(),
            KeyCode::Enter => return self.activate(),
            _ => {}
        }
        FormEffect::None
    }

    fn update_discard(&mut self, cursor: usize, input: Input) -> FormEffect {
        let Input::Key(key) = input else {
            return FormEffect::None;
        };
        match key.code {
            KeyCode::Up | KeyCode::Down => self.discard = Some(1 - cursor),
            KeyCode::Esc => self.discard = None,
            KeyCode::Enter if cursor == 1 => return FormEffect::Close(None),
            KeyCode::Enter => self.discard = None,
            _ => {}
        }
        FormEffect::None
    }

    fn request_cancel(&mut self) -> FormEffect {
        if self.dirty() {
            self.discard = Some(0);
            FormEffect::None
        } else {
            FormEffect::Close(None)
        }
    }

    fn activate(&mut self) -> FormEffect {
        let field = self.selected();
        match field {
            Field::Save => return self.save(),
            Field::Cancel => return self.request_cancel(),
            Field::SkillsBundle | Field::AgentsBundle => {
                self.say(
                    "Choose an additional location below to install another copy.",
                    false,
                );
            }
            Field::AddExtra(group) => {
                let list = group.list_mut(&mut self.draft);
                list.push(PathBuf::new());
                let row = Field::Extra(group, list.len() - 1);
                self.rebuild(Some(row));
                self.editing = Some(Editing {
                    buffer: Buffer::default(),
                    new_location: true,
                });
                self.message = None;
            }
            _ if field.kind() == Kind::Choice => self.cycle(field, true),
            _ => {
                self.editing = Some(Editing {
                    buffer: Buffer::new(self.value(field)),
                    new_location: false,
                });
                self.message = None;
            }
        }
        FormEffect::None
    }

    fn cycle(&mut self, field: Field, forward: bool) {
        let s = &mut self.draft;
        match field {
            Field::Cli => s.cli = !s.cli,
            Field::AddToPath => s.add_to_path = !s.add_to_path,
            Field::Mcp => s.mcp_enabled = !s.mcp_enabled,
            Field::SkillHosts => s.skill_hosts = cycle(&HOSTS, s.skill_hosts, forward),
            Field::AgentHosts => s.agent_hosts = cycle(&HOSTS, s.agent_hosts, forward),
            Field::PluginHosts => s.plugin_hosts = cycle(&HOSTS, s.plugin_hosts, forward),
            Field::Completions => {
                s.completions = cycle(&CompletionShell::ALL, s.completions, forward)
            }
            Field::Shell => {
                let next = cycle(&SHELLS, s.shell.as_str(), forward).to_owned();
                s.set_shell(&next);
            }
            Field::Skill(index) => {
                let mut chosen: Vec<bool> = self
                    .skills
                    .iter()
                    .map(|name| s.selected_skills.includes(name))
                    .collect();
                chosen[index] = !chosen[index];
                s.selected_skills = selection(&self.skills, &chosen);
            }
            _ => return,
        }
        self.message = None;
        self.rebuild(Some(field));
    }

    fn update_editing(&mut self, input: Input) {
        let editing = self.editing.as_mut().expect("editing");
        match input {
            Input::Paste(text) => {
                if text.chars().any(char::is_control) {
                    self.say("Paste a single-line value.", true);
                } else {
                    for c in text.chars() {
                        editing.buffer.handle(InputRequest::InsertChar(c));
                    }
                }
            }
            Input::Key(key) => match key.code {
                KeyCode::Esc => self.abandon_edit(),
                KeyCode::Enter => {
                    let value = editing.buffer.value().to_owned();
                    self.commit(&value);
                }
                _ => {
                    edit(&mut editing.buffer, &key);
                }
            },
        }
    }

    fn abandon_edit(&mut self) {
        let field = self.selected();
        if let Some(Editing {
            new_location: true, ..
        }) = self.editing.take()
            && let Field::Extra(group, index) = field
        {
            group.list_mut(&mut self.draft).remove(index);
            self.rebuild(Some(Field::AddExtra(group)));
        }
        self.message = None;
    }

    /// Validate and store an edited value; errors keep the edit open.
    fn commit(&mut self, value: &str) {
        let field = self.selected();
        if let Err(error) = valid_text(value) {
            self.say(&error.to_string(), true);
            return;
        }
        let (home, cwd) = (self.draft.home.clone(), self.cwd.clone());
        let path = |value: &str| absolute(value, &home, &cwd);
        let mut keep = Some(field);
        let result: anyhow::Result<()> = (|| {
            let s = &mut self.draft;
            match field {
                Field::Extra(group, index) => {
                    let list = group.list_mut(s);
                    if value.is_empty() {
                        list.remove(index);
                        keep = Some(Field::AddExtra(group));
                    } else {
                        let dir = path(value)?;
                        if list
                            .iter()
                            .enumerate()
                            .any(|(i, p)| i != index && *p == dir)
                        {
                            list.remove(index);
                            keep = Some(Field::AddExtra(group));
                        } else {
                            list[index] = dir;
                        }
                    }
                }
                Field::CompletionsDir => {
                    s.completions_dir = if value.is_empty() {
                        None
                    } else {
                        let dir = path(value)?;
                        (s.default_completion_dir(s.completions).as_ref() != Some(&dir))
                            .then_some(dir)
                    };
                }
                Field::CodexAgentsDir => {
                    s.codex_agents_dir = if value.is_empty() {
                        None
                    } else {
                        let dir = path(value)?;
                        (dir != s.codex_dir.join("agents/leanck")).then_some(dir)
                    };
                }
                _ => {
                    anyhow::ensure!(!value.is_empty(), "Enter a directory or file path.");
                    let p = path(value)?;
                    match field {
                        Field::DataDir => s.set_data_dir(p),
                        Field::BinDir => s.bin_dir = p,
                        Field::ClaudeDir => s.claude_dir = p,
                        Field::CodexSkills => s.codex_skills = p,
                        Field::CodexDir => s.codex_dir = p,
                        Field::PathFile => s.path_file = p,
                        _ => {}
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.editing = None;
                self.message = None;
                self.rebuild(keep);
            }
            Err(error) => self.say(&error.to_string(), true),
        }
    }

    fn save(&mut self) -> FormEffect {
        let s = &self.draft;
        if s.mode == Mode::Standalone && !s.has_components() {
            self.say(
                "Select an executable, skills, or custom agents before continuing.",
                true,
            );
            return FormEffect::None;
        }
        if s.add_to_path && ensure_path_shell(&s.shell).is_err() {
            self.say(
                "Choose fish, bash, or zsh before enabling PATH setup.",
                true,
            );
            return FormEffect::None;
        }
        FormEffect::Close(Some(Box::new(self.draft.clone())))
    }

    /// Rows needed for all fields, the title, context, spacing, and hint.
    pub fn height(&self) -> u16 {
        if self.discard.is_some() {
            5
        } else {
            u16::try_from(self.fields.len().saturating_add(4)).unwrap_or(u16::MAX)
        }
    }

    /// Draw the form into `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect, color: bool) {
        let width = area.width as usize;
        let height = area.height as usize;
        if width == 0 || height == 0 {
            return;
        }
        let styles = Styles::new(color);
        let mut lines: Vec<Line> = vec![];
        let mut cursor = None;
        if let Some(choice) = self.discard {
            lines.push(Line::styled(
                "Discard advanced config changes?",
                styles.prompt,
            ));
            lines.push(Line::raw("Your changes have not been saved."));
            lines.push(Line::raw(""));
            for (i, label) in [KEEP_EDITING, DISCARD].into_iter().enumerate() {
                lines.push(if i == choice {
                    Line::styled(format!("> {label}"), styles.selected)
                } else {
                    Line::raw(format!("  {label}"))
                });
            }
        } else {
            lines.push(Line::styled("Advanced configuration", styles.prompt));
            lines.push(Line::raw(match self.draft.mode {
                Mode::Plugin => "Plugin installation",
                Mode::Standalone => "Standalone installation",
            }));
            lines.push(Line::raw(""));
            let visible = height.saturating_sub(4).max(1);
            let mut offset = self.offset.get();
            if self.selected < offset {
                offset = self.selected;
            } else if self.selected >= offset + visible {
                offset = self.selected + 1 - visible;
            }
            offset = offset.min(self.fields.len().saturating_sub(visible));
            self.offset.set(offset);
            let label_width = (width / 2).min(30);
            let value_width = width.saturating_sub(label_width + 5);
            for (row, &field) in self.fields.iter().enumerate().skip(offset).take(visible) {
                let current = row == self.selected;
                let marker = if current { "> " } else { "  " };
                let style = if current {
                    styles.selected
                } else {
                    Style::default()
                };
                let label = truncate(&self.label(field), label_width);
                if field.kind() == Kind::Action {
                    lines.push(Line::styled(format!("{marker}{label}"), style));
                    continue;
                }
                let prefix = format!("{marker}{label:<label_width$}: ");
                match (&self.editing, current) {
                    (Some(editing), true) => {
                        let room = value_width.max(1);
                        let scroll = editing.buffer.visual_scroll(room);
                        let shown: String = editing
                            .buffer
                            .value()
                            .chars()
                            .skip(scroll)
                            .take(room)
                            .collect();
                        cursor = Some((
                            area.x
                                + (prefix.chars().count() + editing.buffer.visual_cursor() - scroll)
                                    .min(width - 1) as u16,
                            area.y + lines.len() as u16,
                        ));
                        lines.push(Line::from(vec![
                            Span::styled(prefix, style),
                            Span::raw(shown),
                        ]));
                    }
                    _ => {
                        let value = tail(&self.value(field), value_width);
                        lines.push(Line::styled(format!("{prefix}{value}"), style));
                    }
                }
            }
            while lines.len() < height.saturating_sub(1) {
                lines.push(Line::raw(""));
            }
            lines.truncate(height.saturating_sub(1));
            lines.push(match &self.message {
                Some(Message { text, error: true }) => {
                    Line::styled(truncate(text, width), styles.error)
                }
                Some(Message { text, .. }) => Line::raw(truncate(text, width)),
                None => Line::raw(truncate(&self.hint(), width)),
            });
        }
        lines.truncate(height);
        frame.render_widget(Paragraph::new(lines), area);
        if let Some(position) = cursor {
            frame.set_cursor_position(position);
        }
    }

    fn hint(&self) -> String {
        if self.editing.is_some() {
            return "Enter: save  Esc: discard this edit  Ctrl-U: clear".into();
        }
        match self.selected() {
            Field::Skill(i) => skill_description(&self.skills[i]).into(),
            field if field.kind() == Kind::Choice => {
                "Left/Right or Enter: change  Up/Down: move  Esc: close".into()
            }
            _ => "Enter: edit or select  Up/Down: move  Esc: close".into(),
        }
    }
}

/// Styles for the terminal UI; plain when color is off.
#[derive(Clone, Copy)]
pub(super) struct Styles {
    pub prompt: Style,
    pub selected: Style,
    pub error: Style,
}
impl Styles {
    pub fn new(color: bool) -> Self {
        if color {
            Styles {
                prompt: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                selected: Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                error: Style::default().fg(Color::Red),
            }
        } else {
            Styles {
                prompt: Style::default(),
                selected: Style::default(),
                error: Style::default(),
            }
        }
    }
}

/// At most `width` characters, ending in `…` when cut.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// At most `width` characters, keeping the end (`…` marks the cut).
fn tail(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - (width - 1)));
    out
}
