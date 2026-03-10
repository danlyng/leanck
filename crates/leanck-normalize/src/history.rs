//! Bounded, read-only Git attribution. No identity is guessed when history is absent.
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "command")]
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
    pub email: String,
    pub years: BTreeSet<i32>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileHistory {
    pub contributors: Vec<Contributor>,
}
impl FileHistory {
    pub fn authors(&self) -> String {
        self.contributors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
    }
    pub fn years(&self) -> String {
        year_range(
            self.contributors
                .iter()
                .flat_map(|c| c.years.iter().copied()),
        )
    }
    pub fn copyright(&self) -> String {
        if self.contributors.is_empty() {
            String::new()
        } else {
            format!("Copyright (c) {} {}.", self.years(), self.authors())
        }
    }
    /// Apply the same exclusions to explicit history as to history read from Git.
    pub fn excluding(&self, ignore_authors: &[String]) -> Self {
        Self {
            contributors: self
                .contributors
                .iter()
                .filter(|author| !ignored(&author.name, &author.email, ignore_authors))
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn map_names(mut self, author_map: &BTreeMap<String, String>) -> Self {
        for author in &mut self.contributors {
            if let Some(display) = author_map.get(&author.name) {
                author.name.clone_from(display);
            }
        }
        self
    }
}

fn ignored(name: &str, email: &str, ignore_authors: &[String]) -> bool {
    let signature = format!("{name} <{email}>");
    ignore_authors.iter().any(|identity| {
        let identity = identity.trim();
        identity.eq_ignore_ascii_case(name)
            || identity.eq_ignore_ascii_case(email)
            || identity.eq_ignore_ascii_case(&signature)
    })
}
fn year_range(years: impl Iterator<Item = i32>) -> String {
    let years: BTreeSet<_> = years.collect();
    match (years.first(), years.last()) {
        (Some(a), Some(b)) if a != b => format!("{a}–{b}"),
        (Some(a), _) => a.to_string(),
        _ => String::new(),
    }
}

#[cfg(feature = "command")]
pub(crate) fn read(
    path: &Path,
    rules: &leanck_config::FrontmatterRules,
) -> Result<FileHistory, String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "log.showSignature=false",
            "-C",
        ])
        .arg(path.parent().ok_or("file has no parent")?)
        .args(["log", "--no-ext-diff", "--use-mailmap"]);
    if rules.follow_renames {
        command.arg("--follow");
    }
    command
        .args(["--format=%an%x00%ae%x00%aN%x00%aE%x00%aI", "-z", "--"])
        .arg(path.file_name().ok_or("file has no name")?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let deadline = Instant::now() + Duration::from_millis(rules.history_timeout_ms);
    let mut shallow = Command::new("git");
    shallow
        .arg("-C")
        .arg(path.parent().unwrap())
        .args(["rev-parse", "--is-shallow-repository"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if capture(shallow, deadline)?.starts_with(b"true") {
        return Err("shallow Git history cannot establish complete file attribution".into());
    }
    let bytes = capture(command, deadline)?;
    parse(&bytes, &rules.ignore_authors)
}
#[cfg(feature = "command")]
fn capture(mut command: Command, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot read Git history: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break Err("Git history timed out".to_owned()),
            Err(e) => break Err(format!("Git history failed: {e}")),
        }
    };
    if status.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let bytes = reader
        .join()
        .map_err(|_| "Git history reader failed")?
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Git history exceeds the 4 MiB attribution limit".into());
    }
    if !status?.success() {
        return Err("Git history unavailable (not a repository, or no readable history)".into());
    }
    Ok(bytes)
}
#[cfg(any(feature = "command", test))]
fn parse(bytes: &[u8], ignore_authors: &[String]) -> Result<FileHistory, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Git attribution is not UTF-8")?;
    let mut fields = text.split_terminator('\0');
    let mut authors: BTreeMap<(String, String), BTreeSet<i32>> = BTreeMap::new();
    let mut found_commit = false;
    while let Some(raw_name) = fields.next() {
        found_commit = true;
        let raw_email = fields.next().ok_or("incomplete Git attribution")?;
        let name = fields.next().ok_or("incomplete Git attribution")?;
        let email = fields.next().ok_or("incomplete Git attribution")?;
        let date = fields.next().ok_or("incomplete Git attribution")?;
        // Never let a mailmap hide a known agent or a configured exclusion.
        if ignored(raw_name, raw_email, ignore_authors) || ignored(name, email, ignore_authors) {
            continue;
        }
        let year: i32 = date
            .get(..4)
            .ok_or("invalid Git author date")?
            .parse()
            .map_err(|_| "invalid Git author year")?;
        if name.contains(['\n', '\r'])
            || name.contains("-/")
            || name.contains("/-")
            || !(1..=9999).contains(&year)
        {
            return Err("unsafe or invalid Git attribution".into());
        }
        authors
            .entry((name.to_owned(), email.to_owned()))
            .or_default()
            .insert(year);
    }
    if authors.is_empty() {
        return Err(if found_commit {
            "all Git authors for this file were excluded by frontmatter.ignore_authors"
        } else {
            "no commits found for this file (untracked or new file)"
        }
        .into());
    }
    Ok(FileHistory {
        contributors: authors
            .into_iter()
            .map(|((name, email), years)| Contributor { name, email, years })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions_match_exact_names_emails_and_signatures() {
        for identity in [
            "Agent",
            "agent@example.test",
            "Agent <agent@example.test>",
            " AGENT@EXAMPLE.TEST ",
        ] {
            assert!(ignored("Agent", "agent@example.test", &[identity.into()]));
        }
        for identity in [
            "Age",
            "@example.test",
            "*@example.test",
            "Agent <other@example.test>",
        ] {
            assert!(!ignored("Agent", "agent@example.test", &[identity.into()]));
        }
        // Aider can append its name to a human author while keeping the human email.
        assert!(!ignored(
            "Human (aider)",
            "human@example.test",
            &leanck_config::FrontmatterRules::default().ignore_authors
        ));
    }

    #[test]
    fn exclusions_apply_before_and_after_mailmaps() {
        let bytes = b"Agent\0agent@example.test\0Human\0human@example.test\x002026-01-01\0";
        for identity in ["Agent", "agent@example.test", "Human", "human@example.test"] {
            assert!(
                parse(bytes, &[identity.into()])
                    .unwrap_err()
                    .contains("all Git authors")
            );
        }
        let parsed = parse(bytes, &[]).unwrap();
        assert_eq!(parsed.authors(), "Human");
        assert!(parse(b"", &[]).unwrap_err().contains("no commits"));
        assert!(parse(b"Agent\0", &[]).unwrap_err().contains("incomplete"));
    }

    #[test]
    fn a_single_notice_deduplicates_names_and_spans_all_retained_years() {
        let history = FileHistory {
            contributors: vec![
                Contributor {
                    name: "Alice".into(),
                    email: "old@example.test".into(),
                    years: [2000].into(),
                },
                Contributor {
                    name: "Alice".into(),
                    email: "new@example.test".into(),
                    years: [2020].into(),
                },
                Contributor {
                    name: "Bob".into(),
                    email: "bob@example.test".into(),
                    years: [2010].into(),
                },
            ],
        };
        assert_eq!(history.authors(), "Alice, Bob");
        assert_eq!(history.copyright(), "Copyright (c) 2000–2020 Alice, Bob.");
        assert_eq!(
            history.excluding(&["Alice".into()]).copyright(),
            "Copyright (c) 2010 Bob."
        );
        assert_eq!(FileHistory::default().copyright(), "");
    }
}
