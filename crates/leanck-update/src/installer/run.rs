//! The `leanck _install` driver: parse, resolve, plan, and report.
use super::{
    Answers, Console, Environment, InstallArgs, Plan, ReleaseInventory, SavedState, Settings,
    Status, Style, config_summary, parse_error_message, plan_observed, render::Outcome, ui,
};
use crate::paths::absolute;
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    io::{IsTerminal, Write},
};

/// Applying a plan arrives with the apply layer.
const APPLY_PENDING: &str = "installing with leanck _install is not yet implemented; rerun with --dry-run to preview, or use the install.sh attached to this release";

struct Run {
    console: Console<std::io::Stderr>,
    style: Style,
    status: Status,
    version: String,
    actions: Vec<String>,
}

/// Run `leanck _install` with the arguments after `_install`; returns the
/// exit code. `version` is the running executable's version.
pub fn main(argv: Vec<OsString>, version: &str) -> i32 {
    let json = argv.iter().any(|a| a == "--json");
    let no_color = argv.iter().any(|a| a == "--no-color");
    let quiet = argv.iter().any(|a| a == "-q" || a == "--quiet");
    // Help works even when saved state is broken, and never prints JSON.
    if argv.iter().any(|a| a == "-h" || a == "--help") || argv == ["help"] {
        let style = Style::detect(no_color, std::io::stdout().is_terminal());
        let help = if style == Style::PLAIN {
            InstallArgs::command().render_help().to_string()
        } else {
            InstallArgs::command().render_help().ansi().to_string()
        };
        print!("{help}");
        return 0;
    }
    let style = Style::detect(no_color, std::io::stderr().is_terminal());
    let mut run = Run {
        console: Console::stderr(style, quiet),
        style,
        status: Status::Failed,
        version: "latest".into(),
        actions: vec![],
    };
    let (code, message) = match drive(&argv, version, &mut run) {
        Ok(code) => (code, String::new()),
        Err(error) => {
            let message = format!("{error:#}");
            run.console.error(&message);
            run.status = Status::Failed;
            (1, message)
        }
    };
    if json {
        let outcome = Outcome {
            status: run.status,
            version: run.version,
            message,
            actions: run.actions,
        };
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{}", outcome.to_json_line());
        let _ = stdout.flush();
    }
    code
}

fn drive(argv: &[OsString], installer: &str, run: &mut Run) -> Result<i32> {
    let args = InstallArgs::parse_from(argv).map_err(|error| {
        anyhow::anyhow!(parse_error_message(&error).unwrap_or_else(|| error.to_string()))
    })?;
    if let Some(version) = &args.version {
        run.version = version.clone();
    }
    let mut env = Environment::capture()?;
    let state_dir = Settings::state_dir(&args, &env)?;
    let saved = SavedState::load(&state_dir)?;
    let mut settings = Settings::resolve(&args, &env, &saved)?;
    run.version = settings.version.clone();

    // Prompts read the controlling terminal: stdin may be the piped script.
    let interactive = args.interactive(
        std::io::stderr().is_terminal()
            && std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .is_ok(),
    );
    args.require_approval(interactive)?;
    if !interactive {
        settings.validate_noninteractive()?;
    }
    run.console.info(&format!("leanck-installer {installer}"));
    if args.dry_run {
        run.console
            .info("Dry run: approval only completes the preview; nothing will be installed.");
    }

    let release_dir = args
        .release_dir
        .as_ref()
        .context("--release-dir is required; run leanck _install through install.sh")?;
    let release_dir = absolute(
        release_dir
            .to_str()
            .context("--release-dir must be valid UTF-8")?,
        &env.home,
        &env.cwd,
    )?;
    let inventory = ReleaseInventory::read(&release_dir)?;
    if settings.from_repo.is_none() {
        inventory.ensure_version(&settings.version)?;
    }
    run.version = match &settings.from_repo {
        Some(_) => format!("{} (local source)", inventory.tag),
        None => inventory.tag.clone(),
    };
    let banner = super::banner(&run.version, env.detected_agents(), run.style);
    if !args.quiet || interactive {
        run.console.raw(&banner);
    }
    let plan = if interactive {
        let session = ui::Session {
            settings,
            env: &mut env,
            saved: &saved,
            inventory: &inventory,
            style: run.style,
            verbose: args.verbose > 0,
            banner,
        };
        match ui::run(session)? {
            ui::Outcome::Confirmed(plan) => *plan,
            ui::Outcome::Cancelled => {
                run.status = Status::Cancelled;
                run.console.info("Cancelled; nothing installed.");
                return Ok(0);
            }
            ui::Outcome::Interrupted => {
                run.status = Status::Cancelled;
                run.console
                    .info("Cancelled; no further changes will be made.");
                return Ok(130);
            }
        }
    } else {
        settings.apply_mode(settings.mode)?;
        let plan = match plan_observed(&settings, &mut env, &saved, &inventory, &Answers::new()) {
            Ok(plan) => plan,
            Err(failure) => {
                run.actions = failure.actions;
                for warning in &failure.warnings {
                    run.console.warn(warning);
                }
                return Err(failure.error.into_anyhow());
            }
        };
        show(run, &plan, args.quiet);
        plan
    };
    run.actions = plan.actions.clone();
    if args.dry_run {
        run.status = Status::Preview;
        run.console.info(
            "Dry run complete; no installed files or host configuration changed, and no host CLIs ran.",
        );
        return Ok(0);
    }
    // The apply layer installs the confirmed plan.
    bail!(APPLY_PENDING)
}

fn show(run: &mut Run, plan: &Plan, quiet: bool) {
    for warning in &plan.warnings {
        run.console.warn(warning);
    }
    if !quiet {
        run.console.raw(&config_summary(&plan.settings));
        run.console.raw(&plan.review(run.style));
    }
}

impl super::PlanError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!(self.to_string())
    }
}
