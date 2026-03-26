//! Guided setup driven through scripted keys on a ratatui `TestBackend`.
//!
//! Ported from `test-support/test_config_wizard.py`, which drove the real
//! binary through a pseudo-terminal. Each script interleaves expectations
//! about the screen with key presses; expectations are checked when the UI
//! next waits for input, so they see exactly what a terminal would show.
//! `test_automation_flags_and_dumb_term_skip_the_wizard` needs a process and
//! lives in `crates/leanck-cli/tests/configuration_wizard.rs`.
use super::{setup, ui::Ui};
use crate::{ConfigArgs, LoadedConfig, NormalizeWidth, load_files};
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use std::{cell::RefCell, collections::VecDeque, path::PathBuf, rc::Rc};

const ENTER: &str = "\r";
const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const LEFT: &str = "\x1b[D";
const RIGHT: &str = "\x1b[C";
/// Ctrl-D clears a multi-select; Ctrl-A selects everything.
const CLEAR: &str = "\x04";
const ALL: &str = "\x01";
const PREVIOUS: &str = "\t\r";
const NEXT: &str = "\t\t\r";
/// The first screen has no Previous button.
const ROOT_NEXT: &str = "\t\r";

/// Terminal bytes as crossterm key events.
fn keys(bytes: &str) -> Vec<Event> {
    let press = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
    let none = KeyModifiers::NONE;
    let mut events = vec![];
    let mut chars = bytes.chars().peekable();
    while let Some(c) = chars.next() {
        events.push(match c {
            '\r' => press(KeyCode::Enter, none),
            '\t' => press(KeyCode::Tab, none),
            '\x7f' => press(KeyCode::Backspace, none),
            '\x1b' if chars.peek() == Some(&'[') => {
                chars.next();
                match chars.next() {
                    Some('A') => press(KeyCode::Up, none),
                    Some('B') => press(KeyCode::Down, none),
                    Some('C') => press(KeyCode::Right, none),
                    Some('D') => press(KeyCode::Left, none),
                    Some('Z') => press(KeyCode::BackTab, KeyModifiers::SHIFT),
                    other => panic!("unsupported escape sequence {other:?}"),
                }
            }
            '\x1b' => press(KeyCode::Esc, none),
            '\x01'..='\x1a' => press(
                KeyCode::Char((b'a' + c as u8 - 1) as char),
                KeyModifiers::CONTROL,
            ),
            c => press(KeyCode::Char(c), none),
        });
    }
    events
}

fn lines(backend: &TestBackend) -> Vec<String> {
    let buffer = backend.buffer();
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect()
        })
        .collect()
}

/// The highlighted row: the first line starting with `> `, up to a border.
fn selected(lines: &[String]) -> String {
    lines
        .iter()
        .find_map(|line| {
            let line = line.trim_start();
            line.strip_prefix("> ")
                .map(|rest| rest.split('│').next().unwrap_or("").trim().to_owned())
        })
        .unwrap_or_default()
}

enum Step {
    Key(Event),
    Expect(String),
    Absent(String),
    Selected(String),
    Check(&'static str, fn(&[String]) -> bool),
    Resize(u16, u16),
}

/// A scripted terminal session.
#[derive(Default)]
struct Script(Vec<Step>);
impl Script {
    fn new() -> Self {
        Self::default()
    }
    fn keys(mut self, bytes: &str) -> Self {
        self.0.extend(keys(bytes).into_iter().map(Step::Key));
        self
    }
    fn expect(mut self, text: &str) -> Self {
        self.0.push(Step::Expect(text.into()));
        self
    }
    fn absent(mut self, text: &str) -> Self {
        self.0.push(Step::Absent(text.into()));
        self
    }
    fn selected(mut self, label: &str) -> Self {
        self.0.push(Step::Selected(label.into()));
        self
    }
    fn check(mut self, description: &'static str, predicate: fn(&[String]) -> bool) -> Self {
        self.0.push(Step::Check(description, predicate));
        self
    }
    fn resize(mut self, rows: u16, columns: u16) -> Self {
        self.0.push(Step::Resize(columns, rows));
        self
    }
    /// Wait for `prompt`, then send `bytes`.
    fn answer(self, prompt: &str, bytes: &str) -> Self {
        self.expect(prompt).keys(bytes)
    }
    /// Accept the defaults of the formatting topic, with `width` first.
    fn formatting(self, width: &str) -> Self {
        self.answer("Comment line width", width)
            .answer("Spelling convention", &format!("b{ENTER}"))
            .answer("Normalize file headers?", ENTER)
            .answer("Default license text", ENTER)
            .answer("Header format", ENTER)
            .answer("Sort imports?", ENTER)
            .answer("Format Markdown headings?", ENTER)
            .answer("Attribute placement", ENTER)
    }

    /// A UI reading this script, and the steps it has not reached yet.
    fn into_ui(self, rows: u16, columns: u16) -> (Ui<TestBackend>, Rc<RefCell<VecDeque<Step>>>) {
        let steps = Rc::new(RefCell::new(VecDeque::from(self.0)));
        let remaining = Rc::clone(&steps);
        let ui = Ui::with_backend(
            TestBackend::new(columns, rows),
            Box::new(move |backend: &mut TestBackend| {
                loop {
                    let screen = lines(backend);
                    let dump = || screen.join("\n");
                    let step = steps.borrow_mut().pop_front();
                    match step {
                        Some(Step::Key(event)) => return Ok(event),
                        Some(Step::Expect(text)) => assert!(
                            screen.iter().any(|line| line.contains(&text))
                                || dump().contains(&text),
                            "expected {text:?} on screen:\n{}",
                            dump()
                        ),
                        Some(Step::Absent(text)) => assert!(
                            !dump().to_lowercase().contains(&text.to_lowercase()),
                            "unexpected {text:?} on screen:\n{}",
                            dump()
                        ),
                        Some(Step::Selected(label)) => {
                            assert_eq!(selected(&screen), label, "selection in:\n{}", dump())
                        }
                        Some(Step::Check(description, predicate)) => {
                            assert!(predicate(&screen), "{description}:\n{}", dump())
                        }
                        Some(Step::Resize(columns, rows)) => {
                            backend.resize(columns, rows);
                            return Ok(Event::Resize(columns, rows));
                        }
                        None => anyhow::bail!("the script ended while setup waited:\n{}", dump()),
                    }
                }
            }),
            false,
        )
        .unwrap();
        (ui, remaining)
    }
}

/// A scratch project with a Lake marker and a user configuration path.
struct Case {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    user: PathBuf,
}
impl Case {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("lean-toolchain"), "").unwrap();
        Self {
            user: root.join("config/leanck/config.toml"),
            root,
            project,
            _dir: dir,
        }
    }

    /// Run `leanck config init [PATH] [--user]` from the project.
    fn run(&self, path: Option<&str>, user: bool, script: Script) -> Result<Option<String>> {
        self.run_sized(path, user, script, 40, 140)
    }
    fn run_sized(
        &self,
        path: Option<&str>,
        user: bool,
        script: Script,
        rows: u16,
        columns: u16,
    ) -> Result<Option<String>> {
        let (mut ui, remaining) = script.into_ui(rows, columns);
        let path = path.map(|p| self.project.join(p));
        let result = setup(
            &mut ui,
            path.as_deref(),
            user,
            Some(&self.project),
            Some(&self.user),
            &ConfigArgs::default(),
        );
        let left = remaining.borrow().len();
        assert_eq!(left, 0, "setup finished with {left} script steps unused");
        result
    }

    /// `leanck config show` for the project: user defaults, then the project file.
    fn show(&self) -> LoadedConfig {
        let mut layers = vec![(self.user.clone(), false)];
        for name in ["leanck.toml", ".leanck.toml"] {
            let file = self.project.join(name);
            if file.exists() {
                layers.push((file, true));
                break;
            }
        }
        load_files(&layers, &[]).unwrap()
    }
    fn project_file(&self) -> PathBuf {
        self.project.join("leanck.toml")
    }
}

fn canceled(result: Result<Option<String>>) -> bool {
    match result {
        Ok(None) => true,
        Err(error) => error.is::<super::ui::Canceled>(),
        Ok(Some(report)) => panic!("unexpectedly saved: {report}"),
    }
}

#[test]
fn project_setup_validation_review_back_and_inheritance() {
    let case = Case::new();
    std::fs::create_dir_all(case.user.parent().unwrap()).unwrap();
    std::fs::write(
        &case.user,
        "[normalize]\nspelling = 'british'\n[normalize.rules.frontmatter]\nlicense_text = 'Team license.'\nheader_template = '/- {authors}: {license} -/'\n[daemon]\nmax_documents = 42\n",
    )
    .unwrap();
    let script = Script::new()
        .answer("Where should these settings apply?", ENTER)
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .answer("Comment line width", "0\r")
        .answer("expected a positive integer or \"unlimited\"", "\x7f72\r")
        .answer("Spelling convention", ENTER)
        .answer("Normalize file headers?", ENTER)
        .answer("Default license text", ENTER)
        .answer("Header format", ENTER)
        .answer("Sort imports?", ENTER)
        .answer("Format Markdown headings?", ENTER)
        .answer("Attribute placement", ENTER)
        .answer("Save this configuration?", "e\r")
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR}{ENTER}"),
        )
        .answer("Save this configuration?", "s\r");
    let report = case.run(None, false, script).unwrap().unwrap();
    assert_eq!(report, format!("created {}", case.project_file().display()));
    let text = std::fs::read_to_string(case.project_file()).unwrap();
    assert!(text.contains("width = 72"));
    for table in ["[daemon]", "[instances]", "[updates]"] {
        assert!(!text.contains(table), "{table}");
    }
    let report = case.show();
    let frontmatter = &report.settings.normalize.rules.frontmatter;
    assert_eq!(
        toml::Value::try_from(&report.settings.normalize).unwrap()["spelling"].as_str(),
        Some("british")
    );
    assert_eq!(frontmatter.license_text, "Team license.");
    assert_eq!(frontmatter.header_template, "/- {authors}: {license} -/");
    for key in [
        "normalize.rules.frontmatter.license_text",
        "normalize.rules.frontmatter.header_template",
    ] {
        assert_eq!(
            report.sources[key],
            case.user.display().to_string(),
            "{key}"
        );
    }
    assert_eq!(report.settings.daemon.max_documents, 42);
}

#[test]
fn license_and_header_format_validate_preserve_and_save() {
    let case = Case::new();
    let license = "Released under Apache 2.0 license as described in the file LICENSE.";
    let template = r"/-\n{copyright}\nAuthors: {authors}\n{license}\n-/";
    let script = Script::new()
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .answer("Comment line width", ENTER)
        .answer("Spelling convention", ENTER)
        .answer("Normalize file headers?", ENTER)
        .expect("Default license text")
        .expect("Released under Apache 2.0 license")
        .expect("LICENSE.")
        .answer("Default license text", &format!("bad -/{NEXT}"))
        .expect("license_text must not contain Lean comment delimiters")
        .answer("Default license text", &format!("\x15{license}{NEXT}"))
        .expect("Header format")
        .expect(r"/-\n{copyright}\n{license}\nAuthors: {authors}\n-/")
        .answer("Header format", &format!(r"/-\n{{unknown}}\n-/{NEXT}"))
        .expect("unknown placeholder {unknown}")
        .answer("Header format", &format!("\x15{template}{NEXT}"))
        .answer("Sort imports?", PREVIOUS)
        .expect(template)
        .answer("Header format", PREVIOUS)
        .expect("Default license text")
        .answer("Default license text", NEXT)
        .answer("Header format", NEXT)
        .answer("Sort imports?", ENTER)
        .answer("Format Markdown headings?", ENTER)
        .answer("Attribute placement", ENTER)
        .answer("Save this configuration?", "s\r");
    case.run(Some("leanck.toml"), false, script)
        .unwrap()
        .unwrap();
    let frontmatter = case.show().settings.normalize.rules.frontmatter;
    assert_eq!(frontmatter.license_text, license);
    assert_eq!(frontmatter.header_template, template.replace(r"\n", "\n"));
}

#[test]
fn existing_hidden_file_preserves_comments_and_custom_settings() {
    let case = Case::new();
    let path = case.project.join(".leanck.toml");
    std::fs::write(
        &path,
        "# Team conventions\n[normalize]\nwidth = 80 # columns\n[daemon]\nmax_documents = 42\n",
    )
    .unwrap();
    let script = Script::new()
        .answer("Where should these settings apply?", ENTER)
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .formatting("72\r")
        .answer("Save this configuration?", "s\r");
    let report = case.run(None, false, script).unwrap().unwrap();
    assert_eq!(report, format!("updated {}", path.display()));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("width = 72 # columns"));
    assert!(text.contains("# Team conventions"));
    assert!(text.contains("max_documents = 42"));
    assert!(!case.project_file().exists());
}

#[test]
fn user_setup_all_topics_and_redirected_stdout() {
    let case = Case::new();
    let script = Script::new()
        .answer(
            "What would you like to customize?",
            &format!("{ALL}{ENTER}"),
        )
        .formatting(ENTER)
        .answer("Diagnostics per page", "12\r")
        .answer("Which diagnostic severities?", &format!("{ALL}{ENTER}"))
        .answer("Include build warnings?", "y\r")
        .answer("Fail builds on warnings?", "y\r")
        .answer("Build timeout (minutes)", "0.5\r")
        .answer(
            "Loogle endpoints",
            &format!("{}https://example.test/json\r", "\x7f".repeat(100)),
        )
        .answer("Search results per query", "20\r")
        .answer("Shut down idle daemons after (minutes)", "0\r")
        .answer("Concurrent elaboration requests per daemon", "4\r")
        .answer("Fetch the Mathlib cache before daemon startup?", "n\r")
        .answer("Maximum running daemons on this machine", "3\r")
        .answer("When the daemon limit is reached", &format!("r{ENTER}"))
        .answer("Check for new leanck releases?", "n\r")
        .answer("Install available releases automatically?", ENTER)
        .answer("Save this configuration?", "s\r");
    // The report is the only stdout output; prompts render on the terminal.
    let report = case.run(None, true, script).unwrap().unwrap();
    assert_eq!(report, format!("created {}", case.user.display()));
    assert!(!report.contains('?'));
    let settings = load_files(&[(case.user.clone(), true)], &[])
        .unwrap()
        .settings;
    let value = toml::Value::try_from(&settings).unwrap();
    assert_eq!(value["instances"]["limit"].as_integer(), Some(3));
    assert_eq!(value["instances"]["on_limit"].as_str(), Some("refuse"));
    assert_eq!(value["build"]["timeout_ms"].as_integer(), Some(30000));
    assert_eq!(value["diagnostics"]["limit"].as_integer(), Some(12));
    assert_eq!(
        value["diagnostics"]["severity"].as_array().unwrap().len(),
        4
    );
    assert_eq!(
        settings.loogle.endpoints,
        ["https://example.test/json".to_owned()]
    );
    assert_eq!(value["daemon"]["idle_timeout_ms"].as_integer(), Some(0));
    assert_eq!(value["updates"]["check"].as_bool(), Some(false));
}

#[test]
fn cancel_and_ctrl_c_do_not_write_or_create_directories() {
    for key in [ENTER, "\x03", "\x1b"] {
        let case = Case::new();
        let mut script = Script::new()
            .answer(
                "What would you like to customize?",
                &format!("{CLEAR}{ENTER}"),
            )
            .answer("Save this configuration?", key);
        if key == "\x1b" {
            script = script.answer("Cancel setup?", &format!("{DOWN}{ENTER}"));
        }
        assert!(canceled(case.run(None, true, script)), "{key:?}");
        assert!(!case.user.parent().unwrap().exists());
    }
}

#[test]
fn user_scope_can_be_selected_without_a_flag() {
    let case = Case::new();
    let script = Script::new()
        .answer(
            "Where should these settings apply?",
            &format!("{DOWN}{ENTER}"),
        )
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR}{ENTER}"),
        )
        .answer("Save this configuration?", &format!("{UP}{UP}{ENTER}"));
    case.run(None, false, script).unwrap().unwrap();
    assert!(case.user.exists());
    assert!(!case.project_file().exists());
}

#[test]
fn examples_appear_on_the_right_and_follow_terminal_resize() {
    let case = Case::new();
    let script = Script::new()
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .expect("Comment line width")
        .check("examples in a right column", |lines| {
            lines.iter().any(|l| {
                l.find("Examples")
                    .is_some_and(|i| l[..i].chars().count() > 60)
            })
        })
        .resize(24, 80)
        .expect("Comment line width")
        .absent("Examples")
        .resize(40, 140)
        .expect("Examples")
        .answer("Comment line width", ENTER)
        .expect("Spelling convention")
        .check("British example in the right column", |lines| {
            lines.iter().any(|l| {
                l.find("The colour of the centre")
                    .is_some_and(|i| l[..i].chars().count() > 60)
            })
        })
        .keys("\x03");
    assert!(canceled(case.run(Some("leanck.toml"), false, script)));
    assert!(!case.project_file().exists());
}

#[test]
fn menus_have_no_filter_arrows_choose_and_typing_replaces_the_default() {
    let case = Case::new();
    let script = Script::new()
        .expect("Where should these settings apply?")
        .absent("filter")
        .answer("Where should these settings apply?", " ")
        .selected("This project")
        .keys(UP)
        .selected("All projects (user defaults)")
        .keys(DOWN)
        .selected("This project")
        .keys(ENTER)
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .expect("Comment line width")
        .expect("│100")
        .answer("Comment line width", "88\r")
        .expect("Spelling convention")
        .absent("filter")
        .answer("Spelling convention", &format!(" {DOWN}{ENTER}"))
        .expect("Normalize file headers?")
        .absent("filter")
        .answer("Normalize file headers?", ENTER)
        .answer("Default license text", ENTER)
        .answer("Header format", ENTER)
        .answer("Sort imports?", ENTER)
        .answer("Format Markdown headings?", ENTER)
        .answer("Attribute placement", ENTER)
        .answer("Save this configuration?", "s\r");
    case.run(None, false, script).unwrap().unwrap();
    let settings = case.show().settings;
    assert_eq!(settings.normalize.width, NormalizeWidth::Finite(88));
    assert_eq!(
        toml::Value::try_from(&settings.normalize).unwrap()["spelling"].as_str(),
        Some("british")
    );
}

#[test]
fn escape_confirmation_preserves_the_current_field_and_draft() {
    let case = Case::new();
    let script = Script::new()
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ENTER}"),
        )
        .answer("Comment line width", "72\x1b")
        .expect("Cancel setup?")
        .selected("Keep editing")
        .answer("Cancel setup?", ENTER)
        .expect("Comment line width")
        .expect("│72")
        .answer("Comment line width", "\x1b")
        .answer("Cancel setup?", "\x1b")
        .expect("Comment line width")
        .expect("│72")
        .answer("Comment line width", ENTER)
        .answer("Spelling convention", "\x1b")
        .answer("Cancel setup?", &format!("{DOWN}{ENTER}"));
    assert!(canceled(case.run(Some("leanck.toml"), false, script)));
    assert!(!case.project_file().exists());
}

#[test]
fn text_arrows_edit_and_tab_focuses_navigation_buttons() {
    let case = Case::new();
    let script = Script::new()
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {ROOT_NEXT}"),
        )
        .expect("Comment line width")
        .expect("Previous")
        .expect("Next")
        .answer("Comment line width", &format!("{LEFT}2"))
        .expect("│1020 ")
        .answer("Comment line width", &format!("{RIGHT}\x7f"))
        .expect("│102 ")
        .answer("Comment line width", "\x1572\t")
        .expect("> Previous <")
        .answer("Comment line width", RIGHT)
        .expect("> Next <")
        .expect("Comment line width")
        .answer("Comment line width", ENTER)
        .answer("Spelling convention", &format!("{DOWN}{PREVIOUS}"))
        .expect("Comment line width")
        .expect("│72 ")
        // Shift-Tab from the field focuses Next; neither this nor arrows submit.
        .answer("Comment line width", "\x1b[Z")
        .expect("> Next <")
        .answer("Comment line width", ENTER)
        .expect("Spelling convention")
        .selected("British English")
        .keys("\x03");
    assert!(canceled(case.run(Some("leanck.toml"), false, script)));
    assert!(!case.project_file().exists());
}

#[test]
fn buttons_navigate_screens_and_preserve_all_field_types() {
    let case = Case::new();
    let script = Script::new()
        .answer("What would you like to customize?", ROOT_NEXT)
        .answer("Comment line width", &format!("0{NEXT}"))
        .expect("expected a positive integer or \"unlimited\"")
        .answer("Comment line width", PREVIOUS)
        .answer("What would you like to customize?", ROOT_NEXT)
        .expect("Comment line width")
        .expect("│0")
        .answer("Comment line width", &format!("\x1572{NEXT}"))
        .answer("Spelling convention", &format!("{DOWN}{PREVIOUS}"))
        .expect("Comment line width")
        .expect("│72")
        .answer("Comment line width", &format!("80{NEXT}"))
        .expect("Spelling convention")
        .selected("British English")
        .answer("Spelling convention", NEXT)
        .answer("Normalize file headers?", &format!("{DOWN}{NEXT}"))
        .answer("Default license text", NEXT)
        .answer("Header format", NEXT)
        .answer("Sort imports?", PREVIOUS)
        .answer("Header format", PREVIOUS)
        .answer("Default license text", PREVIOUS)
        .expect("Normalize file headers?")
        .selected("No")
        .answer("Normalize file headers?", NEXT)
        .answer("Default license text", NEXT)
        .answer("Header format", NEXT)
        .answer("Sort imports?", NEXT)
        .answer("Format Markdown headings?", NEXT)
        .answer("Attribute placement", &format!("{DOWN}{NEXT}"))
        .answer("Diagnostics per page", &format!("9{PREVIOUS}"))
        .expect("Attribute placement")
        .selected("Above the declaration")
        .answer("Attribute placement", NEXT)
        .expect("Diagnostics per page")
        .expect("│9")
        .answer("Diagnostics per page", NEXT)
        .answer("Which diagnostic severities?", &format!("{DOWN} {NEXT}"))
        .answer("Include build warnings?", PREVIOUS)
        .expect("Which diagnostic severities?")
        .expect("[x] warning")
        .answer("Which diagnostic severities?", NEXT)
        .answer("Include build warnings?", &format!("{DOWN}{NEXT}"))
        .answer("Fail builds on warnings?", NEXT)
        .answer("Build timeout (minutes)", NEXT)
        .answer("Save this configuration?", PREVIOUS)
        .answer("Build timeout (minutes)", &format!("0.5{NEXT}"))
        .answer("Save this configuration?", RIGHT)
        .expect("Save this configuration?")
        .answer("Save this configuration?", &format!("s{ENTER}"));
    case.run(Some("leanck.toml"), false, script)
        .unwrap()
        .unwrap();
    let settings = case.show().settings;
    let value = toml::Value::try_from(&settings).unwrap();
    assert_eq!(settings.normalize.width, NormalizeWidth::Finite(80));
    assert_eq!(value["normalize"]["spelling"].as_str(), Some("british"));
    assert_eq!(value["normalize"]["header"].as_bool(), Some(false));
    assert_eq!(
        value["normalize"]["rules"]["attributes"]["placement"].as_str(),
        Some("above")
    );
    assert_eq!(value["diagnostics"]["limit"].as_integer(), Some(9));
    assert_eq!(
        value["diagnostics"]["severity"],
        toml::Value::Array(vec!["error".into(), "warning".into()])
    );
    assert_eq!(value["build"]["warnings"].as_bool(), Some(true));
    assert_eq!(value["build"]["timeout_ms"].as_integer(), Some(30000));
}

#[test]
fn back_to_scope_keeps_project_and_user_drafts_separate() {
    let case = Case::new();
    let script = Script::new()
        .answer("Where should these settings apply?", ROOT_NEXT)
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {NEXT}"),
        )
        .answer("Comment line width", &format!("72{NEXT}"))
        .answer("Spelling convention", &format!("b{NEXT}"))
        .answer("Normalize file headers?", PREVIOUS)
        .answer("Spelling convention", PREVIOUS)
        .answer("Comment line width", PREVIOUS)
        .answer("What would you like to customize?", PREVIOUS)
        .answer(
            "Where should these settings apply?",
            &format!("{DOWN}{ROOT_NEXT}"),
        )
        .answer(
            "What would you like to customize?",
            &format!("{CLEAR} {NEXT}"),
        )
        .expect("Comment line width")
        .expect("│100")
        .answer("Comment line width", &format!("90{NEXT}"))
        .expect("Spelling convention")
        .selected("American English")
        .answer("Spelling convention", PREVIOUS)
        .answer("Comment line width", PREVIOUS)
        .answer("What would you like to customize?", PREVIOUS)
        .answer(
            "Where should these settings apply?",
            &format!("{UP}{ROOT_NEXT}"),
        )
        .answer("What would you like to customize?", NEXT)
        .expect("Comment line width")
        .expect("│72")
        .answer("Comment line width", NEXT)
        .expect("Spelling convention")
        .selected("British English")
        .keys("\x03");
    assert!(canceled(case.run(None, false, script)));
    assert!(!case.project_file().exists());
    assert!(!case.user.exists());
    assert!(case.root.exists());
}

#[test]
fn scripted_keys_match_terminal_bytes() {
    let parsed = keys("a\r\t\x7f\x1b[A\x1b[Z\x1b\x15");
    let codes: Vec<_> = parsed
        .iter()
        .map(|e| match e {
            Event::Key(k) => (k.code, k.modifiers),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        codes,
        [
            (KeyCode::Char('a'), KeyModifiers::NONE),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Tab, KeyModifiers::NONE),
            (KeyCode::Backspace, KeyModifiers::NONE),
            (KeyCode::Up, KeyModifiers::NONE),
            (KeyCode::BackTab, KeyModifiers::SHIFT),
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('u'), KeyModifiers::CONTROL),
        ]
    );
}
