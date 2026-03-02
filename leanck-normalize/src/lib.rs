//! Normalize Lean comment formatting and top-of-file frontmatter.
//!
//! This crate is consumed two ways: as the `leanck normalize` subcommand of the
//! `leanck` CLI (via [`Args`] + [`run`]), and as the standalone `leanck-normalize`
//! binary (`src/main.rs`, a thin wrapper around the same entry point).

mod attributes;
mod frontmatter;
mod reflow;
mod spelling;

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use reflow::SpellingDirection;

/// Default reflow width when none is supplied.
pub const DEFAULT_WIDTH: usize = 100;

/// Options shared by the `leanck normalize` subcommand and the standalone
/// `leanck-normalize` binary. Mirrors the original flags one-for-one.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Convert American spellings to British spellings instead of the default
    /// British-to-American direction.
    #[arg(long, alias = "american-to-british")]
    pub british: bool,

    /// Disable all frontmatter normalization.
    #[arg(long)]
    pub no_frontmatter: bool,

    /// Do not add the standard copyright/license/authors header when missing.
    #[arg(long)]
    pub no_header: bool,

    /// Do not alphabetize imports.
    #[arg(long)]
    pub no_sort_imports: bool,

    /// Do not move `@[expose] public ... section` after the module docstring.
    #[arg(long)]
    pub no_section_relocation: bool,

    /// Do not move a leading namespace after the section line.
    #[arg(long)]
    pub no_namespace_relocation: bool,

    /// Do not move standalone `@[simp]` attributes onto declaration lines.
    #[arg(long)]
    pub no_attributes: bool,

    /// Convert `-- ===` banner comments into `/-! ## Heading -/` doc comments
    /// (opt-in).
    #[arg(long)]
    pub headings: bool,

    /// Disable all Lean block-comment transformations.
    #[arg(long)]
    pub no_comments: bool,

    /// Do not reflow Lean block comments.
    #[arg(long)]
    pub no_comment_reflow: bool,

    /// Turn plain URLs inside comments into markdown links (opt-in).
    #[arg(long)]
    pub linkify_urls: bool,

    /// Do not normalize British/American spelling inside comments.
    #[arg(long)]
    pub no_spelling: bool,

    /// Do not capitalize words after `: ` inside comments.
    #[arg(long)]
    pub no_capitalize_after_colon: bool,

    /// Do not collapse consecutive blank lines.
    #[arg(long)]
    pub no_collapse_blank_lines: bool,

    /// Optional width or path. A number sets the target width for stdin;
    /// anything else is treated as a file to rewrite in place, or a directory
    /// to normalize recursively.
    #[arg(value_name = "WIDTH_OR_PATH")]
    pub width_or_file: Option<String>,

    /// Optional file or directory path when a positional width is supplied.
    #[arg(value_name = "FILE_OR_DIR")]
    pub file: Option<PathBuf>,

    /// With a directory argument, normalize every regular file instead of
    /// only `*.lean` files.
    #[arg(long)]
    pub all_files: bool,

    /// With a directory argument, print a line for every updated file
    /// instead of only the final summary.
    #[arg(long)]
    pub verbose: bool,

    /// Target line width for comment reflow.
    #[arg(short, long, default_value_t = DEFAULT_WIDTH)]
    pub width: usize,
}

/// Run the normalizer. Reads from stdin (writing the result to stdout) when no
/// path is given, otherwise rewrites the file or directory in place.
pub fn run(args: Args) -> anyhow::Result<()> {
    match parse_mode(&args).map_err(|e| anyhow::anyhow!("{e}"))? {
        Mode::Stdin { width, options } => run_stdin(width, options),
        Mode::File {
            width,
            path,
            options,
        } => run_path(width, &path, options, &args),
    }
}

enum Mode {
    Stdin {
        width: usize,
        options: reflow::Options,
    },
    File {
        width: usize,
        path: PathBuf,
        options: reflow::Options,
    },
}

fn parse_mode(args: &Args) -> Result<Mode, String> {
    if args.width == 0 {
        return Err("--width must be greater than zero".to_string());
    }

    let (width, path) =
        parse_positionals(args.width_or_file.as_deref(), args.file.as_ref(), args.width)?;
    let options = options_from_args(args);

    match path {
        Some(path) => Ok(Mode::File {
            width,
            path,
            options,
        }),
        None => Ok(Mode::Stdin { width, options }),
    }
}

fn parse_positionals(
    width_or_file: Option<&str>,
    file: Option<&PathBuf>,
    flag_width: usize,
) -> Result<(usize, Option<PathBuf>), String> {
    match (width_or_file, file) {
        (None, None) => Ok((flag_width, None)),
        (Some(one), None) => match one.parse::<usize>() {
            Ok(width) if width > 0 => Ok((width, None)),
            Ok(_) => Err("width must be greater than zero".to_string()),
            Err(_) => Ok((flag_width, Some(PathBuf::from(one)))),
        },
        (Some(width), Some(file)) => match width.parse::<usize>() {
            Ok(width) if width > 0 => Ok((width, Some(file.clone()))),
            Ok(_) => Err("width must be greater than zero".to_string()),
            Err(_) => Err(format!(
                "when two positional arguments are supplied, the first must be a width: {width}"
            )),
        },
        (None, Some(_)) => unreachable!("clap fills positional arguments in order"),
    }
}

fn options_from_args(args: &Args) -> reflow::Options {
    let mut options = reflow::Options::default();

    options.frontmatter.enabled = !args.no_frontmatter;
    options.frontmatter.header = !args.no_header;
    options.frontmatter.sort_imports = !args.no_sort_imports;
    options.frontmatter.section_after_docstring = !args.no_section_relocation;
    options.frontmatter.namespace_after_section = !args.no_namespace_relocation;

    options.attributes = !args.no_attributes;
    options.headings = args.headings;
    options.comments = !args.no_comments;
    options.comment_reflow = !args.no_comment_reflow;
    options.linkify_urls = args.linkify_urls;
    options.spelling = if args.no_spelling {
        None
    } else if args.british {
        Some(SpellingDirection::AmericanToBritish)
    } else {
        Some(SpellingDirection::BritishToAmerican)
    };
    options.capitalize_after_colon = !args.no_capitalize_after_colon;
    options.collapse_blank_lines = !args.no_collapse_blank_lines;

    options
}

fn run_stdin(width: usize, options: reflow::Options) -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| anyhow::anyhow!("error reading stdin: {e}"))?;

    let output =
        reflow::normalize(&input, width, options).map_err(|e| anyhow::anyhow!("{e}"))?;
    print!("{output}");
    Ok(())
}

enum Outcome {
    Unchanged,
    Updated(reflow::Stats),
}

fn run_path(width: usize, path: &Path, options: reflow::Options, args: &Args) -> anyhow::Result<()> {
    let is_dir = fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    if is_dir {
        return run_dir(width, path, options, args.all_files, args.verbose);
    }

    match normalize_file(width, path, options) {
        Ok(Outcome::Unchanged) => println!("unchanged {}", path.display()),
        Ok(Outcome::Updated(stats)) => print_updated(path, &stats),
        Err(e) => anyhow::bail!("{e}"),
    }
    Ok(())
}

fn run_dir(
    width: usize,
    root: &Path,
    options: reflow::Options,
    all_files: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, all_files, &mut files)
        .map_err(|e| anyhow::anyhow!("error scanning {}: {e}", root.display()))?;

    let mut updated = 0usize;
    let mut failed = 0usize;
    let mut totals = reflow::Stats::default();
    for path in &files {
        match normalize_file(width, path, options) {
            Ok(Outcome::Unchanged) => {}
            Ok(Outcome::Updated(stats)) => {
                updated += 1;
                totals.add(&stats);
                if verbose {
                    print_updated(path, &stats);
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("{e}");
            }
        }
    }

    let failures = if failed > 0 {
        format!(", {failed} failed")
    } else {
        String::new()
    };
    let parts = totals.summary_parts();
    let stat_summary = if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    };
    println!(
        "checked {} file{}, {updated} updated{failures}{stat_summary}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
    );
    if failed > 0 {
        anyhow::bail!("{failed} file(s) failed to normalize");
    }
    Ok(())
}

/// Recursively collect the files to normalize under `dir`, in sorted order.
/// Hidden entries (`.git`, `.lake`, …) and symlinks are skipped; unless
/// `all_files` is set, only `*.lean` files are kept.
fn collect_files(dir: &Path, all_files: bool, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&entry.path(), all_files, out)?;
        } else if file_type.is_file() {
            let path = entry.path();
            if all_files || path.extension().and_then(|e| e.to_str()) == Some("lean") {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn normalize_file(width: usize, path: &Path, options: reflow::Options) -> Result<Outcome, String> {
    let original = fs::read_to_string(path)
        .map_err(|e| format!("error reading {}: {e}", path.display()))?;

    let result = reflow::normalize_with_stats(&original, width, options)
        .map_err(|e| format!("error in {}: {e}", path.display()))?;

    if result.output == original {
        return Ok(Outcome::Unchanged);
    }

    fs::write(path, &result.output)
        .map_err(|e| format!("error writing {}: {e}", path.display()))?;
    Ok(Outcome::Updated(result.stats))
}

fn print_updated(path: &Path, stats: &reflow::Stats) {
    let parts = stats.summary_parts();
    if parts.is_empty() {
        println!("updated {}", path.display());
    } else {
        println!("updated {} ({})", path.display(), parts.join(", "));
    }
}
