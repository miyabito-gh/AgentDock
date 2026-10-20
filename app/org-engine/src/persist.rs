//! Persistence model (DESIGN_ORG 3.4, 4.7): snapshot + event log + usage rows,
//! the `OrgStore` port, replay, and the restart classification. Pure: no I/O.
//! File implementations live in the host (they reuse `store/atomic.rs`).
use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::graph::def::{EdgeId, NodeId, SlotId};
use crate::limits::{LimitError, TurnKind};
use crate::state::{Cause, ConfirmError, TransitionError};
use crate::types::*;
use crate::Counters;

/// 2 = graph events (DESIGN_ORG_GRAPH 7.6). Version 1 lines are still read.
pub const SCHEMA_VERSION: u32 = 2;

/// The graph a workflow was started with (a copy is kept by the host; the fingerprint detects mix-ups).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRef {
    pub graph_id: String,
    pub fingerprint: String,
    pub roles_fingerprint: String,
}

/// A note the user wrote (a reason, an addition). The text is kept in the event log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserNote {
    pub id: NoteId,
    pub text: String,
    pub scope: NoteScope,
    /// Event seq at which the note was added.
    pub seq: u64,
    /// The user took the note back: it is not added to any later turn.
    #[serde(default)]
    pub withdrawn: bool,
}

/// How an interjection (a `turn/steer` into a running turn) ended. Decided once; only the listed later
/// steps may replace an earlier outcome (`interject_can_settle`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum InterjectOutcome {
    /// Accepted into the turn. `note` = a remark (e.g. it went into another turn).
    Delivered { note: Option<String> },
    /// Certainly not delivered (rejected, not live).
    NotDelivered { reason: String },
    /// The outcome of the send is not known. Nothing is sent again.
    Unknown,
    ReconciledDelivered,
    ReconciledNotFound,
    /// The same text was added as a note of the task (the user's choice, or "fall back" at send time).
    FallbackNoted,
    Discarded,
}

/// Whether `new` may be recorded after `prev` (None = nothing decided yet).
pub fn interject_can_settle(prev: Option<&InterjectOutcome>, new: &InterjectOutcome) -> bool {
    use InterjectOutcome::*;
    match prev {
        None => !matches!(new, FallbackNoted | Discarded),
        Some(Unknown) => matches!(new, ReconciledDelivered | ReconciledNotFound | FallbackNoted | Discarded),
        Some(ReconciledNotFound) => matches!(new, FallbackNoted | Discarded),
        Some(NotDelivered { .. }) => matches!(new, FallbackNoted),
        Some(_) => false,
    }
}

/// An interjection the user sent into a running turn (DESIGN_ORG_R2 2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interject {
    pub id: NoteId,
    pub task: TaskId,
    /// The turn the text was addressed to (`expectedTurnId`).
    pub turn: TurnRef,
    pub text: String,
    /// "If it does not arrive, add it to the next send": decided by the user when sending.
    pub fallback: bool,
    pub seq: u64,
    /// None = not decided yet.
    pub outcome: Option<InterjectOutcome>,
}

/// Graph id of a workflow that has no `GraphBound` record: the built-in template 1.
pub const LEGACY_GRAPH_ID: &str = "standard";

/// Why dispatching is stopped for the whole workflow. Cleared only by a user action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaltReason {
    /// Restored after an app restart: user must press "resume".
    Restart,
    /// Shutdown sequence started (scheduler stopped).
    Shutdown,
    User,
    /// A chat that must be sent to is not live (disconnect, wake, App Server restart).
    /// Nothing is resumed automatically; the user's "resume workflow" action does it.
    NeedsResume,
    /// The sandbox health check failed. Cleared by the user's "resume" (re-checks first).
    SandboxUnhealthy,
    /// A plan is waiting for the user's approval. Cleared only by approving the plan.
    AwaitingPlanApproval,
    /// The workflow runs a graph but its start-time copy cannot be used (missing, or the fingerprint differs).
    /// Set when the engine is restored; "resume" does not clear it while the definition is still unavailable.
    DefinitionUnavailable,
    /// A turn failed because of a usage limit. Cleared only by the user's "resume".
    UsageLimit,
}

/// Snapshot of one workflow (`workflow.json`). Fields are read-only outside the
/// crate; the only way to change a state is `record`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowState {
    pub(crate) schema_version: u32,
    pub(crate) workflow_id: WorkflowId,
    pub(crate) limits: Limits,
    pub(crate) counters: Counters,
    pub(crate) tasks: Vec<Task>,
    pub(crate) halt: Option<HaltReason>,
    /// Sequence number of the last event reflected in this state.
    pub(crate) last_seq: u64,
    /// The user's request and the repository (set once by `Requested`).
    #[serde(default)]
    pub(crate) request: String,
    #[serde(default)]
    pub(crate) repo: String,
    /// None = no `GraphBound` event: the workflow runs as the built-in `standard`.
    #[serde(default)]
    pub(crate) graph: Option<GraphRef>,
    /// Per feedback edge: (rounds used, rounds added by the user beyond the definition's `maxRounds`).
    /// Counted over the whole workflow and never reset.
    #[serde(default)]
    pub(crate) feedback: BTreeMap<EdgeId, (u32, u32)>,
    /// Latest version of each slot (the values are files kept by the host).
    #[serde(default)]
    pub(crate) slots: BTreeMap<SlotId, u32>,
    #[serde(default)]
    pub(crate) notes: Vec<UserNote>,
    #[serde(default)]
    pub(crate) interjects: Vec<Interject>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidState {
    SchemaVersion(u32),
    DuplicateTask(TaskId),
    UnknownDependency { task: TaskId, dep: TaskId },
    SelfDependency(TaskId),
}

impl WorkflowState {
    pub fn new(workflow_id: WorkflowId, limits: Limits) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            workflow_id,
            limits,
            counters: Counters::default(),
            tasks: Vec::new(),
            halt: None,
            last_seq: 0,
            request: String::new(),
            repo: String::new(),
            graph: None,
            feedback: BTreeMap::new(),
            slots: BTreeMap::new(),
            notes: Vec::new(),
            interjects: Vec::new(),
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn workflow_id(&self) -> &WorkflowId {
        &self.workflow_id
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn counters(&self) -> &Counters {
        &self.counters
    }
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }
    pub fn halt(&self) -> Option<HaltReason> {
        self.halt
    }
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
    pub fn request(&self) -> &str {
        &self.request
    }
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn graph(&self) -> Option<&GraphRef> {
        self.graph.as_ref()
    }
    /// The graph the workflow runs: the bound one, or the built-in `standard` when none was recorded.
    pub fn graph_id(&self) -> &str {
        self.graph.as_ref().map_or(LEGACY_GRAPH_ID, |g| g.graph_id.as_str())
    }
    /// (rounds used, rounds added by the user) of a feedback edge.
    pub fn feedback_of(&self, edge: &EdgeId) -> (u32, u32) {
        self.feedback.get(edge).copied().unwrap_or((0, 0))
    }
    pub fn slot_version(&self, slot: &SlotId) -> Option<u32> {
        self.slots.get(slot).copied()
    }
    /// The latest version of every slot (what the host reads back after a restart).
    pub fn slot_versions(&self) -> impl Iterator<Item = (&SlotId, u32)> + '_ {
        self.slots.iter().map(|(s, v)| (s, *v))
    }
    pub fn notes(&self) -> &[UserNote] {
        &self.notes
    }
    pub fn interjects(&self) -> &[Interject] {
        &self.interjects
    }
    pub fn interject(&self, id: &NoteId) -> Option<&Interject> {
        self.interjects.iter().find(|i| &i.id == id)
    }

    /// Structural check for a deserialized snapshot. Always run it before use:
    /// `Task.status` / `confirm` are public, so JSON could carry any value.
    pub fn validate(&self) -> Result<(), InvalidState> {
        if self.schema_version == 0 || self.schema_version > SCHEMA_VERSION {
            return Err(InvalidState::SchemaVersion(self.schema_version));
        }
        let mut seen = HashSet::new();
        for t in &self.tasks {
            if !seen.insert(&t.id) {
                return Err(InvalidState::DuplicateTask(t.id.clone()));
            }
        }
        for t in &self.tasks {
            for d in &t.deps {
                if d == &t.id {
                    return Err(InvalidState::SelfDependency(t.id.clone()));
                }
                if !seen.contains(d) {
                    return Err(InvalidState::UnknownDependency { task: t.id.clone(), dep: d.clone() });
                }
            }
        }
        Ok(())
    }

    pub fn task(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| &t.id == id)
    }

    fn task_mut(&mut self, id: &TaskId) -> Result<&mut Task, ApplyError> {
        self.tasks.iter_mut().find(|t| &t.id == id).ok_or_else(|| ApplyError::UnknownTask(id.clone()))
    }
}

/// One line of `events.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgEvent {
    pub schema_version: u32,
    pub seq: u64,
    pub at_ms: i64,
    #[serde(flatten)]
    pub kind: OrgEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgEventKind {
    Created { workflow_id: WorkflowId, limits: Limits },
    TaskAdded { task: PlanTask },
    StatusChanged { task: TaskId, to: TaskStatus, cause: Cause },
    ConfirmChanged { task: TaskId, to: ConfirmState },
    ReworkCounted { task: TaskId },
    TurnSpent { kind: TurnKind },
    AutoReplanSpent,
    LimitsChanged { limits: Limits },
    Halted { reason: Option<HaltReason> },
    /// The user's request and repository (once, right after `Created`).
    Requested { request: String, repo: String },
    /// Creating the role chat starts. Recorded before the attempt so an unknown
    /// outcome still leaves a trace.
    ChatRequested { task: TaskId, cwd: String },
    /// The chat was created and its ownership record saved.
    ChatBound { task: TaskId, chat: ChatRef },
    /// The creation was certainly rejected (no chat exists), or the bound chat can no longer be resumed and
    /// nothing was ever dispatched to it: forget the request (and the binding) so a new chat can be made.
    ChatRequestAbandoned { task: TaskId },
    /// Recorded before sending (`TurnSpent` goes with it).
    /// `notes_upto`: the newest user note that rides along with this send (None = no note).
    Dispatched {
        task: TaskId,
        attempt: AttemptRef,
        client_message_id: String,
        kind: TurnKind,
        #[serde(default)]
        notes_upto: Option<u64>,
    },
    DispatchSettled { task: TaskId, attempt: AttemptRef, outcome: SettleOutcome },
    /// A turn end observed live. State changes only when it is the accepted turn.
    TurnObserved { task: TaskId, turn: TurnRef, end: TurnEndKind },
    /// A worker task's commit.
    Committed { task: TaskId, commit: String },
    /// The worker's turn ended with nothing to commit (it wrote no change). Offers a counted new attempt.
    CommitEmpty { task: TaskId },
    /// The graph the workflow started with (a copy's fingerprint). Without it the workflow runs as `standard`.
    GraphBound { graph_id: String, fingerprint: String, roles_fingerprint: String },
    /// A task was created for a node, through an edge (`via`) or as the entry.
    ActivationAdded { task: PlanTask, node: NodeId, via: Option<EdgeId> },
    /// A new version of a slot (the value is a file; the host writes it just before this event).
    SlotWritten { task: TaskId, slot: SlotId, version: u32, bytes: u32 },
    /// The edge chosen after the task's output (`edge: None` = no edge applies, the path ends here).
    /// `to` is the task the edge led to, if any.
    EdgeTaken { from: TaskId, edge: Option<EdgeId>, to: Option<TaskId> },
    /// One round of a feedback edge was used.
    FeedbackCounted { edge: EdgeId },
    /// The user added rounds to a feedback edge (`UserConfirmed`).
    FeedbackAllowanceRaised { edge: EdgeId, by: u32 },
    /// The user decided a gate.
    GateDecided { task: TaskId, choice: String, note: Option<NoteId> },
    UserNoteAdded { id: NoteId, text: String, scope: NoteScope },
    /// The user asked for a new attempt of a task whose turn failed / was interrupted (counted per task).
    RetryRequested { task: TaskId },
    /// The user adopts a turn they ran by hand as the task's result (the task was `Blocked(Manual)`).
    ManualTurnAdopted { task: TaskId, turn: TurnRef },
    /// The user sends text into a running turn (`turn/steer` with `turn` as `expectedTurnId`). Recorded before the send.
    InterjectRequested { id: NoteId, task: TaskId, turn: TurnRef, text: String, fallback: bool },
    InterjectSettled { id: NoteId, outcome: InterjectOutcome },
    /// The user took a note back; later turns do not carry it.
    UserNoteWithdrawn { id: NoteId },
    /// The user skipped a task (it is also `Cancelled` by a `StatusChanged` that follows).
    TaskSkipped { task: TaskId },
    /// The user sent a finished (or limit-stopped) task back. Not counted in `rework_count`.
    UserReworkRequested { task: TaskId },
    /// The review findings for a worker task (replaces the earlier ones).
    FindingsRecorded { task: TaskId, findings: Vec<String> },
    TaskHeld { task: TaskId, held: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    UnknownTask(TaskId),
    DuplicateTask(TaskId),
    Transition(TransitionError),
    Confirm(ConfirmError),
    Limit(LimitError),
    /// `Created` on an already created workflow.
    AlreadyCreated,
    AlreadyRequested,
    /// One task, one chat: a second request or a re-binding is refused.
    ChatConflict(TaskId),
    ChatNotRequested(TaskId),
    ChatNotBound(TaskId),
    /// A new dispatch while the previous attempt is unsettled or unknown.
    AttemptOpen(TaskId),
    UnknownAttempt(TaskId),
    DuplicateSettle(TaskId),
    AlreadyBound,
    /// A slot version must be the previous one plus one.
    BadSlotVersion(SlotId),
    DuplicateNote(NoteId),
    /// `ManualTurnAdopted` on a task that is not `Blocked(Manual)` and confirmed.
    NotAdoptable(TaskId),
    UnknownNote(NoteId),
    DuplicateInterject(NoteId),
    /// The interjection already has an outcome that cannot be replaced by this one.
    InterjectSettle(NoteId),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl WorkflowState {
    /// Apply an event through the same checked paths the live engine uses
    /// (transitions, counters), so a replay cannot create a state that the
    /// state machine forbids. On error the state is unchanged.
    pub(crate) fn apply(&mut self, ev: &OrgEvent) -> Result<(), ApplyError> {
        match &ev.kind {
            OrgEventKind::Created { .. } => return Err(ApplyError::AlreadyCreated),
            OrgEventKind::TaskAdded { task } => {
                if self.task(&task.id).is_some() {
                    return Err(ApplyError::DuplicateTask(task.id.clone()));
                }
                self.tasks.push(Task::from_plan(task));
            }
            OrgEventKind::StatusChanged { task, to, cause } => {
                let t = self.tasks.iter_mut().find(|t| &t.id == task).ok_or_else(|| ApplyError::UnknownTask(task.clone()))?;
                t.transition(*to, *cause).map_err(ApplyError::Transition)?;
            }
            OrgEventKind::ConfirmChanged { task, to } => {
                let t = self.tasks.iter_mut().find(|t| &t.id == task).ok_or_else(|| ApplyError::UnknownTask(task.clone()))?;
                t.set_confirm(*to).map_err(ApplyError::Confirm)?;
            }
            OrgEventKind::ReworkCounted { task } => {
                let t = self.tasks.iter_mut().find(|t| &t.id == task).ok_or_else(|| ApplyError::UnknownTask(task.clone()))?;
                t.rework_count += 1;
            }
            OrgEventKind::TurnSpent { kind } => {
                let limits = self.limits.clone();
                self.counters.spend_turn(*kind, &limits).map_err(ApplyError::Limit)?;
            }
            OrgEventKind::AutoReplanSpent => {
                let limits = self.limits.clone();
                self.counters.try_auto_replan(&limits).map_err(ApplyError::Limit)?;
            }
            OrgEventKind::LimitsChanged { limits } => self.limits = limits.clone(),
            OrgEventKind::Halted { reason } => self.halt = *reason,
            OrgEventKind::Requested { request, repo } => {
                if !self.request.is_empty() || !self.repo.is_empty() {
                    return Err(ApplyError::AlreadyRequested);
                }
                self.request = request.clone();
                self.repo = repo.clone();
            }
            OrgEventKind::ChatRequested { task, cwd } => {
                let t = self.task_mut(task)?;
                if t.chat.is_some() || t.chat_cwd.is_some() {
                    return Err(ApplyError::ChatConflict(task.clone()));
                }
                t.chat_cwd = Some(cwd.clone());
            }
            OrgEventKind::ChatBound { task, chat } => {
                let t = self.task_mut(task)?;
                if t.chat.is_some() {
                    return Err(ApplyError::ChatConflict(task.clone()));
                }
                if t.chat_cwd.is_none() {
                    return Err(ApplyError::ChatNotRequested(task.clone()));
                }
                t.chat = Some(chat.clone());
            }
            OrgEventKind::ChatRequestAbandoned { task } => {
                let t = self.task_mut(task)?;
                if t.chat_cwd.is_none() {
                    return Err(ApplyError::ChatNotRequested(task.clone()));
                }
                // A bound chat may be dropped only while nothing was ever dispatched to it (the chat never spoke).
                if t.chat.is_some() && (t.last_attempt.is_some() || t.accepted_turn.is_some()) {
                    return Err(ApplyError::ChatConflict(task.clone()));
                }
                t.chat = None;
                t.chat_cwd = None;
            }
            OrgEventKind::Dispatched { task, attempt, kind, notes_upto, .. } => {
                let t = self.task_mut(task)?;
                if t.chat.is_none() {
                    return Err(ApplyError::ChatNotBound(task.clone()));
                }
                if t.last_attempt.is_some() && matches!(t.last_settle, None | Some(SettleOutcome::Unknown)) {
                    return Err(ApplyError::AttemptOpen(task.clone()));
                }
                t.last_attempt = Some(attempt.clone());
                t.last_settle = None;
                // A new attempt keeps the flag: the one allowed format fix of this work turn is not given back.
                if *kind != TurnKind::Retry {
                    t.format_fix_used = *kind == TurnKind::FormatFix;
                }
                t.retry_pending = false;
                t.nothing_written = false;
                t.adopted_manual = false;
                t.notes_sent_upto = *notes_upto;
            }
            OrgEventKind::DispatchSettled { task, attempt, outcome } => {
                let t = self.task_mut(task)?;
                if t.last_attempt.as_ref() != Some(attempt) {
                    return Err(ApplyError::UnknownAttempt(task.clone()));
                }
                match (&t.last_settle, outcome) {
                    (None, _) => {}
                    (Some(SettleOutcome::Unknown), SettleOutcome::Unknown) => return Err(ApplyError::DuplicateSettle(task.clone())),
                    (Some(SettleOutcome::Unknown), _) => {}
                    _ => return Err(ApplyError::DuplicateSettle(task.clone())),
                }
                if let SettleOutcome::Accepted { turn } | SettleOutcome::ReconciledAccepted { turn } = outcome {
                    t.accepted_turn = Some(turn.clone());
                    t.observed = None;
                    // The notes that rode along arrived. A rejected or unknown send does not use them up.
                    t.notes_seen_seq = t.notes_seen_seq.max(t.notes_sent_upto.unwrap_or(0));
                }
                t.last_settle = Some(outcome.clone());
            }
            OrgEventKind::TurnObserved { task, turn, end } => {
                let t = self.task_mut(task)?;
                // A late turn that is not the accepted one is recorded (the event stays in the
                // log) but changes nothing.
                if t.accepted_turn.as_ref() == Some(turn) {
                    t.observed = Some(TurnObservation { turn: turn.clone(), end: *end });
                }
            }
            OrgEventKind::Committed { task, commit } => {
                let t = self.task_mut(task)?;
                t.commit = Some(commit.clone());
                t.nothing_written = false;
            }
            OrgEventKind::CommitEmpty { task } => {
                self.task_mut(task)?.nothing_written = true;
            }
            OrgEventKind::GraphBound { graph_id, fingerprint, roles_fingerprint } => {
                if self.graph.is_some() {
                    return Err(ApplyError::AlreadyBound);
                }
                self.graph = Some(GraphRef { graph_id: graph_id.clone(), fingerprint: fingerprint.clone(), roles_fingerprint: roles_fingerprint.clone() });
            }
            OrgEventKind::ActivationAdded { task, node, .. } => {
                if self.task(&task.id).is_some() {
                    return Err(ApplyError::DuplicateTask(task.id.clone()));
                }
                let mut t = Task::from_plan(task);
                t.node = Some(node.clone());
                self.tasks.push(t);
            }
            OrgEventKind::SlotWritten { task, slot, version, .. } => {
                if *version != self.slots.get(slot).copied().unwrap_or(0) + 1 {
                    return Err(ApplyError::BadSlotVersion(slot.clone()));
                }
                self.task_mut(task)?.last_output_seq = Some(ev.seq);
                self.slots.insert(slot.clone(), *version);
            }
            OrgEventKind::EdgeTaken { from, to, .. } => {
                let t = self.task_mut(from)?;
                t.edge_seq = Some(ev.seq);
                t.edge_to = to.clone();
            }
            OrgEventKind::FeedbackCounted { edge } => {
                self.feedback.entry(edge.clone()).or_default().0 += 1;
            }
            OrgEventKind::FeedbackAllowanceRaised { edge, by } => {
                self.feedback.entry(edge.clone()).or_default().1 += *by;
            }
            OrgEventKind::GateDecided { task, .. } => {
                self.task_mut(task)?;
            }
            OrgEventKind::UserNoteAdded { id, text, scope } => {
                if self.notes.iter().any(|n| &n.id == id) {
                    return Err(ApplyError::DuplicateNote(id.clone()));
                }
                self.notes.push(UserNote { id: id.clone(), text: text.clone(), scope: scope.clone(), seq: ev.seq, withdrawn: false });
            }
            OrgEventKind::UserNoteWithdrawn { id } => {
                let n = self.notes.iter_mut().find(|n| &n.id == id).ok_or_else(|| ApplyError::UnknownNote(id.clone()))?;
                n.withdrawn = true;
            }
            OrgEventKind::InterjectRequested { id, task, turn, text, fallback } => {
                if self.interjects.iter().any(|i| &i.id == id) {
                    return Err(ApplyError::DuplicateInterject(id.clone()));
                }
                self.task_mut(task)?;
                self.interjects.push(Interject { id: id.clone(), task: task.clone(), turn: turn.clone(), text: text.clone(), fallback: *fallback, seq: ev.seq, outcome: None });
            }
            OrgEventKind::InterjectSettled { id, outcome } => {
                let i = self.interjects.iter_mut().find(|i| &i.id == id).ok_or_else(|| ApplyError::UnknownNote(id.clone()))?;
                if !interject_can_settle(i.outcome.as_ref(), outcome) {
                    return Err(ApplyError::InterjectSettle(id.clone()));
                }
                i.outcome = Some(outcome.clone());
            }
            OrgEventKind::TaskSkipped { task } => {
                self.task_mut(task)?.skipped = true;
            }
            OrgEventKind::UserReworkRequested { task } => {
                self.task_mut(task)?.user_rework_count += 1;
            }
            OrgEventKind::FindingsRecorded { task, findings } => {
                self.task_mut(task)?.last_findings = findings.clone();
            }
            OrgEventKind::TaskHeld { task, held } => {
                self.task_mut(task)?.held = *held;
            }
            OrgEventKind::RetryRequested { task } => {
                let max = self.limits.max_retries;
                let t = self.task_mut(task)?;
                if t.retry_count >= max {
                    return Err(ApplyError::Limit(LimitError::RetryLimit { used: t.retry_count, max }));
                }
                t.retry_count += 1;
                t.retry_pending = true;
            }
            OrgEventKind::ManualTurnAdopted { task, turn } => {
                let t = self.task_mut(task)?;
                if t.status != TaskStatus::Blocked(BlockedReason::Manual) || t.confirm != ConfirmState::Confirmed {
                    return Err(ApplyError::NotAdoptable(task.clone()));
                }
                t.accepted_turn = Some(turn.clone());
                t.observed = Some(TurnObservation { turn: turn.clone(), end: TurnEndKind::Completed });
                t.adopted_manual = true;
            }
        }
        self.last_seq = ev.seq;
        Ok(())
    }

    /// Build the next event, apply it, and return it for the caller to append.
    /// Nothing changes when the event is rejected.
    pub fn record(&mut self, kind: OrgEventKind, at_ms: i64) -> Result<OrgEvent, ApplyError> {
        let ev = OrgEvent { schema_version: SCHEMA_VERSION, seq: self.last_seq + 1, at_ms, kind };
        self.apply(&ev)?;
        Ok(ev)
    }
}

/// Result of reading `events.jsonl`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventRead {
    pub events: Vec<OrgEvent>,
    /// A broken last line (crash while appending). Ignored, but reported.
    pub dropped_tail: Option<String>,
    /// 1-based numbers of unreadable lines that are not the last one. Skipped, reported.
    pub skipped_lines: Vec<usize>,
}

/// Parse `events.jsonl` text. An unreadable (or newer-schema) line is skipped; if it is
/// the last non-empty line it is reported as `dropped_tail`, otherwise in `skipped_lines`.
pub fn parse_events(text: &str) -> EventRead {
    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l)).filter(|(_, l)| !l.trim().is_empty()).collect();
    let mut out = EventRead::default();
    for (idx, (no, line)) in lines.iter().enumerate() {
        match serde_json::from_str::<OrgEvent>(line) {
            Ok(ev) if ev.schema_version >= 1 && ev.schema_version <= SCHEMA_VERSION => out.events.push(ev),
            _ if idx + 1 == lines.len() => out.dropped_tail = Some((*line).to_string()),
            _ => out.skipped_lines.push(*no),
        }
    }
    out
}

/// One usage row (`usage.jsonl`). Unknown values are `None`, never 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub schema_version: u32,
    pub at_ms: i64,
    pub workflow_id: WorkflowId,
    pub task: Option<TaskId>,
    pub kind: TurnKind,
    /// Turns used after counting this one.
    pub turns_used: u32,
    /// `thread/tokenUsage/updated` `last.totalTokens` (this turn).
    pub last_total_tokens: Option<u64>,
    /// `total.totalTokens` (thread cumulative).
    pub thread_total_tokens: Option<u64>,
    pub model_context_window: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageSummary {
    pub turns: u32,
    /// Sum of the known `last_total_tokens` only.
    pub known_tokens: u64,
    /// Rows without a token count (shown as "not obtained", not as 0).
    pub rows_without_tokens: u32,
}

pub fn summarize_usage(rows: &[UsageRow]) -> UsageSummary {
    let mut s = UsageSummary::default();
    for r in rows {
        s.turns = s.turns.max(r.turns_used);
        match r.last_total_tokens {
            Some(t) => s.known_tokens += t,
            None => s.rows_without_tokens += 1,
        }
    }
    s
}

/// Read usage rows, skipping unreadable lines (usage is auxiliary).
pub fn parse_usage(text: &str) -> Vec<UsageRow> {
    text.lines().filter_map(|l| serde_json::from_str::<UsageRow>(l).ok()).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    Io(String),
    /// A stored file could not be used; the host kept it under another name.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(s) => write!(f, "io: {s}"),
            StoreError::Corrupt(s) => write!(f, "corrupt: {s}"),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotLoad {
    Missing,
    Loaded(WorkflowState),
    /// Unreadable; the host moved it aside (name given). Events can still rebuild the state.
    Corrupt { aside: String },
}

/// Storage port for one workflow. Replaceable (e.g. SQLite) without touching the engine.
pub trait OrgStore {
    /// Replace `workflow.json` atomically.
    fn save_snapshot(&self, state: &WorkflowState) -> Result<(), StoreError>;
    fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError>;
    fn append_event(&self, ev: &OrgEvent) -> Result<(), StoreError>;
    fn read_events(&self) -> Result<EventRead, StoreError>;
    fn append_usage(&self, row: &UsageRow) -> Result<(), StoreError>;
    fn read_usage(&self) -> Result<Vec<UsageRow>, StoreError>;
    /// Write one version of a slot value (`slots\<slot>.<version>.json`, atomically). A store that cannot keep
    /// slot files fails (the workflow then stops instead of going on without the value).
    fn put_slot(&self, _slot: &SlotId, _version: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        Err(StoreError::Io("この保存口はスロットの値を保存できません".into()))
    }
    /// Write a displayed plannedTasks output (`outputs\<task>.<attempt>.json`, atomically). Display only; the engine never
    /// reads it, so a store that cannot keep it does not hold anything up.
    fn put_output(&self, _task: &TaskId, _attempt: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        Err(StoreError::Io("この保存口は出力を保存できません".into()))
    }
    /// Read one stored output back. `Ok(None)` = not there or not readable ("not obtained"; never guessed).
    fn read_output(&self, _task: &TaskId, _attempt: u32) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(None)
    }
    /// Read one version back. `Ok(None)` = not there or not readable ("not obtained"; never guessed).
    fn read_slot(&self, _slot: &SlotId, _version: u32) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedEvent {
    pub seq: u64,
    pub error: ApplyError,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub snapshot_used: bool,
    pub snapshot_corrupt_aside: Option<String>,
    pub snapshot_invalid: Option<InvalidState>,
    pub events_applied: usize,
    pub events_already_in_snapshot: usize,
    pub rejected: Vec<RejectedEvent>,
    pub dropped_tail: Option<String>,
    pub skipped_lines: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// No usable snapshot and the event log does not start with `Created`.
    NothingToRestore,
    Store(StoreError),
}

/// Rebuild the state from the snapshot (if usable) plus later events. If the snapshot
/// is missing/corrupt/invalid, replay the whole log (it begins with `Created`).
pub fn restore(store: &dyn OrgStore) -> Result<(WorkflowState, RestoreReport), RestoreError> {
    let mut report = RestoreReport::default();
    let mut base = match store.load_snapshot().map_err(RestoreError::Store)? {
        SnapshotLoad::Loaded(s) => match s.validate() {
            Ok(()) => {
                report.snapshot_used = true;
                Some(s)
            }
            Err(e) => {
                report.snapshot_invalid = Some(e);
                None
            }
        },
        SnapshotLoad::Corrupt { aside } => {
            report.snapshot_corrupt_aside = Some(aside);
            None
        }
        SnapshotLoad::Missing => None,
    };
    let read = store.read_events().map_err(RestoreError::Store)?;
    report.dropped_tail = read.dropped_tail;
    report.skipped_lines = read.skipped_lines;
    for ev in &read.events {
        if base.is_none() {
            match &ev.kind {
                OrgEventKind::Created { workflow_id, limits } => {
                    let mut s = WorkflowState::new(workflow_id.clone(), limits.clone());
                    s.last_seq = ev.seq;
                    base = Some(s);
                    report.events_applied += 1;
                }
                _ => report.rejected.push(RejectedEvent { seq: ev.seq, error: ApplyError::UnknownTask(TaskId::new("")) }),
            }
            continue;
        }
        let st = base.as_mut().unwrap();
        if ev.seq <= st.last_seq {
            report.events_already_in_snapshot += 1;
            continue;
        }
        match st.apply(ev) {
            Ok(()) => report.events_applied += 1,
            Err(e) => report.rejected.push(RejectedEvent { seq: ev.seq, error: e }),
        }
    }
    base.map(|s| (s, report)).ok_or(RestoreError::NothingToRestore)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reclassified {
    pub task: TaskId,
    pub from: ConfirmState,
    pub to: ConfirmState,
}

/// Restart classification (DESIGN_ORG 4.7), as events to append:
/// - an in-flight task (`running` / `reviewing`) that was `Confirmed` becomes
///   `NeedsReconcile(Restart)` (it may still be running outside AgentDock);
/// - `SendUnknown`, `InterruptRequested`, `StopUnconfirmed`, `NeedsReconcile(_)` are kept;
/// - terminal tasks and tasks that were not in flight are untouched;
/// - nothing becomes `cancelled` / `failed`;
/// - dispatching is halted (`Halted{Restart}`) until the user resumes.
pub fn classify_after_restart(state: &mut WorkflowState, at_ms: i64) -> Result<(Vec<OrgEvent>, Vec<Reclassified>), ApplyError> {
    let mut events = Vec::new();
    let mut changed = Vec::new();
    let ids: Vec<TaskId> = state
        .tasks
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Reviewing) && t.confirm == ConfirmState::Confirmed)
        // Its own turn has ended (observed): nothing may still be running on the Codex side (H2).
        .filter(|t| !t.is_idle_reviewing())
        // A gate or router that stopped while running sent nothing to Codex; the graph pass repairs it (R-G M2).
        .filter(|t| !t.is_non_model_node())
        .map(|t| t.id.clone())
        .collect();
    for id in ids {
        let to = ConfirmState::NeedsReconcile(ReconcileCause::Restart);
        events.push(state.record(OrgEventKind::ConfirmChanged { task: id.clone(), to }, at_ms)?);
        changed.push(Reclassified { task: id, from: ConfirmState::Confirmed, to });
    }
    // An interjection that was sent but not settled before the stop: whether it arrived is not known.
    // The host's read-only reconciliation may still settle it; nothing is sent again.
    let open: Vec<NoteId> = state.interjects.iter().filter(|i| i.outcome.is_none()).map(|i| i.id.clone()).collect();
    for id in open {
        events.push(state.record(OrgEventKind::InterjectSettled { id, outcome: InterjectOutcome::Unknown }, at_ms)?);
    }
    // A plan waiting for the user's approval stays as it is: overwriting it would drop the approval
    // card and let "resume" start unapproved tasks (R-ORG H1).
    if !matches!(state.halt, Some(HaltReason::Restart | HaltReason::AwaitingPlanApproval)) {
        events.push(state.record(OrgEventKind::Halted { reason: Some(HaltReason::Restart) }, at_ms)?);
    }
    Ok((events, changed))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoverError {
    Restore(RestoreError),
    Apply(ApplyError),
    Store(StoreError),
}

pub struct Recovered {
    pub state: WorkflowState,
    pub report: RestoreReport,
    pub reclassified: Vec<Reclassified>,
}

/// Startup recovery: restore, classify, append the classification events, then
/// write a fresh snapshot. Read-only toward Codex; dispatching stays halted.
pub fn recover_on_restart(store: &dyn OrgStore, at_ms: i64) -> Result<Recovered, RecoverError> {
    let (mut state, report) = restore(store).map_err(RecoverError::Restore)?;
    let (events, reclassified) = classify_after_restart(&mut state, at_ms).map_err(RecoverError::Apply)?;
    for ev in &events {
        store.append_event(ev).map_err(RecoverError::Store)?;
    }
    store.save_snapshot(&state).map_err(RecoverError::Store)?;
    Ok(Recovered { state, report, reclassified })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Mem {
        snap: RefCell<Option<WorkflowState>>,
        log: RefCell<String>,
        usage: RefCell<Vec<UsageRow>>,
    }
    impl OrgStore for Mem {
        fn save_snapshot(&self, s: &WorkflowState) -> Result<(), StoreError> {
            *self.snap.borrow_mut() = Some(s.clone());
            Ok(())
        }
        fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError> {
            Ok(self.snap.borrow().clone().map_or(SnapshotLoad::Missing, SnapshotLoad::Loaded))
        }
        fn append_event(&self, ev: &OrgEvent) -> Result<(), StoreError> {
            let mut l = self.log.borrow_mut();
            if !l.is_empty() && !l.ends_with('\n') {
                l.push('\n');
            }
            l.push_str(&serde_json::to_string(ev).unwrap());
            l.push('\n');
            Ok(())
        }
        fn read_events(&self) -> Result<EventRead, StoreError> {
            Ok(parse_events(&self.log.borrow()))
        }
        fn append_usage(&self, row: &UsageRow) -> Result<(), StoreError> {
            self.usage.borrow_mut().push(row.clone());
            Ok(())
        }
        fn read_usage(&self) -> Result<Vec<UsageRow>, StoreError> {
            Ok(self.usage.borrow().clone())
        }
    }

    fn pt(id: &str, deps: &[&str]) -> PlanTask {
        PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: deps.iter().map(|d| TaskId::new(*d)).collect(), effects: vec![], title: String::new(), instructions: String::new() }
    }

    /// Writes a small history: t1 completed, t2 running, t3 pending. Returns the live state.
    fn history(store: &Mem) -> WorkflowState {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        let mut evs = vec![OrgEvent { schema_version: 1, seq: 0, at_ms: 0, kind: OrgEventKind::Created { workflow_id: s.workflow_id.clone(), limits: s.limits.clone() } }];
        s.last_seq = 0;
        for k in [
            OrgEventKind::TaskAdded { task: pt("t1", &[]) },
            OrgEventKind::TaskAdded { task: pt("t2", &["t1"]) },
            OrgEventKind::TaskAdded { task: pt("t3", &["t2"]) },
            OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Ready, cause: Cause::Engine },
            OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Running, cause: Cause::Engine },
            OrgEventKind::TurnSpent { kind: TurnKind::Task },
            OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Completed, cause: Cause::Engine },
            OrgEventKind::StatusChanged { task: TaskId::new("t2"), to: TaskStatus::Ready, cause: Cause::Engine },
            OrgEventKind::StatusChanged { task: TaskId::new("t2"), to: TaskStatus::Running, cause: Cause::Engine },
        ] {
            evs.push(s.record(k, 1).unwrap());
        }
        for e in &evs {
            store.append_event(e).unwrap();
        }
        s
    }

    #[test]
    fn restore_ignores_broken_tail_and_reports_it() {
        let store = Mem::default();
        let live = history(&store);
        store.log.borrow_mut().push_str("{\"schemaVersion\":1,\"seq\":99,\"atMs\":1,\"type\":\"turnSp");
        let (s, rep) = restore(&store).unwrap();
        assert_eq!(s.tasks, live.tasks);
        assert_eq!(s.counters.turns_used, 1);
        assert!(rep.dropped_tail.is_some());
        assert!(rep.rejected.is_empty());
    }

    #[test]
    fn middle_garbage_is_skipped_and_reported() {
        let store = Mem::default();
        history(&store);
        let text = store.log.borrow().replacen('\n', "\nnot json\n", 1);
        let r = parse_events(&text);
        assert_eq!(r.skipped_lines, vec![2]);
        assert!(r.dropped_tail.is_none());
    }

    #[test]
    fn snapshot_plus_later_events() {
        let store = Mem::default();
        let mut live = history(&store);
        // snapshot taken early (seq 3), later events still in the log
        let (mut early, _) = restore(&store).unwrap();
        early.tasks.clear();
        let mut snap = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        for ev in store.read_events().unwrap().events.iter().take(4) {
            if snap.last_seq == 0 && matches!(ev.kind, OrgEventKind::Created { .. }) {
                continue;
            }
            snap.apply(ev).unwrap();
        }
        store.save_snapshot(&snap).unwrap();
        let (s, rep) = restore(&store).unwrap();
        assert!(rep.snapshot_used);
        assert!(rep.events_already_in_snapshot >= 1);
        live.last_seq = s.last_seq;
        assert_eq!(s, live);
    }

    #[test]
    fn restart_makes_running_needs_reconcile_and_halts() {
        let store = Mem::default();
        history(&store);
        let r = recover_on_restart(&store, 5).unwrap();
        let t2 = r.state.task(&TaskId::new("t2")).unwrap();
        assert_eq!(t2.status, TaskStatus::Running);
        assert_eq!(t2.confirm, ConfirmState::NeedsReconcile(ReconcileCause::Restart));
        assert_eq!(r.state.task(&TaskId::new("t1")).unwrap().status, TaskStatus::Completed);
        assert_eq!(r.state.task(&TaskId::new("t3")).unwrap().confirm, ConfirmState::Confirmed);
        assert_eq!(r.state.halt, Some(HaltReason::Restart));
        assert_eq!(r.reclassified.len(), 1);
        // the classification is persisted: a second restore yields the same state, no new changes
        let again = recover_on_restart(&store, 6).unwrap();
        assert_eq!(again.state, r.state);
        assert!(again.reclassified.is_empty());
    }

    #[test]
    fn restart_keeps_unknown_and_unconfirmed_and_never_cancels() {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        for (id, c) in [
            ("a", ConfirmState::SendUnknown),
            ("b", ConfirmState::StopUnconfirmed),
            ("c", ConfirmState::InterruptRequested),
            ("d", ConfirmState::NeedsReconcile(ReconcileCause::Wake)),
        ] {
            let mut t = Task::from_plan(&pt(id, &[]));
            t.status = TaskStatus::Running;
            t.confirm = c;
            s.tasks.push(t);
        }
        let before: Vec<_> = s.tasks.iter().map(|t| (t.status, t.confirm)).collect();
        let (_, changed) = classify_after_restart(&mut s, 1).unwrap();
        assert!(changed.is_empty());
        assert_eq!(before, s.tasks.iter().map(|t| (t.status, t.confirm)).collect::<Vec<_>>());
    }

    #[test]
    fn corrupt_snapshot_falls_back_to_full_replay() {
        let store = Mem::default();
        let live = history(&store);
        let (s, rep) = {
            struct C<'a>(&'a Mem);
            impl OrgStore for C<'_> {
                fn save_snapshot(&self, s: &WorkflowState) -> Result<(), StoreError> { self.0.save_snapshot(s) }
                fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError> { Ok(SnapshotLoad::Corrupt { aside: "workflow.json.corrupt-1".into() }) }
                fn append_event(&self, e: &OrgEvent) -> Result<(), StoreError> { self.0.append_event(e) }
                fn read_events(&self) -> Result<EventRead, StoreError> { self.0.read_events() }
                fn append_usage(&self, r: &UsageRow) -> Result<(), StoreError> { self.0.append_usage(r) }
                fn read_usage(&self) -> Result<Vec<UsageRow>, StoreError> { self.0.read_usage() }
            }
            restore(&C(&store)).unwrap()
        };
        assert_eq!(rep.snapshot_corrupt_aside.as_deref(), Some("workflow.json.corrupt-1"));
        assert_eq!(s.tasks, live.tasks);
    }

    #[test]
    fn replay_rejects_forbidden_transition() {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        s.record(OrgEventKind::TaskAdded { task: pt("t", &[]) }, 0).unwrap();
        let bad = OrgEvent {
            schema_version: 1,
            seq: 2,
            at_ms: 0,
            kind: OrgEventKind::StatusChanged { task: TaskId::new("t"), to: TaskStatus::Completed, cause: Cause::Engine },
        };
        assert!(s.apply(&bad).is_err());
        assert_eq!(s.last_seq, 1);
    }

    #[test]
    fn validate_catches_bad_snapshots() {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        s.tasks.push(Task::from_plan(&pt("a", &["zz"])));
        assert!(matches!(s.validate(), Err(InvalidState::UnknownDependency { .. })));
        s.tasks[0].deps.clear();
        s.tasks.push(Task::from_plan(&pt("a", &[])));
        assert!(matches!(s.validate(), Err(InvalidState::DuplicateTask(_))));
        s.tasks.pop();
        s.schema_version = 99;
        assert_eq!(s.validate(), Err(InvalidState::SchemaVersion(99)));
    }

    #[test]
    fn json_round_trip_and_usage() {
        let store = Mem::default();
        let live = history(&store);
        let j = serde_json::to_string(&live).unwrap();
        assert_eq!(serde_json::from_str::<WorkflowState>(&j).unwrap(), live);
        let rows = vec![
            UsageRow { schema_version: 1, at_ms: 1, workflow_id: WorkflowId::new("w"), task: None, kind: TurnKind::Plan, turns_used: 1, last_total_tokens: Some(100), thread_total_tokens: Some(100), model_context_window: Some(475000) },
            UsageRow { schema_version: 1, at_ms: 2, workflow_id: WorkflowId::new("w"), task: Some(TaskId::new("t1")), kind: TurnKind::Task, turns_used: 2, last_total_tokens: None, thread_total_tokens: None, model_context_window: None },
        ];
        let text: String = rows.iter().map(|r| serde_json::to_string(r).unwrap() + "\n").collect();
        let back = parse_usage(&text);
        assert_eq!(back, rows);
        let sum = summarize_usage(&back);
        assert_eq!((sum.turns, sum.known_tokens, sum.rows_without_tokens), (2, 100, 1));
    }
}
