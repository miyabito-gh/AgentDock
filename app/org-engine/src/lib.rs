//! Pure organization-run logic. No I/O, no async, no Tauri/Codex types.
//! Design: app/DESIGN_ORG.md (sections 4.1 to 4.9), app/DESIGN_ORG_P4.md (section 2).

pub mod core;
pub mod dag;
pub mod graph;
mod interject;
pub mod intervene;
mod graphrun;
pub mod limits;
pub mod outschema;
pub mod persist;
pub mod notes;
mod planned;
pub mod policy;
pub mod prompt;
pub mod roledef;
pub mod scheduler;
pub mod schemas;
pub mod state;
pub mod types;

pub use core::{
    RestoredExtras,
    retry_kind, retryable, standard_roles, RetryKind, RetryReason, RetryRefusal, CreateResult, NoticeUrgency, OrgAction, OrgCore, OrgInput, OrgNotice, RoleChatSpec, SandboxKind, WorkspaceAction,
    WorkspaceOutcome, WorktreeBase,
};
pub use dag::{isolated_tasks, validate_plan, PlanError};
pub use limits::{Counters, LimitError, TurnKind};
pub use policy::{evaluate_plan, ConfirmReason, Decision, PlanKind, PolicyInput};
pub use roledef::{
    compare_breadth, load_role, permission_fingerprint, standard_definitions, ApprovalPolicy, Breadth, LoadedRole, RoleDefinition, Workplace,
};
pub use intervene::{accept_as_is_check, is_reviewer, is_worker, reviewers_of, send_back_check, skip_plan, SendBackPlan, SkipPlan};
pub use notes::{note_reach, notes_for, pending_note_count, MAX_PENDING_NOTES};
pub use scheduler::{next_dispatch, promote_ready, DispatchGate};
pub use state::{is_allowed, Cause, ConfirmError, TransitionError};
pub use types::*;
pub use persist::{
    interject_can_settle, Interject, InterjectOutcome,
    classify_after_restart, parse_events, parse_usage, recover_on_restart, restore, summarize_usage, ApplyError, EventRead,
    GraphRef, HaltReason, InvalidState, OrgEvent, OrgEventKind, OrgStore, Recovered, Reclassified, RestoreReport, SnapshotLoad, StoreError,
    UsageRow, UsageSummary, UserNote, WorkflowState, LEGACY_GRAPH_ID,
};
