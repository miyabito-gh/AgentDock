//! Sans-IO engine core (DESIGN_ORG_P4 section 2.3). `OrgCore::handle(OrgInput)` returns the
//! `OrgAction`s the host should carry out; the core never performs I/O, never awaits and has
//! no way to send, resume, interrupt or answer Codex requests by itself.
//!
//! Order rule: every event produced while handling an input is returned as
//! `OrgAction::Persist` *before* any `Send` / `CreateRoleChat` / `Workspace` action of that
//! batch. The host appends them first and sends only if the append succeeded.
//!
//! Flow: planner task -> plan validation -> (approval) -> worker tasks (own worktree, commit)
//! -> reviewer tasks (detached at the commit) -> verdict. Nothing here turns termination,
//! disconnection or an unknown send outcome into `cancelled` / `failed`.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::limits::TurnKind;
use crate::persist::{HaltReason, OrgEvent, OrgEventKind, WorkflowState, SCHEMA_VERSION};
use crate::policy::{evaluate_plan, ConfirmReason, Decision, PlanKind, PolicyInput};
use crate::prompt;
use crate::roledef::LoadedRole;
use crate::scheduler::{default_dep_ok, next_dispatch_with, promote_ready_with, DispatchGate};
use crate::schemas::{self, ReviewerVerdict, VerdictKind, WorkerReport, ROLE_PLANNER, ROLE_REVIEWER, ROLE_WORKER};
use crate::state::Cause;
use crate::types::*;
use crate::Counters;

// ---------------------------------------------------------------- host <-> engine messages

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxKind {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleChatSpec {
    pub role: RoleId,
    pub cwd: String,
    pub sandbox: SandboxKind,
    /// None = the chat default (role definition files come in P-ORG-8).
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CreateResult {
    Created(ChatRef),
    /// Certainly not created.
    Rejected(String),
    /// The outcome is not known (a chat may exist).
    Unknown,
    /// Created, but the ownership record could not be saved. Treated like `Unknown`.
    SaveFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorktreeBase {
    /// The repository's current HEAD.
    Head,
    DetachedAt(String),
}

/// Requests for the `Workspace` port (host implementation: P-ORG-5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorkspaceAction {
    CreateWorktree { task: TaskId, repo: String, base: WorktreeBase },
    /// Commit inside the task's own worktree (`cwd`).
    Commit { task: TaskId, cwd: String, message: String },
    /// Move a reviewer's own detached worktree (`cwd`) to the commit under review (a re-review after a
    /// send-back). The host checks the ownership record and that the worktree really is at `commit` afterwards;
    /// the outcome is `WorktreeCreated` (ready at that commit) or `Failed`.
    RefreshDetached { task: TaskId, cwd: String, commit: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorkspaceOutcome {
    WorktreeCreated { path: String },
    Committed { commit: String },
    /// Nothing changed; not inferred as success or failure.
    NothingToCommit,
    Failed { detail: String },
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeUrgency {
    Immediate,
    /// Aggregated by the notifier (2 second window).
    Aggregated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgNotice {
    PlanNeedsApproval { reasons: Vec<ConfirmReason> },
    PlanAutoApproved { tasks: usize },
    TaskCompleted { task: TaskId },
    TaskFailed { task: TaskId, reason: FailReason },
    TaskBlocked { task: TaskId, why: String },
    /// The turn limit was reached; raise it to continue.
    TurnLimitReached,
    UsageLimit { task: TaskId },
    SandboxUnhealthy { detail: String },
    /// Children whose state is unconfirmed were left as unconfirmed when the task finished.
    DescendantsUnconfirmed { task: TaskId, count: u32 },
    /// The task was cancelled by the user; the stop on the Codex side was not confirmed.
    CancelUnconfirmed { task: TaskId },
    WorkflowCompleted,
    Refused { reason: String },
}

impl OrgNotice {
    pub fn urgency(&self) -> NoticeUrgency {
        match self {
            OrgNotice::PlanAutoApproved { .. } | OrgNotice::TaskCompleted { .. } | OrgNotice::DescendantsUnconfirmed { .. } => {
                NoticeUrgency::Aggregated
            }
            _ => NoticeUrgency::Immediate,
        }
    }
}

/// Host -> engine. Serializable; arrival order is not assumed (turns are matched by `TurnRef`).
/// There is deliberately no input that asks the engine to send again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgInput {
    Start { workflow: WorkflowId, request: String, repo: String, limits: Limits },
    /// The host holds (or no longer holds) the send grant. Without it the engine produces no
    /// create / send / workspace-write action. A restored engine starts without it.
    Grant { granted: bool },
    /// Whether a chat's live subscription is active. The engine sends only to live chats.
    Live { chat: ChatRef, live: bool },
    UserResume,
    UserPause,
    /// The app is quitting: stop dispatching (a halt; nothing is cancelled or failed). Cleared only by the user's resume.
    Shutdown,
    UserCancelTask { task: TaskId },
    UserManual { task: TaskId },
    UserAcceptManual { task: TaskId },
    UserApprovePlan,
    UserReplan,
    UserRaiseTurnLimit { to: u32 },
    /// The user asks to try a blocked task again (a new attempt; only when it is certain nothing was sent).
    UserRetryTask { task: TaskId },
    ChatCreated { task: TaskId, result: CreateResult },
    SendSettled { task: TaskId, attempt: AttemptRef, outcome: SettleOutcome },
    TurnEnded { chat: ChatRef, turn: TurnRef, end: TurnEndKind },
    /// `None` = the final message could not be read (not obtained).
    FinalMessage { task: TaskId, turn: TurnRef, text: Option<String> },
    Descendants { chat: ChatRef, working: u32, unknown: u32, scan_complete: bool },
    /// Number of open approvals / questions in the chat (counted only; never answered here).
    RequestPending { chat: ChatRef, open: u32 },
    Freshness { cause: ReconcileCause },
    /// The host did not send because the chat was not live.
    NotLive { task: TaskId },
    WorkspaceResult { task: TaskId, result: WorkspaceOutcome },
    SandboxChecked { ok: bool, detail: String },
    /// The user pressed interrupt on this chat.
    InterruptRequested { chat: ChatRef },
    /// A command in this chat failed because the sandbox could not start (observed while running).
    SandboxFailure { chat: ChatRef },
    /// After a restart: the host matched a chat record to a task whose creation result was lost
    /// (record against record, not a guess). Only binds a task that requested a chat and has none.
    ChatRecovered { task: TaskId, chat: ChatRef },
    /// During the user's resume: the task's chat cannot be resumed (Codex has no history for it). Only a task that
    /// was certainly never sent to gets a new role chat; anything else stays stopped (never sent again).
    ChatUnresumable { task: TaskId },
    Tick { now_ms: i64 },
}

/// Engine -> host. Execute in order; report the result with an `OrgInput`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgAction {
    /// Append to events.jsonl first. If the append fails, do not carry out later actions.
    Persist(OrgEvent),
    CreateRoleChat { task: TaskId, spec: RoleChatSpec },
    CheckSandbox { cwd: String },
    Send { task: TaskId, chat: ChatRef, attempt: AttemptRef, client_message_id: String, text: String, output_schema: Value },
    /// Read-only.
    ReadFinalMessage { task: TaskId, chat: ChatRef, turn: TurnRef },
    Workspace(WorkspaceAction),
    Notify(OrgNotice),
    /// State changed; publish it to the screen.
    Publish,
}

// ---------------------------------------------------------------- roles

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoleKind {
    Planner,
    Worker,
    Reviewer,
}

fn role_kind(r: &RoleId) -> Option<RoleKind> {
    match r.0.as_str() {
        ROLE_PLANNER => Some(RoleKind::Planner),
        ROLE_WORKER => Some(RoleKind::Worker),
        ROLE_REVIEWER => Some(RoleKind::Reviewer),
        _ => None,
    }
}

/// Certain that nothing was sent for the task, so the user may try it again (R-ORG M2).
pub fn retryable(t: &Task) -> bool {
    let attempt_open = t.last_attempt().is_some() && matches!(t.last_settle(), None | Some(SettleOutcome::Unknown));
    t.status() == TaskStatus::Blocked(BlockedReason::NeedsAttention)
        && t.confirm() == ConfirmState::Confirmed
        && t.accepted_turn().is_none()
        && !attempt_open
        && (t.chat().is_some() || t.chat_cwd().is_none())
}

/// The three standard roles (definition files: P-ORG-8).
pub fn standard_roles() -> Vec<Role> {
    vec![
        Role { id: RoleId::new(ROLE_PLANNER), enabled: true, writes: false, max_concurrent: Some(1) },
        Role { id: RoleId::new(ROLE_WORKER), enabled: true, writes: true, max_concurrent: None },
        Role { id: RoleId::new(ROLE_REVIEWER), enabled: true, writes: false, max_concurrent: None },
    ]
}

// ---------------------------------------------------------------- in-memory bookkeeping

#[derive(Clone, Debug)]
struct RoleCfg {
    sandbox: SandboxKind,
    model: Option<String>,
    effort: Option<String>,
    /// Role instructions from the definition file (put before each work prompt).
    instructions: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    Worktree,
    /// A reviewer's worktree is being moved to the commit under review.
    Refresh,
    Chat,
    Sandbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SandboxState {
    Unknown,
    Checking,
    Ok,
}

#[derive(Clone, Debug)]
enum Output {
    Plan { tasks: Vec<PlanTask>, decision: Decision },
    Report(WorkerReport),
    Verdict(ReviewerVerdict),
}

#[derive(Clone, Debug)]
enum Flow {
    ReadingFinal { turn: TurnRef },
    WaitDesc { out: Output },
    WantCommit,
    Committing,
    NeedFix { reason: String },
}

#[derive(Clone, Copy, Debug, Default)]
struct DescInfo {
    working: u32,
    unknown: u32,
    scan_complete: bool,
}

enum Sent {
    Yes,
    NoTurns,
    NotLive,
    NotAllowed,
}

pub struct OrgCore {
    state: Option<WorkflowState>,
    roles: Vec<Role>,
    /// Per-role chat settings from the role definition files (empty = standard defaults).
    role_cfg: HashMap<RoleId, RoleCfg>,
    now_ms: i64,
    granted: bool,
    live: HashMap<ChatRef, bool>,
    sandbox: SandboxState,
    launching: HashMap<TaskId, Launch>,
    flows: HashMap<TaskId, Flow>,
    early_turns: HashMap<ChatRef, Vec<(TurnRef, TurnEndKind)>>,
    descendants: HashMap<ChatRef, DescInfo>,
    open_requests: HashMap<ChatRef, u32>,
    /// Report summaries of this session (not persisted; prompts say "not obtained" when missing).
    artifacts: HashMap<TaskId, String>,
    findings: HashMap<TaskId, Vec<String>>,
    replan_notes: HashMap<TaskId, String>,
    last_kind: HashMap<TaskId, TurnKind>,
    retry_ready: HashSet<TaskId>,
    /// Working folder to reuse for a task whose unused chat was dropped (this session only).
    rechat_cwd: HashMap<TaskId, String>,
    done_notified: bool,
    limit_notified: bool,
    out: Vec<OrgAction>,
    /// Bumped whenever a flow changes, so `drive` knows whether another pass can make progress.
    gen: u64,
}

impl OrgCore {
    /// An engine with no workflow yet; feed `OrgInput::Start`.
    pub fn new(now_ms: i64) -> Self {
        Self {
            state: None,
            roles: standard_roles(),
            role_cfg: HashMap::new(),
            now_ms,
            granted: false,
            live: HashMap::new(),
            sandbox: SandboxState::Unknown,
            launching: HashMap::new(),
            flows: HashMap::new(),
            early_turns: HashMap::new(),
            descendants: HashMap::new(),
            open_requests: HashMap::new(),
            artifacts: HashMap::new(),
            findings: HashMap::new(),
            replan_notes: HashMap::new(),
            last_kind: HashMap::new(),
            retry_ready: HashSet::new(),
            rechat_cwd: HashMap::new(),
            done_notified: false,
            limit_notified: false,
            out: Vec::new(),
            gen: 0,
        }
    }

    /// An engine for a restored workflow (after `recover_on_restart`). No grant, no live chats.
    pub fn restored(state: WorkflowState, now_ms: i64) -> Self {
        let mut c = Self::new(now_ms);
        c.state = Some(state);
        c
    }

    /// Use the loaded role definitions (P-ORG-8). Roles that are not enabled stay in the table, so a
    /// plan using them needs confirmation (policy) and the scheduler never dispatches them. The three
    /// standard roles must be present; otherwise nothing changes and the reason is returned.
    pub fn set_roles(&mut self, loaded: &[LoadedRole]) -> Result<(), String> {
        for std_id in [ROLE_PLANNER, ROLE_WORKER, ROLE_REVIEWER] {
            if !loaded.iter().any(|l| l.def.id == std_id) {
                return Err(format!("標準役割「{std_id}」の定義を読めません"));
            }
        }
        self.roles = loaded.iter().map(LoadedRole::to_role).collect();
        self.role_cfg = loaded
            .iter()
            .map(|l| {
                (
                    RoleId::new(l.def.id.clone()),
                    RoleCfg { sandbox: l.def.sandbox, model: l.def.model.clone(), effort: l.def.effort.clone(), instructions: l.def.instructions.clone() },
                )
            })
            .collect();
        Ok(())
    }

    pub fn state(&self) -> Option<&WorkflowState> {
        self.state.as_ref()
    }

    /// Kind of the latest turn sent for the task in this session (None after a restart).
    pub fn last_turn_kind(&self, id: &TaskId) -> Option<TurnKind> {
        self.last_kind.get(id).copied()
    }

    /// Wall clock for event timestamps; also updated by `Tick`.
    pub fn set_now(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    pub fn handle(&mut self, input: OrgInput) -> Vec<OrgAction> {
        self.out.clear();
        if self.state.is_none() && !matches!(input, OrgInput::Start { .. } | OrgInput::Tick { .. } | OrgInput::Grant { .. }) {
            return Vec::new();
        }
        self.process(input);
        if self.state.is_some() {
            self.drive();
        }
        if self.out.iter().any(|a| matches!(a, OrgAction::Persist(_))) {
            self.out.push(OrgAction::Publish);
        }
        std::mem::take(&mut self.out)
    }

    // ------------------------------------------------------------ small helpers

    fn s(&self) -> &WorkflowState {
        self.state.as_ref().expect("workflow state exists once started")
    }

    fn task(&self, id: &TaskId) -> Option<&Task> {
        self.state.as_ref().and_then(|s| s.task(id))
    }

    fn task_of_chat(&self, chat: &ChatRef) -> Option<TaskId> {
        self.s().tasks().iter().find(|t| t.chat() == Some(chat)).map(|t| t.id.clone())
    }

    fn kind_of(&self, id: &TaskId) -> Option<RoleKind> {
        self.task(id).and_then(|t| role_kind(&t.role))
    }

    fn rec(&mut self, kind: OrgEventKind) -> bool {
        let at = self.now_ms;
        let Some(st) = self.state.as_mut() else { return false };
        match st.record(kind, at) {
            Ok(ev) => {
                self.out.push(OrgAction::Persist(ev));
                true
            }
            Err(_) => false,
        }
    }

    fn set_status(&mut self, id: &TaskId, to: TaskStatus, cause: Cause) -> bool {
        self.rec(OrgEventKind::StatusChanged { task: id.clone(), to, cause })
    }

    fn set_confirm(&mut self, id: &TaskId, to: ConfirmState) -> bool {
        if self.task(id).is_some_and(|t| t.confirm() == to) {
            return true;
        }
        self.rec(OrgEventKind::ConfirmChanged { task: id.clone(), to })
    }

    fn notify(&mut self, n: OrgNotice) {
        self.out.push(OrgAction::Notify(n));
    }

    fn halt_if_none(&mut self, reason: HaltReason) {
        if self.s().halt().is_none() {
            self.rec(OrgEventKind::Halted { reason: Some(reason) });
        }
    }

    fn clear_halt(&mut self) {
        if self.s().halt().is_some() {
            self.rec(OrgEventKind::Halted { reason: None });
        }
    }

    fn can_act(&self) -> bool {
        self.granted && self.s().halt().is_none()
    }

    fn turns_remaining(&self) -> u32 {
        let s = self.s();
        s.counters().turns_remaining(s.limits())
    }

    fn flow_set(&mut self, id: TaskId, flow: Flow) {
        self.gen += 1;
        self.flows.insert(id, flow);
    }

    fn flow_del(&mut self, id: &TaskId) {
        if self.flows.remove(id).is_some() {
            self.gen += 1;
        }
    }

    fn forget(&mut self, id: &TaskId) {
        self.launching.remove(id);
        self.flow_del(id);
        self.retry_ready.remove(id);
    }

    fn block(&mut self, id: &TaskId, reason: BlockedReason, why: &str) {
        if self.set_status(id, TaskStatus::Blocked(reason), Cause::Engine) {
            self.notify(OrgNotice::TaskBlocked { task: id.clone(), why: why.to_string() });
        }
    }

    // ------------------------------------------------------------ input processing

    fn process(&mut self, input: OrgInput) {
        match input {
            OrgInput::Tick { now_ms } => self.now_ms = now_ms,
            OrgInput::Grant { granted } => self.granted = granted,
            OrgInput::Live { chat, live } => {
                self.live.insert(chat, live);
            }
            OrgInput::Start { workflow, request, repo, limits } => self.start(workflow, request, repo, limits),
            OrgInput::UserPause => self.halt_if_none(HaltReason::User),
            OrgInput::Shutdown => {
                // Nothing to stop once every task is finished.
                let finished = self.s().tasks().iter().all(|t| matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled));
                if !finished {
                    self.halt_if_none(HaltReason::Shutdown);
                }
            }
            OrgInput::UserResume => self.user_resume(),
            OrgInput::UserApprovePlan => {
                if self.s().halt() == Some(HaltReason::AwaitingPlanApproval) {
                    self.clear_halt();
                }
            }
            OrgInput::UserReplan => self.user_replan(),
            OrgInput::UserCancelTask { task } => self.user_cancel(&task),
            OrgInput::UserManual { task } => {
                self.forget(&task);
                self.set_status(&task, TaskStatus::Blocked(BlockedReason::Manual), Cause::User);
            }
            OrgInput::UserAcceptManual { task } => {
                let ok = self
                    .task(&task)
                    .is_some_and(|t| t.status() == TaskStatus::Blocked(BlockedReason::Manual) && t.confirm() == ConfirmState::Confirmed);
                if ok && self.set_status(&task, TaskStatus::Running, Cause::User) {
                    self.set_status(&task, TaskStatus::Completed, Cause::User);
                }
            }
            OrgInput::UserRaiseTurnLimit { to } => {
                let Some(limits) = Counters::raised_turn_limit(self.s().limits(), to) else {
                    self.notify(OrgNotice::Refused { reason: "turn上限は今の値より大きくする必要があります".into() });
                    return;
                };
                self.rec(OrgEventKind::LimitsChanged { limits });
                self.limit_notified = false;
                let ids: Vec<TaskId> = self
                    .s()
                    .tasks()
                    .iter()
                    .filter(|t| t.status() == TaskStatus::Blocked(BlockedReason::Limit) && t.confirm() == ConfirmState::Confirmed)
                    .map(|t| t.id.clone())
                    .collect();
                for id in ids {
                    // Back to pending; promotion decides whether it is ready again.
                    self.set_status(&id, TaskStatus::Pending, Cause::User);
                }
            }
            OrgInput::UserRetryTask { task } => self.user_retry(&task),
            OrgInput::ChatCreated { task, result } => self.chat_created(&task, result),
            OrgInput::SendSettled { task, attempt, outcome } => self.send_settled(&task, attempt, outcome),
            OrgInput::TurnEnded { chat, turn, end } => self.turn_ended_input(chat, turn, end),
            OrgInput::FinalMessage { task, turn, text } => self.final_message(&task, turn, text),
            OrgInput::Descendants { chat, working, unknown, scan_complete } => {
                self.descendants.insert(chat, DescInfo { working, unknown, scan_complete });
            }
            OrgInput::RequestPending { chat, open } => {
                self.open_requests.insert(chat, open);
            }
            OrgInput::Freshness { cause } => self.freshness(cause),
            OrgInput::NotLive { task } => self.not_live(&task),
            OrgInput::WorkspaceResult { task, result } => self.workspace_result(&task, result),
            OrgInput::SandboxChecked { ok, detail } => self.sandbox_checked(ok, detail),
            OrgInput::SandboxFailure { chat } => self.sandbox_failure(chat),
            OrgInput::ChatRecovered { task, chat } => {
                let ok = self.task(&task).is_some_and(|t| t.chat().is_none() && t.chat_cwd().is_some());
                if ok {
                    // The chat was chosen by the user (record against record). If the task only waited for this
                    // (nothing was ever sent), it goes on from the start; otherwise the state is left as it is.
                    let unsent_unknown = self.task(&task).is_some_and(|t| {
                        t.last_attempt().is_none()
                            && t.confirm() == ConfirmState::SendUnknown
                            && t.status() == TaskStatus::Blocked(BlockedReason::NeedsAttention)
                    });
                    if self.rec(OrgEventKind::ChatBound { task: task.clone(), chat }) && unsent_unknown {
                        self.set_confirm(&task, ConfirmState::Confirmed);
                        self.set_status(&task, TaskStatus::Ready, Cause::User);
                    }
                }
            }
            OrgInput::ChatUnresumable { task } => self.chat_unresumable(&task),
            OrgInput::InterruptRequested { chat } => {
                if let Some(id) = self.task_of_chat(&chat) {
                    if self.task(&id).is_some_and(|t| t.status() == TaskStatus::Running && t.confirm() == ConfirmState::Confirmed) {
                        self.set_confirm(&id, ConfirmState::InterruptRequested);
                    }
                }
            }
        }
    }

    fn start(&mut self, workflow: WorkflowId, request: String, repo: String, limits: Limits) {
        if self.state.is_some() {
            self.notify(OrgNotice::Refused { reason: "ワークフローは既に開始されています".into() });
            return;
        }
        self.state = Some(WorkflowState::new(workflow.clone(), limits.clone()));
        self.out.push(OrgAction::Persist(OrgEvent {
            schema_version: SCHEMA_VERSION,
            seq: 0,
            at_ms: self.now_ms,
            kind: OrgEventKind::Created { workflow_id: workflow, limits },
        }));
        self.rec(OrgEventKind::Requested { request, repo });
        let planner = PlanTask {
            id: TaskId::new("planner-1"),
            role: RoleId::new(ROLE_PLANNER),
            deps: vec![],
            effects: vec![],
            title: "計画".into(),
            instructions: String::new(),
        };
        self.rec(OrgEventKind::TaskAdded { task: planner });
    }

    fn user_resume(&mut self) {
        match self.s().halt() {
            Some(HaltReason::AwaitingPlanApproval) => {
                self.notify(OrgNotice::Refused { reason: "計画の承認が必要です".into() });
            }
            Some(HaltReason::SandboxUnhealthy) => {
                self.sandbox = SandboxState::Unknown;
                self.clear_halt();
            }
            Some(_) => self.clear_halt(),
            None => {}
        }
        if self.s().halt().is_none() {
            let ids: Vec<TaskId> = self.retry_ready.drain().collect();
            for id in ids {
                let ok = self
                    .task(&id)
                    .is_some_and(|t| t.status() == TaskStatus::Blocked(BlockedReason::NeedsAttention) && t.confirm() == ConfirmState::Confirmed);
                if ok {
                    self.set_status(&id, TaskStatus::Ready, Cause::Engine);
                }
            }
        }
    }

    fn user_replan(&mut self) {
        let s = self.s();
        let planning_done = s.tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).all(|t| t.status() == TaskStatus::Completed);
        let untouched = s
            .tasks()
            .iter()
            .filter(|t| t.role.0 != ROLE_PLANNER)
            .all(|t| matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Cancelled) && t.chat().is_none());
        if !planning_done || !untouched {
            self.notify(OrgNotice::Refused { reason: "作業が始まっているため計画の作り直しはできません".into() });
            return;
        }
        let proposal: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| t.role.0 != ROLE_PLANNER && t.status() != TaskStatus::Cancelled)
            .map(|t| t.id.clone())
            .collect();
        let note: Vec<String> = proposal
            .iter()
            .filter_map(|id| self.task(id))
            .map(|t| format!("{} ({}): {}", t.id, t.role, t.title))
            .collect();
        for id in &proposal {
            self.set_status(id, TaskStatus::Cancelled, Cause::User);
        }
        let n = self.s().tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).count() + 1;
        let id = TaskId::new(format!("planner-{n}"));
        self.replan_notes.insert(id.clone(), note.join("\n"));
        self.clear_halt();
        self.rec(OrgEventKind::TaskAdded {
            task: PlanTask {
                id,
                role: RoleId::new(ROLE_PLANNER),
                deps: vec![],
                effects: vec![],
                title: "再計画".into(),
                instructions: String::new(),
            },
        });
    }

    fn user_cancel(&mut self, id: &TaskId) {
        let Some(t) = self.task(id) else { return };
        let in_flight = matches!(t.status(), TaskStatus::Running | TaskStatus::Reviewing) || self.launching.contains_key(id);
        self.forget(id);
        if self.set_status(id, TaskStatus::Cancelled, Cause::User) && in_flight {
            self.notify(OrgNotice::CancelUnconfirmed { task: id.clone() });
        }
    }

    /// The user's retry of a blocked task. Allowed only when it is certain that nothing was sent for the task:
    /// the first turn was never accepted and no attempt is open (rejected / not found / not live / worktree or chat
    /// creation refused). The task goes back to `Ready` and is sent as a new attempt by the usual path; an unknown
    /// outcome (`SendUnknown`) is never retried.
    fn user_retry(&mut self, id: &TaskId) {
        if !self.is_retryable(id) {
            self.notify(OrgNotice::Refused { reason: "このタスクは、送っていないことを確認できないため、やり直せません".into() });
            return;
        }
        self.forget(id);
        self.set_status(id, TaskStatus::Ready, Cause::User);
    }

    /// Whether `UserRetryTask` is accepted for the task (also used by the screen).
    pub fn is_retryable(&self, id: &TaskId) -> bool {
        self.task(id).is_some_and(retryable)
    }

    // ---- chat creation

    /// The bound chat cannot be resumed. When it is certain that nothing was ever sent to it (no dispatch recorded,
    /// no accepted turn, nothing in flight, confirmed), the binding is dropped and a new role chat is made in the same
    /// working folder; the task then goes on the usual path. Otherwise nothing changes (it stays stopped).
    fn chat_unresumable(&mut self, id: &TaskId) {
        let Some(t) = self.task(id) else { return };
        let certain = matches!(t.status(), TaskStatus::Ready | TaskStatus::Blocked(BlockedReason::NeedsAttention))
            && t.confirm() == ConfirmState::Confirmed
            && t.chat().is_some()
            && t.last_attempt().is_none()
            && t.accepted_turn().is_none()
            && !self.launching.contains_key(id)
            && !self.flows.contains_key(id);
        let (Some(cwd), Some(chat)) = (t.chat_cwd().map(str::to_string), t.chat().cloned()) else { return };
        if !certain {
            return;
        }
        if !self.rec(OrgEventKind::ChatRequestAbandoned { task: id.clone() }) {
            return;
        }
        self.live.remove(&chat);
        self.forget(id);
        self.rechat_cwd.insert(id.clone(), cwd);
        if self.task(id).is_some_and(|t| t.status() != TaskStatus::Ready) {
            self.set_status(id, TaskStatus::Ready, Cause::User);
        }
    }

    fn chat_created(&mut self, id: &TaskId, result: CreateResult) {
        if self.launching.get(id) != Some(&Launch::Chat) {
            return;
        }
        self.launching.remove(id);
        match result {
            CreateResult::Created(chat) => {
                if self.rec(OrgEventKind::ChatBound { task: id.clone(), chat: chat.clone() }) {
                    // A chat created just now is live (thread/start subscribes).
                    self.live.insert(chat, true);
                    self.continue_launch(id);
                }
            }
            CreateResult::Rejected(why) => {
                // Certainly not created: forget the request so that the user can try again.
                self.rec(OrgEventKind::ChatRequestAbandoned { task: id.clone() });
                self.block(id, BlockedReason::NeedsAttention, &format!("役割チャットを作成できませんでした: {why}"));
            }
            CreateResult::Unknown | CreateResult::SaveFailed => self.block_unknown_create(id),
        }
    }

    /// The chat may exist but is not tied to the task. Never guessed: the user decides.
    fn block_unknown_create(&mut self, id: &TaskId) {
        self.block(id, BlockedReason::NeedsAttention, "役割チャットの作成結果が不明です");
        self.set_confirm(id, ConfirmState::SendUnknown);
    }

    // ---- send results

    fn send_settled(&mut self, id: &TaskId, attempt: AttemptRef, outcome: SettleOutcome) {
        if !self.rec(OrgEventKind::DispatchSettled { task: id.clone(), attempt, outcome: outcome.clone() }) {
            return;
        }
        match outcome {
            SettleOutcome::Accepted { turn } | SettleOutcome::ReconciledAccepted { turn } => {
                self.set_confirm(id, ConfirmState::Confirmed);
                self.replay_early_turn(id, &turn);
            }
            SettleOutcome::Rejected | SettleOutcome::ReconciledNotFound => {
                self.set_confirm(id, ConfirmState::Confirmed);
                self.block(id, BlockedReason::NeedsAttention, "送信が受け付けられませんでした。送る／取り消すを選んでください");
            }
            // Stays SendUnknown; the scheduler never touches it again.
            SettleOutcome::Unknown => {}
        }
    }

    fn not_live(&mut self, id: &TaskId) {
        let Some(t) = self.task(id) else { return };
        if t.status() != TaskStatus::Running || t.confirm() != ConfirmState::SendUnknown {
            return;
        }
        let Some(attempt) = t.last_attempt().cloned() else { return };
        // The host did not send: the outcome is known (nothing was sent).
        if !self.rec(OrgEventKind::DispatchSettled { task: id.clone(), attempt, outcome: SettleOutcome::Rejected }) {
            return;
        }
        self.set_confirm(id, ConfirmState::Confirmed);
        self.block(id, BlockedReason::NeedsAttention, "チャットがliveでないため送信しませんでした");
        self.halt_if_none(HaltReason::NeedsResume);
        if self.last_kind.get(id) != Some(&TurnKind::FormatFix) {
            self.retry_ready.insert(id.clone());
        }
    }

    // ---- turn ends

    fn turn_ended_input(&mut self, chat: ChatRef, turn: TurnRef, end: TurnEndKind) {
        let Some(id) = self.task_of_chat(&chat) else { return };
        let Some(t) = self.task(&id) else { return };
        if t.accepted_turn() == Some(&turn) {
            self.turn_ended(&id, turn, end);
        } else if t.confirm() == ConfirmState::SendUnknown {
            // Arrived before the acceptance was reported.
            self.early_turns.entry(chat).or_default().push((turn, end));
        } else {
            // A late turn that is not the accepted one: logged only.
            self.rec(OrgEventKind::TurnObserved { task: id, turn, end });
        }
    }

    fn replay_early_turn(&mut self, id: &TaskId, accepted: &TurnRef) {
        let Some(chat) = self.task(id).and_then(|t| t.chat().cloned()) else { return };
        let Some(list) = self.early_turns.get_mut(&chat) else { return };
        if let Some(pos) = list.iter().position(|(t, _)| t == accepted) {
            let (turn, end) = list.remove(pos);
            self.turn_ended(id, turn, end);
        }
    }

    fn turn_ended(&mut self, id: &TaskId, turn: TurnRef, end: TurnEndKind) {
        self.rec(OrgEventKind::TurnObserved { task: id.clone(), turn: turn.clone(), end });
        let Some(t) = self.task(id) else { return };
        if t.status() != TaskStatus::Running || self.flows.contains_key(id) {
            return;
        }
        match end {
            TurnEndKind::Completed => {
                // The end is now confirmed (also when it came from a read-only reconciliation).
                self.set_confirm(id, ConfirmState::Confirmed);
                let Some(chat) = self.task(id).and_then(|t| t.chat().cloned()) else { return };
                self.flow_set(id.clone(), Flow::ReadingFinal { turn: turn.clone() });
                self.out.push(OrgAction::ReadFinalMessage { task: id.clone(), chat, turn });
            }
            TurnEndKind::Failed { usage_limit } => {
                self.set_confirm(id, ConfirmState::Confirmed);
                if usage_limit {
                    self.block(id, BlockedReason::NeedsAttention, "利用上限に達しました");
                    self.halt_if_none(HaltReason::User);
                    self.notify(OrgNotice::UsageLimit { task: id.clone() });
                } else {
                    self.fail(id, FailReason::ExecutionFailed);
                }
            }
            TurnEndKind::Interrupted => {
                self.set_confirm(id, ConfirmState::Confirmed);
                self.block(id, BlockedReason::ManualStop, "turnが中断されました");
            }
        }
    }

    fn fail(&mut self, id: &TaskId, reason: FailReason) {
        if self.set_status(id, TaskStatus::Failed(reason), Cause::Engine) {
            self.notify(OrgNotice::TaskFailed { task: id.clone(), reason });
        }
    }

    // ---- final message -> artifact

    fn final_message(&mut self, id: &TaskId, turn: TurnRef, text: Option<String>) {
        match self.flows.get(id) {
            Some(Flow::ReadingFinal { turn: t }) if *t == turn => {}
            _ => return,
        }
        self.flow_del(id);
        let Some(text) = text else {
            // Not obtained: do not decide anything from it.
            self.set_confirm(id, ConfirmState::NeedsReconcile(ReconcileCause::Disconnect));
            return;
        };
        match self.parse_output(id, &text) {
            Ok(out) => {
                self.flow_set(id.clone(), Flow::WaitDesc { out });
            }
            Err(reason) => self.parse_failed(id, reason),
        }
    }

    fn parse_output(&self, id: &TaskId, text: &str) -> Result<Output, String> {
        match self.kind_of(id) {
            Some(RoleKind::Planner) => {
                let taken: HashSet<TaskId> = self.s().tasks().iter().map(|t| t.id.clone()).collect();
                let tasks = schemas::parse_plan(text, &taken)?;
                let planners = self.s().tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).count();
                let kind = if planners <= 1 {
                    PlanKind::Initial
                } else {
                    // A user-requested re-plan is always put to the user.
                    PlanKind::Replan { scope_changed: true, auto_replans_used: self.s().counters().auto_replans_used }
                };
                let decision = evaluate_plan(&PolicyInput {
                    tasks: &tasks,
                    roles: &self.roles,
                    limits: self.s().limits(),
                    kind,
                    requested_parallelism: None,
                    turns_remaining: self.turns_remaining(),
                })
                .map_err(|errs| format!("計画の検証に失敗しました: {errs:?}"))?;
                Ok(Output::Plan { tasks, decision })
            }
            Some(RoleKind::Worker) => Ok(Output::Report(schemas::parse_worker_report(text)?)),
            Some(RoleKind::Reviewer) => Ok(Output::Verdict(schemas::parse_verdict(text)?)),
            None => Err("未知の役割です".into()),
        }
    }

    fn parse_failed(&mut self, id: &TaskId, reason: String) {
        if self.task(id).is_some_and(|t| t.format_fix_used()) {
            self.fail(id, FailReason::InvalidArtifact);
        } else {
            self.flow_set(id.clone(), Flow::NeedFix { reason });
        }
    }

    fn desc_state(&self, id: &TaskId) -> Option<u32> {
        let chat = self.task(id)?.chat()?;
        if self.open_requests.get(chat).copied().unwrap_or(0) > 0 {
            return None;
        }
        let d = self.descendants.get(chat)?;
        (d.scan_complete && d.working == 0).then_some(d.unknown)
    }

    fn complete_output(&mut self, id: &TaskId, out: Output) {
        match out {
            Output::Plan { tasks, decision } => {
                let n = tasks.len();
                for t in tasks {
                    self.rec(OrgEventKind::TaskAdded { task: t });
                }
                match decision {
                    Decision::AutoApprove => self.notify(OrgNotice::PlanAutoApproved { tasks: n }),
                    Decision::NeedsConfirmation(reasons) => {
                        self.rec(OrgEventKind::Halted { reason: Some(HaltReason::AwaitingPlanApproval) });
                        self.notify(OrgNotice::PlanNeedsApproval { reasons });
                    }
                }
                if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            Output::Report(r) => {
                self.artifacts.insert(id.clone(), r.summary);
                self.flow_set(id.clone(), Flow::WantCommit);
            }
            Output::Verdict(v) => self.complete_verdict(id, v),
        }
    }

    fn complete_verdict(&mut self, id: &TaskId, v: ReviewerVerdict) {
        let target = self.task(id).and_then(|t| t.deps.first().cloned());
        let Some(target) = target else {
            self.block(id, BlockedReason::NeedsAttention, "検証対象がありません");
            return;
        };
        match v.verdict {
            VerdictKind::Pass => {
                // The reviewed task completes only now; then the reviewer itself.
                if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.set_status(&target, TaskStatus::Completed, Cause::Engine);
                    self.notify(OrgNotice::TaskCompleted { task: target });
                }
                if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            VerdictKind::Rework => {
                let limit_reached = {
                    let limits = self.s().limits().clone();
                    self.task(&target).is_some_and(|t| t.rework_limit_reached(&limits))
                };
                if limit_reached {
                    self.fail(&target, FailReason::LimitReached);
                } else if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.rec(OrgEventKind::ReworkCounted { task: target.clone() });
                    self.set_status(&target, TaskStatus::Rework, Cause::Engine);
                    self.set_status(&target, TaskStatus::Ready, Cause::Engine);
                    self.findings.insert(target.clone(), v.findings.into_iter().filter(|f| !f.trim().is_empty()).collect());
                }
                // The reviewer waits until the worker task is under review again.
                self.set_status(id, TaskStatus::Rework, Cause::Engine);
            }
        }
    }

    // ---- workspace results

    fn workspace_result(&mut self, id: &TaskId, result: WorkspaceOutcome) {
        if self.launching.get(id) == Some(&Launch::Refresh) {
            self.launching.remove(id);
            match result {
                WorkspaceOutcome::WorktreeCreated { .. } => self.continue_launch(id),
                _ => self.block(id, BlockedReason::NeedsAttention, "再レビュー用の作業場所を対象コミットに合わせられませんでした"),
            }
            return;
        }
        if self.launching.get(id) == Some(&Launch::Worktree) {
            self.launching.remove(id);
            match result {
                WorkspaceOutcome::WorktreeCreated { path } => self.begin_chat(id, path),
                _ => self.block(id, BlockedReason::NeedsAttention, "作業用worktreeを作成できませんでした"),
            }
            return;
        }
        if !matches!(self.flows.get(id), Some(Flow::Committing)) {
            return;
        }
        self.flow_del(id);
        match result {
            WorkspaceOutcome::Committed { commit } => {
                self.rec(OrgEventKind::Committed { task: id.clone(), commit });
                let has_reviewer = self.s().tasks().iter().any(|t| t.role.0 == ROLE_REVIEWER && t.deps.contains(id));
                let to = if has_reviewer { TaskStatus::Reviewing } else { TaskStatus::Completed };
                if self.set_status(id, to, Cause::Engine) && to == TaskStatus::Completed {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            WorkspaceOutcome::NothingToCommit => self.block(id, BlockedReason::NeedsAttention, "コミットする変更がありません"),
            _ => self.block(id, BlockedReason::NeedsAttention, "コミットできませんでした"),
        }
    }

    fn sandbox_checked(&mut self, ok: bool, detail: String) {
        if self.sandbox != SandboxState::Checking {
            return;
        }
        let waiting: Vec<TaskId> = self.launching.iter().filter(|(_, l)| **l == Launch::Sandbox).map(|(t, _)| t.clone()).collect();
        for id in &waiting {
            self.launching.remove(id);
        }
        if ok {
            self.sandbox = SandboxState::Ok;
            for id in waiting {
                self.continue_launch(&id);
            }
        } else {
            self.sandbox = SandboxState::Unknown;
            self.halt_if_none(HaltReason::SandboxUnhealthy);
            self.notify(OrgNotice::SandboxUnhealthy { detail });
        }
    }

    /// The sandbox failed to start while a task was running: the task needs the user, not a reconciliation,
    /// and nothing new is launched until the user resumes (which re-checks the sandbox).
    fn sandbox_failure(&mut self, chat: ChatRef) {
        let Some(id) = self.task_of_chat(&chat) else { return };
        let running = self.task(&id).is_some_and(|t| matches!(t.status(), TaskStatus::Running | TaskStatus::Reviewing));
        if running {
            self.forget(&id);
            self.block(&id, BlockedReason::NeedsAttention, "sandbox を起動できませんでした");
        }
        self.sandbox = SandboxState::Unknown;
        self.halt_if_none(HaltReason::SandboxUnhealthy);
        self.notify(OrgNotice::SandboxUnhealthy { detail: "実行中のコマンドが sandbox の更新失敗（setup refresh had errors）を示しました".into() });
    }

    fn freshness(&mut self, cause: ReconcileCause) {
        self.live.clear();
        self.early_turns.clear();
        // Child counts read before the interruption are stale: only a new read may complete a task (R-ORG M3).
        self.descendants.clear();
        if matches!(cause, ReconcileCause::Disconnect | ReconcileCause::Restart) && self.sandbox == SandboxState::Ok {
            self.sandbox = SandboxState::Unknown;
        }
        let ids: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| matches!(t.status(), TaskStatus::Running | TaskStatus::Reviewing) && t.confirm() == ConfirmState::Confirmed)
            // Its own turn already ended: no reconciliation needed (R-ORG H2).
            .filter(|t| !t.is_idle_reviewing())
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            // A commit in progress does not depend on the connection.
            if matches!(self.flows.get(&id), Some(Flow::Committing | Flow::WantCommit)) {
                continue;
            }
            self.flow_del(&id);
            self.set_confirm(&id, ConfirmState::NeedsReconcile(cause));
        }
        // Nothing is resumed automatically: sending waits for the user's resume.
        self.halt_if_none(HaltReason::NeedsResume);
    }

    // ------------------------------------------------------------ driving

    fn drive(&mut self) {
        // Repeat until nothing more is produced (a completion can make the next task ready).
        for _ in 0..32 {
            let before = (self.out.len(), self.gen);
            self.promote();
            self.advance_flows();
            self.dispatch();
            if (self.out.len(), self.gen) == before {
                break;
            }
        }
        self.check_done();
    }

    fn dep_ok(dep: &Task, dependent: &Task) -> bool {
        if dependent.role.0 == ROLE_REVIEWER {
            // A reviewer may start once the reviewed task is under review (committed).
            dep.confirm() == ConfirmState::Confirmed
                && (dep.status() == TaskStatus::Completed || (dep.status() == TaskStatus::Reviewing && dep.commit().is_some()))
        } else {
            default_dep_ok(dep, dependent)
        }
    }

    fn promote(&mut self) {
        let mut ids = promote_ready_with(self.s().tasks(), &Self::dep_ok);
        // A reviewer sent back to wait becomes ready again when its target is under review again.
        let by_id: HashMap<&TaskId, &Task> = self.s().tasks().iter().map(|t| (&t.id, t)).collect();
        for t in self.s().tasks() {
            if t.role.0 == ROLE_REVIEWER
                && t.status() == TaskStatus::Rework
                && t.confirm() == ConfirmState::Confirmed
                && t.deps.iter().all(|d| by_id.get(d).is_some_and(|dep| Self::dep_ok(dep, t)))
            {
                ids.push(t.id.clone());
            }
        }
        for id in ids {
            self.set_status(&id, TaskStatus::Ready, Cause::Engine);
        }
    }

    fn advance_flows(&mut self) {
        let mut ids: Vec<TaskId> = self.flows.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let Some(flow) = self.flows.get(&id).cloned() else { continue };
            match flow {
                Flow::WaitDesc { out } => {
                    if let Some(unknown) = self.desc_state(&id) {
                        self.flow_del(&id);
                        if unknown > 0 {
                            self.notify(OrgNotice::DescendantsUnconfirmed { task: id.clone(), count: unknown });
                        }
                        self.complete_output(&id, out);
                    }
                }
                Flow::WantCommit => {
                    if self.can_act() {
                        let Some(t) = self.task(&id) else { continue };
                        let Some(cwd) = t.chat_cwd().map(str::to_string) else { continue };
                        let message = format!("agentdock: {} {}", t.id, t.title);
                        self.flow_set(id.clone(), Flow::Committing);
                        self.out.push(OrgAction::Workspace(WorkspaceAction::Commit { task: id.clone(), cwd, message }));
                    }
                }
                Flow::NeedFix { reason } => {
                    match self.send_turn(&id, TurnKind::FormatFix, prompt::format_fix_prompt(&reason)) {
                        Sent::Yes => {
                            self.flow_del(&id);
                        }
                        Sent::NoTurns => {
                            self.flow_del(&id);
                            self.block(&id, BlockedReason::Limit, "turn上限に達しました");
                            self.limit_notice();
                        }
                        Sent::NotLive | Sent::NotAllowed => {}
                    }
                }
                Flow::ReadingFinal { .. } | Flow::Committing => {}
            }
        }
    }

    fn limit_notice(&mut self) {
        if !self.limit_notified {
            self.limit_notified = true;
            self.notify(OrgNotice::TurnLimitReached);
        }
    }

    fn dispatch(&mut self) {
        if !self.can_act() {
            return;
        }
        let remaining = self.turns_remaining();
        if remaining == 0 {
            let ids: Vec<TaskId> = self
                .s()
                .tasks()
                .iter()
                .filter(|t| t.status() == TaskStatus::Ready && t.confirm() == ConfirmState::Confirmed && !self.launching.contains_key(&t.id))
                .map(|t| t.id.clone())
                .collect();
            if !ids.is_empty() {
                for id in ids {
                    self.set_status(&id, TaskStatus::Blocked(BlockedReason::Limit), Cause::Engine);
                }
                self.limit_notice();
            }
            return;
        }
        let held: HashSet<TaskId> = self.launching.keys().cloned().collect();
        let gate = DispatchGate::open(remaining.saturating_sub(held.len() as u32));
        let ids = next_dispatch_with(self.s().tasks(), &self.roles, self.s().limits(), &gate, &Self::dep_ok, &held);
        for id in ids {
            self.launch(&id);
        }
    }

    fn launch(&mut self, id: &TaskId) {
        let Some(t) = self.task(id) else { return };
        let Some(kind) = role_kind(&t.role) else { return };
        let (has_chat, has_cwd) = (t.chat().is_some(), t.chat_cwd().is_some());
        let target_commit = t.deps.first().and_then(|d| self.task(d)).and_then(|d| d.commit().map(str::to_string));
        if has_chat {
            let rereview = kind == RoleKind::Reviewer && self.task(id).is_some_and(|t| t.accepted_turn().is_some());
            if rereview {
                // The reviewer's chat is tied to its worktree: move that to the new commit first, and send nothing
                // until it is verified to be there (R-ORG M1).
                let (Some(commit), Some(cwd)) = (target_commit, self.task(id).and_then(|t| t.chat_cwd().map(str::to_string))) else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットまたは作業場所がありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Refresh);
                self.out.push(OrgAction::Workspace(WorkspaceAction::RefreshDetached { task: id.clone(), cwd, commit }));
                return;
            }
            self.continue_launch(id);
            return;
        }
        if has_cwd {
            // A creation was started earlier and its result was lost (e.g. restart).
            self.block_unknown_create(id);
            return;
        }
        // A new chat for a task whose earlier chat was never used: its working folder (worktree) is reused.
        if let Some(cwd) = self.rechat_cwd.remove(id) {
            self.begin_chat(id, cwd);
            return;
        }
        let repo = self.s().repo().to_string();
        match kind {
            RoleKind::Planner => self.begin_chat(id, repo),
            RoleKind::Worker => {
                self.launching.insert(id.clone(), Launch::Worktree);
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: id.clone(), repo, base: WorktreeBase::Head }));
            }
            RoleKind::Reviewer => {
                let Some(commit) = target_commit else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットがありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Worktree);
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree {
                    task: id.clone(),
                    repo,
                    base: WorktreeBase::DetachedAt(commit),
                }));
            }
        }
    }

    fn begin_chat(&mut self, id: &TaskId, cwd: String) {
        let Some(t) = self.task(id) else { return };
        let role = t.role.clone();
        let default_sandbox = match role_kind(&role) {
            Some(RoleKind::Worker) => SandboxKind::WorkspaceWrite,
            _ => SandboxKind::ReadOnly,
        };
        let (sandbox, model, effort) = match self.role_cfg.get(&role) {
            Some(c) => (c.sandbox, c.model.clone(), c.effort.clone()),
            None => (default_sandbox, None, None),
        };
        if !self.rec(OrgEventKind::ChatRequested { task: id.clone(), cwd: cwd.clone() }) {
            return;
        }
        self.launching.insert(id.clone(), Launch::Chat);
        self.out.push(OrgAction::CreateRoleChat { task: id.clone(), spec: RoleChatSpec { role, cwd, sandbox, model, effort } });
    }

    /// The chat exists: check the sandbox (once per App Server), then send.
    fn continue_launch(&mut self, id: &TaskId) {
        if !self.task(id).is_some_and(|t| t.status() == TaskStatus::Ready && t.chat().is_some()) {
            return;
        }
        match self.sandbox {
            SandboxState::Ok => {}
            SandboxState::Checking => {
                self.launching.insert(id.clone(), Launch::Sandbox);
                return;
            }
            SandboxState::Unknown => {
                self.sandbox = SandboxState::Checking;
                let cwd = self.s().repo().to_string();
                self.out.push(OrgAction::CheckSandbox { cwd });
                self.launching.insert(id.clone(), Launch::Sandbox);
                return;
            }
        }
        let Some((kind, text)) = self.ready_turn(id) else {
            self.block(id, BlockedReason::NeedsAttention, "送る内容を組み立てられません");
            return;
        };
        let text = self.with_instructions(id, text);
        if let Sent::NoTurns = self.send_turn(id, kind, text) {
            self.block(id, BlockedReason::Limit, "turn上限に達しました");
            self.limit_notice();
        }
    }

    /// The role's own instructions (definition file) go before the work prompt. Without role definitions
    /// loaded (tests, defaults) the prompt is unchanged.
    fn with_instructions(&self, id: &TaskId, text: String) -> String {
        match self.task(id).and_then(|t| self.role_cfg.get(&t.role)) {
            Some(c) if !c.instructions.trim().is_empty() => format!("【この役割の指示】\n{}\n\n{}", c.instructions.trim(), text),
            _ => text,
        }
    }

    /// Turn kind and text for a ready task.
    fn ready_turn(&self, id: &TaskId) -> Option<(TurnKind, String)> {
        let t = self.task(id)?;
        let s = self.s();
        match role_kind(&t.role)? {
            RoleKind::Planner => {
                let first = s.tasks().iter().filter(|x| x.role.0 == ROLE_PLANNER).count() <= 1;
                if first {
                    Some((TurnKind::Plan, prompt::planner_prompt(s.request(), s.repo())))
                } else {
                    let prev = self.replan_notes.get(id).cloned().unwrap_or_else(|| "(前の計画は未取得)".into());
                    Some((TurnKind::Replan, prompt::replan_prompt(s.request(), s.repo(), &prev)))
                }
            }
            RoleKind::Worker => {
                if t.accepted_turn().is_some() {
                    let f = self.findings.get(id).map(Vec::as_slice);
                    Some((TurnKind::Rework, prompt::worker_rework_prompt(t, f)))
                } else {
                    let deps: Vec<(TaskId, Option<String>)> = t.deps.iter().map(|d| (d.clone(), self.artifacts.get(d).cloned())).collect();
                    Some((TurnKind::Task, prompt::worker_prompt(t, s.request(), &deps)))
                }
            }
            RoleKind::Reviewer => {
                let target = self.task(t.deps.first()?)?;
                let commit = target.commit()?;
                let summary = self.artifacts.get(&target.id).map(String::as_str);
                Some((TurnKind::Review, prompt::reviewer_prompt(t, s.request(), (&target.id, summary, commit), target.rework_count() + 1)))
            }
        }
    }

    fn schema_for(&self, id: &TaskId) -> Value {
        match self.kind_of(id) {
            Some(RoleKind::Planner) => schemas::planner_plan_schema(),
            Some(RoleKind::Reviewer) => schemas::reviewer_verdict_schema(),
            _ => schemas::worker_report_schema(),
        }
    }

    /// The only place that produces `Send`. Events first (turn counted, `SendUnknown`,
    /// `Dispatched`), then the action. Never called for a task with an open or unknown attempt.
    fn send_turn(&mut self, id: &TaskId, kind: TurnKind, text: String) -> Sent {
        if !self.can_act() {
            return Sent::NotAllowed;
        }
        let Some(t) = self.task(id) else { return Sent::NotAllowed };
        let Some(chat) = t.chat().cloned() else { return Sent::NotAllowed };
        let status = t.status();
        let attempt_open = t.last_attempt().is_some() && matches!(t.last_settle(), None | Some(SettleOutcome::Unknown));
        if t.confirm() != ConfirmState::Confirmed || attempt_open {
            return Sent::NotAllowed;
        }
        if !matches!(status, TaskStatus::Ready | TaskStatus::Rework | TaskStatus::Running) {
            return Sent::NotAllowed;
        }
        if self.live.get(&chat) != Some(&true) {
            self.halt_if_none(HaltReason::NeedsResume);
            return Sent::NotLive;
        }
        if self.turns_remaining() == 0 {
            return Sent::NoTurns;
        }
        let attempt = AttemptRef::new(format!("att-{}", self.s().last_seq() + 1));
        let client_message_id = format!("{}:{}:{}", self.s().workflow_id(), id, attempt);
        if !self.rec(OrgEventKind::TurnSpent { kind }) {
            return Sent::NoTurns;
        }
        if status != TaskStatus::Running && !self.set_status(id, TaskStatus::Running, Cause::Engine) {
            return Sent::NotAllowed;
        }
        self.set_confirm(id, ConfirmState::SendUnknown);
        if !self.rec(OrgEventKind::Dispatched { task: id.clone(), attempt: attempt.clone(), client_message_id: client_message_id.clone(), kind }) {
            // Stays SendUnknown and nothing is sent: a stuck-but-safe state.
            return Sent::NotAllowed;
        }
        self.last_kind.insert(id.clone(), kind);
        let output_schema = self.schema_for(id);
        self.out.push(OrgAction::Send { task: id.clone(), chat, attempt, client_message_id, text, output_schema });
        Sent::Yes
    }

    fn check_done(&mut self) {
        if self.done_notified {
            return;
        }
        let s = self.s();
        let all_done = s.halt().is_none()
            && s.tasks().iter().any(|t| t.role.0 != ROLE_PLANNER)
            && s.tasks().iter().all(|t| t.status() == TaskStatus::Completed && t.confirm() == ConfirmState::Confirmed);
        if all_done {
            self.done_notified = true;
            self.notify(OrgNotice::WorkflowCompleted);
        }
    }
}

#[cfg(test)]
mod tests;
