//! Persistence model (DESIGN_ORG 3.4, 4.7): snapshot + event log + usage rows,
//! the `OrgStore` port, replay, and the restart classification. Pure: no I/O.
//! File implementations live in the host (they reuse `store/atomic.rs`).
use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::limits::{LimitError, TurnKind};
use crate::state::{Cause, ConfirmError, TransitionError};
use crate::types::*;
use crate::Counters;

pub const SCHEMA_VERSION: u32 = 1;

/// Why dispatching is stopped for the whole workflow. Cleared only by a user action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaltReason {
    /// Restored after an app restart: user must press "resume".
    Restart,
    /// Shutdown sequence started (scheduler stopped).
    Shutdown,
    User,
}

/// Snapshot of one workflow (`workflow.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowState {
    pub schema_version: u32,
    pub workflow_id: WorkflowId,
    pub limits: Limits,
    pub counters: Counters,
    pub tasks: Vec<Task>,
    pub halt: Option<HaltReason>,
    /// Sequence number of the last event reflected in this state.
    pub last_seq: u64,
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
        }
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
    pub fn apply(&mut self, ev: &OrgEvent) -> Result<(), ApplyError> {
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
        .map(|t| t.id.clone())
        .collect();
    for id in ids {
        let to = ConfirmState::NeedsReconcile(ReconcileCause::Restart);
        events.push(state.record(OrgEventKind::ConfirmChanged { task: id.clone(), to }, at_ms)?);
        changed.push(Reclassified { task: id, from: ConfirmState::Confirmed, to });
    }
    if state.halt != Some(HaltReason::Restart) {
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
        PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: deps.iter().map(|d| TaskId::new(*d)).collect(), effects: vec![] }
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
        early.tasks.truncate(0);
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
