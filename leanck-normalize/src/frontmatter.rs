//! Normalizes the top-of-file "frontmatter" of a Lean source file to a fixed
//! shape:
//!
//! ```text
//! /-
//! Copyright (c) <year> Daniel Lyng. All rights reserved.
//! Released under Apache 2.0 license as described in the file LICENSE.
//! Authors: Daniel Lyng
//! -/
//! module
//!
//! <IMPORTS>
//!
//! <MODULE DOCSTRING>
//!
//! @[expose] public <noncomputable?> section
//! ```
//!
//! Three transformations are applied, all conservative — they only touch the
//! structural lines they recognize and leave everything else in place:
//!
//! 1. Prepend the copyright header when the file does not already begin with a
//!    `/- ... -/` block comment.
//! 2. Alphabetize the import block, with `public import`s sorted ahead of plain
//!    `import`s.
//! 3. Relocate the `@[expose] public … section` line to just after the module
//!    docstring when it currently appears before it.

use std::time::{SystemTime, UNIX_EPOCH};

const SUFFIX: &str = "-/";

/// The calendar year in UTC, derived without pulling in a date dependency.
pub fn current_year() -> i32 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    year_from_epoch_days((secs / 86_400) as i64)
}

fn year_from_epoch_days(mut days: i64) -> i32 {
    let mut year = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let len = if leap { 366 } else { 365 };
        if days < len {
            return year;
        }
        days -= len;
        year += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub enabled: bool,
    pub header: bool,
    pub sort_imports: bool,
    pub section_after_docstring: bool,
    pub namespace_after_section: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: true,
            header: true,
            sort_imports: true,
            section_after_docstring: true,
            namespace_after_section: true,
        }
    }
}

/// Normalize the frontmatter of `source`. Files that show no sign of being a
/// Lean module (no `module`, `import`, or `@[expose] … section` line) are
/// returned unchanged, so comment snippets and partial input are never touched.
#[allow(dead_code)]
pub fn normalize(source: &str, year: i32) -> String {
    normalize_with_options(source, year, Options::default())
}

pub fn normalize_with_options(source: &str, year: i32, options: Options) -> String {
    if !options.enabled || !is_module_file(source) {
        return source.to_string();
    }

    let mut lines: Vec<String> = source.lines().map(|s| s.to_string()).collect();
    if options.header {
        ensure_header(&mut lines, year);
    }
    if options.sort_imports {
        sort_imports(&mut lines);
    }
    if options.section_after_docstring {
        move_section_after_docstring(&mut lines);
    }
    if options.namespace_after_section {
        move_namespace_after_section(&mut lines);
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Line classification
// ---------------------------------------------------------------------------

fn is_import(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("import ") || t.starts_with("public import ")
}

fn is_public_import(line: &str) -> bool {
    line.trim_start().starts_with("public import ")
}

/// Matches `@[expose] public section` and `@[expose] public noncomputable
/// section` (the `noncomputable` keyword is carried along verbatim).
fn is_section(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("@[expose]") && t.contains("public") && t.ends_with("section")
}

fn is_namespace(line: &str) -> bool {
    line.trim().starts_with("namespace ")
}

fn is_module_file(source: &str) -> bool {
    source.lines().any(|l| {
        let t = l.trim();
        t == "module" || is_import(l) || is_section(l)
    })
}

// ---------------------------------------------------------------------------
// 1. Copyright header
// ---------------------------------------------------------------------------

fn ensure_header(lines: &mut Vec<String>, year: i32) {
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return;
    };

    // A leading `/- … -/` block (but not `/--` or `/-!`) is assumed to be an
    // existing copyright header and is left untouched.
    let t = lines[first].trim_start();
    if t.starts_with("/-") && !t.starts_with("/--") && !t.starts_with("/-!") {
        return;
    }

    // Drop any blank lines preceding the first content, then prepend.
    lines.drain(0..first);
    let header = [
        "/-".to_string(),
        format!("Copyright (c) {year} Daniel Lyng. All rights reserved."),
        "Released under Apache 2.0 license as described in the file LICENSE.".to_string(),
        "Authors: Daniel Lyng".to_string(),
        "-/".to_string(),
    ];
    for (i, line) in header.into_iter().enumerate() {
        lines.insert(i, line);
    }
}

// ---------------------------------------------------------------------------
// 2. Import sorting
// ---------------------------------------------------------------------------

fn sort_imports(lines: &mut Vec<String>) {
    let Some(start) = lines.iter().position(|l| is_import(l)) else {
        return;
    };

    // Extend over the run of import-or-blank lines, then trim back to the last
    // actual import so the blank line separating imports from the docstring is
    // preserved.
    let mut end = start;
    while end < lines.len() && (is_import(&lines[end]) || lines[end].trim().is_empty()) {
        end += 1;
    }
    let last = (start..end).rev().find(|&i| is_import(&lines[i])).unwrap();

    let imports: Vec<&String> = lines[start..=last]
        .iter()
        .filter(|l| is_import(l))
        .collect();
    if imports.len() < 2 {
        return;
    }

    let (mut public, mut plain): (Vec<String>, Vec<String>) = imports
        .iter()
        .map(|l| l.trim().to_string())
        .partition(|l| is_public_import(l));
    public.sort();
    plain.sort();
    public.extend(plain);

    lines.splice(start..=last, public);
}

// ---------------------------------------------------------------------------
// 3. Section relocation
// ---------------------------------------------------------------------------

fn move_section_after_docstring(lines: &mut Vec<String>) {
    let Some(section) = lines.iter().position(|l| is_section(l)) else {
        return;
    };
    let Some(doc_start) = lines.iter().position(|l| l.trim_start().starts_with("/-!")) else {
        return;
    };

    // Only relocate when the section currently precedes the docstring.
    if section >= doc_start {
        return;
    }

    let Some(doc_end) = (doc_start..lines.len()).find(|&i| lines[i].contains(SUFFIX)) else {
        return;
    };

    let section_line = lines.remove(section);
    // Removing the earlier section line shifts the docstring end up by one.
    let doc_end = doc_end - 1;
    lines.insert(doc_end + 1, String::new());
    lines.insert(doc_end + 2, section_line);
}

/// Ensure the `namespace …` declaration follows the section. This runs after
/// [`move_section_after_docstring`], so the docstring already precedes the
/// section; relocating the namespace to just after the section then yields the
/// canonical `docstring → section → namespace` order, whether the namespace
/// started between the docstring and section or before both (the inverse case
/// where the docstring was the last of the three).
///
/// Only the *first* namespace is considered: in a well-formed file a body-level
/// namespace always follows the section, so a namespace appearing *before* the
/// section can only be the frontmatter one.
fn move_namespace_after_section(lines: &mut Vec<String>) {
    let Some(section) = lines.iter().position(|l| is_section(l)) else {
        return;
    };
    let Some(ns) = lines.iter().position(|l| is_namespace(l)) else {
        return;
    };
    if ns >= section {
        return;
    }

    let ns_line = lines.remove(ns);
    // Removing the earlier namespace line shifts the section up by one.
    let section = section - 1;
    lines.insert(section + 1, String::new());
    lines.insert(section + 2, ns_line);
}

#[cfg(test)]
mod tests {
    use super::*;

    const YEAR: i32 = 2026;

    #[test]
    fn snippets_without_module_markers_are_untouched() {
        let src = "/-- a doc comment -/\ndef x := 1\n";
        assert_eq!(normalize(src, YEAR), src);
    }

    #[test]
    fn header_is_added_when_missing() {
        let src = "module\n\nimport Foo\n";
        let out = normalize(src, YEAR);
        assert_eq!(
            out,
            "/-\n\
             Copyright (c) 2026 Daniel Lyng. All rights reserved.\n\
             Released under Apache 2.0 license as described in the file LICENSE.\n\
             Authors: Daniel Lyng\n\
             -/\n\
             module\n\
             \n\
             import Foo"
        );
    }

    #[test]
    fn existing_header_is_left_untouched() {
        let src = "/-\nCopyright (c) 1999 Someone Else.\n-/\nmodule\n\nimport Foo\n";
        let out = normalize(src, YEAR);
        assert!(out.starts_with("/-\nCopyright (c) 1999 Someone Else.\n-/"));
    }

    #[test]
    fn imports_are_alphabetized_with_public_first() {
        let src = "/-\nx\n-/\nmodule\n\nimport Zebra\npublic import Apple\nimport Mango\npublic import Cat\n";
        let out = normalize(src, YEAR);
        let imports: Vec<&str> = out.lines().filter(|l| is_import(l)).collect();
        assert_eq!(
            imports,
            vec![
                "public import Apple",
                "public import Cat",
                "import Mango",
                "import Zebra",
            ]
        );
    }

    #[test]
    fn single_import_is_left_as_is() {
        let src = "/-\nx\n-/\nmodule\n\nimport OnlyOne\n";
        let out = normalize(src, YEAR);
        assert!(out.contains("import OnlyOne"));
    }

    #[test]
    fn section_is_moved_after_docstring() {
        let src =
            "/-\nx\n-/\nmodule\n\nimport Foo\n\n@[expose] public section\n\n/-!\n# Title\n-/\n";
        let out = normalize(src, YEAR);
        let section = out
            .lines()
            .position(|l| l.trim() == "@[expose] public section")
            .unwrap();
        let title = out.lines().position(|l| l.contains("# Title")).unwrap();
        // section now follows the docstring body.
        assert!(section > title);
    }

    #[test]
    fn noncomputable_section_is_carried_along() {
        let src =
            "/-\nx\n-/\nmodule\n\n@[expose] public noncomputable section\n\n/-!\n# Title\n-/\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let section = lines
            .iter()
            .position(|l| l.trim() == "@[expose] public noncomputable section")
            .unwrap();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        assert!(section > title);
    }

    #[test]
    fn section_already_after_docstring_stays() {
        let src = "/-\nx\n-/\nmodule\n\n/-!\n# Title\n-/\n\n@[expose] public section\n";
        let out = normalize(src, YEAR);
        // Idempotent: running again yields the same text.
        assert_eq!(normalize(&out, YEAR), out);
    }

    #[test]
    fn namespace_between_docstring_and_section_moves_after_section() {
        let src =
            "/-\nx\n-/\nmodule\n\n/-!\n# Title\n-/\n\nnamespace Foo\n\n@[expose] public section\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        assert!(title < section, "section after docstring");
        assert!(section < ns, "namespace after section");
    }

    #[test]
    fn namespace_moves_when_section_precedes_docstring() {
        // section before docstring, namespace already after: section is moved
        // after the docstring, and the namespace ends up after the section.
        let src =
            "/-\nx\n-/\nmodule\n\n@[expose] public section\n\n/-!\n# Title\n-/\n\nnamespace Foo\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        assert!(title < section && section < ns);
        // and stable under re-running.
        assert_eq!(normalize(&out, YEAR), out);
    }

    #[test]
    fn inverse_case_docstring_last_is_fully_reordered() {
        // Original order: section, namespace, docstring, body. Must become
        // docstring, section, namespace, body.
        let src = "/-\nx\n-/\nmodule\n\n@[expose] public section\n\nnamespace Foo\n\n/-!\n# Title\n-/\n\ndef foo := 1\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        let body = lines.iter().position(|l| l.contains("def foo")).unwrap();
        assert!(
            title < section && section < ns && ns < body,
            "expected docstring < section < namespace < body, got {title} {section} {ns} {body}"
        );
        // Idempotent.
        assert_eq!(normalize(&out, YEAR), out);
    }

    #[test]
    fn year_conversion_is_correct() {
        // 2026-05-28 sample timestamp: 1748390400 s ≈ day 20235.
        assert_eq!(year_from_epoch_days(0), 1970);
        assert_eq!(year_from_epoch_days(365), 1971); // 1970 was not a leap year
        assert_eq!(year_from_epoch_days(20_235), 2025);
    }
}
