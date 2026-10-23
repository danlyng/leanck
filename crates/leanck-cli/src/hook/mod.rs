//! Quiet, synchronous PostToolUse hooks, dispatched before ordinary CLI startup.

mod push_not;
mod toolchain;

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use leanck_config::ConfigArgs;
use serde_json::Value;

/// Hooks must never block an edit with an error exit or diagnostic output.
pub fn main(args: &[OsString]) -> i32 {
    if !args.first().is_some_and(|arg| arg == "post-edit") {
        return 0;
    }
    let Ok(payload) = serde_json::from_reader::<_, Value>(std::io::stdin().lock()) else {
        return 0;
    };
    let paths = edited_paths(&payload);
    let mut configs = HashMap::new();
    let mut toolchains = HashMap::new();
    let mut rewritten = Vec::new();
    let mut count = 0;
    for file in paths {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        if !source.contains("push_neg") {
            continue;
        }
        let Ok(absolute) = std::path::absolute(&file) else {
            continue;
        };
        let Some(directory) = absolute.parent() else {
            continue;
        };
        let enabled = configs.entry(directory.to_path_buf()).or_insert_with(|| {
            ConfigArgs::default()
                .load(&file)
                .is_ok_and(|loaded| loaded.settings.hooks.push_not)
        });
        if !*enabled {
            continue;
        }
        let supported = toolchains
            .entry(directory.to_path_buf())
            .or_insert_with(|| project_supports_push_not(directory));
        if !*supported {
            continue;
        }
        let Some((updated, occurrences)) = push_not::rewrite(&source) else {
            continue;
        };
        if fs::write(&file, updated).is_err() {
            continue;
        }
        rewritten.push(file.display().to_string());
        count += occurrences;
    }
    if count > 0 {
        let plural = if count == 1 {
            "occurrence"
        } else {
            "occurrences"
        };
        let files = rewritten.join(", ");
        let message = format!(
            "Auto-rewrote {count} deprecated `push_neg` {plural} to `push Not` in {files}. Re-read the file before making further edits to it."
        );
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": message,
            }
        });
        let _ = writeln!(std::io::stdout().lock(), "{output}");
    }
    0
}

fn project_supports_push_not(directory: &Path) -> bool {
    for ancestor in directory.ancestors() {
        let path = ancestor.join("lean-toolchain");
        match path.try_exists() {
            Ok(false) => continue,
            Ok(true) => {
                return fs::read_to_string(path)
                    .is_ok_and(|contents| toolchain::supports_push_not(&contents));
            }
            Err(_) => return false,
        }
    }
    false
}

/// Claude Code supplies a file path; Codex supplies apply_patch text and a cwd.
fn edited_paths(payload: &Value) -> Vec<PathBuf> {
    let Some(input) = payload.get("tool_input") else {
        return Vec::new();
    };
    if let Some(file) = input
        .get("file_path")
        .and_then(Value::as_str)
        .filter(|file| !file.is_empty())
    {
        return if file.ends_with(".lean") {
            vec![PathBuf::from(file)]
        } else {
            Vec::new()
        };
    }
    let Some(patch) = input.get("command").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(cwd) = payload
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
    else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    patch
        .lines()
        .filter_map(|line| {
            ["*** Add File: ", "*** Update File: ", "*** Move to: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(str::trim_end)
        .filter(|path| path.ends_with(".lean"))
        .map(|path| cwd.join(path))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::edited_paths;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn extracts_claude_file_path() {
        assert_eq!(
            edited_paths(&json!({"tool_input": {"file_path": "X.lean"}})),
            vec![PathBuf::from("X.lean")]
        );
        assert!(edited_paths(&json!({"tool_input": {"file_path": "X.rs"}})).is_empty());
    }

    #[test]
    fn extracts_codex_patch_targets_in_order() {
        let payload = json!({
            "cwd": "/project",
            "tool_input": {
                "command": "*** Begin Patch\n*** Add File: A.lean\n*** Update File: B.lean\n*** Move to: C.lean  \n*** Update File: A.lean\n*** Add File: notes.md\n*** Delete File: D.lean\n*** End Patch"
            }
        });
        assert_eq!(
            edited_paths(&payload),
            ["/project/A.lean", "/project/B.lean", "/project/C.lean"].map(PathBuf::from)
        );
    }

    #[test]
    fn patch_paths_fall_back_to_process_cwd() {
        assert_eq!(
            edited_paths(&json!({"tool_input": {"command": "*** Update File: X.lean"}})),
            vec![std::env::current_dir().unwrap().join("X.lean")]
        );
    }

    #[test]
    fn ignores_payloads_without_edited_paths() {
        for payload in [
            json!(null),
            json!([]),
            json!({}),
            json!({"tool_input": null}),
            json!({"tool_input": {"command": 1}}),
            json!({"tool_input": {"command": "*** Add File: X.txt"}}),
        ] {
            assert!(edited_paths(&payload).is_empty(), "{payload}");
        }
    }
}
