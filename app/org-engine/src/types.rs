use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($n:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $n(pub String);
        impl $n {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
        }
        impl fmt::Display for $n {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
id_type!(WorkflowId);
id_type!(TaskId);
id_type!(RoleId);
// Opaque references. The host encodes its own keys into them; the engine only
// compares and stores them.
id_type!(ChatRef);
id_type!(TurnRef);
id_type!(AttemptRef);
id_type!(NoteId);

use crate::graph::def::NodeId;

/// Roles of the graph tasks that are not sent to a model.
pub(crate) const ROLE_GATE: &str = "gate";
pub(crate) const ROLE_ROUTER: &str = "router";

/// How an observed turn ended (live observation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TurnEndKind {
    Completed,
    Failed { usage_limit: bool },
    /// The history shows the turn failed, but the cause cannot be classified (never guessed as a usage limit or not).
    FailedUnclassified,
    Interrupted,
}

/// Result of one send attempt. `Unknown` is never turned into a retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SettleOutcome {
    Accepted { turn: TurnRef },
    Rejected,
    Unknown,
    ReconciledAccepted { turn: TurnRef },
    ReconciledNotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnObservation {
    pub turn: TurnRef,
    pub end: TurnEndKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockedReason {
    AwaitingApproval,
    Manual,
    ManualStop,
    Limit,
    NeedsAttention,
    /// The turn failed because of a usage limit. The user decides (retry as a new attempt, fix as failed, ...).
    UsageLimit,
    /// The turn failed (not a usage limit, or the cause is unclassified). The user decides.
    TurnFailed,
    /// A feedback edge ran out of rounds (`onExhausted: ask`); the user decides (DESIGN_ORG_GRAPH 7.3).
    RoundLimit,
    /// A worker task used up its automatic send-backs after a failed review. The user decides
    /// (send it back once more, accept it as it is, or settle it as failed; DESIGN_ORG_R2 2.3).
    ReworkLimit,
    /// The reviewer said pass, but a check it ran failed. The user decides
    /// (send it back once more, accept it as it is, or settle it as failed; DESIGN_ORG_R4 2.2).
    CheckFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailReason {
    ExecutionFailed,
    InvalidArtifact,
    LimitReached,
}

/// Business state (axis 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Ready,
    Running,
    Blocked(BlockedReason),
    Reviewing,
    Rework,
    Completed,
    Failed(FailReason),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReconcileCause {
    Disconnect,
    Wake,
    Restart,
}

/// Confirmation state (axis 2). Anything but `Confirmed` freezes the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfirmState {
    Confirmed,
    SendUnknown,
    NeedsReconcile(ReconcileCause),
    InterruptRequested,
    StopUnconfirmed,
}

/// Declared effects of a task (self-reported by the planner; used only to ask
/// for confirmation, never as the sole basis for auto-approval).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    UserTreeWrite,
    PushMergeTag,
    OutsideWorkdir,
    NetworkOrInstall,
    SecretsOrAuth,
    DeletionPrimary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
    pub id: RoleId,
    /// false = definition widens permissions and has not been enabled by the user.
    pub enabled: bool,
    /// Counts toward the writer limit.
    pub writes: bool,
    /// Per-role concurrency cap from the role definition.
    pub max_concurrent: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub task: TaskId,
}

/// What a plannedTasks worker task produces. Absent in older data = `Change`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    /// Changes files; the result is a commit.
    #[default]
    Change,
    /// Reads only; the result is the structured report (no commit, no worktree branch).
    Investigate,
}

impl TaskKind {
    /// `Change` is not written (older lines stay byte-identical).
    pub fn is_change(&self) -> bool {
        *self == TaskKind::Change
    }
}

/// A task as proposed by a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: TaskId,
    pub role: RoleId,
    pub deps: Vec<TaskId>,
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "TaskKind::is_change")]
    pub kind: TaskKind,
}

/// Fields marked `pub(crate)` change only through `WorkflowState::record`
/// (checked transition plus the event to append). Read them via the getters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub role: RoleId,
    pub deps: Vec<TaskId>,
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "TaskKind::is_change")]
    pub kind: TaskKind,
    pub(crate) status: TaskStatus,
    pub(crate) confirm: ConfirmState,
    /// Rework (send-back) count so far. Attempts = 1 + rework_count.
    pub(crate) rework_count: u32,
    /// Role chat bound to this task (one task, one chat).
    #[serde(default)]
    pub(crate) chat: Option<ChatRef>,
    /// cwd requested for the role chat (set when creation starts, kept after binding).
    #[serde(default)]
    pub(crate) chat_cwd: Option<String>,
    #[serde(default)]
    pub(crate) last_attempt: Option<AttemptRef>,
    #[serde(default)]
    pub(crate) last_settle: Option<SettleOutcome>,
    /// Turn accepted by the backend for the latest settled attempt.
    #[serde(default)]
    pub(crate) accepted_turn: Option<TurnRef>,
    /// Last live-observed end of `accepted_turn`.
    #[serde(default)]
    pub(crate) observed: Option<TurnObservation>,
    /// Commit made for a worker task.
    #[serde(default)]
    pub(crate) commit: Option<String>,
    /// A format-fix request was sent for the current work turn (only one is allowed).
    #[serde(default)]
    pub(crate) format_fix_used: bool,
    /// Times the user asked for a new attempt after a failed / interrupted turn.
    #[serde(default)]
    pub(crate) retry_count: u32,
    /// The last turn ended with nothing to commit (the worker wrote no change). Cleared by `Dispatched` / `Committed`.
    #[serde(default)]
    pub(crate) nothing_written: bool,
    /// The next send is a new attempt (`TurnKind::Retry`). Cleared by `Dispatched`.
    #[serde(default)]
    pub(crate) retry_pending: bool,
    /// The result of a turn the user ran by hand is being adopted (the task is `Running` meanwhile).
    #[serde(default)]
    pub(crate) adopted_manual: bool,
    /// The graph node this task runs (None = a plannedTasks item, or template-1 data without a graph).
    #[serde(default)]
    pub(crate) node: Option<NodeId>,
    /// Event seq of the latest `SlotWritten` of this task.
    #[serde(default)]
    pub(crate) last_output_seq: Option<u64>,
    /// Event seq of the latest `EdgeTaken` out of this task (the edge decision for its latest output).
    #[serde(default)]
    pub(crate) edge_seq: Option<u64>,
    /// The task the latest `EdgeTaken` led to (None = no edge applied, or it ended the path / ran out of rounds).
    #[serde(default)]
    pub(crate) edge_to: Option<TaskId>,
    /// Newest note seq that rode along with the latest dispatch (becomes `notes_seen_seq` once the send is accepted).
    #[serde(default)]
    pub(crate) notes_sent_upto: Option<u64>,
    /// Notes up to this seq reached the task by an accepted send.
    #[serde(default)]
    pub(crate) notes_seen_seq: u64,
    /// Cancelled by the user's "skip" (not an ordinary cancel).
    #[serde(default)]
    pub(crate) skipped: bool,
    /// The user holds the task: the scheduler does not send it.
    #[serde(default)]
    pub(crate) held: bool,
    /// Send-backs the user asked for (not counted in `rework_count`).
    #[serde(default)]
    pub(crate) user_rework_count: u32,
    /// The latest review findings for this task (kept so a restart does not lose them).
    #[serde(default)]
    pub(crate) last_findings: Vec<String>,
    /// Attempt whose investigation report was recorded (`ReportRecorded`); the file is `outputs\<task>.<attempt>.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) report_attempt: Option<u32>,
}

impl Task {
    pub fn from_plan(p: &PlanTask) -> Self {
        Self {
            id: p.id.clone(),
            role: p.role.clone(),
            deps: p.deps.clone(),
            effects: p.effects.clone(),
            title: p.title.clone(),
            instructions: p.instructions.clone(),
            kind: p.kind,
            status: TaskStatus::Pending,
            confirm: ConfirmState::Confirmed,
            rework_count: 0,
            chat: None,
            chat_cwd: None,
            last_attempt: None,
            last_settle: None,
            accepted_turn: None,
            observed: None,
            commit: None,
            format_fix_used: false,
            retry_count: 0,
            retry_pending: false,
            nothing_written: false,
            adopted_manual: false,
            node: None,
            last_output_seq: None,
            edge_seq: None,
            edge_to: None,
            notes_sent_upto: None,
            notes_seen_seq: 0,
            skipped: false,
            held: false,
            user_rework_count: 0,
            last_findings: Vec::new(),
            report_attempt: None,
        }
    }
    pub fn status(&self) -> TaskStatus {
        self.status
    }
    pub fn confirm(&self) -> ConfirmState {
        self.confirm
    }
    pub fn rework_count(&self) -> u32 {
        self.rework_count
    }
    pub fn chat(&self) -> Option<&ChatRef> {
        self.chat.as_ref()
    }
    pub fn chat_cwd(&self) -> Option<&str> {
        self.chat_cwd.as_deref()
    }
    pub fn last_attempt(&self) -> Option<&AttemptRef> {
        self.last_attempt.as_ref()
    }
    pub fn last_settle(&self) -> Option<&SettleOutcome> {
        self.last_settle.as_ref()
    }
    pub fn accepted_turn(&self) -> Option<&TurnRef> {
        self.accepted_turn.as_ref()
    }
    pub fn observed(&self) -> Option<&TurnObservation> {
        self.observed.as_ref()
    }
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }
    pub fn format_fix_used(&self) -> bool {
        self.format_fix_used
    }
    pub fn retry_count(&self) -> u32 {
        self.retry_count
    }
    pub fn nothing_written(&self) -> bool {
        self.nothing_written
    }
    pub fn retry_pending(&self) -> bool {
        self.retry_pending
    }
    pub fn adopted_manual(&self) -> bool {
        self.adopted_manual
    }
    /// The user's manual result is being adopted: the task is `Running` and ordinary sends to its chat are refused.
    pub fn adopting(&self) -> bool {
        self.adopted_manual && self.status == TaskStatus::Running
    }
    pub fn node(&self) -> Option<&NodeId> {
        self.node.as_ref()
    }
    pub fn last_output_seq(&self) -> Option<u64> {
        self.last_output_seq
    }
    pub fn edge_seq(&self) -> Option<u64> {
        self.edge_seq
    }
    pub fn edge_to(&self) -> Option<&TaskId> {
        self.edge_to.as_ref()
    }
    pub fn notes_sent_upto(&self) -> Option<u64> {
        self.notes_sent_upto
    }
    pub fn notes_seen_seq(&self) -> u64 {
        self.notes_seen_seq
    }
    pub fn skipped(&self) -> bool {
        self.skipped
    }
    pub fn held(&self) -> bool {
        self.held
    }
    pub fn user_rework_count(&self) -> u32 {
        self.user_rework_count
    }
    pub fn last_findings(&self) -> &[String] {
        &self.last_findings
    }
    pub fn report_attempt(&self) -> Option<u32> {
        self.report_attempt
    }
    /// An investigation task: reads only, its result is a report.
    pub fn is_investigation(&self) -> bool {
        self.kind == TaskKind::Investigate
    }
    /// The latest event seq at which this task produced something (an output or an edge decision).
    pub(crate) fn seen_seq(&self) -> u64 {
        self.last_output_seq.unwrap_or(0).max(self.edge_seq.unwrap_or(0))
    }
    pub fn attempts(&self) -> u32 {
        1 + self.rework_count
    }
    /// A gate or router node that never sent anything to a model: nothing of it can be running on the Codex side,
    /// so a restart or a disconnect does not turn it into "needs reconcile" (the graph pass repairs it; R-G M2).
    pub fn is_non_model_node(&self) -> bool {
        self.node.is_some() && self.last_attempt.is_none() && (self.role.0 == ROLE_GATE || self.role.0 == ROLE_ROUTER)
    }
    /// Under review with its own accepted turn already observed to end: nothing of this task can still
    /// be running on the Codex side, so it needs no reconciliation and holds no running slot.
    pub fn is_idle_reviewing(&self) -> bool {
        self.status == TaskStatus::Reviewing
            && self.confirm == ConfirmState::Confirmed
            && (self.observed.as_ref().is_some_and(|o| Some(&o.turn) == self.accepted_turn.as_ref())
                // A graph node that never sent a turn (router, gate) has nothing running on the Codex side.
                || (self.node.is_some() && self.last_attempt.is_none()))
    }
}

/// Where a user's note (reason, addition) applies (DESIGN_ORG_GRAPH 7.6, 10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum NoteScope {
    /// The next turn of this node.
    Node { node: NodeId },
    /// Every turn sent from now on.
    AllFollowing,
    /// The next turn of this task (DESIGN_ORG_R2 2.2).
    Task { task: TaskId },
}

/// Limits. Defaults follow DESIGN_ORG section 0.2 (Q4, Q5, Q11) and 4.3/4.7.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_writers: u32,
    pub max_total: u32,
    pub max_rework: u32,
    pub max_auto_replans: u32,
    pub max_turns: u32,
    pub max_tasks: u32,
    /// Notification threshold only (never auto-interrupts).
    pub stagnation_minutes: u32,
    /// Per task: how many times the user may ask for a new attempt after a failed turn (0..=10).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

pub const DEFAULT_MAX_RETRIES: u32 = 3;
pub const MAX_RETRIES_CEILING: u32 = 10;

fn default_max_retries() -> u32 {
    DEFAULT_MAX_RETRIES
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_writers: 2,
            max_total: 3,
            max_rework: 2,
            max_auto_replans: 1,
            max_turns: 30,
            max_tasks: 20,
            stagnation_minutes: 30,
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

impl Limits {
    /// The retry limit is user-settable within 0..=10.
    pub fn with_max_retries(mut self, n: u32) -> Result<Self, String> {
        if n > MAX_RETRIES_CEILING {
            return Err("max retries must be within 0..=10".into());
        }
        self.max_retries = n;
        Ok(self)
    }
    /// Parallelism is user-settable within 1..=4 (Q4).
    pub fn with_parallelism(mut self, writers: u32, total: u32) -> Result<Self, String> {
        if !(1..=4).contains(&writers) || !(1..=4).contains(&total) {
            return Err("parallelism must be within 1..=4".into());
        }
        self.max_writers = writers;
        self.max_total = total;
        Ok(self)
    }
}
