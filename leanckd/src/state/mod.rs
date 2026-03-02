//! URI-keyed elaboration state machine and wait-for-elaboration gate.
//!
//! [`docstate`] owns generation/version transitions; [`wait`] checks only pinned versions.

pub mod docstate;
pub mod wait;

pub use docstate::{
    DocState, ElaborationStore, WireDiagnostic, WirePosition, WireRange, range_overlaps,
};
pub use wait::{filter_by_range, wait_for_version};
