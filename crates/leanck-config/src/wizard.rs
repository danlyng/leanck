//! Terminal setup for common settings. Drafts are written only after review.
use crate::{ConfigArgs, NormalizeWidth, Settings, load_files, user_config_path};
use anyhow::{Context, Result, ensure};
#[cfg(test)]
mod screens;
mod ui;
use ratatui::backend::Backend;
use std::{
    collections::BTreeMap,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};
use ui::{Navigation, Ui, examples};

pub fn interactive(json: bool, disabled: bool) -> bool {
    prompts_allowed(
        json,
        disabled,
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
    )
}

/// Guided setup needs a terminal and is skipped by `--json`,
/// `--no-interactive`, and `TERM=dumb`.
fn prompts_allowed(json: bool, disabled: bool, terminal: bool, term: Option<&str>) -> bool {
    !json && !disabled && terminal && term != Some("dumb")
}

pub fn run(
    path: Option<&Path>,
    user: bool,
    workspace: Option<&Path>,
    args: &ConfigArgs,
) -> Result<()> {
    let user_path = user_config_path();
    let result = Ui::new().and_then(|mut ui| {
        let result = setup(&mut ui, path, user, workspace, user_path.as_deref(), args);
        // Restore the terminal before reporting.
        drop(ui);
        result
    });
    match result {
        Ok(Some(report)) => {
            println!("{report}");
            Ok(())
        }
        Ok(None) => {
            eprintln!("Setup canceled; no configuration was written.");
            Ok(())
        }
        Err(error) if error.is::<ui::Canceled>() => {
            eprintln!("Setup canceled; no configuration was written.");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Run the prompts on `ui` and save the reviewed draft. Returns the line to
/// print on stdout, or `None` when setup was cancelled at review; Ctrl-C
/// and a confirmed Escape return a [`ui::Canceled`] error. `user_path` is
/// the user configuration file.
fn setup<B: Backend>(
    ui: &mut Ui<B>,
    path: Option<&Path>,
    mut user: bool,
    workspace: Option<&Path>,
    user_path: Option<&Path>,
    args: &ConfigArgs,
) -> Result<Option<String>>
where
    B::Error: Send + Sync + 'static,
{
    let choose_scope = path.is_none() && !user;
    let mut drafts = BTreeMap::<PathBuf, (Draft, Vec<usize>)>::new();
    loop {
        ui.navigation = Navigation::Forward;
        if choose_scope {
            ui.context = "Choose project settings or user defaults for all projects.".into();
            user = ui.select(
                "Where should these settings apply?",
                &["This project", "All projects (user defaults)"],
                usize::from(user),
                "Project settings inherit your user defaults. User settings apply across projects.",
                None,
            )? == 1;
        }
        let path = if user {
            user_path
                .context("cannot locate user configuration; set HOME or XDG_CONFIG_HOME")?
                .to_owned()
        } else {
            crate::loader::init_path(path, false, workspace)?
        };
        // An explicit path to the user file has the same scope as --user.
        user |= user_path.is_some_and(|p| same_path(p, &path));
        let (mut draft, mut selected_topics) = match drafts.remove(&path) {
            Some(draft) => draft,
            None => (
                Draft::load(path.clone(), user, user_path, args)?,
                vec![0, 1],
            ),
        };
        ui.context = format!(
            "{} {}\nEnter keeps the current value. Only changed settings are saved.",
            if draft.original.is_some() {
                "Editing"
            } else {
                "Creating"
            },
            draft.path.display()
        );
        let mut topics = vec![
            "Formatting and spelling",
            "Diagnostics and builds",
            "Declaration search",
            "Daemon and cache",
        ];
        if user {
            topics.push("Daemon limit and updates");
        }
        let mut steps: Vec<Step> = Vec::new();
        let mut screen = Screen::Topics;
        loop {
            match screen {
                Screen::Topics => {
                    ui.navigation = if choose_scope {
                        Navigation::Topics
                    } else {
                        Navigation::Forward
                    };
                    selected_topics = match ui.multi("What would you like to customize?", &topics, &selected_topics,
                    "Space toggles a topic; Enter or Next continues. Select none to go straight to review.", None) {
                    Err(error) if error.is::<ui::Back>() => {
                        drafts.insert(path, (draft, selected_topics));
                        break;
                    }
                    answer => answer?,
                };
                    steps = selected_topics
                        .iter()
                        .flat_map(|&topic| topic_steps(topic))
                        .collect();
                    screen = if steps.is_empty() {
                        Screen::Review
                    } else {
                        Screen::Setting(0)
                    };
                }
                Screen::Setting(index) => {
                    ui.navigation = Navigation::Setting;
                    match steps[index](&mut draft, &mut *ui) {
                        Ok(()) => {
                            screen = if index + 1 == steps.len() {
                                Screen::Review
                            } else {
                                Screen::Setting(index + 1)
                            }
                        }
                        Err(error) if error.is::<ui::Back>() => {
                            screen = if index == 0 {
                                Screen::Topics
                            } else {
                                Screen::Setting(index - 1)
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                Screen::Review => {
                    ui.navigation = Navigation::Review;
                    draft.validate()?;
                    let mut review = format!("Review: {}\n", draft.path.display());
                    if draft.edits.is_empty() {
                        review.push_str(
                        "No settings changed. Omitted settings continue to inherit their values.\n",
                    );
                    }
                    for (key, value) in &draft.edits {
                        review.push_str(&format!("{key} = {value}\n"));
                    }
                    if !user {
                        review.push_str(
                        "Daemon limits and update policy are configured in user settings (--user).",
                    );
                    }
                    let action = match ui.select(
                        "Save this configuration?",
                        &["Save", "Edit choices", "Cancel"],
                        2,
                        &review,
                        None,
                    ) {
                        Err(error) if error.is::<ui::Back>() => {
                            screen = if steps.is_empty() {
                                Screen::Topics
                            } else {
                                Screen::Setting(steps.len() - 1)
                            };
                            continue;
                        }
                        answer => answer?,
                    };
                    match action {
                        0 => {
                            if draft.original.is_some() && draft.edits.is_empty() {
                                return Ok(Some(format!(
                                    "configuration unchanged ({})",
                                    draft.path.display()
                                )));
                            }
                            draft.save()?;
                            return Ok(Some(format!(
                                "{} {}",
                                if draft.original.is_some() {
                                    "updated"
                                } else {
                                    "created"
                                },
                                draft.path.display()
                            )));
                        }
                        1 => screen = Screen::Topics,
                        _ => return Ok(None),
                    }
                }
            }
        }
    }
}

enum Screen {
    Topics,
    Setting(usize),
    Review,
}

type Step = fn(&mut Draft, &mut dyn Prompts) -> Result<()>;

/// The prompts setting steps use; [`Ui`] implements them for any backend.
trait Prompts {
    fn select(
        &mut self,
        label: &str,
        options: &[&str],
        default: usize,
        help: &str,
        example: Option<&str>,
    ) -> Result<usize>;
    fn multi(
        &mut self,
        label: &str,
        options: &[&str],
        defaults: &[usize],
        help: &str,
        example: Option<&str>,
    ) -> Result<Vec<usize>>;
    fn text(
        &mut self,
        label: &str,
        initial: &str,
        default: Option<&str>,
        help: &str,
        example: Option<&str>,
        validate: &dyn Fn(&str) -> Result<()>,
    ) -> Result<String>;
}

impl<B: Backend> Prompts for Ui<B>
where
    B::Error: Send + Sync + 'static,
{
    fn select(
        &mut self,
        label: &str,
        options: &[&str],
        default: usize,
        help: &str,
        example: Option<&str>,
    ) -> Result<usize> {
        Ui::select(self, label, options, default, help, example)
    }
    fn multi(
        &mut self,
        label: &str,
        options: &[&str],
        defaults: &[usize],
        help: &str,
        example: Option<&str>,
    ) -> Result<Vec<usize>> {
        Ui::multi(self, label, options, defaults, help, example)
    }
    fn text(
        &mut self,
        label: &str,
        initial: &str,
        default: Option<&str>,
        help: &str,
        example: Option<&str>,
        validate: &dyn Fn(&str) -> Result<()>,
    ) -> Result<String> {
        Ui::text(self, label, initial, default, help, example, validate)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => std::path::absolute(a).ok() == std::path::absolute(b).ok(),
    }
}

#[derive(Clone)]
struct Draft {
    path: PathBuf,
    user: bool,
    original: Option<String>,
    document: DocumentMut,
    baseline: toml::Value,
    edits: BTreeMap<String, Value>,
}

impl Draft {
    /// Load the draft for `path`; project drafts inherit `user_path`.
    fn load(
        path: PathBuf,
        user: bool,
        user_path: Option<&Path>,
        args: &ConfigArgs,
    ) -> Result<Self> {
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => {
                ensure!(
                    !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
                    "{} is a symbolic link; run setup with the real configuration path",
                    path.display()
                );
                Some(text)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        let mut layers = Vec::new();
        if !user
            && !args.no_config
            && let Some(user) = user_path
        {
            layers.push((user.to_owned(), false));
        }
        // Read only the destination and its inherited user defaults. A project must
        // never seed user defaults, and an unrelated nearby project is not the target.
        layers.push((path.clone(), original.is_some()));
        let loaded = load_files(&layers, &[])?;
        let document = original
            .clone()
            .unwrap_or(crate::loader::starter_config(&Settings::default())?)
            .parse()?;
        Ok(Self {
            path,
            user,
            original,
            document,
            baseline: toml::Value::try_from(&loaded.settings)?,
            edits: BTreeMap::new(),
        })
    }

    fn get(&self, key: &str) -> toml::Value {
        if let Some(value) = self.edits.get(key) {
            // Values are produced from typed TOML below; this cannot fail.
            return toml::from_str::<toml::Table>(&format!("value = {value}"))
                .unwrap()
                .remove("value")
                .unwrap();
        }
        lookup(&self.baseline, key).clone()
    }

    fn set(&mut self, key: &str, value: toml::Value) -> Result<()> {
        if &value == lookup(&self.baseline, key) {
            self.edits.remove(key);
        } else {
            // The TOML serializer handles quoting, arrays, and control characters.
            let text = toml::to_string(&BTreeMap::from([("value", value)]))?;
            let doc: DocumentMut = text.parse()?;
            self.edits.insert(
                key.to_owned(),
                doc["value"]
                    .as_value()
                    .context("expected a TOML value")?
                    .clone(),
            );
        }
        Ok(())
    }

    fn render(&self) -> Result<String> {
        let mut document = self.document.clone();
        for (key, value) in &self.edits {
            assign(
                document.as_table_mut(),
                &key.split('.').collect::<Vec<_>>(),
                value.clone(),
            )?;
        }
        Ok(document.to_string())
    }

    fn validate(&self) -> Result<()> {
        let mut effective = self.baseline.clone();
        for key in self.edits.keys() {
            let mut item = &mut effective;
            for part in key.split('.') {
                item = item.get_mut(part).context("unknown setup setting")?;
            }
            *item = self.get(key);
        }
        let settings: Settings = effective.try_into()?;
        settings.validate()?;
        // Check the actual output independently of the inherited settings.
        let _: Settings = toml::from_str(&self.render()?)?;
        Ok(())
    }

    fn save(&self) -> Result<()> {
        self.validate()?;
        let path = std::path::absolute(&self.path)?;
        let parent = path.parent().context("configuration path has no parent")?;
        if self.user {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(self.render()?.as_bytes())?;
        if let Some(original) = &self.original {
            ensure!(
                !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
                "configuration became a symbolic link; rerun setup"
            );
            ensure!(
                std::fs::read_to_string(&path)? == *original,
                "configuration changed during setup; rerun setup to include those changes"
            );
            file.as_file()
                .set_permissions(std::fs::metadata(&path)?.permissions())?;
            file.as_file().sync_all()?;
            file.persist(&path)?;
        } else {
            file.as_file().sync_all()?;
            file.persist_noclobber(&path).with_context(|| {
                format!(
                    "cannot create {}; existing files are never overwritten",
                    path.display()
                )
            })?;
        }
        Ok(())
    }
}

fn lookup<'a>(value: &'a toml::Value, key: &str) -> &'a toml::Value {
    key.split('.').fold(value, |value, part| &value[part])
}

fn assign(table: &mut dyn TableLike, parts: &[&str], mut value: Value) -> Result<()> {
    if parts.len() == 1 {
        if let Some(old) = table.get(parts[0]).and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        table.insert(parts[0], Item::Value(value));
    } else {
        let item = table.entry(parts[0]).or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        });
        assign(
            item.as_table_like_mut()
                .context("expected a settings table")?,
            &parts[1..],
            value,
        )?;
    }
    Ok(())
}

fn boolean(
    draft: &mut Draft,
    ui: &mut dyn Prompts,
    key: &str,
    label: &str,
    help: &str,
) -> Result<()> {
    let default = if draft.get(key).as_bool().unwrap() {
        0
    } else {
        1
    };
    let selected = ui.select(label, &["Yes", "No"], default, help, examples(key))?;
    draft.set(key, (selected == 0).into())
}

fn number(
    draft: &mut Draft,
    ui: &mut dyn Prompts,
    key: &str,
    label: &str,
    help: &str,
    min: u64,
    max: u64,
) -> Result<()> {
    let default = draft.get(key).as_integer().unwrap().to_string();
    let answer = ui.text(label, "", Some(&default), help, examples(key), &|text| {
        let value = text.parse::<u64>().context("Enter a whole number.")?;
        ensure!(
            (min..=max).contains(&value),
            "Enter a number between {min} and {max}."
        );
        Ok(())
    })?;
    draft.set(key, toml::Value::Integer(answer.parse::<i64>()?))
}

fn normalization_width(draft: &mut Draft, ui: &mut dyn Prompts) -> Result<()> {
    let key = "normalize.width";
    let width: NormalizeWidth = draft.get(key).try_into()?;
    let answer = ui.text(
        "Comment line width",
        "",
        Some(&width.to_string()),
        "Enter a positive column count or unlimited to join prose without wrapping.",
        examples(key),
        &|text| {
            let width = text.parse::<NormalizeWidth>().map_err(anyhow::Error::msg)?;
            toml::Value::try_from(width)?;
            Ok(())
        },
    )?;
    let width = answer
        .parse::<NormalizeWidth>()
        .map_err(anyhow::Error::msg)?;
    draft.set(key, toml::Value::try_from(width)?)
}

fn duration(
    draft: &mut Draft,
    ui: &mut dyn Prompts,
    key: &str,
    label: &str,
    help: &str,
    allow_zero: bool,
) -> Result<()> {
    let default = (draft.get(key).as_integer().unwrap() as f64 / 60_000.0).to_string();
    let answer = ui.text(label, "", Some(&default), help, examples(key), &|text| {
        let value = text
            .parse::<f64>()
            .context("Enter a number of minutes (fractions are allowed).")?;
        ensure!(
            value.is_finite()
                && value <= 525_600.0
                && (value >= 1.0 / 60_000.0 || (allow_zero && value == 0.0)),
            "Enter a positive duration up to one year{}.",
            if allow_zero {
                ", or 0 to disable idle shutdown"
            } else {
                ""
            }
        );
        Ok(())
    })?;
    if answer != default {
        draft.set(
            key,
            toml::Value::Integer((answer.parse::<f64>()? * 60_000.0).round() as i64),
        )?;
    }
    Ok(())
}

fn choice(
    draft: &mut Draft,
    ui: &mut dyn Prompts,
    key: &str,
    label: &str,
    options: &[(&str, &str)],
) -> Result<()> {
    let current = draft.get(key);
    let index = options
        .iter()
        .position(|(_, value)| Some(*value) == current.as_str())
        .unwrap_or(0);
    let selected = ui.select(
        label,
        &options.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        index,
        "Choose a value; Enter accepts the current selection.",
        examples(key),
    )?;
    draft.set(key, options[selected].1.into())
}

fn escaped_text(
    draft: &mut Draft,
    ui: &mut dyn Prompts,
    key: &str,
    label: &str,
    help: &str,
) -> Result<()> {
    let current = draft.get(key);
    let initial = escape_lines(current.as_str().unwrap());
    let value = ui.text(label, &initial, None, help, examples(key), &|text| {
        let mut candidate = draft.clone();
        candidate.set(key, unescape_lines(text)?.into())?;
        candidate.validate()
    })?;
    draft.set(key, unescape_lines(&value)?.into())
}

fn escape_lines(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn unescape_lines(text: &str) -> Result<String> {
    let mut output = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        output.push(if c == '\\' {
            match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('\\') => '\\',
                _ => anyhow::bail!("Use \\n for line breaks or \\\\ for a literal backslash."),
            }
        } else {
            c
        });
    }
    Ok(output)
}

fn topic_steps(topic: usize) -> Vec<Step> {
    match topic {
        0 => vec![
            normalization_width,
            |d, u| {
                choice(
                    d,
                    u,
                    "normalize.spelling",
                    "Spelling convention",
                    &[
                        ("American English", "american"),
                        ("British English", "british"),
                        ("Leave spelling unchanged", "off"),
                    ],
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "normalize.header",
                    "Normalize file headers?",
                    "Format existing headers and derive missing attribution from Git history.",
                )
            },
            |d, u| {
                escaped_text(
                    d,
                    u,
                    "normalize.rules.frontmatter.license_text",
                    "Default license text",
                    "Used by {license} in generated headers. Leave empty for no license notice. Use \\n for line breaks and \\\\ for a backslash.",
                )
            },
            |d, u| {
                escaped_text(
                    d,
                    u,
                    "normalize.rules.frontmatter.header_template",
                    "Header format",
                    "Use \\n for line breaks and \\\\ for a backslash. Wrap the template in /- ... -/. Placeholders: {copyright}, {license}, {authors}, {years}, {first_year}, {last_year}.",
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "normalize.sort_imports",
                    "Sort imports?",
                    "Applies when frontmatter normalization is enabled.",
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "normalize.headings",
                    "Format Markdown headings?",
                    "Normalize headings in Lean comments.",
                )
            },
            |d, u| {
                choice(
                    d,
                    u,
                    "normalize.rules.attributes.placement",
                    "Attribute placement",
                    &[
                        ("Same line as the declaration", "same_line"),
                        ("Above the declaration", "above"),
                        ("Leave placement unchanged", "off"),
                    ],
                )
            },
        ],
        1 => vec![
            |d, u| {
                number(
                    d,
                    u,
                    "diagnostics.limit",
                    "Diagnostics per page",
                    "Maximum diagnostics returned per page; 0 returns no diagnostics.",
                    0,
                    i64::MAX as u64,
                )
            },
            diagnostic_severities,
            |d, u| {
                boolean(
                    d,
                    u,
                    "build.warnings",
                    "Include build warnings?",
                    "Show recognized warnings from Lake and dependencies.",
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "build.strict",
                    "Fail builds on warnings?",
                    "Strict mode also includes warnings, even when Lake exits successfully.",
                )
            },
            |d, u| {
                duration(
                    d,
                    u,
                    "build.timeout_ms",
                    "Build timeout (minutes)",
                    "Includes waiting for another leanck build to finish; fractions are allowed.",
                    false,
                )
            },
        ],
        2 => vec![search_endpoints, |d, u| {
            number(
                d,
                u,
                "loogle.limit",
                "Search results per query",
                "Use 0 to return no hits.",
                0,
                i64::MAX as u64,
            )
        }],
        3 => vec![
            |d, u| {
                duration(
                    d,
                    u,
                    "daemon.idle_timeout_ms",
                    "Shut down idle daemons after (minutes)",
                    "Use 0 to disable idle shutdown. Restart existing daemons to apply changes.",
                    true,
                )
            },
            |d, u| {
                number(
                    d,
                    u,
                    "daemon.max_inflight",
                    "Concurrent elaboration requests per daemon",
                    "Lower values reduce concurrent work; higher values serve more agents at once.",
                    1,
                    65536,
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "cache.enabled",
                    "Fetch the Mathlib cache before daemon startup?",
                    "Best effort; skips fetching when disabled.",
                )
            },
        ],
        4 => vec![
            |d, u| {
                number(
                    d,
                    u,
                    "instances.limit",
                    "Maximum running daemons on this machine",
                    "Use 0 for no cap. Each worktree uses one daemon.",
                    0,
                    crate::Instances::MAX_LIMIT,
                )
            },
            |d, u| {
                choice(
                    d,
                    u,
                    "instances.on_limit",
                    "When the daemon limit is reached",
                    &[
                        ("Stop the longest-idle daemon", "evict_idle"),
                        ("Refuse to start another daemon", "refuse"),
                        ("Start anyway with a warning", "warn"),
                    ],
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "updates.check",
                    "Check for new leanck releases?",
                    "Checks run in the foreground at most once daily by default.",
                )
            },
            |d, u| {
                boolean(
                    d,
                    u,
                    "updates.auto_install",
                    "Install available releases automatically?",
                    "Requires an installer-managed release; waits until no CLI, MCP, or daemon session is using it.",
                )
            },
        ],
        _ => unreachable!(),
    }
}

fn diagnostic_severities(draft: &mut Draft, ui: &mut dyn Prompts) -> Result<()> {
    let current = draft.get("diagnostics.severity");
    let options = ["error", "warning", "information", "hint"];
    let defaults: Vec<usize> = options
        .iter()
        .enumerate()
        .filter_map(|(i, value)| {
            current
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some(value))
                .then_some(i)
        })
        .collect();
    let selected = ui.multi(
        "Which diagnostic severities?",
        &options,
        &defaults,
        "Space toggles a severity. Select none to return no diagnostics.",
        examples("diagnostics.severity"),
    )?;
    draft.set(
        "diagnostics.severity",
        toml::Value::Array(selected.into_iter().map(|i| options[i].into()).collect()),
    )?;
    Ok(())
}

fn search_endpoints(draft: &mut Draft, ui: &mut dyn Prompts) -> Result<()> {
    let current = draft.get("loogle.endpoints");
    let initial = current
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(", ");
    let value = ui.text(
        "Loogle endpoints (comma-separated)",
        &initial,
        None,
        "Tried in order. Use local-only URLs to keep queries local; clear to disable search.",
        examples("loogle.endpoints"),
        &|text| {
            let mut settings = Settings::default();
            settings.loogle.endpoints = endpoints(text);
            settings.validate()
        },
    )?;
    draft.set(
        "loogle.endpoints",
        toml::Value::Array(endpoints(&value).into_iter().map(Into::into).collect()),
    )?;
    Ok(())
}

fn endpoints(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_flags_and_dumb_term_skip_the_wizard() {
        assert!(prompts_allowed(false, false, true, Some("xterm-256color")));
        assert!(prompts_allowed(false, false, true, None));
        for (json, disabled, terminal, term) in [
            (true, false, true, Some("xterm-256color")),
            (false, true, true, Some("xterm-256color")),
            (false, false, true, Some("dumb")),
            (false, false, false, Some("xterm-256color")),
        ] {
            assert!(
                !prompts_allowed(json, disabled, terminal, term),
                "{json} {disabled} {terminal} {term:?}"
            );
        }
    }

    #[test]
    fn multiline_fields_preserve_line_breaks_and_literal_backslashes() {
        for text in [
            "",
            "License text.",
            "/-\n{copyright}\n{license}\n-/",
            "\\new\\path\r\n\tÉlodie",
        ] {
            assert_eq!(unescape_lines(&escape_lines(text)).unwrap(), text);
        }
        assert!(unescape_lines("bad\\").is_err());
        assert!(unescape_lines(r"bad\x").is_err());
    }

    fn draft(path: &Path) -> Draft {
        Draft::load(
            path.to_owned(),
            false,
            None,
            &ConfigArgs {
                no_config: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn edits_preserve_comments_custom_rules_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leanck.toml");
        let original = "# Project conventions\nversion = 1\n[normalize]\nwidth = 80 # team width\n[normalize.rules.frontmatter.author_map]\n'Git Name' = 'Full Name'\n[normalize.rules.spelling]\nfiles = ['dictionary.json']\n[daemon]\nmax_documents = 42\n";
        std::fs::write(&path, original).unwrap();
        let mut draft = draft(&path);
        assert_eq!(draft.render().unwrap(), original);
        draft.set("normalize.width", 72.into()).unwrap();
        draft.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.replace("width = 80", "width = 72")
        );
        draft.set("normalize.width", 80.into()).unwrap();
        assert_eq!(draft.render().unwrap(), original);
        assert!(draft.edits.is_empty());
    }

    #[test]
    fn unchanged_inherited_values_stay_omitted_and_inline_tables_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leanck.toml");
        std::fs::write(
            &path,
            "normalize = { width = 72, spelling = 'british' } # keep\n",
        )
        .unwrap();
        let mut draft = draft(&path);
        draft.set("build.strict", false.into()).unwrap();
        draft.set("normalize.width", 90.into()).unwrap();
        draft.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("width = 90"));
        assert!(text.contains("# keep"));
        assert!(!text.contains("[build]"));
    }

    #[test]
    fn concurrent_changes_and_new_destinations_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leanck.toml");
        let mut new = draft(&path);
        new.set("normalize.width", 72.into()).unwrap();
        let concurrent = "[normalize]\nwidth = 90\n";
        std::fs::write(&path, concurrent).unwrap();
        assert!(new.save().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), concurrent);
        let mut existing = draft(&path);
        existing.set("normalize.width", 72.into()).unwrap();
        std::fs::write(&path, "version = 1\n").unwrap();
        assert!(
            existing
                .save()
                .unwrap_err()
                .to_string()
                .contains("changed during setup")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = 1\n");
    }

    #[test]
    fn invalid_settings_cannot_be_saved_and_new_files_remain_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leanck.toml");
        let mut draft = draft(&path);
        draft.set("normalize.width", 0.into()).unwrap();
        assert!(draft.save().is_err());
        assert!(!path.exists());
        draft.set("normalize.width", 72.into()).unwrap();
        draft.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value.as_table().unwrap().len(), 2);
        assert_eq!(value["normalize"]["width"].as_integer(), Some(72));
        assert!(!text.contains("[instances]"));
        assert!(!text.contains("[updates]"));
    }

    #[test]
    fn unlimited_width_can_be_saved_and_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leanck.toml");
        let mut draft = draft(&path);
        draft.set("normalize.width", "unlimited".into()).unwrap();
        draft.save().unwrap();
        let settings: Settings = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings.normalize.width, NormalizeWidth::Unlimited);
        let reopened = Draft::load(
            path,
            false,
            None,
            &ConfigArgs {
                no_config: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reopened.get("normalize.width").as_str(), Some("unlimited"));
    }
}
