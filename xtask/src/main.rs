use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use clap::builder::PossibleValuesParser;
use xtask::{TARGETS, package_local, package_plugin, package_release, repo_root};

/// leanck repository tasks.
#[derive(Parser)]
#[command(name = "cargo xtask")]
// Variant names are the subcommand names.
#[allow(clippy::enum_variant_names)]
enum Task {
    /// Assemble the leanck Claude Code and Codex plugin from repository sources.
    PackagePlugin {
        /// Directory holding the staged leanck executable.
        #[arg(long)]
        bin_dir: PathBuf,
        /// Generated plugin directory to create or replace.
        #[arg(long)]
        output: PathBuf,
    },
    /// Package a native leanck binary plus integrations for install.sh.
    ///
    /// Build the binary for --target before running this task. The release
    /// workflow runs it on each target's native operating system and CPU.
    PackageRelease {
        #[arg(long)]
        bin_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_parser = PossibleValuesParser::new(TARGETS))]
        target: String,
        /// Release tag matching `leanck --version`.
        #[arg(long)]
        version: String,
    },
    /// Assemble local source for `install.sh --from-repo` after the user approves building it.
    PackageLocal {
        /// Omit for standalone skills and agents only.
        #[arg(long)]
        bin_dir: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        version: String,
        /// An approved inventory whose component names the package must match.
        #[arg(long)]
        inventory: Option<PathBuf>,
    },
}

/// Resolve links like Python's non-strict `Path.resolve`, even when `path`
/// does not exist yet.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => resolve(parent).join(name),
        _ => absolute,
    }
}

fn main() -> ExitCode {
    let task = Task::parse();
    let repo = repo_root();
    let (prefix, result) = match task {
        Task::PackagePlugin { bin_dir, output } => (
            "package-plugin",
            std::path::absolute(&output)
                .map_err(Into::into)
                .and_then(|output| {
                    package_plugin(&repo, Some(&resolve(&bin_dir)), &output, true)
                        .map(|packaged| print!("{}", packaged.summary()))
                }),
        ),
        Task::PackageRelease {
            bin_dir,
            output,
            target,
            version,
        } => (
            "package-release",
            package_release(
                &repo,
                &resolve(&bin_dir),
                &resolve(&output),
                &target,
                &version,
            )
            .map(|(packaged, archive)| {
                print!("{}", packaged.summary());
                println!("release archive: {}", archive.display());
            }),
        ),
        Task::PackageLocal {
            bin_dir,
            output,
            version,
            inventory,
        } => (
            "package-local",
            package_local(
                &repo,
                bin_dir.as_deref().map(resolve).as_deref(),
                &resolve(&output),
                &version,
                inventory.as_deref().map(resolve).as_deref(),
            )
            .map(|packaged| print!("{}", packaged.summary())),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{prefix}: {error:#}");
            ExitCode::FAILURE
        }
    }
}
