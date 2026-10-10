//! The terminal UI: [`Installer::update`] maps keys to the [`Flow`] and the
//! advanced-config [`Form`]; [`Installer::render`] draws the current state.
//!
//! Only the active menu, prompt, or form is drawn. Answered questions and
//! flow messages are drained as transcript text for the driver to print above it.
use super::{
    Input,
    flow::{Effect, Flow, Output, Question},
    form::{Form, FormEffect, Styles, edit, is_interrupt},
};
use crate::installer::{Answers, Plan, PlanFailure, Settings, Style};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    style::Style as TextStyle,
    text::{Line, Span},
    widgets::Paragraph,
};
use tui_input::Input as Buffer;

/// The interactive installer's terminal state.
#[derive(Clone, Debug)]
pub struct Installer {
    flow: Flow,
    log: Vec<Output>,
    color: bool,
    serial: u64,
    cursor: usize,
    buffer: Buffer,
    form: Option<Form>,
}

impl Installer {
    /// `color` enables widget styling.
    pub fn new(flow: Flow, color: bool) -> Self {
        let mut installer = Self {
            flow,
            log: vec![],
            color,
            serial: u64::MAX,
            cursor: 0,
            buffer: Buffer::default(),
            form: None,
        };
        installer.sync();
        installer
    }

    pub fn color(&self) -> bool {
        self.color
    }

    pub fn flow(&self) -> &Flow {
        &self.flow
    }
    pub fn settings(&self) -> &Settings {
        self.flow.settings()
    }
    pub fn answers(&self) -> &Answers {
        self.flow.answers()
    }
    /// The open form, if any.
    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }
    /// Transcript entries so far.
    pub fn log(&self) -> &[Output] {
        &self.log
    }
    /// The highlighted menu option's label, when a menu is shown.
    pub fn menu_selection(&self) -> Option<&'static str> {
        match self.flow.question() {
            Question::Menu { options, .. } => options.get(self.cursor).map(|o| o.label),
            _ => None,
        }
    }
    /// The prompt's edit buffer.
    pub fn buffer(&self) -> &str {
        self.buffer.value()
    }

    /// Handle one key or paste.
    pub fn update(&mut self, input: Input) -> Effect {
        if let Input::Key(key) = &input
            && is_interrupt(key)
        {
            let effect = self.flow.interrupt();
            self.sync();
            return effect;
        }
        let effect = match self.flow.question() {
            Question::Menu { options, .. } => match input {
                Input::Key(key) => {
                    let n = options.len();
                    match key.code {
                        KeyCode::Up => {
                            self.cursor = (self.cursor + n - 1) % n;
                            Effect::None
                        }
                        KeyCode::Down => {
                            self.cursor = (self.cursor + 1) % n;
                            Effect::None
                        }
                        KeyCode::Enter => self.flow.choose(self.cursor),
                        // Menus ignore text; there is no Escape action.
                        _ => Effect::None,
                    }
                }
                Input::Paste(_) => Effect::None,
            },
            Question::Prompt { .. } => match input {
                Input::Key(key) if key.code == KeyCode::Enter => {
                    let reply = self.buffer.value().to_owned();
                    self.flow.answer(&reply)
                }
                Input::Key(key) => {
                    edit(&mut self.buffer, &key);
                    Effect::None
                }
                Input::Paste(text) => {
                    let text = text.strip_suffix('\n').unwrap_or(&text);
                    let text = text.strip_suffix('\r').unwrap_or(text);
                    if text.chars().any(char::is_control) {
                        self.log
                            .push(Output::Warn("Paste a single-line value.".into()));
                    } else {
                        for c in text.chars() {
                            self.buffer.handle(tui_input::InputRequest::InsertChar(c));
                        }
                    }
                    Effect::None
                }
            },
            Question::Form => match self.form.as_mut().map(|form| form.update(input)) {
                Some(FormEffect::Close(saved)) => self.flow.form_closed(saved.map(|s| *s)),
                Some(FormEffect::Interrupted) => self.flow.interrupt(),
                _ => Effect::None,
            },
            Question::Planning | Question::Done => Effect::None,
        };
        self.sync();
        effect
    }

    /// Report the plan that [`Effect::Plan`] requested.
    pub fn planned(&mut self, result: Result<Plan, PlanFailure>) -> Effect {
        let effect = self.flow.planned(result);
        self.sync();
        effect
    }

    /// Collect output and reset input state when the question changes.
    fn sync(&mut self) {
        self.log.extend(self.flow.take_output());
        if self.flow.serial() == self.serial {
            return;
        }
        self.serial = self.flow.serial();
        self.buffer = Buffer::default();
        let question = self.flow.question();
        self.cursor = match &question {
            Question::Menu { default, .. } => *default,
            _ => 0,
        };
        self.form = match question {
            Question::Form => Some(self.form.take().unwrap_or_else(|| {
                Form::new(
                    self.flow.settings().clone(),
                    self.flow.skills().to_vec(),
                    self.flow.cwd().to_owned(),
                )
            })),
            _ => None,
        };
    }

    /// Drain newly produced transcript text, for printing above the widget.
    pub fn take_transcript(&mut self, style: Style) -> String {
        let text = self.transcript(style);
        self.log.clear();
        text
    }

    /// Pending transcript entries as text.
    pub fn transcript(&self, style: Style) -> String {
        let mut out = String::new();
        for entry in &self.log {
            match entry {
                Output::Answer { prompt, reply } => out.push_str(&format!(
                    "{}{}{prompt}{} {reply}\n",
                    style.bold, style.cyan, style.reset
                )),
                Output::Warn(message) => out.push_str(&format!(
                    "{}[WARN]{} {message}\n",
                    style.yellow, style.reset
                )),
                Output::Error(message) => {
                    out.push_str(&format!("{} [ERR]:{} {message}\n", style.red, style.reset))
                }
                Output::Text(text) => out.push_str(text),
            }
        }
        out
    }

    /// The rows needed by the current widget at `width` columns.
    pub fn height(&self, width: u16) -> u16 {
        if width == 0 {
            return 0;
        }
        if let Some(form) = &self.form {
            return form.height();
        }
        u16::try_from(self.widget(width as usize).0.len()).unwrap_or(u16::MAX)
    }

    /// Build the current menu or prompt, without any transcript or header.
    fn widget(&self, width: usize) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
        let styles = Styles::new(self.color);
        let mut lines = vec![];
        let mut cursor = None;
        match self.flow.question() {
            Question::Menu { title, options, .. } => {
                lines.push(Line::styled(title, styles.prompt));
                for (i, option) in options.iter().enumerate() {
                    lines.push(if i == self.cursor {
                        Line::styled(format!("> {}", option.label), styles.selected)
                    } else {
                        Line::raw(format!("  {}", option.label))
                    });
                }
            }
            Question::Prompt { label, default } => {
                let mut prompt = wrap(
                    &[
                        (&label, styles.prompt),
                        (&format!(" [{default}]: "), TextStyle::default()),
                    ],
                    width,
                );
                let last = prompt.last().map(Line::width).unwrap_or(0);
                let start = if width.saturating_sub(last) >= 8 {
                    last
                } else {
                    prompt.push(Line::raw(""));
                    0
                };
                let room = width.saturating_sub(start + 1).max(1);
                let scroll = self.buffer.visual_scroll(room);
                let shown: String = self
                    .buffer
                    .value()
                    .chars()
                    .skip(scroll)
                    .take(room)
                    .collect();
                let column = (start + self.buffer.visual_cursor() - scroll).min(width - 1);
                if let Some(line) = prompt.last_mut() {
                    line.spans.push(Span::raw(shown));
                }
                lines.extend(prompt);
                cursor = Some((column, lines.len() - 1));
            }
            Question::Planning | Question::Form | Question::Done => {}
        }
        (lines, cursor)
    }

    /// Draw only the active widget into the inline frame.
    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.is_empty() {
            return;
        }
        if let Some(form) = &self.form {
            form.render(frame, area, self.color);
            return;
        }
        let (lines, cursor) = self.widget(area.width as usize);
        let skip = lines.len().saturating_sub(area.height as usize);
        let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
        frame.render_widget(Paragraph::new(lines), area);
        if let Some((x, y)) = cursor {
            frame.set_cursor_position((area.x + x as u16, area.y + (y - skip) as u16));
        }
    }
}

/// Wrap styled segments to `width` columns, breaking at spaces where possible.
fn wrap(segments: &[(&str, TextStyle)], width: usize) -> Vec<Line<'static>> {
    let chars: Vec<(char, TextStyle)> = segments
        .iter()
        .flat_map(|(text, style)| text.chars().map(move |c| (c, *style)))
        .collect();
    let width = width.max(1);
    let mut rows: Vec<&[(char, TextStyle)]> = vec![];
    let mut start = 0;
    while chars.len() - start > width {
        match (start + 1..=start + width)
            .rev()
            .find(|&i| chars[i].0 == ' ')
        {
            Some(space) => {
                rows.push(&chars[start..space]);
                start = space + 1;
            }
            None => {
                rows.push(&chars[start..start + width]);
                start += width;
            }
        }
    }
    rows.push(&chars[start..]);
    rows.into_iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = vec![];
            for (c, style) in row {
                match spans.last_mut() {
                    Some(span) if span.style == *style => span.content.to_mut().push(*c),
                    _ => spans.push(Span::styled(c.to_string(), *style)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_splits_long_words() {
        let plain = TextStyle::default();
        assert_eq!(
            text(&wrap(&[("alpha beta gamma", plain)], 10)),
            ["alpha beta", "gamma"]
        );
        assert_eq!(
            text(&wrap(&[("abcdefghij", plain)], 4)),
            ["abcd", "efgh", "ij"]
        );
        assert_eq!(text(&wrap(&[("", plain)], 4)), [""]);
        let bold = plain.add_modifier(ratatui::style::Modifier::BOLD);
        let lines = wrap(&[("Label", bold), (" [x]: ", plain)], 80);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].style, bold);
        assert_eq!(text(&lines), ["Label [x]: "]);
    }
}
