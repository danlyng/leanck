//! Interactive configuration: the mode menu, the plugin or standalone
//! prompts, conflict prompts, the confirmation menu, and the advanced-config
//! editor.
//!
//! - [`Flow`] is the dialog as a pure question-and-answer state machine.
//! - [`Installer`] is the terminal UI over it: `update(key) -> Effect` and
//!   `render(frame)`, with the advanced-config [`Form`].
//! - [`line`] runs the same flow one prompt line at a time for `TERM=dumb`
//!   and terminals that cannot be drawn on, with the plain editor.
//! - [`terminal`] sets up and restores the real terminal.
//!
//! Nothing is written and no command runs here; a confirmed plan is
//! returned to the caller to apply.
mod flow;
mod form;
pub mod line;
mod screen;
pub mod terminal;
#[cfg(test)]
mod tests;

pub use flow::{
    Choice, Effect, Flow, Group, Output, Question, confirmation_title, selection, skill_description,
};
pub use form::{Field, Form, FormEffect};
pub use screen::Installer;

use super::{Environment, Plan, ReleaseInventory, SavedState, Settings, Style, plan_observed};
use crossterm::event::KeyEvent;

/// One terminal input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Key(KeyEvent),
    /// Bracketed paste text.
    Paste(String),
}

/// How the interactive dialog ended without failing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user approved this plan.
    Confirmed(Box<Plan>),
    /// Cancel at the confirmation menu: nothing installed (exit 0).
    Cancelled,
    /// Ctrl-C, Ctrl-D, the end of input, or `cancel` at a conflict (exit 130).
    Interrupted,
}

/// Everything the interactive run needs.
pub struct Session<'a> {
    /// Settings from [`Settings::resolve`]; the mode is chosen interactively.
    pub settings: Settings,
    pub env: &'a mut Environment,
    pub saved: &'a SavedState,
    pub inventory: &'a ReleaseInventory,
    /// Message styling for stderr.
    pub style: Style,
    /// `--verbose`: show the plan after advanced edits.
    pub verbose: bool,
    /// The plain-text welcome banner, shown above the prompts.
    pub banner: String,
}

/// Run the interactive dialog on the controlling terminal: the full UI,
/// or line prompts under `TERM=dumb` or when the terminal cannot be drawn on.
pub fn run(session: Session<'_>) -> anyhow::Result<Outcome> {
    let Session {
        settings,
        env,
        saved,
        inventory,
        style,
        verbose,
        banner,
    } = session;
    let dumb = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    let cwd = env.cwd.clone();
    let mut planner = |settings: &Settings, answers: &super::Answers| {
        plan_observed(settings, env, saved, inventory, answers)
    };
    let terminal = if dumb {
        None
    } else {
        terminal::Session::start().ok()
    };
    let effect = match terminal {
        Some(mut terminal) => {
            let flow = Flow::new(settings, inventory, cwd, false, verbose);
            let mut installer = Installer::new(flow, &banner, style != Style::PLAIN);
            let effect = terminal.run(&mut installer, &mut planner);
            drop(terminal);
            // Keep the dialog in the scrollback once the screen is restored.
            eprint!("{}", installer.transcript(style));
            effect?
        }
        None => {
            let mut flow = Flow::new(settings, inventory, cwd, true, verbose);
            let tty = std::fs::File::open("/dev/tty")?;
            line::run(
                &mut flow,
                &mut std::io::BufReader::new(tty),
                &mut std::io::stderr(),
                style,
                &mut planner,
            )?
        }
    };
    match effect {
        Effect::Confirmed(plan) => Ok(Outcome::Confirmed(plan)),
        Effect::Cancelled => Ok(Outcome::Cancelled),
        Effect::Interrupted => Ok(Outcome::Interrupted),
        Effect::Failed(message) => Err(anyhow::anyhow!(message)),
        Effect::None | Effect::Plan => {
            Err(anyhow::anyhow!("the interactive installer stopped early"))
        }
    }
}
