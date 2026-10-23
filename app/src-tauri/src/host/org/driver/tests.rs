use super::*;
use crate::backend::backend::BackendError;
use crate::backend::model::{ChatKey, Known, SendAttempt, SendState, TurnEnd, TurnKey, UnixMillis};
use crate::backend::parity::SandboxCheck;
use crate::host::org::grant::{CreatePermit, DispatchPermit, GrantRegistry};
use crate::host::org::ports::{CreateOutcome, ReconcileRead, WorktreeFinding};
use crate::rules::org::DescendantReport;
use async_trait::async_trait;
use org_engine::{EventRead, Limits, OrgEvent, RoleChatSpec, SettleOutcome, SnapshotLoad, StoreError, UsageRow as Row, WorkflowState};
use std::sync::Mutex;

type Log = Arc<Mutex<Vec<String>>>;

struct MemStore {
    log: Log,
    events: Mutex<Vec<OrgEvent>>,
    fail_after: Option<usize>,
    /// 書いたスロットの値（`<スロット>.<版>`）。
    slots: Mutex<Vec<String>>,
    fail_slots: bool,
}

impl OrgStore for MemStore {
    fn save_snapshot(&self, _s: &WorkflowState) -> Result<(), StoreError> {
        Ok(())
    }
    fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError> {
        Ok(SnapshotLoad::Missing)
    }
    fn append_event(&self, ev: &OrgEvent) -> Result<(), StoreError> {
        let mut v = self.events.lock().unwrap();
        if self.fail_after.is_some_and(|n| v.len() >= n) {
            return Err(StoreError::Io("disk full".into()));
        }
        self.log.lock().unwrap().push(format!("persist:{:?}", ev.kind));
        v.push(ev.clone());
        Ok(())
    }
    fn read_events(&self) -> Result<EventRead, StoreError> {
        Ok(EventRead::default())
    }
    fn append_usage(&self, _r: &Row) -> Result<(), StoreError> {
        Ok(())
    }
    fn read_usage(&self) -> Result<Vec<Row>, StoreError> {
        Ok(Vec::new())
    }
    fn put_slot(&self, slot: &org_engine::graph::SlotId, version: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        if self.fail_slots {
            return Err(StoreError::Io("disk full".into()));
        }
        self.log.lock().unwrap().push(format!("slot:{}.{version}", slot.0));
        self.slots.lock().unwrap().push(format!("{}.{version}", slot.0));
        Ok(())
    }
    fn put_output(&self, task: &TaskId, attempt: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        if self.fail_slots {
            return Err(StoreError::Io("disk full".into()));
        }
        self.log.lock().unwrap().push(format!("report:{}.{attempt}", task.0));
        Ok(())
    }
}

struct Fake {
    reg: GrantRegistry,
    log: Log,
    live: Mutex<bool>,
    sandbox_exit: Mutex<i32>,
    sends: Mutex<usize>,
    creates: Mutex<usize>,
    clock: Arc<std::sync::atomic::AtomicI64>,
    reconcile: Mutex<Option<TurnEnd>>,
    reconcile_reads: Mutex<usize>,
    reconcile_failure: Mutex<Known<crate::backend::backend::FailureClass>>,
}

#[async_trait]
impl AgentRuntime for Fake {
    fn grants(&self) -> &GrantRegistry {
        &self.reg
    }
    async fn create_role_chat(&self, permit: CreatePermit<'_>, _spec: RoleChatSpec) -> CreateOutcome {
        *self.creates.lock().unwrap() += 1;
        self.log.lock().unwrap().push("create".into());
        CreateOutcome { result: CreateResult::Created(ChatRef::new(format!("codex:chat-{}", permit.task()))), trust: None }
    }
    async fn steer(&self, _permit: crate::host::org::grant::InterjectPermit, req: crate::host::org::ports::OrgSteerSpec) -> OrgSendResult {
        self.log.lock().unwrap().push("steer".into());
        OrgSendResult::Dispatched(SendAttempt { attempt_id: req.attempt, client_message_id: req.client_message_id, at: UnixMillis(0), state: SendState::Sending })
    }
    async fn send(&self, permit: DispatchPermit<'_>, req: OrgSendSpec) -> OrgSendResult {
        assert_eq!(permit.chat(), &req.chat);
        if !*self.live.lock().unwrap() {
            return OrgSendResult::NotLive;
        }
        *self.sends.lock().unwrap() += 1;
        self.log.lock().unwrap().push("send".into());
        OrgSendResult::Dispatched(SendAttempt { attempt_id: req.attempt, client_message_id: req.client_message_id, at: UnixMillis(0), state: SendState::Sending })
    }
}

#[async_trait]
impl RuntimeRead for Fake {
    async fn read_final_message(&self, _c: &ChatKey, _t: &TurnKey) -> Result<Option<String>, BackendError> {
        Ok(None)
    }
    async fn list_by_cwd(&self, _cwd: &str) -> Result<Vec<ChatKey>, BackendError> {
        Ok(Vec::new())
    }
    async fn sandbox_check(&self, _cwd: &str) -> Result<SandboxCheck, BackendError> {
        Ok(SandboxCheck { readiness: Known::direct("ready".to_string()), exit_code: Known::direct(*self.sandbox_exit.lock().unwrap()), stderr_head: Known::NotFetched })
    }
    async fn read_reconcile(&self, _chat: &ChatKey, _turn: &TurnKey, _cwd: Option<&str>) -> ReconcileRead {
        *self.reconcile_reads.lock().unwrap() += 1;
        ReconcileRead { turn_read: true, turn_end: *self.reconcile.lock().unwrap(), turn_failure: self.reconcile_failure.lock().unwrap().clone(), worktree: WorktreeFinding::NotOwned }
    }
    async fn descendants(&self, _chat: &ChatKey) -> DescendantReport {
        DescendantReport { working: 0, unknown: 0, scan_complete: true }
    }
}

#[async_trait]
impl Workspace for Fake {
    async fn run(&self, _workflow: &WorkflowId, _action: WorkspaceAction) -> WorkspaceOutcome {
        WorkspaceOutcome::Unsupported
    }
}

#[derive(Default)]
struct Sink {
    notices: Mutex<Vec<OrgHostNotice>>,
}
impl OrgSink for Sink {
    fn publish(&self, _s: OrgSnapshot) {}
    fn notice(&self, _w: &WorkflowId, n: OrgHostNotice) {
        self.notices.lock().unwrap().push(n);
    }
}

struct Rig {
    driver: OrgDriver<Fake>,
    fake: Arc<Fake>,
    sink: Arc<Sink>,
    log: Log,
}

fn rig(fail_after: Option<usize>) -> Rig {
    rig_full(fail_after, false)
}

fn rig_full(fail_after: Option<usize>, fail_slots: bool) -> Rig {
    let log: Log = Arc::default();
    let fake = Arc::new(Fake { reg: GrantRegistry::default(), log: log.clone(), live: Mutex::new(true), sandbox_exit: Mutex::new(0), sends: Mutex::new(0), creates: Mutex::new(0), clock: Arc::new(std::sync::atomic::AtomicI64::new(1000)), reconcile: Mutex::new(None), reconcile_reads: Mutex::new(0), reconcile_failure: Mutex::new(Known::NotFetched) });
    let sink = Arc::new(Sink::default());
    let store: DynStore = Arc::new(MemStore { log: log.clone(), events: Mutex::new(Vec::new()), fail_after, slots: Mutex::new(Vec::new()), fail_slots });
    let clock = fake.clock.clone();
    let driver = OrgDriver::new(OrgCore::new(0), store, fake.clone(), sink.clone(), Box::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)));
    Rig { driver, fake, sink, log }
}

async fn start(r: &mut Rig) {
    let grant = OrgGrant::issue_for_test(WorkflowId::new("w1"), &r.fake.reg);
    r.driver.on_msg(DriverMsg::Install(grant)).await;
    let input = OrgInput::Start { workflow: WorkflowId::new("w1"), request: "do it".into(), repo: "C:/repo".into(), limits: Limits::default() };
    r.driver.on_msg(DriverMsg::Input(input)).await;
}

fn sends(r: &Rig) -> usize {
    *r.fake.sends.lock().unwrap()
}

#[tokio::test]
async fn dispatched_is_recorded_before_the_send_and_the_send_happens_once() {
    let mut r = rig(None);
    start(&mut r).await;
    assert_eq!(sends(&r), 1);
    let log = r.log.lock().unwrap().clone();
    let send_at = log.iter().position(|l| l == "send").unwrap();
    let dispatched_at = log.iter().position(|l| l.contains("Dispatched")).unwrap();
    let create_at = log.iter().position(|l| l == "create").unwrap();
    let requested_at = log.iter().position(|l| l.contains("ChatRequested")).unwrap();
    assert!(dispatched_at < send_at, "{log:?}");
    assert!(requested_at < create_at, "{log:?}");
}

#[tokio::test]
async fn a_failed_append_stops_every_later_effect() {
    // Created, Requested, TaskAdded and the first status change are appended; the chat request is not.
    let mut r = rig(Some(4));
    start(&mut r).await;
    assert_eq!(*r.fake.creates.lock().unwrap(), 0, "no chat is created without its record");
    assert_eq!(sends(&r), 0);
    assert!(r.sink.notices.lock().unwrap().iter().any(|n| matches!(n, OrgHostNotice::PersistFailed)));
}

#[tokio::test]
async fn an_unknown_send_is_never_sent_again() {
    let mut r = rig(None);
    start(&mut r).await;
    let attempt = r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().last_attempt().unwrap().clone();
    r.driver.on_msg(DriverMsg::Input(OrgInput::SendSettled { task: TaskId::new("planner-1"), attempt, outcome: SettleOutcome::Unknown })).await;
    for n in 1..=6 {
        r.driver.on_tick(n).await;
    }
    assert_eq!(sends(&r), 1);
}

#[tokio::test]
async fn freshness_drops_the_grant_and_nothing_is_sent_until_a_new_one_is_installed() {
    let mut r = rig(None);
    start(&mut r).await;
    assert_eq!(sends(&r), 1);
    r.driver.on_msg(DriverMsg::Input(OrgInput::Freshness { cause: org_engine::ReconcileCause::Disconnect })).await;
    assert!(r.driver.grant.is_none());
    let chat = crate::rules::org::chat_key_of_ref(&ChatRef::new("codex:x")).unwrap();
    // A grant issued before the loss of freshness cannot make permits any more.
    let stale = OrgGrant::issue_for_test(WorkflowId::new("w1"), &r.fake.reg);
    r.fake.reg.revoke_all();
    assert!(stale.permit(&r.fake.reg, &TaskId::new("t"), &chat).is_err());
    for n in 1..=4 {
        r.driver.on_tick(n).await;
    }
    r.driver.on_msg(DriverMsg::Input(OrgInput::UserResume)).await;
    assert_eq!(sends(&r), 1);
}

#[tokio::test]
async fn a_chat_that_is_not_live_is_not_sent_to_and_stops_for_resume() {
    let mut r = rig(None);
    *r.fake.live.lock().unwrap() = false;
    start(&mut r).await;
    assert_eq!(sends(&r), 0);
    let state = r.driver.core().state().unwrap();
    assert_eq!(state.halt(), Some(org_engine::HaltReason::NeedsResume));
    assert!(!matches!(state.task(&TaskId::new("planner-1")).unwrap().status(), TaskStatus::Failed(_) | TaskStatus::Cancelled));
}

#[tokio::test]
async fn an_unhealthy_sandbox_stops_before_the_first_turn() {
    let mut r = rig(None);
    *r.fake.sandbox_exit.lock().unwrap() = 1;
    start(&mut r).await;
    assert_eq!(sends(&r), 0);
    assert_eq!(r.driver.core().state().unwrap().halt(), Some(org_engine::HaltReason::SandboxUnhealthy));
    let notices = r.sink.notices.lock().unwrap().clone();
    assert!(notices.iter().any(|n| matches!(n, OrgHostNotice::Engine(org_engine::OrgNotice::SandboxUnhealthy { detail }) if detail.contains("sandbox"))));
}

#[tokio::test]
async fn workspace_actions_are_reported_unsupported_not_success() {
    let mut r = rig(None);
    start(&mut r).await;
    let action = OrgAction::Workspace(WorkspaceAction::Commit { task: TaskId::new("t"), cwd: "c".into(), message: "m".into() });
    let result = r.driver.exec(action, &mut false).await.unwrap();
    assert!(matches!(&result[0], OrgInput::WorkspaceResult { result: WorkspaceOutcome::Unsupported, .. }));
}

use std::sync::atomic::Ordering;

/// 計画役のタスクが送信を受理され、実行中（確認状態は確定）になるまで進める。
async fn running_planner(r: &mut Rig) -> (ChatRef, org_engine::TurnRef) {
    start(r).await;
    let task = TaskId::new("planner-1");
    let attempt = r.driver.core().state().unwrap().task(&task).unwrap().last_attempt().unwrap().clone();
    let turn = org_engine::TurnRef::new("turn-1");
    r.driver.on_msg(DriverMsg::Input(OrgInput::SendSettled { task: task.clone(), attempt, outcome: SettleOutcome::Accepted { turn: turn.clone() } })).await;
    let chat = r.driver.core().state().unwrap().task(&task).unwrap().chat().unwrap().clone();
    (chat, turn)
}

fn planner_confirm(r: &Rig) -> org_engine::ConfirmState {
    r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().confirm()
}

fn reconciled(r: &Rig) -> Vec<Option<TurnEnd>> {
    r.sink.notices.lock().unwrap().iter().filter_map(|n| if let OrgHostNotice::Reconciled { turn_end, .. } = n { Some(*turn_end) } else { None }).collect()
}

#[tokio::test]
async fn reconciliation_only_reads_and_brings_back_a_confirmed_end() {
    let mut r = rig(None);
    running_planner(&mut r).await;
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::Confirmed);
    r.driver.on_msg(DriverMsg::Input(OrgInput::Freshness { cause: org_engine::ReconcileCause::Restart })).await;
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::NeedsReconcile(org_engine::ReconcileCause::Restart));

    // the history does not show an end: it stays "needs reconcile" (not completed, not failed)
    r.driver.on_msg(DriverMsg::Reconcile).await;
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::NeedsReconcile(org_engine::ReconcileCause::Restart));
    assert_eq!(reconciled(&r), vec![None]);

    // an end confirmed by the history comes back; nothing is sent, created or resumed, and no grant is taken
    *r.fake.reconcile.lock().unwrap() = Some(TurnEnd::Interrupted);
    r.driver.on_msg(DriverMsg::Reconcile).await;
    assert_eq!(reconciled(&r), vec![None, Some(TurnEnd::Interrupted)]);
    let t = r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().clone();
    assert_eq!(t.confirm(), org_engine::ConfirmState::Confirmed);
    assert_eq!(t.status(), TaskStatus::Blocked(org_engine::BlockedReason::ManualStop));
    assert_eq!((sends(&r), *r.fake.creates.lock().unwrap()), (1, 1));
    assert!(r.driver.grant.is_none());
    assert_eq!(*r.fake.reconcile_reads.lock().unwrap(), 2);
}

#[tokio::test]
async fn a_restored_workflow_does_not_dispatch_by_itself() {
    let mut r1 = rig(None);
    running_planner(&mut r1).await;
    let state = r1.driver.core().state().unwrap().clone();
    let mut r = rig(None);
    r.driver.core = OrgCore::restored(state, 0);
    assert!(r.driver.grant.is_none());
    for n in 1..=6 {
        r.driver.on_tick(n).await;
    }
    r.driver.on_msg(DriverMsg::Reconcile).await;
    r.driver.on_msg(DriverMsg::Input(OrgInput::Freshness { cause: org_engine::ReconcileCause::Restart })).await;
    for n in 7..=10 {
        r.driver.on_tick(n).await;
    }
    assert_eq!((sends(&r), *r.fake.creates.lock().unwrap()), (0, 0), "start-up and reconciliation never send or create");
    assert!(r.driver.grant.is_none());
}

#[tokio::test]
async fn shutdown_stops_dispatch_and_never_turns_a_task_into_cancelled_or_failed() {
    let mut r = rig(None);
    running_planner(&mut r).await;
    r.driver.on_msg(DriverMsg::Revoke).await;
    r.driver.on_msg(DriverMsg::Input(OrgInput::Shutdown)).await;
    assert_eq!(r.driver.core().state().unwrap().halt(), Some(org_engine::HaltReason::Shutdown));
    for n in 1..=6 {
        r.driver.on_tick(n).await;
    }
    // without a new grant even the user's resume sends nothing
    r.driver.on_msg(DriverMsg::Input(OrgInput::UserResume)).await;
    assert_eq!(sends(&r), 1);
    let t = r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().clone();
    assert!(!matches!(t.status(), TaskStatus::Cancelled | TaskStatus::Failed(_)));
    assert!(r.driver.grant.is_none());
}

#[tokio::test]
async fn a_quiet_running_task_gets_a_stall_notice_only() {
    let mut r = rig(None);
    running_planner(&mut r).await;
    r.driver.set_stall_threshold_ms(10 * 60_000);
    let (chat, _) = running_planner_chat(&r);
    let mut n = 0;
    let mut t = 1000;
    while t <= 1000 + 12 * 60_000 {
        r.fake.clock.store(t, Ordering::SeqCst);
        n += 1;
        r.driver.on_tick(n).await;
        t += 60_000;
    }
    let stalls = |r: &Rig| r.sink.notices.lock().unwrap().iter().filter(|x| matches!(x, OrgHostNotice::Stalled { .. })).count();
    assert_eq!(stalls(&r), 1);
    // notification only: not interrupted, nothing resent, the task is still running and confirmed
    assert_eq!(sends(&r), 1);
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::Confirmed);
    assert_eq!(r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().status(), TaskStatus::Running);
    // output restarts the count
    r.driver.on_msg(DriverMsg::Activity { chat, at_ms: t }).await;
    for _ in 0..5 {
        r.fake.clock.store(t, Ordering::SeqCst);
        n += 1;
        r.driver.on_tick(n).await;
        t += 60_000;
    }
    assert_eq!(stalls(&r), 1);
}

fn running_planner_chat(r: &Rig) -> (ChatRef, ()) {
    (r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().chat().unwrap().clone(), ())
}

#[tokio::test]
async fn a_task_that_asked_for_interrupt_is_settled_by_a_read_only_reconciliation() {
    let mut r = rig(None);
    let (chat, _) = running_planner(&mut r).await;
    r.driver.on_msg(DriverMsg::Input(OrgInput::InterruptRequested { chat })).await;
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::InterruptRequested);
    // nothing in the history yet: it stays as it is
    r.driver.on_msg(DriverMsg::Reconcile).await;
    assert_eq!(planner_confirm(&r), org_engine::ConfirmState::InterruptRequested);
    *r.fake.reconcile.lock().unwrap() = Some(TurnEnd::Interrupted);
    r.driver.on_msg(DriverMsg::Reconcile).await;
    let t = r.driver.core().state().unwrap().task(&TaskId::new("planner-1")).unwrap().clone();
    assert_eq!((t.confirm(), t.status()), (org_engine::ConfirmState::Confirmed, TaskStatus::Blocked(org_engine::BlockedReason::ManualStop)));
    assert_eq!((sends(&r), *r.fake.creates.lock().unwrap()), (1, 1), "reading only");
}

#[tokio::test]
async fn the_driver_task_ends_once_every_task_is_finished_so_a_new_workflow_can_start() {
    let mut r = rig(None);
    start(&mut r).await;
    assert!(!r.driver.is_finished());
    assert!(r.fake.reg.is_active(&WorkflowId::new("w1")));
    let Rig { driver, fake, .. } = r;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(driver.run(rx));
    // cancelling the only task (by the user) finishes the workflow
    tx.send(DriverMsg::Input(OrgInput::UserCancelTask { task: TaskId::new("planner-1") })).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("the driver stops by itself").unwrap();
    assert!(tx.is_closed(), "the send side is closed, which is what lets the host start another workflow");
    assert!(!fake.reg.is_active(&WorkflowId::new("w1")), "the send grant is revoked when the driver finishes");
}

fn slot_written(version: u32) -> OrgAction {
    OrgAction::Persist(OrgEvent {
        schema_version: SCHEMA_VERSION,
        seq: 9,
        at_ms: 0,
        kind: org_engine::OrgEventKind::SlotWritten { task: TaskId::new("plan-1"), slot: org_engine::graph::SlotId::new("spec"), version, bytes: 1 },
    })
}

fn put_slot(version: u32) -> OrgAction {
    OrgAction::PutSlot { slot: org_engine::graph::SlotId::new("spec"), version, value: serde_json::json!({"goal": "g"}) }
}

#[tokio::test]
async fn a_slot_value_is_written_before_its_record_is_appended() {
    let mut r = rig(None);
    let mut stop = false;
    assert!(r.driver.exec(put_slot(1), &mut stop).await.is_none());
    assert!(r.driver.exec(slot_written(1), &mut stop).await.is_none());
    assert!(!stop && !r.driver.persist_failed);
    let log = r.log.lock().unwrap().clone();
    let put = log.iter().position(|l| l == "slot:spec.1").unwrap();
    let rec = log.iter().position(|l| l.contains("SlotWritten")).unwrap();
    assert!(put < rec, "{log:?}");
}

#[tokio::test]
async fn a_slot_value_that_cannot_be_written_stops_everything_after_it() {
    let mut r = rig_full(None, true);
    start(&mut r).await;
    let creates_before = *r.fake.creates.lock().unwrap();
    let mut stop = false;
    assert!(r.driver.exec(put_slot(1), &mut stop).await.is_none());
    assert!(stop && r.driver.persist_failed);
    assert!(r.sink.notices.lock().unwrap().iter().any(|n| matches!(n, OrgHostNotice::SlotWriteFailed { slot } if slot == "spec")));
    // the record of that version is not appended, and no later effect runs (nothing is chosen from a value that is not there)
    assert!(r.driver.exec(slot_written(1), &mut stop).await.is_none());
    assert!(!r.log.lock().unwrap().iter().any(|l| l.contains("SlotWritten")));
    let create = OrgAction::CreateRoleChat {
        task: TaskId::new("plan-1"),
        spec: org_engine::RoleChatSpec { role: org_engine::RoleId::new("planner"), cwd: "C:/repo".into(), sandbox: org_engine::SandboxKind::ReadOnly, model: None, effort: None, approval: org_engine::ApprovalPolicy::OnRequest },
    };
    assert!(r.driver.exec(create, &mut stop).await.is_none());
    assert_eq!(*r.fake.creates.lock().unwrap(), creates_before);
    // a later write is not even tried
    assert!(r.driver.exec(put_slot(2), &mut stop).await.is_none());
    assert!(!r.log.lock().unwrap().iter().any(|l| l.starts_with("slot:")));
}

fn report_recorded() -> OrgAction {
    OrgAction::Persist(OrgEvent {
        schema_version: SCHEMA_VERSION,
        seq: 9,
        at_ms: 0,
        kind: org_engine::OrgEventKind::ReportRecorded { task: TaskId::new("plan-1"), attempt: 0, bytes: 1 },
    })
}

fn put_report() -> OrgAction {
    OrgAction::PutReport { task: TaskId::new("plan-1"), attempt: 0, value: serde_json::json!({"summary": "s"}) }
}

#[tokio::test]
async fn an_investigation_report_is_written_before_it_is_recorded() {
    let mut r = rig(None);
    let mut stop = false;
    assert!(r.driver.exec(put_report(), &mut stop).await.is_none());
    assert!(r.driver.exec(report_recorded(), &mut stop).await.is_none());
    assert!(!stop && !r.driver.persist_failed);
    let log = r.log.lock().unwrap().clone();
    let put = log.iter().position(|l| l == "report:plan-1.0").unwrap();
    let rec = log.iter().position(|l| l.contains("ReportRecorded")).unwrap();
    assert!(put < rec, "{log:?}");
}

#[tokio::test]
async fn an_investigation_report_that_cannot_be_written_stops_everything_after_it() {
    let mut r = rig_full(None, true);
    start(&mut r).await;
    let mut stop = false;
    assert!(r.driver.exec(put_report(), &mut stop).await.is_none());
    assert!(stop && r.driver.persist_failed);
    assert!(r.sink.notices.lock().unwrap().iter().any(|n| matches!(n, OrgHostNotice::SlotWriteFailed { slot } if slot.contains("plan-1"))));
    // the completion is not recorded without its report
    assert!(r.driver.exec(report_recorded(), &mut stop).await.is_none());
    assert!(!r.log.lock().unwrap().iter().any(|l| l.contains("ReportRecorded")));
}

/// 照合（読取りだけ）で失敗の終端を確認したときの行き先。区分が読めたものだけ上限か否かを決め、読めないものは区分不明にする。
/// どれも失敗（終端）にはならず、止まって利用者の選択を待つ。
#[tokio::test]
async fn a_failure_found_by_reconciliation_blocks_by_its_class_and_never_fails_the_task() {
    use crate::backend::backend::FailureClass;
    let cases: [(Known<FailureClass>, org_engine::BlockedReason); 4] = [
        (Known::direct(FailureClass::UsageLimit), org_engine::BlockedReason::UsageLimit),
        (Known::direct(FailureClass::Other), org_engine::BlockedReason::TurnFailed),
        (Known::Missing, org_engine::BlockedReason::TurnFailed),
        (Known::NotFetched, org_engine::BlockedReason::TurnFailed),
    ];
    for (class, reason) in cases {
        let mut r = rig(None);
        running_planner(&mut r).await;
        r.driver.on_msg(DriverMsg::Input(OrgInput::Freshness { cause: org_engine::ReconcileCause::Restart })).await;
        *r.fake.reconcile.lock().unwrap() = Some(TurnEnd::Failed);
        *r.fake.reconcile_failure.lock().unwrap() = class.clone();
        r.driver.on_msg(DriverMsg::Reconcile).await;
        let state = r.driver.core().state().unwrap();
        let t = state.task(&TaskId::new("planner-1")).unwrap();
        assert_eq!(t.status(), TaskStatus::Blocked(reason), "{class:?}");
        assert_eq!(t.confirm(), org_engine::ConfirmState::Confirmed);
        // Freshness で付いた「再開待ち」の停止のまま（照合は新しい送信も再開もしない）。
        assert_eq!(state.halt(), Some(org_engine::HaltReason::NeedsResume));
        assert_eq!(sends(&r), 1, "nothing is sent again");
    }
}
