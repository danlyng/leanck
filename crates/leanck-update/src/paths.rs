//! Path input normalization shared by the installer and the updater.
//!
//! Every user- or environment-supplied path is made absolute and lexically
//! normalized exactly once, at input. `~` and `~/…` expand against `HOME`,
//! relative paths resolve against the working directory, and `.`/`..`
//! components are folded away, so recorded paths never contain `..` and every
//! installation stays updatable and uninstallable.
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// Reject multiline and control-character input before it reaches state files or profiles.
pub fn valid_text(value: &str) -> Result<()> {
    if value.contains('\n') {
        bail!("values must be single-line text");
    }
    if value.chars().any(char::is_control) {
        bail!("values must be single-line text without control characters");
    }
    Ok(())
}

/// Make `value` absolute and lexically normalized.
pub fn absolute(value: &str, home: &Path, cwd: &Path) -> Result<PathBuf> {
    valid_text(value)?;
    let path = if value == "~" {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        cwd.join(value)
    };
    Ok(normalize(&path))
}

/// Fold `.` and `..` components without touching the filesystem.
/// `..` never climbs above the root.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(name) => out.push(name),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Abbreviate `HOME` as `~` for display.
pub fn display(path: &Path, home: &Path) -> String {
    if path == home {
        return "~".into();
    }
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_become_absolute_and_lose_parent_components() {
        let home = Path::new("/home/user");
        let cwd = Path::new("/work/dir");
        for (input, expected) in [
            ("~", "/home/user"),
            ("~/bin", "/home/user/bin"),
            ("/opt/./leanck/../bin/", "/opt/bin"),
            ("bin", "/work/dir/bin"),
            ("../bin", "/work/bin"),
            ("../../../../x", "/x"),
            ("./a/./b/..", "/work/dir/a"),
            ("~user", "/work/dir/~user"),
        ] {
            assert_eq!(
                absolute(input, home, cwd).unwrap(),
                Path::new(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn control_characters_and_newlines_are_rejected_with_distinct_messages() {
        let home = Path::new("/h");
        assert_eq!(
            absolute("bad\npath", home, home).unwrap_err().to_string(),
            "values must be single-line text"
        );
        assert_eq!(
            absolute("bad\tpath", home, home).unwrap_err().to_string(),
            "values must be single-line text without control characters"
        );
    }

    #[test]
    fn display_abbreviates_only_whole_home_components() {
        let home = Path::new("/home/user");
        assert_eq!(display(Path::new("/home/user"), home), "~");
        assert_eq!(
            display(Path::new("/home/user/.local/bin"), home),
            "~/.local/bin"
        );
        assert_eq!(display(Path::new("/home/username"), home), "/home/username");
    }
}
