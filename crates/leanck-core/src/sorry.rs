//! Text-only sorry detection shared by the daemon and daemon-free frontends.
//!
//! The scan reports tokens that elaborate to `sorryAx` and notes constructs
//! that can hide a sorry from Lean's own warning or replace it outright. It
//! cannot see sorries produced by macros that never spell one of these tokens.

use std::path::{Path, PathBuf};

use crate::api::{SorryDirectory, SorryEntry, SorryFile, SorryKind, SorryResult};
use crate::scan::{Token, tokens};

/// Directories never searched by recursive scans.
pub const SKIPPED_DIRECTORIES: &[&str] = &[".git", ".lake", "build", "target", "node_modules"];
pub const TEXT_ONLY_DIRECTORY_NOTE: &str =
    "text-only directory scan; declaration diagnostics are not included";
pub const TEXT_ONLY_FILE_NOTE: &str = "text-only scan; declaration diagnostics are not included";

/// Constructs that defeat or bypass sorry detection, reported as notes.
#[derive(Clone, Copy)]
enum Flag {
    Axiom,
    WarnSorry,
    GuardMsgs,
    Native,
    SkipKernel,
}

const FLAGS: [(Flag, &str, &str); 5] = [
    (Flag::Axiom, "`axiom`", "axioms are not reported as sorries"),
    (
        Flag::WarnSorry,
        "`warn.sorry`",
        "can disable Lean's 'declaration uses sorry' warning",
    ),
    (
        Flag::GuardMsgs,
        "`#guard_msgs`",
        "can absorb Lean's 'declaration uses sorry' warning",
    ),
    (
        Flag::Native,
        "native evaluation",
        "trusts compiled code through `Lean.ofReduceBool`",
    ),
    (
        Flag::SkipKernel,
        "`debug.skipKernelTC`",
        "skips kernel checking",
    ),
];

#[derive(Debug, Default)]
pub struct TextScan {
    pub sorries: Vec<SorryEntry>,
    pub notes: Vec<String>,
}

/// `sorry`, `admit`, and `stop` are keywords, so a bare token is always the
/// term or tactic; `sorryAx` is an ordinary constant and may be qualified.
fn is_sorry(token: &Token) -> bool {
    matches!(token.text.as_str(), "sorry" | "admit" | "stop")
        || token.text.rsplit('.').next() == Some("sorryAx")
}

fn adjacent(tokens: &[Token], i: usize, first: &str, second: &str) -> bool {
    tokens[i].text == first
        && tokens
            .get(i + 1)
            .is_some_and(|next| next.text == second && next.range.start == tokens[i].range.end)
}

fn flag(tokens: &[Token], i: usize) -> Option<Flag> {
    let text = tokens[i].text.as_str();
    let last = text.rsplit('.').next().unwrap_or(text);
    match text {
        "axiom" => Some(Flag::Axiom),
        "warn.sorry" => Some(Flag::WarnSorry),
        "debug.skipKernelTC" => Some(Flag::SkipKernel),
        "native_decide" => Some(Flag::Native),
        _ if matches!(last, "ofReduceBool" | "ofReduceNat" | "trustCompiler") => Some(Flag::Native),
        _ if adjacent(tokens, i, "#", "guard_msgs") => Some(Flag::GuardMsgs),
        _ if adjacent(tokens, i, "+", "native") => Some(Flag::Native),
        _ => None,
    }
}

pub fn scan_source(source: &str) -> TextScan {
    let tokens = tokens(source);
    let mut sorries = Vec::new();
    let mut lines: [Vec<u32>; FLAGS.len()] = Default::default();
    for (i, token) in tokens.iter().enumerate() {
        if is_sorry(token) {
            sorries.push(SorryEntry {
                range: token.range,
                kind: SorryKind::Tactic,
            });
        } else if let Some(flag) = flag(&tokens, i) {
            let line = token.range.start.line + 1;
            let seen = &mut lines[flag as usize];
            if seen.last() != Some(&line) {
                seen.push(line);
            }
        }
    }
    let notes = FLAGS
        .iter()
        .zip(lines)
        .filter(|(_, lines)| !lines.is_empty())
        .map(|((_, label, reason), lines)| {
            let at = lines
                .iter()
                .map(|line| format!("L{line}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("red flag: {label} at {at}; {reason}")
        })
        .collect();
    TextScan { sorries, notes }
}

/// Text-scan a canonical file or directory. `check` runs between filesystem
/// steps so callers can stop a long directory walk.
pub fn scan_path<E>(
    path: &Path,
    mut check: impl FnMut() -> Result<(), E>,
    invalid: impl Fn(String) -> E,
) -> Result<SorryResult, E> {
    let uri = |path: &Path| crate::document::path_to_uri(path).map_err(|e| invalid(e.to_string()));
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| invalid(format!("{}: {e}", path.display())))
    };
    let meta =
        std::fs::metadata(path).map_err(|_| invalid(format!("not found: {}", path.display())))?;
    if !meta.is_dir() {
        let scan = scan_source(&read(path)?);
        let mut notes = vec![TEXT_ONLY_FILE_NOTE.to_string()];
        notes.extend(scan.notes);
        return Ok(SorryResult::File(SorryFile {
            uri: uri(path)?,
            sorries: scan.sorries,
            notes,
        }));
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for leaf in lean_files(path, &mut check, &invalid)? {
        check()?;
        let scan = scan_source(&read(&leaf)?);
        if scan.sorries.is_empty() && scan.notes.is_empty() {
            continue;
        }
        total +=
            u64::try_from(scan.sorries.len()).map_err(|_| invalid("too many sorries".into()))?;
        files.push(SorryFile {
            uri: uri(&leaf)?,
            sorries: scan.sorries,
            notes: scan.notes,
        });
    }
    Ok(SorryResult::Directory(SorryDirectory {
        uri: uri(path)?,
        total,
        files,
        notes: vec![TEXT_ONLY_DIRECTORY_NOTE.into()],
    }))
}

/// Collect all regular `.lean` files below `root` in path order, excluding
/// [`SKIPPED_DIRECTORIES`] and symlinks. Includes files with no lexical sorries
/// so elaboration-based callers can also find macro-generated sorries.
/// `check` runs between filesystem steps to support cancellation and deadlines.
pub fn lean_files<E>(
    root: &Path,
    check: &mut impl FnMut() -> Result<(), E>,
    invalid: &impl Fn(String) -> E,
) -> Result<Vec<PathBuf>, E> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        check()?;
        let entries =
            std::fs::read_dir(&dir).map_err(|e| invalid(format!("{}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| invalid(e.to_string()))?;
            check()?;
            let path = entry.path();
            let ft = entry
                .file_type()
                .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
            if ft.is_dir() {
                let name = entry.file_name();
                if SKIPPED_DIRECTORIES
                    .iter()
                    .any(|d| std::ffi::OsStr::new(d) == name)
                {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() && path.extension().is_some_and(|e| e == "lean") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(scan: &TextScan) -> Vec<u32> {
        scan.sorries.iter().map(|s| s.range.start.line).collect()
    }

    #[test]
    fn reports_sorry_synonyms_outside_comments_and_strings() {
        let scan = scan_source(
            "theorem a : P := by admit\ntheorem b : P := by\n  stop\n  simp\ntheorem c : P := sorryAx _ false\ntheorem d : P := @_root_.sorryAx _ true\n-- admit\ndef s := \"stop\"\n",
        );
        assert_eq!(lines(&scan), [0, 2, 4, 5]);
        assert!(scan.notes.is_empty());
    }

    #[test]
    fn ignores_identifiers_that_only_contain_synonyms() {
        let scan = scan_source("def admitted := 1\ndef Foo.stop := 2\ndef mySorryAx := 3\n«stop»");
        assert!(scan.sorries.is_empty());
    }

    #[test]
    fn notes_constructs_that_hide_or_replace_sorry() {
        let scan = scan_source(
            "axiom cheat : False\nset_option warn.sorry false in\n#guard_msgs in\ntheorem t : 2 = 2 := by native_decide\nexample : 1 = 1 := by decide +native\naxiom other : False\nset_option debug.skipKernelTC true\n-- axiom hidden : False\n",
        );
        assert!(scan.sorries.is_empty());
        assert_eq!(
            scan.notes,
            [
                "red flag: `axiom` at L1, L6; axioms are not reported as sorries",
                "red flag: `warn.sorry` at L2; can disable Lean's 'declaration uses sorry' warning",
                "red flag: `#guard_msgs` at L3; can absorb Lean's 'declaration uses sorry' warning",
                "red flag: native evaluation at L4, L5; trusts compiled code through `Lean.ofReduceBool`",
                "red flag: `debug.skipKernelTC` at L7; skips kernel checking",
            ]
        );
    }

    #[test]
    fn separated_hash_is_not_guard_msgs() {
        assert!(scan_source("# guard_msgs").notes.is_empty());
    }

    #[test]
    fn scans_files_and_directories_without_skipped_directories() {
        let root = std::env::temp_dir().join(format!("leanck-sorry-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::create_dir_all(root.join(".lake")).unwrap();
        std::fs::write(root.join("A/B.lean"), "example : True := by admit\n").unwrap();
        std::fs::write(root.join("Clean.lean"), "example : True := trivial\n").unwrap();
        std::fs::write(root.join("Flag.lean"), "axiom x : False\n").unwrap();
        std::fs::write(root.join(".lake/Dep.lean"), "example : True := sorry\n").unwrap();
        let root = root.canonicalize().unwrap();
        let invalid = |message: String| message;

        let SorryResult::Directory(directory) = scan_path(&root, || Ok(()), invalid).unwrap()
        else {
            panic!("expected directory");
        };
        assert_eq!(directory.total, 1);
        assert_eq!(directory.notes, [TEXT_ONLY_DIRECTORY_NOTE]);
        let names: Vec<_> = directory
            .files
            .iter()
            .map(|file| file.uri.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["B.lean", "Flag.lean"]);
        assert_eq!(directory.files[1].notes.len(), 1);

        let SorryResult::File(file) =
            scan_path(&root.join("A/B.lean"), || Ok(()), invalid).unwrap()
        else {
            panic!("expected file");
        };
        assert_eq!(file.sorries.len(), 1);
        assert_eq!(file.notes, [TEXT_ONLY_FILE_NOTE]);

        assert!(scan_path(&root.join("Missing.lean"), || Ok(()), invalid).is_err());
        assert_eq!(
            scan_path(&root, || Err("stopped".to_string()), invalid).unwrap_err(),
            "stopped"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
