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

/// How an observed turn ended (live observation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TurnEndKind {
    Completed,
    Failed { usage_limit: bool },
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
    pub fn attempts(&self) -> u32 {
        1 + self.rework_count
    }
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
        }
    }
}

impl Limits {
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
