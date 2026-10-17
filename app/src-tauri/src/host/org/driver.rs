//! 組織実行の司令task（DESIGN_ORG_P4 §3.1）。`OrgCore`（sans-IO のエンジン）を所有し、`OrgAction` をポート経由で実行して、
//! 結果を `OrgInput` で返す。`OrgGrant` を持つのはこのtaskだけ。
//!
//! 守ること:
//! - `Persist`（events.jsonl への追記）が成功してから、後続の作成・送信・作業場所の action を実行する。追記に失敗したら以後は送らない。
//! - 送信の結果（受理・拒否・受理不明、照合の結果）は監視の受け口（`observe.rs`）から届く。ここでは送信の戻り値から `SendSettled` を作らない
//!   （二重に知らせない）。live でない・送信を止める条件に当たったときだけ、送っていないことをここで知らせる。
//! - 受理不明の送信を再送しない。再送の入力はエンジンに存在しない。承認・質問・中断・resume を行う経路を持たない。
//! - 切断・wake・再起動で `Freshness` を受けたら送信権を手放す（再び送るには利用者の再開操作で新しい送信権が要る）。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use org_engine::persist::SCHEMA_VERSION;
use org_engine::{
    AttemptRef, ChatRef, ConfirmState, CreateResult, OrgAction, OrgCore, OrgInput, OrgStore, TaskId, TaskStatus,
    TurnRef, UsageRow, WorkflowId, WorkspaceAction, WorkspaceOutcome,
};
use tokio::sync::mpsc::UnboundedReceiver;

use super::grant::OrgGrant;
use super::ports::{AgentRuntime, OrgHostNotice, OrgSendResult, OrgSendSpec, RuntimeRead};
use crate::rules::org::{chat_key_of_ref, judge_sandbox, turn_key_of_ref, SandboxVerdict, SANDBOX_UNHEALTHY_NOTICE};

/// 使用量の標本（`thread/tokenUsage/updated`）。turn の終端で1行にまとめて記録する。取れない値は None。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSample {
    pub chat: ChatRef,
    pub turn: Option<TurnRef>,
    pub last_total_tokens: Option<u64>,
    pub thread_total_tokens: Option<u64>,
    pub model_context_window: Option<u64>,
}

/// 司令taskへのメッセージ。
pub enum DriverMsg {
    Input(OrgInput),
    Usage(UsageSample),
    /// 送信権を受け取る（利用者の開始・再開の操作で作られたもの）。
    Install(OrgGrant),
    /// 送信権を手放す（一時停止・取消・完全終了の開始・wake・再起動）。
    Revoke,
    Shutdown,
}

/// 画面向けの状態の写し（P-ORG-7 が IPC の型へ変換する）。
#[derive(Debug, Clone, PartialEq)]
pub struct OrgSnapshot {
    pub workflow: WorkflowId,
    pub halt: Option<org_engine::HaltReason>,
    pub tasks: Vec<TaskSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub role: String,
    pub status: TaskStatus,
    pub confirm: ConfirmState,
    pub chat: Option<ChatRef>,
}

/// エンジンの状態から画面向けの写しを作る。
pub fn snapshot_of(state: &org_engine::WorkflowState) -> OrgSnapshot {
    OrgSnapshot {
        workflow: state.workflow_id().clone(),
        halt: state.halt(),
        tasks: state
            .tasks()
            .iter()
            .map(|t| TaskSnapshot { id: t.id.clone(), role: t.role.0.clone(), status: t.status(), confirm: t.confirm(), chat: t.chat().cloned() })
            .collect(),
    }
}

/// 状態の公開先（画面への合図・通知・状態の写し）。
pub trait OrgSink: Send + Sync {
    fn publish(&self, snapshot: OrgSnapshot);
    fn notice(&self, workflow: &WorkflowId, notice: OrgHostNotice);
}

type DynStore = Arc<dyn OrgStore + Send + Sync>;

pub struct OrgDriver<P: AgentRuntime + RuntimeRead> {
    core: OrgCore,
    store: DynStore,
    ports: Arc<P>,
    sink: Arc<dyn OrgSink>,
    grant: Option<OrgGrant>,
    /// 追記に失敗した。以後は何も送らない（再起動で復元する）。
    persist_failed: bool,
    latest_usage: HashMap<ChatRef, UsageSample>,
    last_desc: HashMap<ChatRef, (u32, u32, bool)>,
    now: Box<dyn Fn() -> i64 + Send>,
}

impl<P: AgentRuntime + RuntimeRead + 'static> OrgDriver<P> {
    pub fn new(core: OrgCore, store: DynStore, ports: Arc<P>, sink: Arc<dyn OrgSink>, now: Box<dyn Fn() -> i64 + Send>) -> Self {
        Self { core, store, ports, sink, grant: None, persist_failed: false, latest_usage: HashMap::new(), last_desc: HashMap::new(), now }
    }

    pub fn core(&self) -> &OrgCore {
        &self.core
    }

    /// メッセージを受けて処理し続ける。送信側がすべて閉じる、または `Shutdown` で終わる。
    pub async fn run(mut self, mut rx: UnboundedReceiver<DriverMsg>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut n: u64 = 0;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    None | Some(DriverMsg::Shutdown) => break,
                    Some(m) => self.on_msg(m).await,
                },
                _ = tick.tick() => {
                    n += 1;
                    self.on_tick(n).await;
                }
            }
        }
    }

    pub async fn on_msg(&mut self, msg: DriverMsg) {
        match msg {
            DriverMsg::Input(i) => self.step(i).await,
            DriverMsg::Usage(u) => {
                self.latest_usage.insert(u.chat.clone(), u);
            }
            DriverMsg::Install(g) => {
                self.grant = Some(g);
                self.step(OrgInput::Grant { granted: true }).await;
            }
            DriverMsg::Revoke => self.drop_grant_then(None).await,
            DriverMsg::Shutdown => {}
        }
    }

    /// 1秒ごと: 時刻を進め、完了判定の待ちにある task の子孫の様子を読み直して知らせる（読取りのみ）。
    pub async fn on_tick(&mut self, n: u64) {
        let now = (self.now)();
        self.step(OrgInput::Tick { now_ms: now }).await;
        if n % 2 == 0 {
            self.refresh_descendants().await;
        }
    }

    async fn refresh_descendants(&mut self) {
        let Some(state) = self.core.state() else { return };
        let waiting: Vec<ChatRef> = state
            .tasks()
            .iter()
            .filter(|t| {
                matches!(t.status(), TaskStatus::Running | TaskStatus::Reviewing)
                    && t.confirm() == ConfirmState::Confirmed
                    && t.observed().is_some_and(|o| Some(&o.turn) == t.accepted_turn())
            })
            .filter_map(|t| t.chat().cloned())
            .collect();
        for chat in waiting {
            let Some(key) = chat_key_of_ref(&chat) else { continue };
            let r = self.ports.descendants(&key).await;
            let now = (r.working, r.unknown, r.scan_complete);
            if self.last_desc.get(&chat) == Some(&now) {
                continue;
            }
            self.last_desc.insert(chat.clone(), now);
            self.step(OrgInput::Descendants { chat, working: r.working, unknown: r.unknown, scan_complete: r.scan_complete }).await;
        }
    }

    /// 送信権を手放してエンジンへ知らせる。`then` があればその後に処理する。
    async fn drop_grant_then(&mut self, then: Option<OrgInput>) {
        self.grant = None;
        self.ports.grants().revoke_all();
        let mut q = VecDeque::from([OrgInput::Grant { granted: false }]);
        q.extend(then);
        self.run_queue(q).await;
    }

    /// 入力を1件処理する。切断・wake・再起動の鮮度の入力では、先に送信権を手放す。
    pub async fn step(&mut self, input: OrgInput) {
        if matches!(input, OrgInput::Freshness { .. }) {
            self.last_desc.clear();
            self.drop_grant_then(Some(input)).await;
            return;
        }
        self.run_queue(VecDeque::from([input])).await;
    }

    async fn run_queue(&mut self, mut pending: VecDeque<OrgInput>) {
        while let Some(input) = pending.pop_front() {
            let ended = match &input {
                OrgInput::TurnEnded { chat, .. } => Some(chat.clone()),
                _ => None,
            };
            let actions = self.core.handle(input);
            let mut stop_effects = false;
            for action in actions {
                match self.exec(action, &mut stop_effects).await {
                    Some(follow) => pending.extend(follow),
                    None => {}
                }
            }
            if let Some(chat) = ended {
                self.flush_usage(&chat);
            }
            if self.persist_failed && self.grant.is_some() {
                // 追記できない以上、以後は何も送らない。
                self.grant = None;
                self.ports.grants().revoke_all();
                pending.push_back(OrgInput::Grant { granted: false });
            }
        }
    }

    /// 1つの action を実行する。結果として返す入力があれば返す。`stop_effects` は、このバッチで追記に失敗した後は副作用を実行しない印。
    async fn exec(&mut self, action: OrgAction, stop_effects: &mut bool) -> Option<Vec<OrgInput>> {
        match action {
            OrgAction::Persist(ev) => {
                if self.persist_failed {
                    *stop_effects = true;
                    return None;
                }
                if let Err(e) = self.store.append_event(&ev) {
                    crate::diag::log("org", &format!("append failed: {e:?}"));
                    self.persist_failed = true;
                    *stop_effects = true;
                    self.notice(OrgHostNotice::PersistFailed);
                }
                None
            }
            OrgAction::Notify(n) => {
                self.notice(OrgHostNotice::Engine(n));
                None
            }
            OrgAction::Publish => {
                self.publish_snapshot();
                None
            }
            // ここから先は副作用。追記に失敗した後は実行しない（記録のないまま作る・送ることを避ける）。
            _ if *stop_effects || self.persist_failed => None,
            OrgAction::CreateRoleChat { task, spec } => {
                let result = match self.grant.as_ref().map(|g| g.create_permit(self.ports.grants(), &task)) {
                    Some(Ok(permit)) => {
                        let out = self.ports.create_role_chat(permit, spec).await;
                        if let Some(t) = out.trust {
                            self.notice(OrgHostNotice::Config(t));
                        }
                        out.result
                    }
                    Some(Err(e)) => CreateResult::Rejected(e.message().to_string()),
                    None => CreateResult::Rejected("送信権がありません".to_string()),
                };
                Some(vec![OrgInput::ChatCreated { task, result }])
            }
            OrgAction::CheckSandbox { cwd } => {
                let verdict = judge_sandbox(&self.ports.sandbox_check(&cwd).await);
                Some(vec![match verdict {
                    SandboxVerdict::Healthy => OrgInput::SandboxChecked { ok: true, detail: String::new() },
                    SandboxVerdict::Unhealthy { reason } => OrgInput::SandboxChecked { ok: false, detail: format!("{SANDBOX_UNHEALTHY_NOTICE}\n（{reason}）") },
                }])
            }
            OrgAction::Send { task, chat, attempt, client_message_id, text, output_schema } => {
                Some(self.exec_send(task, chat, attempt, client_message_id, text, output_schema).await)
            }
            OrgAction::ReadFinalMessage { task, chat, turn } => {
                let text = match chat_key_of_ref(&chat) {
                    Some(key) => self.ports.read_final_message(&key, &turn_key_of_ref(&key, &turn)).await.ok().flatten(),
                    None => None,
                };
                Some(vec![OrgInput::FinalMessage { task, turn, text }])
            }
            // 作業場所（worktree・コミット）のホスト実装は P-ORG-5。それまでは未対応として返す（成功にも失敗にもしない）。
            OrgAction::Workspace(w) => {
                let task = match w {
                    WorkspaceAction::CreateWorktree { task, .. } | WorkspaceAction::Commit { task, .. } => task,
                };
                Some(vec![OrgInput::WorkspaceResult { task, result: WorkspaceOutcome::Unsupported }])
            }
        }
    }

    async fn exec_send(&mut self, task: TaskId, chat: ChatRef, attempt: AttemptRef, client_message_id: String, text: String, output_schema: serde_json::Value) -> Vec<OrgInput> {
        // 送っていないことが確かな場合は、結果を知らせる（`Rejected` は「送っていない」ことが確定している）。
        let not_sent = |task: &TaskId, attempt: &AttemptRef| vec![OrgInput::SendSettled { task: task.clone(), attempt: attempt.clone(), outcome: org_engine::SettleOutcome::Rejected }];
        let Some(key) = chat_key_of_ref(&chat) else { return not_sent(&task, &attempt) };
        let Some(grant) = self.grant.as_ref() else { return not_sent(&task, &attempt) };
        let permit = match grant.permit(self.ports.grants(), &task, &key) {
            Ok(p) => p,
            Err(e) => {
                self.notice(OrgHostNotice::SendRefused { task: task.clone(), why: e.message().to_string() });
                return not_sent(&task, &attempt);
            }
        };
        let spec = OrgSendSpec { chat: key, attempt: crate::backend::model::LocalId(attempt.0.clone()), client_message_id, text, output_schema };
        match self.ports.send(permit, spec).await {
            // 受理・拒否・受理不明は監視の受け口が知らせる（ここでは作らない）。
            OrgSendResult::Dispatched(_) => Vec::new(),
            OrgSendResult::NotLive => vec![OrgInput::NotLive { task }],
            OrgSendResult::Held(why) => {
                self.notice(OrgHostNotice::SendRefused { task: task.clone(), why });
                not_sent(&task, &attempt)
            }
        }
    }

    fn notice(&self, n: OrgHostNotice) {
        if let Some(s) = self.core.state() {
            self.sink.notice(s.workflow_id(), n);
        }
    }

    fn publish_snapshot(&self) {
        let Some(state) = self.core.state() else { return };
        if !self.persist_failed {
            if let Err(e) = self.store.save_snapshot(state) {
                // イベントから再構築できるので致命的ではない。
                crate::diag::log("org", &format!("snapshot failed: {e:?}"));
            }
        }
        self.sink.publish(snapshot_of(state));
    }

    /// turn の終端で、最後に届いた使用量の標本を1行にして記録する。種別が分からない（再起動後など）ときは書かない。
    fn flush_usage(&mut self, chat: &ChatRef) {
        let Some(sample) = self.latest_usage.remove(chat) else { return };
        let Some(state) = self.core.state() else { return };
        let Some(task) = state.tasks().iter().find(|t| t.chat() == Some(chat)) else { return };
        let Some(kind) = self.core.last_turn_kind(&task.id) else { return };
        let row = UsageRow {
            schema_version: SCHEMA_VERSION,
            at_ms: (self.now)(),
            workflow_id: state.workflow_id().clone(),
            task: Some(task.id.clone()),
            kind,
            turns_used: state.counters().turns_used,
            last_total_tokens: sample.last_total_tokens,
            thread_total_tokens: sample.thread_total_tokens,
            model_context_window: sample.model_context_window,
        };
        if let Err(e) = self.store.append_usage(&row) {
            crate::diag::log("org", &format!("usage append failed: {e:?}"));
        }
    }
}


#[cfg(test)]
mod tests;
