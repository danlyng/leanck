//! Custom agents for both hosts, assembled from shared sources.
//!
//! Each `leanck-subagents/<name>/` holds `instructions.md`, the shared body,
//! plus a header for each host it targets: `claude.yaml` (Claude Code
//! frontmatter keys) and `codex.toml` (Codex keys). The directory supplies
//! `name`; `instructions.md` supplies the body and Codex's
//! `developer_instructions`. Headers that set either are rejected.

use std::path::Path;

use anyhow::{Result, anyhow, bail, ensure};

use crate::{
    create_dir_all, file_name, frontmatter_text, header_keys, read_text, repr, sorted_entries,
    split_lines, write,
};

const CLAUDE_HEADER: &str = "claude.yaml";
const CODEX_HEADER: &str = "codex.toml";
const INSTRUCTIONS: &str = "instructions.md";

/// Write `agents/<name>.md` and `codex/agents/<name>.toml` for each agent,
/// returning the Claude Code and Codex agent names.
pub(crate) fn add_agents(repo: &Path, out: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let (mut claude, mut codex) = (Vec::new(), Vec::new());
    for agent in sorted_entries(&repo.join("leanck-subagents"))? {
        if !agent.is_dir() {
            continue;
        }
        let name = file_name(&agent)?;
        let instructions = agent.join(INSTRUCTIONS);
        if !instructions.is_file() {
            bail!("{}: missing {INSTRUCTIONS}", agent.display());
        }
        let body = read_text(&instructions)?;
        let (claude_header, codex_header) = (agent.join(CLAUDE_HEADER), agent.join(CODEX_HEADER));
        if !claude_header.is_file() && !codex_header.is_file() {
            bail!(
                "{}: needs {CLAUDE_HEADER}, {CODEX_HEADER}, or both",
                agent.display()
            );
        }
        if claude_header.is_file() {
            let text = claude_agent(&name, &claude_header, &read_text(&claude_header)?, &body)?;
            create_dir_all(&out.join("agents"))?;
            write(&out.join("agents").join(format!("{name}.md")), text)?;
            claude.push(name.clone());
        }
        if codex_header.is_file() {
            let text = codex_agent(&name, &codex_header, &read_text(&codex_header)?, &body)?;
            let directory = out.join("codex").join("agents");
            create_dir_all(&directory)?;
            write(&directory.join(format!("{name}.toml")), text)?;
            codex.push(name);
        }
    }
    Ok((claude, codex))
}

/// Claude Code agent: `name` and `header` as YAML frontmatter, then the body.
pub(crate) fn claude_agent(name: &str, path: &Path, header: &str, body: &str) -> Result<String> {
    let lines = split_lines(header);
    if lines.iter().any(|line| line == "---") {
        bail!(
            "{}: frontmatter keys must not contain a '---' line",
            path.display()
        );
    }
    if header_keys(path, &lines)?.contains_key("name") {
        bail!(
            "{}: sets 'name'; the directory name is used",
            path.display()
        );
    }
    let newline = if header.is_empty() || header.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let text = format!("---\nname: {name}\n{header}{newline}---\n\n{body}");
    frontmatter_text(path, &text)?;
    Ok(text)
}

/// Codex agent: `name`, the verbatim `header`, then the body as
/// `developer_instructions`. The result is parsed back to confirm it.
pub(crate) fn codex_agent(name: &str, path: &Path, header: &str, body: &str) -> Result<String> {
    let keys: toml::Table = header
        .parse()
        .map_err(|error| anyhow!("{}: invalid TOML: {error}", path.display()))?;
    for key in ["name", "developer_instructions"] {
        if keys.contains_key(key) {
            bail!("{}: sets {}; {}", path.display(), repr(key), injected(key));
        }
    }
    let text = format!(
        "name = {}\n{header}\ndeveloper_instructions = \"\"\"\n{}\"\"\"\n",
        basic_string(name),
        multiline_basic_string(body),
    );
    let agent: toml::Table = text.parse().map_err(|error| {
        anyhow!(
            "{}: assembled agent is invalid TOML: {error}",
            path.display()
        )
    })?;
    for required in ["name", "description", "developer_instructions"] {
        if !agent.get(required).is_some_and(truthy) {
            bail!("{}: lacks {}", path.display(), repr(required));
        }
    }
    ensure!(
        agent["name"].as_str() == Some(name),
        "{}: name {} differs from directory",
        path.display(),
        repr(&agent["name"].to_string())
    );
    ensure!(
        agent["developer_instructions"].as_str() == Some(body),
        "{}: developer_instructions does not reproduce {INSTRUCTIONS}",
        path.display()
    );
    Ok(text)
}

fn injected(key: &str) -> &'static str {
    if key == "name" {
        "the directory name is used"
    } else {
        "instructions.md supplies it"
    }
}

/// Python truthiness, as the original packager tested required keys.
fn truthy(value: &toml::Value) -> bool {
    match value {
        toml::Value::String(text) => !text.is_empty(),
        toml::Value::Integer(number) => *number != 0,
        toml::Value::Float(number) => *number != 0.0,
        toml::Value::Boolean(flag) => *flag,
        toml::Value::Array(items) => !items.is_empty(),
        toml::Value::Table(table) => !table.is_empty(),
        toml::Value::Datetime(_) => true,
    }
}

fn escape(c: char, out: &mut String) -> bool {
    match c {
        '\\' => out.push_str("\\\\"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        '\r' => out.push_str("\\r"),
        c if (c < ' ' && c != '\t' && c != '\n') || c == '\u{7f}' => {
            out.push_str(&format!("\\u{:04X}", c as u32));
        }
        _ => return false,
    }
    true
}

fn basic_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c if escape(c, &mut out) => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Escape text for a TOML multi-line basic string, keeping its lines readable.
/// A quote is escaped when another quote or the closing delimiter follows it,
/// so no run of three quotes can end the string early.
fn multiline_basic_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek().is_none_or(|next| *next == '"') {
                out.push_str("\\\"");
            } else {
                out.push('"');
            }
        } else if !escape(c, &mut out) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_strings_round_trip_quotes_backslashes_and_controls() {
        for body in [
            "plain\n",
            "say \"\"\"hi\"\"\" now\n",
            "ends with quote\"",
            "two\"\"",
            "back\\slash \\n \\\nline\n",
            "tab\tcr\r\nbell\u{7}del\u{7f}\n",
            "\nleading newline\n",
        ] {
            let text = codex_agent(
                "agent",
                Path::new("codex.toml"),
                "description = \"d\"\n",
                body,
            )
            .unwrap_or_else(|error| panic!("{body:?}: {error:#}"));
            let parsed: toml::Table = text.parse().unwrap();
            assert_eq!(parsed["developer_instructions"].as_str(), Some(body));
        }
    }

    #[test]
    fn codex_agent_layout_matches_hand_written_files() {
        let text = codex_agent(
            "lean-grind",
            Path::new("codex.toml"),
            "description = \"Fast.\"\nmodel = \"m\"\n",
            "Use `leanck`.\n\n## Workflow\n",
        )
        .unwrap();
        assert_eq!(
            text,
            "name = \"lean-grind\"\ndescription = \"Fast.\"\nmodel = \"m\"\n\n\
             developer_instructions = \"\"\"\nUse `leanck`.\n\n## Workflow\n\"\"\"\n"
        );
    }

    #[test]
    fn codex_headers_must_not_set_injected_keys() {
        let path = Path::new("a/codex.toml");
        let error =
            codex_agent("a", path, "name = \"a\"\ndescription = \"d\"\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/codex.toml: sets 'name'; the directory name is used"
        );
        let error = codex_agent(
            "a",
            path,
            "description = \"d\"\ndeveloper_instructions = \"x\"\n",
            "b\n",
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/codex.toml: sets 'developer_instructions'; instructions.md supplies it"
        );
        let error = codex_agent("a", path, "model = \"m\"\n", "b\n").unwrap_err();
        assert_eq!(error.to_string(), "a/codex.toml: lacks 'description'");
        let error = codex_agent("a", path, "description = \"d\"\n[table]\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/codex.toml: lacks 'developer_instructions'"
        );
        let error = codex_agent("a", path, "description = \n", "b\n").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("a/codex.toml: invalid TOML: "),
            "{error}"
        );
    }

    #[test]
    fn claude_agent_layout_and_validation() {
        let path = Path::new("a/claude.yaml");
        let text = claude_agent(
            "a",
            path,
            "model: opus\ndescription: Does things.",
            "Body\n",
        )
        .unwrap();
        assert_eq!(
            text,
            "---\nname: a\nmodel: opus\ndescription: Does things.\n---\n\nBody\n"
        );
        let error = claude_agent("a", path, "name: a\ndescription: d\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/claude.yaml: sets 'name'; the directory name is used"
        );
        let error = claude_agent("a", path, "model: opus\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/claude.yaml: frontmatter lacks 'description'"
        );
        let error = claude_agent("a", path, "description: d\n---\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/claude.yaml: frontmatter keys must not contain a '---' line"
        );
        let error = claude_agent("a", path, "description: d\nbad line\n", "b\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "a/claude.yaml: malformed frontmatter line 'bad line'"
        );
    }
}
