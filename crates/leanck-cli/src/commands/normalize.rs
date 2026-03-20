//! Normalization command, file traversal, and stdin/stdout handling.
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use leanck_normalize::{DEFAULT_WIDTH, NormalizeWidth, Options, SpellingDirection, Stats};

/// Options for the `leanck normalize` subcommand.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Print the editable built-in JSON dictionary to stdout, then exit.
    #[arg(long, value_enum, hide_short_help = true, help_heading = "Spelling")]
    pub export_dictionary: Option<DictionaryDirection>,
    /// JSON dictionary files; repeat to replace configured files in this order.
    #[arg(
        long = "dictionary",
        value_name = "FILE",
        hide_short_help = true,
        help_heading = "Spelling"
    )]
    pub dictionary: Vec<PathBuf>,
    /// Report formatting changes without writing files; exit nonzero if changes are needed.
    #[arg(long)]
    pub dry_run: bool,
    /// Exclude a glob relative to the directory being normalized (repeatable).
    ///
    /// Replaces configured exclusions when supplied.
    #[arg(long)]
    pub exclude: Vec<String>,
    /// Convert American spellings to British spellings.
    ///
    /// The default direction is British to American.
    #[arg(
        long = "to-british",
        alias = "american-to-british",
        hide_short_help = true,
        help_heading = "Spelling"
    )]
    pub british: bool,

    /// Disable all frontmatter normalization.
    #[arg(long, hide_short_help = true, help_heading = "Frontmatter")]
    pub no_frontmatter: bool,

    /// Disable attribution header insertion and formatting.
    #[arg(long, hide_short_help = true, help_heading = "Frontmatter")]
    pub no_header: bool,

    /// Do not alphabetize imports.
    #[arg(long, hide_short_help = true, help_heading = "Frontmatter")]
    pub no_sort_imports: bool,

    /// Leave declaration attribute placement unchanged.
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub no_attributes: bool,

    /// Convert banner comments and format Markdown heading titles (opt-in).
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub headings: bool,

    /// Disable all Lean block-comment transformations.
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub no_comments: bool,

    /// Do not reflow Lean block comments.
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub no_comment_reflow: bool,

    /// Turn plain URLs inside comments into markdown links (opt-in).
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub linkify_urls: bool,

    /// Do not normalize British/American spelling inside comments.
    #[arg(long, hide_short_help = true, help_heading = "Spelling")]
    pub no_spelling: bool,

    /// Do not capitalize words after `: ` inside comments.
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub no_capitalize_after_colon: bool,

    /// Do not collapse consecutive blank lines.
    #[arg(long, hide_short_help = true, help_heading = "Formatting")]
    pub no_collapse_blank_lines: bool,

    /// File to rewrite in place, or directory to normalize recursively.
    ///
    /// Omit to read stdin and write the result to stdout.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// With a directory argument, normalize every regular file instead of
    /// only `*.lean` files.
    #[arg(long, hide_short_help = true)]
    pub all_files: bool,

    /// Print every updated file instead of only the final summary.
    ///
    /// Applies when normalizing a directory.
    #[arg(long, hide_short_help = true)]
    pub verbose: bool,

    /// Target line width for comments and attributes: a positive integer or unlimited.
    #[arg(short, long, default_value_t = NormalizeWidth::Finite(DEFAULT_WIDTH), value_name = "WIDTH")]
    pub width: NormalizeWidth,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum DictionaryDirection {
    American,
    British,
}

/// Execute already-resolved settings. The pure normalize API never loads configuration.
pub(crate) fn run(args: Args, settings: &leanck_config::Normalize) -> anyhow::Result<()> {
    if let Some(direction) = args.export_dictionary {
        print!(
            "{}",
            leanck_normalize::builtin_json(match direction {
                DictionaryDirection::American => SpellingDirection::BritishToAmerican,
                DictionaryDirection::British => SpellingDirection::AmericanToBritish,
            })
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.width.is_valid(),
        "--width must be a positive integer or \"unlimited\""
    );
    settings.rules.validate()?;
    let options = options_from_args(&args, settings);
    let paths: &[PathBuf] = if !options.comments || options.spelling.is_none() {
        &[]
    } else if args.dictionary.is_empty() {
        &settings.rules.spelling.files
    } else {
        &args.dictionary
    };
    let mut words = std::collections::HashMap::new();
    // Validate every resource before processing or rewriting any source file.
    for path in paths {
        let text = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read dictionary {}: {e}", path.display()))?;
        let map: std::collections::BTreeMap<String, String> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("invalid dictionary {}: {e}", path.display()))?;
        let mut seen = std::collections::HashSet::new();
        for (from, to) in map {
            leanck_config::validate_word_pair(&from, &to)?;
            let key = if settings.rules.spelling.case_sensitive {
                from
            } else {
                from.to_lowercase()
            };
            anyhow::ensure!(
                seen.insert(key.clone()),
                "ambiguous case-insensitive spelling {key:?} in {}",
                path.display()
            );
            words.insert(key, to);
        }
    }
    let dictionary = options
        .spelling
        .map(|direction| {
            leanck_normalize::configured_map(direction, &settings.rules.spelling, &words)
        })
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let patterns = if args.exclude.is_empty() {
        &settings.exclude
    } else {
        &args.exclude
    };
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            globset::Glob::new(pattern)
                .map_err(|e| anyhow::anyhow!("invalid normalize.exclude glob {pattern:?}: {e}"))?,
        );
    }
    let exclude = builder.build()?;
    let mut effective = options.configuration(args.width, &settings.rules);
    if !args.dictionary.is_empty() {
        effective.rules.spelling.files = args.dictionary.clone();
    }
    for warning in effective.warnings() {
        eprintln!("warning: {}", warning.message);
    }
    if options.comments
        && options.spelling.is_some()
        && !effective.rules.spelling.use_builtin
        && !paths.is_empty()
        && words.is_empty()
        && effective.rules.spelling.replacements.is_empty()
    {
        eprintln!(
            "warning: custom dictionary files contain no entries and normalize.rules.spelling.use_builtin=false; spelling substitutions will have no effect"
        );
    }
    match args.path.as_deref().filter(|p| p.as_os_str() != "-") {
        Some(path) => run_path(
            &args,
            path,
            options,
            &exclude,
            &settings.rules,
            dictionary.as_ref(),
        ),
        None => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if options.frontmatter.enabled
                && options.frontmatter.header
                && leanck_normalize::is_module_file(&input)
                && leanck_normalize::leading_header(&input).is_none()
            {
                eprintln!(
                    "note: stdin has no file history; missing attribution header left absent"
                );
            }
            let _panics = PanicCapture::install();
            let result = catch_panic(|| {
                Ok(leanck_normalize::normalize_with_context(
                    &input,
                    args.width,
                    options,
                    &settings.rules,
                    &leanck_normalize::Context {
                        dictionary: dictionary.as_ref(),
                        ..Default::default()
                    },
                )?)
            })?;
            for warning in &result.warnings {
                if let Some(line) = warning.line {
                    eprintln!("warning: stdin:{line}: {}", warning.message);
                }
            }
            if args.dry_run {
                anyhow::ensure!(result.output == input, "stdin needs normalization");
            } else {
                print!("{}", result.output);
            }
            Ok(())
        }
    }
}

fn options_from_args(args: &Args, s: &leanck_config::Normalize) -> Options {
    Options {
        frontmatter: leanck_normalize::FrontmatterOptions {
            enabled: s.frontmatter && !args.no_frontmatter,
            header: s.header && !args.no_header,
            sort_imports: s.sort_imports && !args.no_sort_imports,
        },
        attributes: s.attributes && !args.no_attributes,
        headings: s.headings || args.headings,
        comments: s.comments && !args.no_comments,
        comment_reflow: s.comment_reflow && !args.no_comment_reflow,
        linkify_urls: s.linkify_urls || args.linkify_urls,
        spelling: if args.no_spelling {
            None
        } else if args.british {
            Some(SpellingDirection::AmericanToBritish)
        } else {
            match s.spelling {
                leanck_config::Spelling::American => Some(SpellingDirection::BritishToAmerican),
                leanck_config::Spelling::British => Some(SpellingDirection::AmericanToBritish),
                leanck_config::Spelling::Off => None,
            }
        },
        capitalize_after_colon: s.capitalize_after_colon && !args.no_capitalize_after_colon,
        collapse_blank_lines: s.collapse_blank_lines && !args.no_collapse_blank_lines,
    }
}

fn run_path(
    args: &Args,
    path: &Path,
    options: Options,
    exclude: &globset::GlobSet,
    rules: &leanck_config::NormalizeRules,
    dictionary: Option<&leanck_normalize::Dictionary>,
) -> anyhow::Result<()> {
    let is_dir = fs::metadata(path)?.is_dir();
    let mut files = Vec::new();
    if is_dir {
        collect_files(path, path, args.all_files, exclude, &mut files)?;
    } else {
        files.push(path.to_owned());
    }
    let mut changed = 0;
    let mut failed = 0;
    let mut totals = Stats::default();
    // Directory runs group missing-history notes by reason, so a tree outside
    // any repository yields one note rather than one per file.
    let mut history_notes: std::collections::BTreeMap<String, Vec<&Path>> = Default::default();
    let _panics = PanicCapture::install();
    for path in &files {
        let result = catch_panic(|| -> anyhow::Result<_> {
            let original = fs::read_to_string(path)?;
            #[cfg(debug_assertions)]
            if std::env::var("LEANCK_NORMALIZE_TEST_PANIC")
                .is_ok_and(|marker| !marker.is_empty() && original.contains(&marker))
            {
                panic!("test panic requested by LEANCK_NORMALIZE_TEST_PANIC");
            }
            let existing_header = leanck_normalize::leading_header(&original).is_some();
            let mut replacement_history_error = None;
            let history = if (path.extension().is_some_and(|e| e == "lean")
                || leanck_normalize::is_module_file(&original)
                || existing_header)
                && leanck_normalize::needs_history(
                    &original,
                    options.frontmatter,
                    &rules.frontmatter,
                ) {
                match leanck_normalize::history::read(path, &rules.frontmatter) {
                    Ok(history) => Some(history),
                    Err(reason) => {
                        if existing_header {
                            replacement_history_error = Some(reason);
                        } else if is_dir && !args.verbose {
                            history_notes.entry(reason).or_default().push(path);
                        } else {
                            eprintln!(
                                "note: {}: {reason}; missing attribution header left absent",
                                path.display()
                            );
                        }
                        None
                    }
                }
            } else {
                None
            };
            let module = if path.extension().is_some_and(|e| e == "lean")
                || leanck_normalize::is_module_file(&original)
            {
                path.file_stem().and_then(|s| s.to_str())
            } else {
                None
            };
            let context = leanck_normalize::Context {
                history: history.as_ref(),
                dictionary,
                module_name: module,
                seen_module_doc: false,
            };
            let result = leanck_normalize::normalize_with_context(
                &original, args.width, options, rules, &context,
            )?;
            for warning in &result.warnings {
                if let Some(line) = warning.line {
                    let detail = replacement_history_error
                        .as_ref()
                        .filter(|_| warning.code == "header_replacement_unavailable")
                        .map(|reason| format!(" ({reason})"))
                        .unwrap_or_default();
                    eprintln!(
                        "warning: {}:{line}: {}{detail}",
                        path.display(),
                        warning.message
                    );
                }
            }
            if result.output == original {
                return Ok(None);
            }
            if !args.dry_run {
                write_atomically(path, &result.output)?;
            }
            Ok(Some(result.stats))
        });
        match result {
            Ok(Some(stats)) => {
                changed += 1;
                totals.add(&stats);
                if args.dry_run {
                    println!("would update {}", path.display());
                } else if args.verbose || !is_dir {
                    println!(
                        "updated {} ({})",
                        path.display(),
                        stats.summary_parts().join(", ")
                    );
                }
            }
            Ok(None) => {
                if !is_dir {
                    println!("unchanged {}", path.display());
                }
            }
            Err(error) => {
                failed += 1;
                eprintln!("error in {}: {error:#}", path.display());
            }
        }
    }
    for (reason, paths) in &history_notes {
        match paths.as_slice() {
            [path] => eprintln!(
                "note: {}: {reason}; missing attribution header left absent",
                path.display()
            ),
            _ => eprintln!(
                "note: {reason}; missing attribution headers left absent in {} files (use --verbose to list them)",
                paths.len()
            ),
        }
    }
    if is_dir {
        let failures = if failed == 0 {
            String::new()
        } else {
            format!(", {failed} failed")
        };
        let parts = totals.summary_parts();
        let stats = if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        };
        println!(
            "checked {} file{}, {changed} {}{failures}{stats}",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            if args.dry_run {
                "need normalization"
            } else {
                "updated"
            }
        );
    }
    anyhow::ensure!(failed == 0, "{failed} file(s) failed to normalize");
    anyhow::ensure!(
        !args.dry_run || changed == 0,
        "{changed} file(s) need normalization"
    );
    Ok(())
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>;

std::thread_local! {
    static LAST_PANIC: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// While installed, panics on this thread are recorded for the file being
/// normalized instead of printed; other threads keep the previous hook.
struct PanicCapture(Option<std::sync::Arc<PanicHook>>);

impl PanicCapture {
    fn install() -> Self {
        let previous = std::sync::Arc::new(std::panic::take_hook());
        let fallback = previous.clone();
        let thread = std::thread::current().id();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().id() == thread {
                let message = info.to_string().replace('\n', " ");
                LAST_PANIC.with(|last| *last.borrow_mut() = Some(message));
            } else {
                fallback(info);
            }
        }));
        Self(Some(previous))
    }
}

impl Drop for PanicCapture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        drop(std::panic::take_hook());
        if let Some(previous) = self.0.take() {
            match std::sync::Arc::try_unwrap(previous) {
                Ok(previous) => std::panic::set_hook(previous),
                Err(previous) => std::panic::set_hook(Box::new(move |info| previous(info))),
            }
        }
    }
}

/// Turn a panic while normalizing one file into that file's error, so the
/// remaining files are still processed and the summary is still printed.
fn catch_panic<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    // Forget panics that were caught and handled while normalizing earlier files.
    LAST_PANIC.with(|last| last.borrow_mut().take());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = LAST_PANIC
            .with(|last| last.borrow_mut().take())
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned());
        Err(anyhow::anyhow!(
            "internal error ({message}); file left unchanged"
        ))
    })
}

/// Replace a file through a temporary sibling and a rename, so an interrupted
/// run never leaves a partially written source. Permissions are preserved, and
/// a symbolic link is followed so the link itself stays in place. A rename
/// would separate the file from its other hard links and give it the new
/// file's owner and group, and it needs a writable directory, so in those
/// cases the file is overwritten in place, as an editor would. Extended
/// attributes and access control lists are not copied to the new file. There
/// is no fsync: it guards only against power loss and more than triples the
/// time of a large directory run.
fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    let target = fs::canonicalize(path)?;
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(io::Error::other("cannot replace a path without a parent"));
    };
    let metadata = fs::metadata(&target)?;
    if has_other_links(&metadata) {
        return fs::write(&target, contents);
    }
    let mut attempt = 0;
    loop {
        let temp = dir.join(format!(
            ".{}.leanck-{}-{attempt}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return fs::write(&target, contents);
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("cannot create {}: {error}", temp.display()),
                ));
            }
        };
        if !same_owner(&metadata, &file.metadata()?) {
            drop(file);
            let _ = fs::remove_file(&temp);
            return fs::write(&target, contents);
        }
        let result = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.set_permissions(metadata.permissions()))
            .and_then(|()| fs::rename(&temp, &target));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        return result;
    }
}

/// Whether other hard links share the file, which a rename would leave with
/// the old contents.
#[cfg(unix)]
fn has_other_links(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(not(unix))]
fn has_other_links(_: &fs::Metadata) -> bool {
    false
}

/// Whether two files have the same owner and group.
#[cfg(unix)]
fn same_owner(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    (a.uid(), a.gid()) == (b.uid(), b.gid())
}

#[cfg(not(unix))]
fn same_owner(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    true
}

/// Hidden entries and symlinks are always skipped during recursive traversal.
fn collect_files(
    root: &Path,
    dir: &Path,
    all_files: bool,
    exclude: &globset::GlobSet,
    out: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if exclude.is_match(path.strip_prefix(root).unwrap_or(&path)) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(root, &path, all_files, exclude, out)?;
        } else if kind.is_file()
            && (all_files || path.extension().and_then(|e| e.to_str()) == Some("lean"))
        {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Parser, error::ErrorKind};

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn omitted_path_uses_stdin_and_default_width() {
        let args = Cli::try_parse_from(["normalize"]).unwrap().args;
        assert_eq!(args.path, None);
        assert_eq!(args.width, NormalizeWidth::Finite(DEFAULT_WIDTH));
    }

    #[test]
    fn numeric_positionals_are_always_paths() {
        for path in ["80", "0", "184467440737095516160"] {
            let args = Cli::try_parse_from(["normalize", path]).unwrap().args;
            assert_eq!(args.path, Some(PathBuf::from(path)));
            assert_eq!(args.width, NormalizeWidth::Finite(DEFAULT_WIDTH));
        }
    }

    #[test]
    fn width_flags_work_with_stdin_and_paths() {
        for flag in ["--width", "-w"] {
            let args = Cli::try_parse_from(["normalize", flag, "72"]).unwrap().args;
            assert_eq!(args.path, None);
            assert_eq!(args.width, NormalizeWidth::Finite(72));

            for path in ["Main.lean", "MyProject", "80"] {
                let args = Cli::try_parse_from(["normalize", path, flag, "72"])
                    .unwrap()
                    .args;
                assert_eq!(args.path, Some(PathBuf::from(path)));
                assert_eq!(args.width, NormalizeWidth::Finite(72));
            }
        }
    }

    #[test]
    fn second_positional_is_rejected_including_legacy_width_syntax() {
        for positionals in [["80", "Main.lean"], ["Main.lean", "80"]] {
            let error =
                Cli::try_parse_from(["normalize", positionals[0], positionals[1]]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn width_flag_requires_a_positive_integer_or_unlimited() {
        for width in [
            "abc",
            "0",
            "-1",
            "80.5",
            "Unlimited",
            "184467440737095516160",
        ] {
            assert!(Cli::try_parse_from(["normalize", "--width", width]).is_err());
        }
    }

    #[test]
    fn unlimited_width_flags_work_with_stdin_and_paths() {
        for flag in ["--width", "-w"] {
            for path in [None, Some("Main.lean")] {
                let mut argv = vec!["normalize", flag, "unlimited"];
                argv.extend(path);
                let args = Cli::try_parse_from(argv).unwrap().args;
                assert_eq!(args.width, NormalizeWidth::Unlimited);
                assert_eq!(args.path, path.map(PathBuf::from));
            }
        }
    }
}
