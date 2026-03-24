use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use leanck_update::{
    Checker,
    gate::{Lease, Maintenance},
    install,
    receipt::Receipt,
};
use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
};

#[derive(clap::Args, Debug)]
#[command(
    after_help = crate::help::examples("  leanck update --check\n  leanck update --dry-run\n  leanck update --yes\n  leanck update --to-version latest")
)]
pub(crate) struct Args {
    /// Check release availability without installing.
    #[arg(long, conflicts_with_all = ["dry_run", "retry_registration"])]
    check: bool,
    /// Run the update flow without writing anything.
    #[arg(long, conflicts_with = "retry_registration")]
    dry_run: bool,
    /// Update to the specified release version (or pass "latest").
    #[arg(
        long,
        value_name = "VERSION",
        conflicts_with = "retry_registration",
        hide_short_help = true
    )]
    to_version: Option<String>,
    /// Approve this update without an interactive prompt.
    #[arg(short, long)]
    yes: bool,
    /// Print machine-readable output.
    #[arg(long, hide_short_help = true)]
    pub(crate) json: bool,
    /// Total seconds allowed to resolve, download, and verify a release.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600), hide_short_help = true)]
    timeout: u64,
    /// Installer state directory for this executable.
    #[arg(long, hide_short_help = true)]
    state_dir: Option<PathBuf>,
    /// Retry plugin registration after an otherwise completed update.
    #[arg(long, hide_short_help = true)]
    retry_registration: bool,
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
}

pub(crate) async fn run(args: Args) -> Result<()> {
    let exe = std::env::current_exe()?;
    let known_state = args.state_dir.clone().or(Receipt::recorded_state(&exe)?);
    let state = match &known_state {
        Some(state) => state.clone(),
        None => leanck_update::state_directory()?.join("installer"),
    };
    if !args.dry_run && !args.check {
        install::recover(&state)?;
    }
    let receipt = if state.join("receipt.json").try_exists()? {
        Receipt::discover(&exe, Some(&state)).context("this executable wasn't installed by the leanck installer; update it through its package manager or rerun the installer")?
    } else {
        None
    };
    if let Some(receipt) = &receipt
        && known_state.is_none()
    {
        // A default state directory is not a discovery marker for relocated copies.
        ensure!(
            receipt.binary.canonicalize()? == exe.canonicalize()?,
            "this executable wasn't installed by the leanck installer; update it through its package manager or rerun the installer"
        );
    }
    if args.check {
        let checker = if let Some(receipt) = receipt {
            Checker::new(
                env!("CARGO_PKG_VERSION"),
                leanck_update::state_directory()?.join("updates"),
                receipt.source.clone(),
            )
            .with_receipt(receipt)
        } else {
            Checker::for_current(env!("CARGO_PKG_VERSION"))?
        };
        let status = checker.check(&args.settings.updates, true).await;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            println!("{}", status.notice.as_deref().unwrap_or(&status.state));
            if let Some(error) = &status.error {
                eprintln!("update check failed: {error}");
            }
        }
        ensure!(status.error.is_none(), "release check failed");
        return Ok(());
    }
    if args.retry_registration {
        let receipt = receipt.context("installation ownership has not been recorded; run leanck update before retrying plugin registration")?;
        approve(args.yes)?;
        let _lease = Lease::acquire(&receipt)?;
        let mut receipt =
            Receipt::discover(&exe, Some(&state))?.context("installation receipt disappeared")?;
        show(&install::register(&mut receipt).await?, args.json)?;
        return Ok(());
    }
    if !args.json {
        eprintln!("Checking and preparing leanck update…");
    }
    let prepared = tokio::time::timeout(std::time::Duration::from_secs(args.timeout), async {
        match receipt {
            Some(receipt) => install::prepare(receipt, args.to_version.as_deref()).await,
            None => install::prepare_from_installer(&state, &exe, args.to_version.as_deref()).await,
        }
    })
    .await
    .context("release preparation timed out")??;
    if let Some(binary) = &prepared.plan.recording_installer_ownership
        && (!args.json || !args.yes)
    {
        eprintln!("Recording installer ownership for {}", binary.display());
    }
    if args.json && args.dry_run {
        println!("{}", serde_json::to_string_pretty(&prepared.plan)?);
    } else if !args.json || !args.yes {
        eprintln!(
            "leanck {} → {} ({} managed paths)",
            prepared.plan.from_version,
            prepared.plan.to_version,
            prepared.plan.changes.len()
        );
        for path in &prepared.plan.changes {
            eprintln!("  {}", path.display());
        }
    }
    if args.dry_run {
        return Ok(());
    }
    approve(args.yes)?;
    show(&prepared.apply().await?, args.json)
}

fn approve(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    ensure!(
        std::io::stdin().is_terminal(),
        "use --yes to approve an update without a terminal"
    );
    eprint!("Apply this update? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    ensure!(
        matches!(answer.trim(), "y" | "Y" | "yes"),
        "update cancelled; nothing installed"
    );
    Ok(())
}
fn show(outcome: &install::Outcome, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(outcome)?);
    } else {
        println!("{}: leanck {}", outcome.status, outcome.version);
        for e in &outcome.registration_errors {
            eprintln!("{e}");
        }
        if outcome.restart_required {
            eprintln!("Restart agent hosts to load the updated plugin.");
        }
    }
    ensure!(
        outcome.registration_errors.is_empty(),
        "host registration is incomplete; run `leanck update --retry-registration --yes`"
    );
    Ok(())
}

#[derive(clap::Args, Debug)]
pub(crate) struct InternalArgs {
    #[command(subcommand)]
    command: Internal,
}
#[derive(Subcommand, Debug)]
enum Internal {
    /// Emit a receipt for the bootstrap installer's prepared inventory.
    Receipt {
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        registration_complete: bool,
    },
    /// Lock an open file description inherited from the bootstrap shell.
    LockFd {
        #[arg(long)]
        fd: i32,
        #[arg(long)]
        binary: Vec<PathBuf>,
    },
    /// Configure an unpacked plugin directory in place for an installation.
    ConfigurePlugin {
        #[arg(long)]
        plugin: PathBuf,
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        disable_mcp: bool,
    },
}
pub(crate) fn internal(args: InternalArgs) -> Result<()> {
    match args.command {
        Internal::Receipt {
            state_dir,
            binary,
            registration_complete,
        } => {
            let mut receipt = Receipt::from_installer(&state_dir, &binary)?;
            if registration_complete {
                receipt.registration_pending = false;
            }
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        Internal::LockFd { fd, binary } => {
            use std::os::fd::FromRawFd;
            ensure!(fd >= 3, "invalid maintenance descriptor");
            // SAFETY: fcntl validates the caller's descriptor before we create
            // an owned File. The duplicate retains the parent's open description.
            let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
            if duplicate < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: a successful fcntl returned a new descriptor owned by this scope.
            let file = unsafe { std::fs::File::from_raw_fd(duplicate) };
            file.try_lock().context(
                "installation is in use; close leanck/MCP clients and stop daemons before retrying",
            )?;
            for path in binary {
                leanck_update::gate::ensure_no_legacy_users(&path)?;
            }
        }
        Internal::ConfigurePlugin {
            plugin,
            binary,
            disable_mcp,
        } => install::configure_plugin(&plugin, &binary, !disable_mcp)?,
    }
    Ok(())
}

/// Check foreground commands before taking their lifetime usage lease.
pub(crate) async fn foreground(
    settings: &leanck_config::Settings,
    eligible: bool,
) -> Result<Option<Lease>> {
    let exe = std::env::current_exe()?;
    let receipt = Receipt::discover(&exe, None)?;
    if eligible && settings.updates.check && receipt.is_some() {
        let status =
            leanck_update::current_status(env!("CARGO_PKG_VERSION"), &settings.updates).await;
        if let Some(notice) = &status.notice {
            eprintln!("update: {notice}");
        }
        if settings.updates.auto_install
            && let (Some(version), Some(receipt)) = (status.eligible_version, receipt.as_ref())
            && receipt.provenance == "release"
            && Maintenance::acquire(&receipt.state_dir).is_ok()
        {
            match install::prepare(receipt.clone(), Some(&version)).await {
                Ok(prepared) => match prepared.preserve_pin(receipt.pin.clone()).apply().await {
                    Ok(_) => reexec(&receipt.binary)?,
                    Err(error) => eprintln!("info: automatic update deferred: {error:#}"),
                },
                Err(error) => eprintln!("info: automatic update unavailable: {error:#}"),
            }
        }
    }
    if let Some(receipt) = receipt {
        let lease = Lease::acquire(&receipt)?;
        let current = Receipt::discover(&exe, None)?.context("installation receipt disappeared")?;
        if current.generation != receipt.generation || current.version != env!("CARGO_PKG_VERSION")
        {
            reexec(&current.binary)?;
        }
        Ok(Some(lease))
    } else {
        Ok(None)
    }
}
fn reexec(binary: &std::path::Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let receipt = Receipt::discover(binary, None)?
        .context("cannot re-execute without an installation receipt")?;
    let generation = format!("{}:{}", receipt.version, receipt.generation);
    ensure!(
        std::env::var("LEANCK_UPDATE_REEXEC").ok().as_deref() != Some(generation.as_str()),
        "installed executable does not match its receipt; rerun the installer to repair it"
    );
    Err(std::process::Command::new(binary)
        .args(std::env::args_os().skip(1))
        .env("LEANCK_UPDATE_REEXEC", generation)
        .exec()
        .into())
}
