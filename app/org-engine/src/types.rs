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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub role: RoleId,
    pub deps: Vec<TaskId>,
    pub effects: Vec<Effect>,
    pub status: TaskStatus,
    pub confirm: ConfirmState,
    /// Rework (send-back) count so far. Attempts = 1 + rework_count.
    pub rework_count: u32,
}

impl Task {
    pub fn from_plan(p: &PlanTask) -> Self {
        Self {
            id: p.id.clone(),
            role: p.role.clone(),
            deps: p.deps.clone(),
            effects: p.effects.clone(),
            status: TaskStatus::Pending,
            confirm: ConfirmState::Confirmed,
            rework_count: 0,
        }
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
