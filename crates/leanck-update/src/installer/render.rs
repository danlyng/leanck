//! Text and JSON forms of the configuration, the plan, and the outcome.
use super::{Mode, Plan, Settings, Style};
use crate::paths::display;
use serde::Serialize;

/// The welcome banner printed before configuration.
pub fn banner(version: &str, detected_agents: &str, style: Style) -> String {
    format!(
        "\n{}Welcome to the leanck {version} installer{}\nDetected coding agents: {detected_agents}\n\n\
         Plugin: bundles the CLI, all skills, agents, and hooks, with optional MCP.\n\
         Standalone (a la carte): choose the CLI, individual skills, and agents; no plugin registration or hooks.\n\n",
        style.bold, style.reset
    )
}

/// The configuration summary, with `HOME` shown as `~`.
pub fn config_summary(settings: &Settings) -> String {
    let s = settings;
    let home = &s.home;
    let mut out = String::from("\n`leanck` installation config:\n");
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    match s.mode {
        Mode::Plugin => {
            line(format!("- Plugin data dir: {}", display(&s.data_dir, home)));
            line("- Install skills: [bundled with plugin]".into());
            line("- Claude agents: [bundled with plugin]".into());
            line(if s.mcp_enabled {
                "- MCP: enabled".into()
            } else {
                "- MCP: disabled (definitions saved as *.disabled.json)".into()
            });
            line(format!("- Register plugins: {}", s.plugin_hosts.label()));
            if s.plugin_hosts.codex {
                line(format!(
                    "- Codex custom agents: {}",
                    display(&s.codex_agent_dir(), home)
                ));
            }
        }
        Mode::Standalone => {
            line(format!("- Skills: {}", s.skill_hosts.label()));
            line(format!("- Selected skills: {}", s.selected_skills.render()));
            line(format!("- Agents: {}", s.agent_hosts.label()));
            line("- MCP: disabled (available through leanck serve mcp)".into());
            if s.skill_hosts.claude {
                line(format!(
                    "- Claude skills directory: {}",
                    display(&s.claude_skills(), home)
                ));
            }
            if s.agent_hosts.claude {
                line(format!(
                    "- Claude agents directory: {}",
                    display(&s.claude_agents(), home)
                ));
            }
            if s.agent_hosts.codex {
                line(format!(
                    "- Codex custom agents: {}",
                    display(&s.codex_agent_dir(), home)
                ));
            }
        }
    }
    if s.cli {
        line(format!("- Executable: {}", display(&s.binary(), home)));
    }
    for (label, list) in [
        ("skills", &s.extra_skills),
        ("Claude agents", &s.extra_claude_agents),
        ("Codex agents", &s.extra_codex_agents),
    ] {
        for location in list {
            line(format!("- Additional {label}: {}", display(location, home)));
        }
    }
    line(format!("- Shell completions: {}", s.completions.as_str()));
    if let Some(dir) = s.completion_dir() {
        line(format!("- Completions directory: {}", display(&dir, home)));
    }
    if s.add_to_path {
        line("- Add leanck to PATH: yes".into());
        line(format!("- Shell: {}", s.shell));
        line(format!("- Shell profile: {}", display(&s.path_file, home)));
    } else {
        line("- Add leanck to PATH: no".into());
    }
    out.push('\n');
    out
}

impl Plan {
    /// The review block: a heading, a tree of items, and a blank line.
    pub fn review(&self, style: Style) -> String {
        let mut out = format!(
            "\n{}{}Install Plan{}{}: leanck {}{}\n",
            style.bold, style.cyan, style.reset, style.bold, self.version, style.reset
        );
        if let Some(repo) = &self.settings.from_repo {
            out.push_str(&format!(
                "{}Source checkout: {}{}\n",
                style.dim,
                repo.display(),
                style.reset
            ));
        }
        for (index, item) in self.items.iter().enumerate() {
            let last = index + 1 == self.items.len();
            let branch = if last { "└─ " } else { "├─ " };
            out.push_str(&format!("{}{branch}{}", style.dim, style.reset));
            if item.notice {
                out.push_str(style.yellow);
            }
            out.push_str(&item.text);
            if item.notice {
                out.push_str(style.reset);
            }
            out.push('\n');
            for (index, detail) in item.details.iter().enumerate() {
                let prefix = if last { "   " } else { "│  " };
                let branch = if index + 1 == item.details.len() {
                    "└─ "
                } else {
                    "├─ "
                };
                out.push_str(&format!(
                    "{}{prefix}{branch}{}{}{detail}{}\n",
                    style.dim, style.reset, style.dim, style.reset
                ));
            }
        }
        out.push('\n');
        out
    }
    /// The JSON outcome for this plan.
    pub fn outcome(&self, status: Status, message: impl Into<String>) -> Outcome {
        Outcome {
            status,
            version: self.version.clone(),
            message: message.into(),
            actions: self.actions(),
        }
    }
}

/// The one outcome vocabulary shared by installs and updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Installed,
    Updated,
    Partial,
    Preview,
    Failed,
    Cancelled,
}

/// The single JSON document `--json` prints on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub status: Status,
    pub version: String,
    pub message: String,
    pub actions: Vec<String>,
}
impl Outcome {
    /// One line: `{"status":…,"version":…,"message":…,"actions":[…]}`.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("outcomes serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_renders_notices_multiple_details_and_last_item_details() {
        use super::super::plan::PlanItem;
        use super::super::{Environment, FileSnapshot, InstallArgs, SavedState};
        use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
        let vars = BTreeMap::from([("HOME".into(), OsString::from("/home/user"))]);
        let mut env = Environment::from_vars(&vars, PathBuf::from("/work")).unwrap();
        env.files = FileSnapshot::assume_missing();
        let args = InstallArgs::parse_from(["--standalone"]).unwrap();
        let settings = Settings::resolve(&args, &env, &SavedState::default()).unwrap();
        let plan = Plan {
            settings,
            version: "v1.2.3".into(),
            items: vec![
                PlanItem {
                    text: "Keep /existing unchanged".into(),
                    details: vec![],
                    notice: true,
                },
                PlanItem {
                    text: "Register the Codex plugin".into(),
                    details: vec!["$ first".into(), "$ second".into()],
                    notice: false,
                },
                PlanItem {
                    text: "Last item".into(),
                    details: vec!["first detail".into(), "last detail".into()],
                    notice: false,
                },
            ],
            warnings: vec![],
            skipped: Default::default(),
            replaced: Default::default(),
            steps: vec![],
            registrations: vec![],
        };
        assert_eq!(
            plan.review(Style::PLAIN),
            concat!(
                "\nInstall Plan: leanck v1.2.3\n",
                "├─ Keep /existing unchanged\n",
                "├─ Register the Codex plugin\n",
                "│  ├─ $ first\n",
                "│  └─ $ second\n",
                "└─ Last item\n",
                "   ├─ first detail\n",
                "   └─ last detail\n\n",
            )
        );
    }

    #[test]
    fn outcome_is_one_line_with_keys_in_order_and_escaped_controls() {
        let outcome = Outcome {
            status: Status::Failed,
            version: "latest".into(),
            message: "bad \"value\"\n\u{1}".into(),
            actions: vec!["Would install /x".into()],
        };
        assert_eq!(
            outcome.to_json_line(),
            r#"{"status":"failed","version":"latest","message":"bad \"value\"\n\u0001","actions":["Would install /x"]}"#
        );
    }
}
