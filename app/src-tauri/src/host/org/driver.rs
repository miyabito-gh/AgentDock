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
use super::ports::{AgentRuntime, OrgHostNotice, OrgSendResult, OrgSendSpec, RuntimeRead, Workspace};
use super::stall::{StallWatch, DEFAULT_STALL_MS};
use crate::rules::org::{chat_key_of_ref, judge_sandbox, sandbox_notice, turn_key_of_ref, SandboxVerdict};

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
    /// そのチャットに新しい出力（活動）があった（停滞の判断だけに使う。送信・状態の変更はしない）。
    Activity { chat: ChatRef, at_ms: i64 },
    /// 再起動・wake・再接続後の照合（読取りだけ）。終端を履歴で確認できた `running` のタスクの終端をエンジンへ知らせる。
    Reconcile,
    /// ここまでのメッセージを処理し終えたら知らせる（完全終了の前に状態の保存を待つため）。
    Sync(tokio::sync::oneshot::Sender<()>),
    /// 停滞とみなす時間（ミリ秒）を変える（設定の反映。通知の条件だけで、送信・状態は変えない）。
    SetStallThreshold(i64),
    Shutdown,
}

/// 画面向けの状態の写し（P-ORG-7 が IPC の型へ変換する）。
#[derive(Debug, Clone, PartialEq)]
pub struct OrgSnapshot {
    pub workflow: WorkflowId,
    /// 依頼の先頭（一覧・確認画面の名前。依頼が空なら空文字ではなく None）。
    pub title: Option<String>,
    pub halt: Option<org_engine::HaltReason>,
    pub tasks: Vec<TaskSnapshot>,
    /// 依頼の全文と作業するリポジトリ（画面の見出し用）。
    pub request: String,
    pub repo: String,
    pub limits: org_engine::Limits,
    pub turns_used: u32,
    pub auto_replans_used: u32,
}

/// 依頼文の先頭1行を名前にする（長いものは切る）。
fn title_of(request: &str) -> Option<String> {
    let line = request.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(60).collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub role: String,
    pub status: TaskStatus,
    pub confirm: ConfirmState,
    pub chat: Option<ChatRef>,
    pub title: String,
    pub deps: Vec<TaskId>,
    pub effects: Vec<org_engine::Effect>,
    pub attempts: u32,
    pub commit: Option<String>,
    /// 役割チャットの作成を始めたときの作業フォルダ（作成結果が不明なタスクの候補探しに使う）。
    pub chat_cwd: Option<String>,
    /// 「やり直す」を出せる（送っていないことが確かで、初回のturnが受理されていない。R-ORG M2）。
    pub retryable: bool,
    /// 送信の記録（Dispatched）も受理されたturnもない（チャットは一度も発話を受けていない）。
    pub never_sent: bool,
}

/// エンジンの状態から画面向けの写しを作る。
pub fn snapshot_of(state: &org_engine::WorkflowState) -> OrgSnapshot {
    OrgSnapshot {
        workflow: state.workflow_id().clone(),
        title: title_of(state.request()),
        halt: state.halt(),
        tasks: state
            .tasks()
            .iter()
            .map(|t| TaskSnapshot {
                id: t.id.clone(),
                role: t.role.0.clone(),
                status: t.status(),
                confirm: t.confirm(),
                chat: t.chat().cloned(),
                title: t.title.clone(),
                deps: t.deps.clone(),
                effects: t.effects.clone(),
                attempts: t.attempts(),
                commit: t.commit().map(str::to_string),
                chat_cwd: t.chat_cwd().map(str::to_string),
                retryable: org_engine::retryable(t),
                never_sent: t.last_attempt().is_none() && t.accepted_turn().is_none(),
            })
            .collect(),
        request: state.request().to_string(),
        repo: state.repo().to_string(),
        limits: state.limits().clone(),
        turns_used: state.counters().turns_used,
        auto_replans_used: state.counters().auto_replans_used,
    }
}

/// 状態の公開先（画面への合図・通知・状態の写し）。
pub trait OrgSink: Send + Sync {
    fn publish(&self, snapshot: OrgSnapshot);
    fn notice(&self, workflow: &WorkflowId, notice: OrgHostNotice);
}

type DynStore = Arc<dyn OrgStore + Send + Sync>;

pub struct OrgDriver<P: AgentRuntime + RuntimeRead + Workspace> {
    core: OrgCore,
    store: DynStore,
    ports: Arc<P>,
    sink: Arc<dyn OrgSink>,
    grant: Option<OrgGrant>,
    /// 追記に失敗した。以後は何も送らない（再起動で復元する）。
    persist_failed: bool,
    latest_usage: HashMap<ChatRef, UsageSample>,
    last_desc: HashMap<ChatRef, (u32, u32, bool)>,
    /// 停滞の検出（通知だけ。自動で中断しない）。
    stall: StallWatch,
    now: Box<dyn Fn() -> i64 + Send>,
}

impl<P: AgentRuntime + RuntimeRead + Workspace + 'static> OrgDriver<P> {
    pub fn new(core: OrgCore, store: DynStore, ports: Arc<P>, sink: Arc<dyn OrgSink>, now: Box<dyn Fn() -> i64 + Send>) -> Self {
        Self { core, store, ports, sink, grant: None, persist_failed: false, latest_usage: HashMap::new(), last_desc: HashMap::new(), stall: StallWatch::new(DEFAULT_STALL_MS), now }
    }

    /// 停滞とみなす時間（既定30分）。設定の画面は P-ORG-7。
    pub fn set_stall_threshold_ms(&mut self, ms: i64) {
        self.stall.set_threshold_ms(ms);
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
            if self.is_finished() {
                // すべて終わった。最後の状態は公開・保存済み。送信権を手放し、この task を終える（`Host` が登録を外す）。
                self.grant = None;
                self.ports.grants().revoke_all();
                break;
            }
        }
    }

    /// ワークフローのすべてのタスクが終端（完了・失敗・取消）になった。以後この task にできることはない。
    pub fn is_finished(&self) -> bool {
        self.core.state().is_some_and(|s| {
            !s.tasks().is_empty() && s.tasks().iter().all(|t| matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled))
        })
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
            DriverMsg::Activity { chat, at_ms } => self.stall.note_activity(&chat, at_ms),
            DriverMsg::Reconcile => self.reconcile_read_only().await,
            DriverMsg::Sync(done) => {
                let _ = done.send(());
            }
            DriverMsg::SetStallThreshold(ms) => self.stall.set_threshold_ms(ms),
            DriverMsg::Shutdown => {}
        }
    }

    /// 1秒ごと: 時刻を進め、完了判定の待ちにある task の子孫の様子を読み直して知らせる（読取りのみ）。停滞も見る（通知だけ）。
    pub async fn on_tick(&mut self, n: u64) {
        let now = (self.now)();
        self.step(OrgInput::Tick { now_ms: now }).await;
        if n % 2 == 0 {
            self.refresh_descendants().await;
        }
        self.check_stalls(now);
    }

    /// 確定していて `running` のままのタスクに、設定時間の間、新しい出力がなければ通知する。中断・再送・resume はしない。
    fn check_stalls(&mut self, now: i64) {
        let Some(state) = self.core.state() else { return };
        let running: Vec<(TaskId, Option<ChatRef>)> = state
            .tasks()
            .iter()
            .filter(|t| t.status() == TaskStatus::Running && t.confirm() == ConfirmState::Confirmed)
            .map(|t| (t.id.clone(), t.chat().cloned()))
            .collect();
        for s in self.stall.check(now, &running) {
            self.notice(OrgHostNotice::Stalled { task: s.task, quiet_minutes: (s.quiet_ms / 60_000) as u32 });
        }
    }

    /// 再起動・wake・再接続後の照合（および「再開」の後の読み直し）。読取りだけ（履歴の読込みと `git status`）で、resume・送信・中断をしない。
    /// 対象は、確認状態が要照合・中断要求済み・停止未確認の `running` のタスク。
    /// 終端を履歴で確認できたタスクだけ `TurnEnded` を知らせる（その後の完了判定は通常どおり：成果物の検証・子孫の確認・書込みは再開後）。
    /// 確認できない（進行中・不明・読取り失敗）ものは要照合のまま残し、結果を通知に控える。
    async fn reconcile_read_only(&mut self) {
        let Some(state) = self.core.state() else { return };
        let targets: Vec<(TaskId, ChatRef, org_engine::TurnRef, Option<String>)> = state
            .tasks()
            .iter()
            .filter(|t| {
                t.status() == TaskStatus::Running
                    && matches!(t.confirm(), ConfirmState::NeedsReconcile(_) | ConfirmState::InterruptRequested | ConfirmState::StopUnconfirmed)
            })
            .filter_map(|t| Some((t.id.clone(), t.chat()?.clone(), t.accepted_turn()?.clone(), t.chat_cwd().map(str::to_string))))
            .collect();
        for (task, chat, turn, cwd) in targets {
            let Some(key) = chat_key_of_ref(&chat) else { continue };
            let read = self.ports.read_reconcile(&key, &turn_key_of_ref(&key, &turn), cwd.as_deref()).await;
            self.notice(OrgHostNotice::Reconciled { task, turn_read: read.turn_read, turn_end: read.turn_end, worktree: read.worktree });
            if let Some(end) = read.turn_end {
                self.step(OrgInput::TurnEnded { chat, turn, end: crate::rules::org::turn_end_kind_of(end) }).await;
            }
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
        if let OrgInput::RequestPending { chat, open } = &input {
            self.stall.note_requests(chat, *open);
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
                let ok = verdict == SandboxVerdict::Healthy;
                Some(vec![OrgInput::SandboxChecked { ok, detail: sandbox_notice(&verdict) }])
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
            // 作業場所（worktree作成・所有worktree内のコミット）。結果は成功・失敗・変更なし・未対応のまま返す（推定しない）。
            OrgAction::Workspace(w) => {
                let task = match &w {
                    WorkspaceAction::CreateWorktree { task, .. } | WorkspaceAction::Commit { task, .. } | WorkspaceAction::RefreshDetached { task, .. } => task.clone(),
                };
                let result = match self.core.state().map(|s| s.workflow_id().clone()) {
                    Some(workflow) => self.ports.run(&workflow, w).await,
                    None => WorkspaceOutcome::Failed { detail: "ワークフローの状態がありません".to_string() },
                };
                Some(vec![OrgInput::WorkspaceResult { task, result }])
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
