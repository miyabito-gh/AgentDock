use super::*;
use crate::backend::backend::BackendError;
use crate::backend::model::{ChatKey, Known, SendAttempt, SendState, TurnKey, UnixMillis};
use crate::backend::parity::SandboxCheck;
use crate::host::org::grant::{CreatePermit, DispatchPermit, GrantRegistry};
use crate::host::org::ports::CreateOutcome;
use crate::rules::org::DescendantReport;
use async_trait::async_trait;
use org_engine::{EventRead, Limits, OrgEvent, RoleChatSpec, SettleOutcome, SnapshotLoad, StoreError, UsageRow as Row, WorkflowState};
use std::sync::Mutex;

type Log = Arc<Mutex<Vec<String>>>;

struct MemStore {
    log: Log,
    events: Mutex<Vec<OrgEvent>>,
    fail_after: Option<usize>,
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
}

struct Fake {
    reg: GrantRegistry,
    log: Log,
    live: Mutex<bool>,
    sandbox_exit: Mutex<i32>,
    sends: Mutex<usize>,
    creates: Mutex<usize>,
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
    async fn descendants(&self, _chat: &ChatKey) -> DescendantReport {
        DescendantReport { working: 0, unknown: 0, scan_complete: true }
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
    let log: Log = Arc::default();
    let fake = Arc::new(Fake { reg: GrantRegistry::default(), log: log.clone(), live: Mutex::new(true), sandbox_exit: Mutex::new(0), sends: Mutex::new(0), creates: Mutex::new(0) });
    let sink = Arc::new(Sink::default());
    let store: DynStore = Arc::new(MemStore { log: log.clone(), events: Mutex::new(Vec::new()), fail_after });
    let driver = OrgDriver::new(OrgCore::new(0), store, fake.clone(), sink.clone(), Box::new(|| 1000));
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
