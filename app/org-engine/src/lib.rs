//! Pure organization-run logic. No I/O, no async, no Tauri/Codex types.
//! Design: app/DESIGN_ORG.md (sections 4.1 to 4.9).

pub mod dag;
pub mod limits;
pub mod persist;
pub mod policy;
pub mod scheduler;
pub mod state;
pub mod types;

pub use dag::{isolated_tasks, validate_plan, PlanError};
pub use limits::{Counters, LimitError, TurnKind};
pub use policy::{evaluate_plan, ConfirmReason, Decision, PlanKind, PolicyInput};
pub use scheduler::{next_dispatch, promote_ready, DispatchGate};
pub use state::{is_allowed, Cause, ConfirmError, TransitionError};
pub use types::*;
pub use persist::{
    classify_after_restart, parse_events, parse_usage, recover_on_restart, restore, summarize_usage, ApplyError, EventRead,
    HaltReason, InvalidState, OrgEvent, OrgEventKind, OrgStore, Recovered, Reclassified, RestoreReport, SnapshotLoad, StoreError,
    UsageRow, UsageSummary, WorkflowState,
};
