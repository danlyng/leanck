//! URI-keyed elaboration state machine and wait-for-elaboration gate.
//!
//! Ported from `leanck-lsp/src/state.ts` (`StateStore`, `DocState`) and
//! `waitFor.ts` (`waitForRange`). The synchronous transitions live in
//! [`docstate`] and are unit-tested directly against the TS test suite; the
//! async gate lives in [`wait`].

pub mod docstate;
pub mod wait;

pub use docstate::{
    overlaps_any, range_overlaps, DocState, StateStore, WireDiagnostic, WirePosition, WireRange,
};
pub use wait::{filter_by_range, wait_for_range, WaitResult};
