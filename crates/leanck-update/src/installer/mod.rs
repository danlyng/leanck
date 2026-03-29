//! The native installer behind `leanck _install`, which `install.sh` runs
//! from an extracted, checksum-verified release.
//!
//! Layers:
//! 1. **Planning** (this module's [`plan`]/[`plan_settings`]): pure. Options,
//!    an [`Environment`] snapshot, [`SavedState`], and a [`ReleaseInventory`]
//!    produce a [`Plan`]: the exact preview lines, warnings, and the
//!    structured [`Step`]s and [`Registration`]s that apply performs.
//! 2. **Apply** ([`apply`]): commits a plan through the installation journal,
//!    with host CLIs behind [`HostCli`] and daemon maintenance behind [`Daemons`].
//!    [`uninstall`] removes what the inventory records.
//! 3. **UI** ([`ui`]): interactive configuration, conflict prompts, the
//!    confirmation menu, and the advanced-config editor.
mod apply;
#[cfg(test)]
mod apply_tests;
mod console;
mod environment;
mod hosts;
mod options;
mod plan;
mod release;
mod render;
mod run;
mod settings;
mod state;
#[cfg(test)]
mod tests;
pub mod ui;
pub mod uninstall;

pub use apply::{Applied, Runtime, apply, recover_interrupted};
pub use console::{Console, Style};
pub use environment::{
    Environment, FileSnapshot, Node, PATH_BLOCK_END, PATH_BLOCK_START, Probe, balanced_markers,
    observe_node,
};
pub use hosts::{
    Captured, DaemonCheck, Daemons, Failure, HostCall, HostCli, Ran, SystemDaemons, SystemHosts,
};
pub use options::{InstallArgs, Mode, parse_error_message};
pub use plan::{
    Answer, Answers, ExampleSource, HostName, OWNERSHIP_MARKER, Plan, PlanError, PlanFailure,
    Registration, Step, plan, plan_observed, plan_settings, sidecar,
};
pub use release::ReleaseInventory;
pub use render::{Outcome, Status, banner, config_summary};
pub use run::main;
pub use settings::{
    CompletionShell, DEFAULT_RELEASE_URL, DEFAULT_TIMEOUT, Hosts, Settings, SkillSelection,
    ensure_path_shell, valid_skill_name,
};
pub use state::{InventoryRecord, Recorded, SELECTION_KEYS, SavedState, UninstallInventory};
pub use uninstall::{Approval, UninstallArgs, UninstallOptions, Uninstalled, uninstall};
