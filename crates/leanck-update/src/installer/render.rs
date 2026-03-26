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
    /// The plan lines, one per line.
    pub fn text(&self) -> String {
        self.actions.iter().map(|a| format!("{a}\n")).collect()
    }
    /// The review block: a heading, the plan lines, and a blank line.
    pub fn review(&self, style: Style) -> String {
        format!(
            "\n{}Ready to install leanck {}{}\n{}\n",
            style.bold,
            self.version,
            style.reset,
            self.text()
        )
    }
    /// The JSON outcome for this plan.
    pub fn outcome(&self, status: Status, message: impl Into<String>) -> Outcome {
        Outcome {
            status,
            version: self.version.clone(),
            message: message.into(),
            actions: self.actions.clone(),
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
