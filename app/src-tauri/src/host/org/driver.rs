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
    AttemptRef, ChatRef, ConfirmState, CreateResult, InterjectOutcome, NoteId, OrgAction, OrgCore, OrgInput, OrgStore, TaskId, TaskStatus,
    TurnRef, UsageRow, WorkflowId, WorkspaceAction, WorkspaceOutcome,
};
use tokio::sync::mpsc::UnboundedReceiver;

use super::grant::{InterjectPermit, OrgGrant};
use super::ports::{AgentRuntime, OrgHostNotice, OrgSendResult, OrgSendSpec, OrgSteerSpec, RuntimeRead, Workspace};
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
    /// グラフで動くワークフローの流れの写し（従来の計画経路と、読めなかったときは None）。
    pub graph: Option<GraphSnapshot>,
    /// 追加指示（古い順。取り消したものも含む）と、それぞれの届き具合。
    pub notes: Vec<NoteSnapshot>,
    /// 割り込み指示（古い順）。
    pub interjects: Vec<org_engine::Interject>,
    /// 計画の承認待ちのとき、計画の作成後に追加した追加指示の件数。
    pub notes_after_plan: u32,
}

/// 追加指示1件と、届き具合（終わっていないタスクのうち未達・届いたタスク）。
#[derive(Debug, Clone, PartialEq)]
pub struct NoteSnapshot {
    pub note: org_engine::UserNote,
    pub pending: Vec<TaskId>,
    pub delivered: Vec<TaskId>,
}

/// タスクごとの介入の可否（エンジンの純粋な判定を、状態の写しに添えたもの）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskExtras {
    pub held: bool,
    /// 計画の作業・検証タスクだけ Some。スキップの見込み（一緒に飛ばすタスク・検証なしで完了にするタスク）か、できない理由。
    pub skip: Option<Result<org_engine::SkipPlan, String>>,
    /// 計画の作業タスクだけ Some。差し戻せるか（指摘の有無は問わない）か、できない理由。
    pub send_back: Option<Result<(), String>>,
    pub can_accept_as_is: bool,
    /// 調査タスク（ファイルを変えず、結果は報告）。
    pub investigation: bool,
    pub can_hold: bool,
    /// このタスクにまだ届いていない追加指示の件数。
    pub notes_pending: u32,
    /// 割り込みの経路の判定の材料（受理済みの turn・決着・終端の観測）。
    pub accepted_turn: Option<TurnRef>,
    pub accepted_settled: bool,
    pub has_settle: bool,
    pub turn_ended: bool,
    /// 前の割り込みの結果がまだ決まっていない。
    pub open_interject: bool,
    /// 直近の Reviewer の所見（イベントの FindingsRecorded から復元済み。空は「指摘なし」か未記録）。
    pub last_findings: Vec<String>,
    /// 直近のやりとりが変更なしで終わった（Worker が何も書かなかった）。
    pub nothing_written: bool,
}

/// グラフ定義と、画面に出すための進み具合（周回の回数・スロットの最新の版）。
#[derive(Debug, Clone, PartialEq)]
pub struct GraphSnapshot {
    pub graph: org_engine::graph::CompiledGraph,
    /// 差し戻しの辺ごとの（使った回数, 利用者が足した回数）。
    pub feedback: std::collections::BTreeMap<org_engine::graph::EdgeId, (u32, u32)>,
    /// スロットごとの最新の版。
    pub slot_versions: std::collections::BTreeMap<org_engine::graph::SlotId, u32>,
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
    /// 再送の見込み（最初からやり直す／新しい試行で再送）か、できない理由。送信権・実行中の処理の有無はエンジンが送信時に確かめる。
    pub retry: Result<org_engine::RetryKind, org_engine::RetryRefusal>,
    /// これまでの新しい試行での再送回数。
    pub retry_count: u32,
    /// 手動の結果を取り込み中（`Running` の間、通常の送信を拒否する）。
    pub adopting: bool,
    /// 手動に切り替え中のタスクの結果の形式（JSONスキーマの文）。取得できなければ None。
    pub manual_format: Option<String>,
    /// このタスクが動かすグラフのノード（グラフで動くワークフローだけ）。
    pub node: Option<org_engine::graph::NodeId>,
    pub extras: TaskExtras,
}

/// エンジンの状態から画面向けの写しを作る。`graph` はグラフで動くワークフローの定義（なければ None）。
pub fn snapshot_of(state: &org_engine::WorkflowState, graph: Option<&org_engine::graph::CompiledGraph>) -> OrgSnapshot {
    let graph = graph.filter(|_| state.graph().is_some()).map(|g| GraphSnapshot {
        graph: g.clone(),
        feedback: g.loops.iter().map(|l| (l.edge.clone(), state.feedback_of(&l.edge))).collect(),
        slot_versions: state.slot_versions().map(|(s, v)| (s.clone(), v)).collect(),
    });
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
                retry: org_engine::retry_kind(t, state.limits(), state.counters().turns_remaining(state.limits())),
                retry_count: t.retry_count(),
                adopting: t.adopting(),
                manual_format: None,
                node: t.node().cloned(),
                extras: extras_of(state, t),
            })
            .collect(),
        request: state.request().to_string(),
        repo: state.repo().to_string(),
        limits: state.limits().clone(),
        turns_used: state.counters().turns_used,
        auto_replans_used: state.counters().auto_replans_used,
        graph,
        notes: state
            .notes()
            .iter()
            .map(|n| {
                let (pending, delivered) = org_engine::note_reach(state, n);
                NoteSnapshot { note: n.clone(), pending, delivered }
            })
            .collect(),
        interjects: state.interjects().to_vec(),
        notes_after_plan: notes_after_plan(state),
    }
}

/// タスクごとの介入の可否（エンジンの純粋な判定。画面の表示と、操作の事前確認に使う）。
fn extras_of(state: &org_engine::WorkflowState, t: &org_engine::Task) -> TaskExtras {
    let plan = org_engine::is_worker(t) || org_engine::is_reviewer(t);
    let worker = org_engine::is_worker(t);
    let remaining = state.counters().turns_remaining(state.limits());
    let (_, delivered_pending) = (0, state.notes().iter().filter(|n| !n.withdrawn && org_engine::note_reach(state, n).0.contains(&t.id)).count() as u32);
    TaskExtras {
        held: t.held(),
        skip: plan.then(|| org_engine::skip_plan(state, &t.id)),
        send_back: worker.then(|| org_engine::send_back_check(state, &t.id, remaining, Some("x")).map(|_| ())),
        can_accept_as_is: worker && org_engine::accept_as_is_check(state, &t.id).is_ok(),
        investigation: t.is_investigation(),
        can_hold: plan
            && matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready)
            && t.confirm() == ConfirmState::Confirmed
            && !t.held(),
        notes_pending: delivered_pending,
        accepted_turn: t.accepted_turn().cloned(),
        accepted_settled: matches!(t.last_settle(), Some(org_engine::SettleOutcome::Accepted { .. } | org_engine::SettleOutcome::ReconciledAccepted { .. })),
        has_settle: t.last_settle().is_some(),
        turn_ended: t.observed().is_some_and(|o| Some(&o.turn) == t.accepted_turn()),
        open_interject: state.interjects().iter().any(|i| i.task == t.id && i.outcome.is_none()),
        last_findings: t.last_findings().to_vec(),
        nothing_written: t.nothing_written(),
    }
}

/// 計画の承認待ちのとき、計画の作成後（最新の Planner へ渡した後）に追加した、全体・タスク宛ての追加指示の件数。
fn notes_after_plan(state: &org_engine::WorkflowState) -> u32 {
    if state.halt() != Some(org_engine::HaltReason::AwaitingPlanApproval) {
        return 0;
    }
    let seen = state.tasks().iter().filter(|t| t.role.0 == "planner").map(|t| t.notes_seen_seq()).max().unwrap_or(0);
    state
        .notes()
        .iter()
        .filter(|n| !n.withdrawn && n.seq > seen && matches!(n.scope, org_engine::NoteScope::AllFollowing | org_engine::NoteScope::Task { .. }))
        .count() as u32
}

/// 状態の公開先（画面への合図・通知・状態の写し）。
pub trait OrgSink: Send + Sync {
    fn publish(&self, snapshot: OrgSnapshot);
    fn notice(&self, workflow: &WorkflowId, notice: OrgHostNotice);
    /// 記録（events.jsonl）への追記が成功した。進行タイムラインの材料（追記に失敗した記録は渡さない）。
    fn persisted(&self, _workflow: &WorkflowId, _event: &org_engine::OrgEvent) {}
    /// 進行タイムラインに新しい行がある（ドライバの1秒の刻みで、1秒に1回まで）。
    fn timeline_updated(&self, _workflow: &WorkflowId, _last_seq: u64) {}
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
    /// 追記したがタイムラインの合図をまだ出していない記録の最後の番号。
    timeline_dirty: Option<u64>,
}

impl<P: AgentRuntime + RuntimeRead + Workspace + 'static> OrgDriver<P> {
    pub fn new(core: OrgCore, store: DynStore, ports: Arc<P>, sink: Arc<dyn OrgSink>, now: Box<dyn Fn() -> i64 + Send>) -> Self {
        Self { core, store, ports, sink, grant: None, persist_failed: false, latest_usage: HashMap::new(), last_desc: HashMap::new(), stall: StallWatch::new(DEFAULT_STALL_MS), now, timeline_dirty: None }
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
        if let (Some(seq), Some(s)) = (self.timeline_dirty.take(), self.core.state()) {
            self.sink.timeline_updated(s.workflow_id(), seq);
        }
    }

    /// 確定していて `running` のままのタスクに、設定時間の間、新しい出力がなければ通知する。中断・再送・resume はしない。
    fn check_stalls(&mut self, now: i64) {
        let Some(state) = self.core.state() else { return };
        let running: Vec<(TaskId, Option<ChatRef>)> = state
            .tasks()
            .iter()
            .filter(|t| t.status() == TaskStatus::Running && t.confirm() == ConfirmState::Confirmed)
            // 承認・質問の回答待ち（利用者の対応待ち）は停滞ではない。
            .filter(|t| t.chat().is_none_or(|c| self.core.open_requests_of(c) == 0))
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
                self.step(OrgInput::TurnEnded { chat, turn, end: crate::rules::org::turn_end_kind_of(end, &read.turn_failure) }).await;
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
                } else if let Some(s) = self.core.state() {
                    self.sink.persisted(s.workflow_id(), &ev);
                    self.timeline_dirty = Some(ev.seq);
                }
                None
            }
            // スロットの値の書込み（`Persist(SlotWritten)` の直前に来る）。外へ出る副作用ではないが、書けないまま次へ進むと、
            // 値のない版を指す記録ができる。書けなければ以後の副作用を止める（追記の失敗と同じ扱い。辺を推測で選ばない）。
            OrgAction::PutSlot { slot, version, value } => {
                if self.persist_failed {
                    *stop_effects = true;
                    return None;
                }
                if let Err(e) = self.store.put_slot(&slot, version, &value) {
                    crate::diag::log("org", &format!("slot write failed slot={} version={version}: {e:?}", slot.0));
                    self.persist_failed = true;
                    *stop_effects = true;
                    self.notice(OrgHostNotice::SlotWriteFailed { slot: slot.0.clone() });
                }
                None
            }
            // 調査報告（調査タスクの唯一の成果）。書けないまま完了を記録すると、後続へ渡す報告のない完了ができる。
            // 書けなければ以後の副作用を止める（追記の失敗と同じ扱い）。
            OrgAction::PutReport { task, attempt, value } => {
                if self.persist_failed {
                    *stop_effects = true;
                    return None;
                }
                if let Err(e) = self.store.put_output(&task, attempt, &value) {
                    crate::diag::log("org", &format!("report write failed task={} attempt={attempt}: {e:?}", task.0));
                    self.persist_failed = true;
                    *stop_effects = true;
                    self.notice(OrgHostNotice::SlotWriteFailed { slot: format!("調査報告 {}", task.0) });
                }
                None
            }
            // Worker報告・Reviewer判定の控え（表示用）。経路の判定には使わないので、書けなくても以後の副作用は止めない（診断ログだけ）。
            OrgAction::PutOutput { task, attempt, value } => {
                if let Err(e) = self.store.put_output(&task, attempt, &value) {
                    crate::diag::log("org", &format!("output write failed task={} attempt={attempt}: {e:?}", task.0));
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
            OrgAction::Steer { id, chat, turn, client_message_id, text, .. } => Some(self.exec_steer(id, chat, turn, client_message_id, text).await),
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

    /// 実行中のturnへの割り込み。新しいturnは始めない。結果（届いた・届かなかった・不明）は監視の受け口が `InterjectSettled` で知らせる。
    /// ここで返すのは、送っていないことが確かな場合の「届かなかった」だけ。受理不明を再送しない。
    async fn exec_steer(&mut self, id: NoteId, chat: ChatRef, turn: TurnRef, client_message_id: String, text: String) -> Vec<OrgInput> {
        let not_delivered = |reason: &str| vec![OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::NotDelivered { reason: reason.to_string() } }];
        let Some(key) = chat_key_of_ref(&chat) else { return not_delivered("チャットを特定できません") };
        let permit = match InterjectPermit::issue(self.ports.grants(), key.clone(), turn_key_of_ref(&key, &turn)) {
            Ok(p) => p,
            Err(e) => return not_delivered(e.message()),
        };
        let spec = OrgSteerSpec { attempt: crate::backend::model::LocalId(id.0.clone()), client_message_id, text: org_engine::prompt::interject_prompt(&text) };
        match self.ports.steer(permit, spec).await {
            OrgSendResult::Dispatched(_) => Vec::new(),
            OrgSendResult::NotLive => not_delivered("チャットがliveでないため、送りませんでした"),
            OrgSendResult::Held(why) => not_delivered(&why),
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
        let mut snap = snapshot_of(state, self.core.graph());
        for t in snap.tasks.iter_mut().filter(|t| t.status == TaskStatus::Blocked(org_engine::BlockedReason::Manual)) {
            t.manual_format = self.core.output_format_of(&t.id);
        }
        self.sink.publish(snap);
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
