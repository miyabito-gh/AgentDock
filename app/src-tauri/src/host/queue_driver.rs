//! 完了後キューの実行・追加指示の設定・受理不明の照合（P3、要件§3.7・DESIGN_P2 §2）。
//!
//! 判断は `rules::queue`（純粋）に任せ、ここは状態の収集・保存・送信・イベント発行だけを行う。
//! - 自動送信は、ユーザーが登録した依頼を条件がそろったときに1件ずつ送るもので、監視目的の操作ではない。
//!   `resume`（ensure_live）は自動送信から呼ばない。親がliveでなければ保留し、再開はユーザーの「キューを再開」で行う。
//! - 送信の前に `Sending` を保存し、保存できなければ送らない。再起動後は自動送信しない（PausedAfterRestart）。
//! - 受理不明は時間で打ち切らず間隔を延ばして照合を続ける。切断中は止め、再接続後に再開する。再送はしない。
//! - 履歴の読取り（終端の確認）は読み取りのみで、resume・送信・承認をしない。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::cloud::paths_overlap;
use super::state::{agent_key_of, history_tail_of, tail_confirms_idle, HistoryTail, HostData};
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::rules::queue as q;
use crate::store::records::{QueueFile, UnresolvedSendRecord, SCHEMA_VERSION};

const QUEUE_TICK: Duration = Duration::from_secs(2);
/// 受理不明の照合間隔（接続中）。以後は最後の値のまま続ける。
const RECONCILE_STEPS: [Duration; 4] = [Duration::from_secs(5), Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)];
/// 同じエージェントの履歴確認の最短間隔。
const TERMINAL_RECHECK: Duration = Duration::from_secs(10);
const TERMINAL_READS_PER_TICK: usize = 8;

/// 履歴の読取りで確認した、エージェントの最新turnの終端。読み取り専用の確認結果で、状態の根拠は `terminal_explicit` のときだけ使う。
#[derive(Debug, Clone)]
struct TerminalFact {
    turn: Option<ExternalId>,
    end: Option<TurnEnd>,
    completed_at: Option<UnixMillis>,
    checked_at: UnixMillis,
}

/// 再読（履歴の読取り）が要るか。記録がなければ要る。あれば、古く（stale）、かつ最後の確認・最後の読取り失敗のどちらからも
/// `recheck_ms` 以上経っているときだけ。読めない間も間隔を守る（失敗時刻は確認時刻とは別に持つ）。
fn terminal_reread_due(has_fact: bool, stale: bool, checked_at: UnixMillis, last_failed_at: Option<UnixMillis>, now: UnixMillis, recheck_ms: i64) -> bool {
    if !has_fact {
        return last_failed_at.is_none_or(|f| now.0 - f.0 >= recheck_ms);
    }
    let last = last_failed_at.map_or(checked_at.0, |f| f.0.max(checked_at.0));
    stale && now.0 - last >= recheck_ms
}

/// 履歴で確認した終端を、このエージェントの最新turnの根拠として使えるか。
/// 一覧・走査で作った子は最新turnを持たない（`thread/list` はturnを返さない）ので、その場合は履歴の最新turnを採る。
/// 最新turnを持つのに履歴の最新turnと違うときは、新しいturnが始まっているので使わない。
fn history_term<'a>(latest: Option<&ExternalId>, term: Option<&'a TerminalFact>) -> Option<&'a TerminalFact> {
    term.filter(|t| t.end.is_some() && t.turn.is_some() && latest.is_none_or(|l| t.turn.as_ref() == Some(l)))
}

/// 履歴の末尾の様子を、再確認の結果にする。終端（またはturnなし）を確認できたものだけ `TerminalFound`、それ以外は状態不明のまま。
/// 状態を終端扱いにする条件（`unknown_confirmed_idle`）と同じ判定にそろえ、最新turnが食い違うときは確認済みと言わない。
fn recheck_outcome_of(latest_turn: Option<&ExternalId>, tail: &HistoryTail) -> RecheckOutcome {
    if tail_confirms_idle(latest_turn, tail) {
        RecheckOutcome::TerminalFound
    } else {
        RecheckOutcome::StillUnknown
    }
}

/// キュー実行の作業状態（保存しない）。
#[derive(Default)]
pub struct QueueRuntime {
    notify: Notify,
    /// 評価・送信中のチャット（同じチャットで同時に送らない）。
    busy: Mutex<HashSet<ChatKey>>,
    terminals: Mutex<HashMap<AgentKey, TerminalFact>>,
    /// 履歴を読めなかった最後の時刻（確認時刻とは別。成功したら消す）。読めない間の再読の間隔を守る。
    read_failed: Mutex<HashMap<AgentKey, UnixMillis>>,
    /// 照合を回している受理不明の送信。
    reconciling: Mutex<HashSet<LocalId>>,
    /// 子孫の走査が完了したか（ルート別。未走査はキーなし＝未完了）。
    scans: Mutex<HashMap<AgentKey, bool>>,
    /// 走査を最後に（再）開始した時刻。失敗が続くとき、作り直しの間隔を空ける。
    scan_started: Mutex<HashMap<AgentKey, UnixMillis>>,
    awaiting_checked: Mutex<HashMap<ChatKey, UnixMillis>>,
    /// チャット単位の送信ロック。手動送信（再開を含む）とキューの自動送信が同時に `turn/start` を送らないよう、両方が取る。
    send_locks: Mutex<HashMap<ChatKey, Arc<tokio::sync::Mutex<()>>>>,
    /// 手動送信を受理したturn（受理の時刻つき）。開始・終端を観測するまで、キューは親が作業中として保留する。
    manual_accepts: Mutex<HashMap<ChatKey, (ExternalId, UnixMillis)>>,
    /// レビュー・圧縮など、turnを始める操作の受付時刻。そのturnの終端を観測するまで（上限15秒）、キューは保留する。
    op_accepts: Mutex<HashMap<ChatKey, UnixMillis>>,
    /// 作業フォルダを書き換える操作（変更を戻す・クラウド取込み）の実行中のフォルダ。重なる作業フォルダのチャットのキューを保留する。
    folder_ops: Mutex<Vec<String>>,
    /// 送信前の保存に失敗して送らなかったチャット。保存が成功するまで保留にする（2秒ごとの警告を繰り返さない）。
    save_blocked: Mutex<HashSet<ChatKey>>,
}

impl QueueRuntime {
    pub fn send_lock(&self, chat: &ChatKey) -> Arc<tokio::sync::Mutex<()>> {
        self.send_locks.lock().unwrap().entry(chat.clone()).or_default().clone()
    }

    pub fn note_manual_accept(&self, chat: &ChatKey, turn: ExternalId) {
        self.manual_accepts.lock().unwrap().insert(chat.clone(), (turn, now_ms()));
    }

    /// turnを始める操作（レビュー・圧縮）の受付を記録する。応答の時点では、そのturnの開始通知が未処理のことがある。
    pub fn note_op_accept(&self, chat: &ChatKey) {
        self.op_accepts.lock().unwrap().insert(chat.clone(), now_ms());
    }

    pub fn note_scan(&self, root: &AgentKey, complete: bool) {
        self.scans.lock().unwrap().insert(root.clone(), complete);
    }

    /// 新しいturnの受理・走査の開始で、前の走査結果を未完了へ戻す（そのturnで生まれた子孫を見落とさないため）。
    pub fn reset_scan(&self, root: &AgentKey) {
        self.scans.lock().unwrap().insert(root.clone(), false);
        self.scan_started.lock().unwrap().insert(root.clone(), now_ms());
    }
}

/// 同じチャットの走査を、失敗が続くときに作り直す最短間隔。
const SCAN_RESTART_INTERVAL_MS: i64 = 10_000;

/// 走査を（再）起動すべきか。完了しておらず、実行中でなく、接続がlive（切断中は起動しない）で、
/// 前回の開始から間隔が空いているとき。reset後に走査が `note_scan` 前に失敗しても、ここで作り直される。
fn should_start_scan(ready: bool, scan_running: bool, connected: bool, last_start: Option<UnixMillis>, now: UnixMillis) -> bool {
    !ready && !scan_running && connected && last_start.is_none_or(|t| now.0 - t.0 >= SCAN_RESTART_INTERVAL_MS)
}

/// 自動送信の判断に使う「子孫の走査が完了している」か。走査の実行中は、結果が途中なので未完了として扱う。
fn scan_ready(flag: Option<bool>, scan_running: bool) -> bool {
    flag == Some(true) && !scan_running
}

/// 手動送信の受理から、そのturnの終端を観測するまでの間か（受理から一定時間で打ち切り、観測できないまま保留し続けない）。
const MANUAL_PENDING_MS: i64 = 15_000;
fn manual_turn_pending(accepted: Option<&(ExternalId, UnixMillis)>, last_end_turn: Option<&ExternalId>, now: UnixMillis) -> bool {
    accepted.is_some_and(|(turn, at)| now.0 - at.0 < MANUAL_PENDING_MS && last_end_turn != Some(turn))
}

/// 操作（レビュー・圧縮）の受付から、受付以後のturnの終端を観測するまでの間か（上限は手動送信と同じ。観測できないまま保留し続けない）。
fn op_turn_pending(accepted: Option<UnixMillis>, last_end_at: Option<UnixMillis>, now: UnixMillis) -> bool {
    accepted.is_some_and(|since| now.0 - since.0 < MANUAL_PENDING_MS && !last_end_at.is_some_and(|at| at.0 >= since.0))
}

/// 実行中のフォルダ操作の対象と、チャットの作業フォルダが重なるか。
fn folder_op_overlaps(active: &[String], cwd: Option<&str>) -> bool {
    cwd.is_some_and(|w| active.iter().any(|f| paths_overlap(w, f)))
}

/// フォルダ操作の実行中を示す（破棄で解除し、キューを起こす）。
pub(super) struct FolderOpGuard {
    host: Arc<Host>,
    folders: Vec<String>,
}

impl Drop for FolderOpGuard {
    fn drop(&mut self) {
        {
            let mut v = self.host.queue_rt.folder_ops.lock().unwrap();
            for f in &self.folders {
                if let Some(i) = v.iter().position(|x| x == f) {
                    v.remove(i);
                }
            }
        }
        self.host.kick_queue();
    }
}

/// 照合1回の結果。
pub(super) enum ReconcileStep {
    Resolved(SendAttempt),
    StillUnknown(SendAttempt),
    /// 別のtaskが照合中。
    Busy,
    /// すでに確定している。
    Gone,
}

fn is_working(s: AgentState) -> bool {
    matches!(s, AgentState::Running | AgentState::Waiting | AgentState::Initializing)
}

fn new_file(chat: &ChatKey) -> QueueFile {
    QueueFile { schema_version: SCHEMA_VERSION, queue: q::new_queue(chat.clone()), unresolved_sends: Vec::new() }
}

fn queue_event(file: &QueueFile) -> HostEvent {
    HostEvent::ChatQueueUpdated { queue: file.queue.clone() }
}

/// ホスト側の状況（データのロックの外で取るもの）。
struct GateContext {
    quitting: bool,
    deleting: bool,
    manual: Option<(ExternalId, UnixMillis)>,
    /// 操作（レビュー・圧縮）の受付時刻。
    op_accept: Option<UnixMillis>,
    /// 作業フォルダが、実行中のフォルダ操作（戻す・クラウド取込み）と重なる。
    folder_busy: bool,
}

/// 状態（`HostData`）と履歴確認の結果から、純粋な判断の入力を組む。
fn gate_input(
    d: &HostData,
    chat: &ChatKey,
    terminals: &HashMap<AgentKey, TerminalFact>,
    scan_complete: bool,
    unresolved: Option<LocalId>,
    ctx: GateContext,
    user_confirmed_unknown: bool,
) -> q::GateInput {
    let root_key = agent_key_of(chat);
    let fact = |v: &AgentView| -> q::AgentFact {
        let key = &v.agent.key;
        let latest = v.agent.latest_turn.as_ref();
        let live_end = d.last_end.get(key).filter(|(t, _, _)| Some(t) == latest).map(|(_, end, at)| (*end, *at));
        let term = history_term(latest, terminals.get(key));
        let history_end = term.and_then(|t| t.end.map(|e| (e, t.completed_at.unwrap_or(t.checked_at))));
        q::AgentFact {
            agent: key.clone(),
            state: v.status.state,
            freshness: v.freshness,
            // 最新turnが未取得の子は、終端を確認した履歴のturnを最新とする（終端だけがあってturnがない値は評価が信用しない）。
            latest_turn: v.agent.latest_turn.clone().or_else(|| term.and_then(|t| t.turn.clone())),
            latest_end: live_end.or(history_end),
            terminal_explicit: term.is_some(),
            observed_at: term.map(|t| t.checked_at).unwrap_or(v.status.evidence.observed_at),
        }
    };
    q::GateInput {
        connected: d.sources.first().is_some_and(|s| matches!(s.connection, ConnectionState::Connected)),
        root: d.view(&root_key).map(fact),
        descendants: d.agents.iter().filter(|v| &v.agent.chat == chat && v.agent.key != root_key).map(fact).collect(),
        descendant_scan_complete: scan_complete,
        open_stop: d.open_stop(chat).map(|r| r.id.clone()),
        unresolved_attempt: unresolved,
        unresolved_op: d.locals.get(chat).and_then(|l| l.pending_ops.first()).map(|p| p.id.clone()),
        delete_pending: ctx.deleting || d.locals.get(chat).is_some_and(|l| l.delete_pending.is_some()),
        quitting: ctx.quitting,
        manual_turn_pending: manual_turn_pending(ctx.manual.as_ref(), d.last_end.get(&root_key).map(|(t, _, _)| t), now_ms())
            || op_turn_pending(ctx.op_accept, d.last_end.get(&root_key).map(|(_, _, at)| *at), now_ms())
            || ctx.folder_busy,
        user_confirmed_unknown,
    }
}

/// 送信時点で有効なチャットの設定。
struct Plan {
    text: String,
    attachments: Vec<LocalId>,
    model: Option<ModelChoice>,
    permission: Option<PermissionPreset>,
    cwd: Option<String>,
    work_mode: Option<WorkMode>,
}

impl Host {
    // ───────────── 起動・周期 ─────────────

    /// キューの周期評価と、復元した受理不明の照合を始める（イベントpump開始時に1回）。
    pub(super) fn start_queue_driver(self: &Arc<Self>) {
        let ids: Vec<LocalId> = self.unconfirmed.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.start_reconcile(id);
        }
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = host.queue_rt.notify.notified() => {}
                    _ = tokio::time::sleep(QUEUE_TICK) => {}
                }
                host.queue_tick().await;
            }
        });
    }

    /// 作業フォルダを書き換える操作の間、重なるチャットのキューの自動送信を保留する。戻り値を持っている間が対象。
    pub(super) fn begin_folder_op(self: &Arc<Self>, folders: Vec<String>) -> FolderOpGuard {
        self.queue_rt.folder_ops.lock().unwrap().extend(folders.iter().cloned());
        FolderOpGuard { host: self.clone(), folders }
    }

    pub(super) fn folder_op_active(&self, chat: &ChatKey) -> bool {
        let active = self.queue_rt.folder_ops.lock().unwrap().clone();
        !active.is_empty() && folder_op_overlaps(&active, self.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned())).as_deref())
    }

    /// 状態が変わったかもしれないので、次の評価を早める。
    pub(super) fn kick_queue(&self) {
        self.queue_rt.notify.notify_one();
    }

    async fn queue_tick(self: &Arc<Self>) {
        let chats: Vec<ChatKey> = self.read(|d| {
            d.queues
                .iter()
                .filter(|(_, f)| q::has_pending(&f.queue) || f.queue.awaiting.is_some() || f.queue.hold.is_some())
                .map(|(c, _)| c.clone())
                .collect()
        });
        for chat in chats {
            if !self.queue_rt.busy.lock().unwrap().insert(chat.clone()) {
                continue;
            }
            self.drive_chat(&chat).await;
            self.queue_rt.busy.lock().unwrap().remove(&chat);
        }
    }

    pub(super) fn is_connected(&self) -> bool {
        self.read(|d| d.sources.first().is_some_and(|s| matches!(s.connection, ConnectionState::Connected)))
    }

    /// 1チャット分: 待っているturnの終端確認 → 判断 → 送信・停止・保留表示。
    async fn drive_chat(self: &Arc<Self>, chat: &ChatKey) {
        // 手動送信（再開を含む）の最中は評価しない（同時に `turn/start` を送らない）。次の周期で見る。
        let send_lock = self.queue_rt.send_lock(chat);
        let Ok(_send_guard) = send_lock.try_lock() else { return };
        self.settle_awaiting(chat).await;
        let (active, pending) = self.read(|d| d.queues.get(chat).map(|f| (f.queue.run == QueueRun::Active, q::has_pending(&f.queue))).unwrap_or((false, false)));
        if !(active && pending) {
            // 評価しない間は保留理由を持ち越さない。
            self.set_hold(chat, None);
            return;
        }
        let root = agent_key_of(chat);
        let scan_running = self.scanning.lock().unwrap().contains(&root);
        let flag = self.queue_rt.scans.lock().unwrap().get(&root).copied();
        let last_start = self.queue_rt.scan_started.lock().unwrap().get(&root).copied();
        if should_start_scan(scan_ready(flag, scan_running), scan_running, self.is_connected(), last_start, now_ms()) {
            // 走査が完了していない（未実施、または失敗して終わった）。読み取りだけの走査を始める（完了まで状態不明として保留）。
            self.start_scan(root.clone());
        }
        self.refresh_terminal_facts(chat).await;
        let terminals = self.queue_rt.terminals.lock().unwrap().clone();
        let scan_running = self.scanning.lock().unwrap().contains(&root);
        let scan_complete = scan_ready(self.queue_rt.scans.lock().unwrap().get(&root).copied(), scan_running);
        let unresolved = self.unknown_attempt(chat);
        // 送信前の保存に失敗したままなら、保存が成功するまで保留する（2秒ごとに送信を試して警告を出し続けない）。
        if self.queue_rt.save_blocked.lock().unwrap().contains(chat) {
            if self.read(|d| d.save_status.get(&SaveScope::Queue { chat: chat.clone() }).is_some_and(|s| matches!(s.state, SaveState::SaveFailed { .. }))) {
                self.set_hold(chat, Some(QueueHold::SaveFailed));
                return;
            }
            self.queue_rt.save_blocked.lock().unwrap().remove(chat);
        }
        let ctx = GateContext {
            quitting: self.quit_phase() != QuitPhase::Idle,
            deleting: self.manage_rt.is_deleting(chat),
            manual: self.queue_rt.manual_accepts.lock().unwrap().get(chat).cloned(),
            op_accept: self.queue_rt.op_accepts.lock().unwrap().get(chat).copied(),
            folder_busy: self.folder_op_active(chat),
        };
        let decision = self.read(|d| d.queues.get(chat).map(|f| q::evaluate(&f.queue, &gate_input(d, chat, &terminals, scan_complete, unresolved, ctx, false))));
        match decision {
            Some(q::GateDecision::Send { entry }) => self.send_queue_entry(chat, entry).await,
            Some(q::GateDecision::Hold(h)) => self.set_hold(chat, Some(h)),
            Some(q::GateDecision::Stop(cause)) => {
                self.mutate(|d| {
                    let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
                    q::stop(&mut f.queue, cause, now_ms());
                    ((), vec![queue_event(f)])
                });
                self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
            }
            Some(q::GateDecision::Nothing) | Some(q::GateDecision::Paused) | None => self.set_hold(chat, None),
        }
    }

    /// 保留理由（表示用・保存しない）。変わったときだけUIへ出す。
    fn set_hold(&self, chat: &ChatKey, hold: Option<QueueHold>) {
        self.mutate(|d| {
            let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
            if f.queue.hold == hold {
                return ((), vec![]);
            }
            f.queue.hold = hold;
            ((), vec![queue_event(f)])
        });
    }

    /// 自動送信したturnの終端を確認する。liveで観測済みならそれを使い、なければ（停止後の再確認として）履歴を読む。
    async fn settle_awaiting(self: &Arc<Self>, chat: &ChatKey) {
        let Some(turn) = self.read(|d| d.queues.get(chat).and_then(|f| f.queue.awaiting.clone())) else { return };
        let live = self.read(|d| d.last_end.get(&turn.agent).filter(|(t, _, _)| t == &turn.turn_id).map(|(_, end, _)| *end));
        let end = match live {
            Some(e) => Some(e),
            None => {
                // 状態が作業中でも、liveでなければ（切断・再起動をまたいだ死んだturnかもしれないので）履歴を読む。
                let working = self.read(|d| d.view(&turn.agent).is_none_or(|v| is_working(v.status.state) && v.freshness == Freshness::Live));
                let now = now_ms();
                let due = {
                    let mut m = self.queue_rt.awaiting_checked.lock().unwrap();
                    let due = m.get(chat).is_none_or(|t| now.0 - t.0 >= TERMINAL_RECHECK.as_millis() as i64);
                    if due && !working {
                        m.insert(chat.clone(), now);
                    }
                    due && !working
                };
                if !due {
                    return;
                }
                match self.read_history(turn.agent.clone(), ReadOptions { include_turns: true }).await {
                    Ok(h) => h.turns.iter().find(|t| t.key.turn_id == turn.turn_id).and_then(|t| t.end),
                    Err(_) => None,
                }
            }
        };
        let Some(end) = end else { return };
        let changed = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(chat) else { return (false, vec![]) };
            let c = q::turn_ended(&mut f.queue, &turn, end, now_ms());
            (c, if c { vec![queue_event(f)] } else { vec![] })
        });
        if changed {
            self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
        }
    }

    /// live購読でない子孫の最新turnの終端を、履歴の読取りで確認する（resumeしない）。
    async fn refresh_terminal_facts(self: &Arc<Self>, chat: &ChatKey) {
        let now = now_ms();
        let cands: Vec<AgentKey> = {
            let terminals = self.queue_rt.terminals.lock().unwrap().clone();
            let failed = self.queue_rt.read_failed.lock().unwrap().clone();
            self.read(|d| {
                let baseline = d.queues.get(chat).and_then(|f| f.queue.baseline_at);
                let root = agent_key_of(chat);
                d.agents
                    .iter()
                    .filter(|v| &v.agent.chat == chat && v.agent.key != root && v.freshness != Freshness::Live && !is_working(v.status.state))
                    .filter(|v| {
                        let failed_at = failed.get(&v.agent.key).copied();
                        let recheck = TERMINAL_RECHECK.as_millis() as i64;
                        match terminals.get(&v.agent.key) {
                            None => terminal_reread_due(false, true, now, failed_at, now, recheck),
                            Some(t) => {
                                let stale = v.agent.latest_turn.as_ref().is_some_and(|l| t.turn.as_ref() != Some(l)) || baseline.is_some_and(|b| t.checked_at < b) || t.end.is_none();
                                terminal_reread_due(true, stale, t.checked_at, failed_at, now, recheck)
                            }
                        }
                    })
                    .map(|v| v.agent.key.clone())
                    .take(TERMINAL_READS_PER_TICK)
                    .collect()
            })
        };
        for agent in cands {
            self.read_terminal_fact(agent).await;
        }
    }

    /// 1エージェントの履歴を読み、最新turnの終端を確認結果として記録する（読み取りのみ。resumeしない）。終端を確認できたら true。
    async fn read_terminal_fact(self: &Arc<Self>, agent: AgentKey) -> bool {
        matches!(self.read_tail_fact(agent).await, Ok(HistoryTail::Ended(..)))
    }

    /// 履歴を読み、確認結果をキュー用の記録（`terminals`）へ残して、末尾の様子を返す（読み取りのみ）。読めなければ理由を返す。
    async fn read_tail_fact(self: &Arc<Self>, agent: AgentKey) -> Result<HistoryTail, String> {
        let (fact, res) = match self.read_history(agent.clone(), ReadOptions { include_turns: true }).await {
            Ok(h) => {
                let last = h.turns.last();
                let fact = TerminalFact {
                    turn: last.map(|t| t.key.turn_id.clone()),
                    end: last.and_then(|t| t.end),
                    completed_at: last.and_then(|t| t.completed_at.value().copied()),
                    checked_at: now_ms(),
                };
                (fact, Ok(history_tail_of(&h.turns)))
            }
            // 読めなかった: 確認できていないまま。次の間隔で再試行する（終端があったことにしない）。
            // 以前に確認できた記録があれば、空の事実で上書きしない（何も変えない）。
            // 失敗時刻だけを別に記録し（確認時刻は新しく見せない）、再読の間隔を守る。
            Err(e) => {
                self.queue_rt.read_failed.lock().unwrap().insert(agent, now_ms());
                return Err(e.to_string());
            }
        };
        self.queue_rt.read_failed.lock().unwrap().remove(&agent);
        self.queue_rt.terminals.lock().unwrap().insert(agent, fact);
        res
    }

    /// 「履歴で再確認」（M52。ユーザー操作）。状態不明の子孫の保存履歴を読むだけ（resume・送信・停止をしない。キューの有無に依存しない）。
    /// 終端を確認できたものは「履歴で確認」として記録し、確認できなければ状態不明のまま。読めなければ何も変えない。
    pub async fn recheck_unknown_agents(self: &Arc<Self>, args: RecheckAgentsArgs) -> Result<RecheckAgentsResult, IpcError> {
        if !self.is_connected() {
            return Err(err(IpcErrorCode::NotConnected, "Codex に接続していないため、履歴を確認できません"));
        }
        self.require_chat(&args.chat)?;
        let root = agent_key_of(&args.chat);
        let facts: Vec<q::RecheckFact> = self.read(|d| {
            d.agents
                .iter()
                .filter(|v| v.agent.chat == args.chat)
                .map(|v| q::RecheckFact { agent: v.agent.key.clone(), state: v.status.state, freshness: v.freshness })
                .collect()
        });
        let mut results = Vec::new();
        for plan in q::recheck_candidates(&root, &args.agents, &facts) {
            let outcome = match plan.skip {
                Some(reason) => RecheckOutcome::Skipped { reason },
                None => match self.read_tail_fact(plan.agent.clone()).await {
                    Ok(tail) => {
                        let agent = plan.agent.clone();
                        let latest = self.read(|d| d.view(&agent).and_then(|v| v.agent.latest_turn.clone()));
                        let outcome = recheck_outcome_of(latest.as_ref(), &tail);
                        self.mutate(|d| ((), d.note_history_tail(&agent, tail, now_ms())));
                        outcome
                    }
                    Err(message) => RecheckOutcome::Unreadable { message },
                },
            };
            results.push(AgentRecheck { agent: plan.agent, outcome });
        }
        // 確認できた終端は、キューの送信条件（合意済みの規則のまま）の判断に使われる。ボタンはキューを直接送らない。
        self.kick_queue();
        Ok(RecheckAgentsResult { results })
    }

    /// 「状態を再確認」（ユーザー操作）。ライブでなく作業中でもない子孫の履歴を、間隔を待たずに読み直す（読み取りのみ。resumeしない）。
    /// 走査が完了していなければ読み取りの走査も始める。終端を確認できた結果は、次の評価（すぐ）で自動送信の判断に使われる。
    pub async fn recheck_queue_state(self: &Arc<Self>, args: ChatArgs) -> Result<ChatQueue, IpcError> {
        if !self.is_connected() {
            return Err(err(IpcErrorCode::NotConnected, "Codex に接続していないため、状態を再確認できません"));
        }
        if self.read(|d| d.queues.get(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "送信待ちがありません"));
        }
        let root = agent_key_of(&args.chat);
        let scan_running = self.scanning.lock().unwrap().contains(&root);
        let flag = self.queue_rt.scans.lock().unwrap().get(&root).copied();
        if !scan_running && !scan_ready(flag, false) {
            self.start_scan(root.clone());
        }
        let cands: Vec<AgentKey> = self.read(|d| {
            d.agents
                .iter()
                .filter(|v| v.agent.chat == args.chat && v.agent.key != root && v.freshness != Freshness::Live && !is_working(v.status.state))
                .map(|v| v.agent.key.clone())
                .collect()
        });
        for agent in cands {
            self.read_terminal_fact(agent).await;
        }
        self.kick_queue();
        self.read(|d| d.queues.get(&args.chat).map(|f| f.queue.clone())).ok_or_else(|| err(IpcErrorCode::NotFound, "送信待ちがありません"))
    }

    /// 「確認して今すぐ送る」（ユーザー操作。確認ダイアログの承認後に呼ばれる）。先頭の送信待ち1件だけを、
    /// 状態不明・ライブ未復旧の子孫と走査未完了を承知のうえで送る。作業中の子孫・失敗・中断・親の条件・停止未確認・受理不明は緩めない。
    /// 送信は通常の経路（Sending保存→送信、受理不明の扱い）を通る。指定は1回の評価だけで、次の項目は通常の判定に戻る。
    pub async fn send_queue_entry_now(self: &Arc<Self>, args: QueueEntryArgs, _confirmed: &UserConfirmed) -> Result<ChatQueue, IpcError> {
        let chat = args.chat.clone();
        // 手動送信・自動送信と同時に `turn/start` を送らない。
        let send_lock = self.queue_rt.send_lock(&chat);
        let _send_guard = send_lock.lock().await;
        self.check_not_delete_pending(&chat)?;
        self.precheck_space()?;
        let root = agent_key_of(&chat);
        let scan_running = self.scanning.lock().unwrap().contains(&root);
        let scan_complete = scan_ready(self.queue_rt.scans.lock().unwrap().get(&root).copied(), scan_running);
        let terminals = self.queue_rt.terminals.lock().unwrap().clone();
        let unresolved = self.unknown_attempt(&chat);
        let ctx = GateContext {
            quitting: self.quit_phase() != QuitPhase::Idle,
            deleting: self.manage_rt.is_deleting(&chat),
            manual: self.queue_rt.manual_accepts.lock().unwrap().get(&chat).cloned(),
            op_accept: self.queue_rt.op_accepts.lock().unwrap().get(&chat).copied(),
            folder_busy: self.folder_op_active(&chat),
        };
        let decision = self.read(|d| d.queues.get(&chat).map(|f| q::evaluate(&f.queue, &gate_input(d, &chat, &terminals, scan_complete, unresolved, ctx, true))));
        match decision {
            None => Err(err(IpcErrorCode::NotFound, "送信待ちがありません")),
            Some(q::GateDecision::Send { entry }) if entry == args.entry => {
                self.send_queue_entry(&chat, entry).await;
                self.read(|d| d.queues.get(&chat).map(|f| f.queue.clone())).ok_or_else(|| err(IpcErrorCode::NotFound, "送信待ちがありません"))
            }
            Some(q::GateDecision::Send { .. }) => Err(err(IpcErrorCode::InvalidArgs, "先頭の送信待ちの依頼ではないため、今すぐ送れません")),
            Some(q::GateDecision::Hold(h)) => {
                self.set_hold(&chat, Some(h));
                Err(err(IpcErrorCode::Blocked, "状態不明の対象以外の理由（作業中・接続・停止未確認・受理不明など）で保留されているため、送りませんでした"))
            }
            Some(q::GateDecision::Stop(_)) => Err(err(IpcErrorCode::Blocked, "失敗または中断を検出したため、送りませんでした。キューの状態を確認してください")),
            Some(q::GateDecision::Paused) => Err(err(IpcErrorCode::Blocked, "キューが止まっているため送りませんでした。先に「キューを再開」してください")),
            Some(q::GateDecision::Nothing) => Err(err(IpcErrorCode::InvalidArgs, "送れる依頼がありません")),
        }
    }

    // ───────────── 送信 ─────────────

    /// 送信時点のチャット設定（モデル・権限・次のturnの作業フォルダ）。
    fn chat_overrides(&self, chat: &ChatKey) -> (Option<ModelChoice>, Option<PermissionPreset>, Option<String>) {
        self.read(|d| {
            let l = d.locals.get(chat);
            (d.model_settings.get(chat).and_then(|s| s.selected.clone()), l.and_then(|l| l.permission), l.and_then(|l| l.next_cwd.clone()))
        })
    }

    /// 送信時点の計画／実行の選択（未選択なら None）。
    pub(super) fn chat_work_mode(&self, chat: &ChatKey) -> Option<WorkMode> {
        self.read(|d| d.model_settings.get(chat).and_then(|s| s.work_mode))
    }

    /// 計画／実行を送るときにバックエンドが要る「モデル」。選択値がなければ、受理を確認できた値（速度は引き継がない）。
    /// どちらもなければ None（バックエンドが送らずに拒否する）。計画／実行の選択がなければ選択値をそのまま返す。
    pub(super) fn model_for_send(&self, chat: &ChatKey, selected: Option<ModelChoice>, work_mode: Option<WorkMode>) -> Option<ModelChoice> {
        if work_mode.is_none() {
            return selected;
        }
        let accepted = self.read(|d| d.model_settings.get(chat).and_then(|s| s.accepted.value().cloned()));
        super::chat_prefs::resolve_model_for_work_mode(selected, accepted, &self.models_cache.lock().unwrap())
    }

    /// 項目を Sending にして、送信時点の設定を記録する。送れない状態の項目なら None。
    fn begin_entry_send(&self, chat: &ChatKey, entry: &LocalId, attempt: SendAttempt) -> Option<Plan> {
        let now = attempt.at;
        self.mutate(|d| {
            let (model, permission, cwd_override, work_mode) = {
                let l = d.locals.get(chat);
                let work_mode = d.model_settings.get(chat).and_then(|s| s.work_mode);
                let selected = d.model_settings.get(chat).and_then(|s| s.selected.clone());
                let model = if work_mode.is_some() {
                    let accepted = d.model_settings.get(chat).and_then(|s| s.accepted.value().cloned());
                    super::chat_prefs::resolve_model_for_work_mode(selected, accepted, &self.models_cache.lock().unwrap())
                } else {
                    selected
                };
                (model, l.and_then(|l| l.permission), l.and_then(|l| l.next_cwd.clone()), work_mode)
            };
            let cwd_known = match &cwd_override {
                Some(c) => Known::direct(c.clone()),
                None => d.chat(chat).map(|c| c.cwd.clone()).unwrap_or(Known::NotFetched),
            };
            let applied = AppliedSettings { model: model.clone(), permission: permission.unwrap_or(PermissionPreset::WorkspaceWriteOnRequest), cwd: cwd_known, decided_at: now, work_mode };
            let Some(f) = d.queues.get_mut(chat) else { return (None, vec![]) };
            let Some((text, attachments)) = f.queue.entries.iter().find(|e| &e.id == entry).map(|e| (e.text.clone(), e.attachments.clone())) else { return (None, vec![]) };
            if !q::begin_send(&mut f.queue, entry, attempt, applied) {
                return (None, vec![]);
            }
            (Some(Plan { text, attachments, model, permission, cwd: cwd_override, work_mode }), vec![queue_event(f)])
        })
    }

    /// 自動送信（評価が `Send` を返した先頭の項目）。
    async fn send_queue_entry(self: &Arc<Self>, chat: &ChatKey, entry: LocalId) {
        let attempt = SendAttempt { attempt_id: self.local_id("att"), client_message_id: self.local_id("cm").0, at: now_ms(), state: SendState::Sending };
        let Some(plan) = self.begin_entry_send(chat, &entry, attempt.clone()) else { return };
        let attachments = match self.prepare_attachments(chat, &plan.attachments).await {
            Ok(a) => a,
            Err(e) => {
                // 使えない添付（欠損・失敗など）がある。送っていないので送信待ちに戻し、キューを止める（再開はユーザー操作）。
                let now = now_ms();
                let name = self.attachment_name_for(chat, &e, &plan.attachments);
                self.mutate(|d| {
                    let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
                    q::abort_send(&mut f.queue, &entry);
                    q::stop(&mut f.queue, QueueStopCause::AttachmentUnavailable { entry: entry.clone(), name: name.clone() }, now);
                    ((), vec![queue_event(f)])
                });
                self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
                self.warn(format!("送信待ちの依頼の添付を使えないため、依頼を送っていません。キューを止めました: {}", e.message));
                return;
            }
        };
        let request = SendRequest {
            chat: chat.clone(),
            mode: SendMode::NewTurn,
            text: plan.text,
            attachments,
            speed_tier: plan.model.as_ref().and_then(|m| m.speed_tier.clone()),
            model: plan.model,
            permission: plan.permission,
            cwd: plan.cwd,
            work_mode: plan.work_mode,
            client_message_id: attempt.client_message_id.clone(),
        };
        self.run_entry_send(chat, entry, attempt, request, None).await;
    }

    /// 受理なしが確定した項目の、ユーザー確認つきの再送（`retry_send` から）。
    pub(super) async fn retry_entry_send(self: &Arc<Self>, chat: &ChatKey, entry: LocalId, mut request: SendRequest) -> Result<SendAttempt, IpcError> {
        let attempt = SendAttempt { attempt_id: self.local_id("att"), client_message_id: request.client_message_id.clone(), at: now_ms(), state: SendState::Sending };
        // 送信前の保存に失敗したときに元の受理なしへ戻せるよう、再送前の状態を控える。
        let prior = self.read(|d| {
            d.queues.get(chat).and_then(|f| f.queue.entries.iter().find(|e| e.id == entry)).and_then(|e| match &e.state {
                QueueEntryState::NotAccepted { attempt, message } => Some((attempt.clone(), message.clone())),
                _ => None,
            })
        });
        let Some(plan) = self.begin_entry_send(chat, &entry, attempt.clone()) else {
            return Err(err(IpcErrorCode::NotFound, "再送できる依頼がありません"));
        };
        // 送信時点で有効な設定を使う。
        request.model = plan.model;
        request.permission = plan.permission;
        request.cwd = plan.cwd;
        Ok(self.run_entry_send(chat, entry, attempt, request, prior).await)
    }

    /// Sending を保存してから送り、結果を項目へ反映する。保存できなければ送らない。
    async fn run_entry_send(self: &Arc<Self>, chat: &ChatKey, entry: LocalId, attempt: SendAttempt, request: SendRequest, prior: Option<(LocalId, String)>) -> SendAttempt {
        let scope = SaveScope::Queue { chat: chat.clone() };
        if let Some(st) = self.save_now(scope).await {
            if !matches!(st.state, SaveState::Saved { .. }) {
                self.mutate(|d| {
                    let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
                    match &prior {
                        // 再送の取消し: 元の「受理なし」に戻し、再送権（保管した送信内容）も戻す。
                        Some((old, message)) => q::abort_retry(&mut f.queue, &entry, old.clone(), message.clone()),
                        None => q::abort_send(&mut f.queue, &entry),
                    }
                    ((), vec![queue_event(f)])
                });
                if let Some((old, _)) = &prior {
                    self.rejected.lock().unwrap().insert(old.clone(), NotAccepted::new(request));
                }
                self.queue_rt.save_blocked.lock().unwrap().insert(chat.clone());
                self.warn("送信前に送信待ちを保存できなかったため、依頼を送っていません。保存の状態を確認してください。");
                return SendAttempt { state: SendState::Rejected { message: "送信前の保存に失敗したため送っていません".into() }, ..attempt };
            }
        }
        let sent_cwd = request.cwd.clone();
        // 変更の控え（基準B）は送信の直前に取る。失敗しても送信は止めない。
        if matches!(request.mode, SendMode::NewTurn) {
            self.baseline_take_base(chat, &attempt.attempt_id, request.cwd.clone()).await;
        }
        let outcome = self.backend.send(request).await;
        let now = now_ms();
        let mut done = attempt.clone();
        let result = match outcome {
            SendOutcome::Accepted { turn, .. } => {
                self.baseline_bound(chat, &attempt.attempt_id, &turn.turn_id);
                done.state = SendState::Accepted { turn: turn.clone() };
                q::SendResult::Accepted { attempt: done.clone(), turn, turn_started_at: now }
            }
            SendOutcome::Rejected { error, request } => {
                let message = error.to_string();
                done.state = SendState::Rejected { message: message.clone() };
                self.baseline_abandoned(chat, &attempt.attempt_id, "rejected");
                self.rejected.lock().unwrap().insert(attempt.attempt_id.clone(), request);
                q::SendResult::Rejected { attempt: done.clone(), message }
            }
            SendOutcome::AcceptanceUnknown(u) => {
                done.state = SendState::AcceptanceUnknown { since: u.since };
                let applied = self.read(|d| d.queues.get(chat).and_then(|f| f.queue.entries.iter().find(|e| e.id == entry)).and_then(|e| e.applied.clone()));
                self.register_unknown(chat, &attempt.attempt_id, Some(entry.clone()), applied, u);
                q::SendResult::Unknown { attempt: done.clone() }
            }
        };
        let accepted = matches!(done.state, SendState::Accepted { .. });
        self.mutate(|d| {
            let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
            q::apply_send_result(&mut f.queue, &entry, result, now);
            ((), vec![queue_event(f)])
        });
        self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
        self.emit_send(chat, &done);
        if accepted {
            // 新しいturnで子孫が生まれ得る。前の走査結果は使わず、再走査（読み取りのみ）の完了まで保留する。
            let root = agent_key_of(chat);
            self.queue_rt.reset_scan(&root);
            self.start_scan(root);
            if let Some(cwd) = sent_cwd {
                self.apply_cwd_after_accept(chat, cwd);
            }
            self.update_local(chat, true, Duration::ZERO, |f| f.last_used_at = Some(now));
        }
        self.kick_queue();
        done
    }

    /// 作業フォルダ付きで受理されたら、チャットの作業フォルダを更新し、次のturn用の値を外す（同じ値のときだけ）。
    pub(super) fn apply_cwd_after_accept(self: &Arc<Self>, chat: &ChatKey, cwd: String) {
        self.mutate(|d| {
            if let Some(c) = d.chats.iter_mut().find(|c| &c.key == chat) {
                c.cwd = Known::direct(cwd.clone());
                let c = c.clone();
                return ((), vec![HostEvent::ChatUpdated { chat: c }]);
            }
            ((), vec![])
        });
        self.update_local(chat, true, Duration::ZERO, |f| {
            if f.next_cwd.as_deref() == Some(cwd.as_str()) {
                f.next_cwd = None;
            }
        });
    }

    /// 直接送信に使う、送信時点のモデル以外の設定（権限・次のturnの作業フォルダ）。追加指示には付けない。
    pub(super) fn send_overrides(&self, chat: &ChatKey) -> (Option<PermissionPreset>, Option<String>) {
        let (_, permission, cwd) = self.chat_overrides(chat);
        (permission, cwd)
    }

    pub(super) fn queue_entry_for_attempt(&self, attempt: &LocalId) -> Option<(ChatKey, LocalId)> {
        self.read(|d| {
            d.queues.iter().find_map(|(c, f)| {
                f.queue
                    .entries
                    .iter()
                    .find(|e| matches!(&e.state, QueueEntryState::NotAccepted { attempt: a, .. } if a == attempt))
                    .map(|e| (c.clone(), e.id.clone()))
            })
        })
    }

    // ───────────── 受理不明 ─────────────

    /// 受理不明の送信を登録する（送信・再送を止める根拠。保存して再起動後も照合を続ける）。照合を始める。
    pub(super) fn register_unknown(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, entry: Option<LocalId>, applied: Option<AppliedSettings>, u: UnconfirmedSend) {
        let record = UnresolvedSendRecord {
            attempt: attempt.clone(),
            chat: chat.clone(),
            client_message_id: u.client_message_id().to_string(),
            since: u.since,
            entry,
            text: u.text().to_string(),
            attachments: Vec::new(),
            applied,
        };
        self.unresolved.lock().unwrap().add(attempt.clone(), chat.clone());
        self.unconfirmed.lock().unwrap().insert(attempt.clone(), u);
        self.mutate(|d| {
            let f = d.queues.entry(chat.clone()).or_insert_with(|| new_file(chat));
            // 送信前の記録（手動送信）があれば置き換える。
            f.unresolved_sends.retain(|r| &r.attempt != attempt);
            f.unresolved_sends.push(record);
            ((), vec![])
        });
        self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
        self.start_reconcile(attempt.clone());
    }

    /// 手動送信の送信前記録を保存する（受理不明と同じ形。受理・拒否が確定したら消す）。保存に失敗したら Err（送らない）。
    pub(super) async fn record_presend(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, request: &SendRequest, since: UnixMillis) -> Result<(), String> {
        let record = UnresolvedSendRecord {
            attempt: attempt.clone(),
            chat: chat.clone(),
            client_message_id: request.client_message_id.clone(),
            since,
            entry: None,
            text: request.text.clone(),
            attachments: Vec::new(),
            applied: None,
        };
        self.mutate(|d| {
            d.queues.entry(chat.clone()).or_insert_with(|| new_file(chat)).unresolved_sends.push(record);
            ((), vec![])
        });
        if let Some(st) = self.save_now(SaveScope::Queue { chat: chat.clone() }).await {
            if !matches!(st.state, SaveState::Saved { .. }) {
                self.drop_unresolved_record(chat, attempt);
                self.queue_rt.save_blocked.lock().unwrap().insert(chat.clone());
                return Err("送信前の保存に失敗したため送っていません".into());
            }
        }
        Ok(())
    }

    pub(super) fn drop_unresolved_record(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId) {
        let changed = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(chat) else { return (false, vec![]) };
            let n = f.unresolved_sends.len();
            f.unresolved_sends.retain(|r| &r.attempt != attempt);
            (f.unresolved_sends.len() != n, vec![])
        });
        if changed {
            self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
        }
    }

    /// 起動時: 保存してあった受理不明の記録から、照合用の値を作り直す（再送はしない。照合はアダプターが `client_message_id` だけで行う）。
    pub(super) fn restore_unresolved(&mut self) {
        let mut found: Vec<(LocalId, ChatKey, UnconfirmedSend)> = Vec::new();
        let mut warnings = Vec::new();
        for (chat, f) in &self.data.get_mut().unwrap().queues {
            let mut seen: HashSet<LocalId> = HashSet::new();
            for e in &f.queue.entries {
                if let QueueEntryState::AcceptanceUnknown { attempt } = &e.state {
                    match e.attempts.iter().find(|a| &a.attempt_id == attempt) {
                        Some(a) => {
                            seen.insert(attempt.clone());
                            found.push((attempt.clone(), chat.clone(), UnconfirmedSend::restore(chat.clone(), a.client_message_id.clone(), e.text.clone(), a.at)));
                        }
                        None => warnings.push(StartupWarning::acknowledgeable(format!("受理不明の送信の記録（照合に必要なID）が残っていません。履歴で送信の有無を確認してください（チャット {}）。", chat.id.0))),
                    }
                }
            }
            for r in &f.unresolved_sends {
                if seen.insert(r.attempt.clone()) {
                    found.push((r.attempt.clone(), chat.clone(), UnconfirmedSend::restore(chat.clone(), r.client_message_id.clone(), r.text.clone(), r.since)));
                }
            }
        }
        self.data.get_mut().unwrap().startup_warnings.extend(warnings);
        for (id, chat, u) in found {
            self.unresolved.get_mut().unwrap().add(id.clone(), chat);
            self.unconfirmed.get_mut().unwrap().insert(id, u);
        }
    }

    /// 照合を繰り返す（読み取りのみ）。5→10→30→60秒、以後60秒。打ち切らない。切断中は止め、再接続後に最初の間隔から再開する。
    fn start_reconcile(self: &Arc<Self>, attempt: LocalId) {
        if !self.queue_rt.reconciling.lock().unwrap().insert(attempt.clone()) {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            let mut step = 0usize;
            loop {
                tokio::time::sleep(RECONCILE_STEPS[step.min(RECONCILE_STEPS.len() - 1)]).await;
                if !host.unresolved.lock().unwrap().contains(&attempt) {
                    break;
                }
                if !host.is_connected() {
                    step = 0;
                    continue;
                }
                match host.reconcile_once(&attempt).await {
                    ReconcileStep::Resolved(_) | ReconcileStep::Gone => break,
                    ReconcileStep::StillUnknown(_) => step += 1,
                    ReconcileStep::Busy => {}
                }
            }
            host.queue_rt.reconciling.lock().unwrap().remove(&attempt);
        });
    }

    /// 履歴との照合を1回行う（再送しない）。確定したら項目・キューへ反映する。
    pub(super) async fn reconcile_once(self: &Arc<Self>, attempt_id: &LocalId) -> ReconcileStep {
        let Some(pending) = self.unconfirmed.lock().unwrap().remove(attempt_id) else {
            return if self.unresolved.lock().unwrap().contains(attempt_id) { ReconcileStep::Busy } else { ReconcileStep::Gone };
        };
        let chat = pending.chat().clone();
        let client_message_id = pending.client_message_id().to_string();
        let since = pending.since;
        let make = |state: SendState| SendAttempt { attempt_id: attempt_id.clone(), client_message_id: client_message_id.clone(), at: since, state };
        match self.backend.reconcile_send(pending).await {
            ReconcileOutcome::Accepted { turn } => {
                self.unresolved.lock().unwrap().resolve(attempt_id);
                self.drop_unresolved_record(&chat, attempt_id);
                self.baseline_bound(&chat, attempt_id, &turn.turn_id);
                self.apply_reconciled(&chat, attempt_id, |a| q::SendResult::ReconciledAccepted { attempt: a, turn: turn.clone() });
                let a = make(SendState::Accepted { turn });
                self.emit_send(&chat, &a);
                ReconcileStep::Resolved(a)
            }
            ReconcileOutcome::NotFound(na) => {
                self.unresolved.lock().unwrap().resolve(attempt_id);
                self.rejected.lock().unwrap().insert(attempt_id.clone(), na);
                self.drop_unresolved_record(&chat, attempt_id);
                self.baseline_abandoned(&chat, attempt_id, "notFoundAfterReconcile");
                self.apply_reconciled(&chat, attempt_id, |a| q::SendResult::ReconciledNotFound { attempt: a });
                let a = make(SendState::NotFoundAfterReconcile);
                self.emit_send(&chat, &a);
                ReconcileStep::Resolved(a)
            }
            ReconcileOutcome::StillUnknown(u) => {
                self.unconfirmed.lock().unwrap().insert(attempt_id.clone(), u);
                ReconcileStep::StillUnknown(make(SendState::AcceptanceUnknown { since }))
            }
        }
    }

    /// 受理不明だった項目（あれば）へ、照合の結果を反映する。キュー外の直接送信なら何もしない。
    fn apply_reconciled(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, make: impl FnOnce(LocalId) -> q::SendResult) {
        let changed = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(chat) else { return (false, vec![]) };
            let Some(entry) = f.queue.entries.iter().find(|e| matches!(&e.state, QueueEntryState::AcceptanceUnknown { attempt: a } if a == attempt)).map(|e| e.id.clone()) else {
                return (false, vec![]);
            };
            q::apply_send_result(&mut f.queue, &entry, make(attempt.clone()), now_ms());
            (true, vec![queue_event(f)])
        });
        if changed {
            self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
            self.kick_queue();
        }
    }

    /// 「履歴と照合」（ユーザー操作）。自動照合と同じ読み取りを今すぐ1回行う。再送しない。
    pub async fn reconcile_send(self: &Arc<Self>, args: ReconcileSendArgs) -> Result<SendAttempt, IpcError> {
        match self.reconcile_once(&args.attempt).await {
            ReconcileStep::Resolved(a) | ReconcileStep::StillUnknown(a) => Ok(a),
            ReconcileStep::Busy => Err(err(IpcErrorCode::InvalidArgs, "ちょうど自動で照合しています。少し待ってからもう一度お試しください")),
            ReconcileStep::Gone => Err(err(IpcErrorCode::NotFound, "照合が必要な送信がありません（すでに確定しています）")),
        }
    }

    // ───────────── キュー操作（IPC） ─────────────

    fn editable(e: q::EditError) -> IpcError {
        match e {
            q::EditError::NotFound => err(IpcErrorCode::NotFound, "依頼が見つかりません"),
            q::EditError::NotEditable => blocked(BlockedReason::QueueEntryNotEditable, "送信待ち以外の依頼は編集・取消できません"),
        }
    }

    /// 完了後に送る依頼を登録する。設定（モデル・権限・作業フォルダ）は登録時に固定せず、送信時点のものを使う。
    pub fn enqueue(self: &Arc<Self>, args: EnqueueArgs) -> Result<QueueEntry, IpcError> {
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.precheck_space()?;
        // 登録の時点で使えない添付（コピー中・失敗・欠損）は受け付けない（送信時にも確認する）。
        self.verify_attachments(&args.chat, &args.attachments)?;
        let id = self.local_id("q");
        let now = now_ms();
        let used = args.attachments.clone();
        let chat = args.chat.clone();
        let entry = self.mutate(|d| {
            let f = d.queues.entry(args.chat.clone()).or_insert_with(|| new_file(&args.chat));
            let e = q::enqueue(&mut f.queue, id, args.text, args.attachments, now);
            (e, vec![queue_event(f)])
        });
        self.attachments_used(&chat, &used, &entry.id);
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        self.kick_queue();
        Ok(entry)
    }

    pub fn edit_queue_entry(self: &Arc<Self>, args: EditQueueEntryArgs) -> Result<QueueEntry, IpcError> {
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        self.precheck_space()?;
        self.verify_attachments(&args.chat, &args.attachments)?;
        let used = args.attachments.clone();
        let r = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(&args.chat) else { return (Err(q::EditError::NotFound), vec![]) };
            match q::edit(&mut f.queue, &args.entry, args.text, args.attachments) {
                Ok(()) => (Ok(f.queue.entries.iter().find(|e| e.id == args.entry).cloned()), vec![queue_event(f)]),
                Err(e) => (Err(e), vec![]),
            }
        });
        let entry = r.map_err(Self::editable)?.ok_or_else(|| err(IpcErrorCode::NotFound, "依頼が見つかりません"))?;
        self.attachments_used(&args.chat, &used, &entry.id);
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        Ok(entry)
    }

    pub fn cancel_queue_entry(self: &Arc<Self>, args: QueueEntryArgs) -> Result<(), IpcError> {
        self.precheck_space()?;
        // 取り消す項目が「受理なし」なら、保管している送信内容も捨てる（再送で手動送信として送れないように）。
        let dropped_attempt = self.read(|d| {
            d.queues.get(&args.chat).and_then(|f| f.queue.entries.iter().find(|e| e.id == args.entry)).and_then(|e| match &e.state {
                QueueEntryState::NotAccepted { attempt, .. } => Some(attempt.clone()),
                _ => None,
            })
        });
        self.mutate(|d| {
            let Some(f) = d.queues.get_mut(&args.chat) else { return (Err(q::EditError::NotFound), vec![]) };
            match q::cancel(&mut f.queue, &args.entry) {
                Ok(()) => (Ok(()), vec![queue_event(f)]),
                Err(e) => (Err(e), vec![]),
            }
        })
        .map_err(Self::editable)?;
        if let Some(a) = dropped_attempt {
            self.rejected.lock().unwrap().remove(&a);
        }
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        self.kick_queue();
        Ok(())
    }

    /// 「キューを再開」（ユーザー操作）。親がliveでなければ、ここで再開（resume）する。「確認済み」とは別の操作。
    pub async fn resume_queue(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<ChatQueue, IpcError> {
        let send_lock = self.queue_rt.send_lock(&args.chat);
        let _send_guard = send_lock.lock().await;
        // 停止を確認できないチャットを、再開（resume）で触らない。
        self.check_not_delete_pending(&args.chat)?;
        let state = self.read(|d| d.queues.get(&args.chat).map(|f| (f.queue.run.clone(), q::has_pending(&f.queue))));
        let root_live = self.read(|d| d.root_view(&args.chat).is_some_and(|v| v.freshness == Freshness::Live));
        let was_active = match state {
            None => return Err(err(IpcErrorCode::NotFound, "送信待ちがありません")),
            // 有効でも親がliveでなければ（wake・切断のあと）、ユーザー操作として再開処理を行って受け付ける。
            Some((QueueRun::Active, _)) if root_live => return Err(err(IpcErrorCode::InvalidArgs, "キューはすでに有効です")),
            Some((QueueRun::Active, _)) => true,
            _ => false,
        };
        if let Some(attempt) = self.unknown_attempt(&args.chat) {
            return Err(blocked(BlockedReason::AcceptanceUnknown { attempt }, "受理を確認できるまで、キューは再開できません"));
        }
        self.precheck_space()?;
        if self.read(|d| d.root_view(&args.chat).is_none_or(|v| v.freshness != Freshness::Live)) {
            self.ensure_live(&args.chat, confirmed).await?;
            self.start_scan(agent_key_of(&args.chat));
        }
        if was_active {
            let queue = self.read(|d| d.queues.get(&args.chat).map(|f| f.queue.clone()));
            self.kick_queue();
            return queue.ok_or_else(|| err(IpcErrorCode::NotFound, "送信待ちがありません"));
        }
        let now = now_ms();
        let r = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(&args.chat) else { return (Err(err(IpcErrorCode::NotFound, "送信待ちがありません")), vec![]) };
            match q::resume(&mut f.queue, now) {
                Ok(()) => (Ok(f.queue.clone()), vec![queue_event(f)]),
                Err(q::ResumeError::AlreadyActive) => (Err(err(IpcErrorCode::InvalidArgs, "キューはすでに有効です")), vec![]),
                Err(q::ResumeError::AcceptanceUnknown) => {
                    let attempt = f.queue.entries.iter().find_map(|e| match &e.state {
                        QueueEntryState::AcceptanceUnknown { attempt } => Some(attempt.clone()),
                        _ => None,
                    });
                    (
                        Err(match attempt {
                            Some(attempt) => blocked(BlockedReason::AcceptanceUnknown { attempt }, "受理を確認できるまで、キューは再開できません"),
                            None => err(IpcErrorCode::InvalidArgs, "受理不明の依頼があるため再開できません"),
                        }),
                        vec![],
                    )
                }
            }
        });
        let queue = r?;
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        self.kick_queue();
        Ok(queue)
    }

    // ───────────── 設定変更（IPC） ─────────────

    /// 権限の変更。次の送信から適用し、送信待ちの依頼にも及ぶ件数を返す（追加指示・実行中のturnには使わない）。
    pub fn set_chat_permission(self: &Arc<Self>, args: SetChatPermissionArgs) -> Result<SettingsImpact, IpcError> {
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.precheck_space()?;
        self.update_local(&args.chat, true, Duration::ZERO, |f| f.permission = Some(args.permission));
        Ok(self.settings_impact(&args.chat))
    }

    /// 次のturnから使う作業フォルダの変更（M44）。作業中・停止未確認なら止め、送信待ちがあれば確認を求める。
    pub fn set_chat_cwd(self: &Arc<Self>, args: SetChatCwdArgs) -> Result<SettingsImpact, IpcError> {
        let cwd = args.cwd.trim().to_string();
        let info = self.read(|d| {
            d.chat(&args.chat).map(|c| {
                let busy = d.agents.iter().any(|v| v.agent.chat == args.chat && d.running_turn.contains_key(&v.agent.key)) || d.open_stop(&args.chat).is_some();
                (c.kind, busy)
            })
        });
        let Some((kind, busy)) = info else { return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません")) };
        if kind == ChatKind::General {
            return Err(err(IpcErrorCode::InvalidArgs, "一般チャットの作業フォルダは変更できません"));
        }
        if cwd.is_empty() || !std::path::Path::new(&cwd).is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, format!("作業フォルダが見つかりません: {cwd}")));
        }
        let busy = busy || self.unknown_attempt(&args.chat).is_some();
        let queue = self.read(|d| d.queues.get(&args.chat).map(|f| f.queue.clone())).unwrap_or_else(|| q::new_queue(args.chat.clone()));
        match q::check_cwd_change(&queue, busy, args.queue_retarget_confirmed) {
            q::CwdChangeCheck::Busy => {
                return Err(blocked(BlockedReason::ChatBusy, "作業中または停止未確認のため、作業フォルダは変更できません。完了・停止を確認してから変更してください"))
            }
            q::CwdChangeCheck::NeedsQueueConfirmation { waiting } => {
                return Err(blocked(BlockedReason::QueueRetargetUnconfirmed { waiting }, format!("送信待ちの依頼 {waiting} 件も、新しいフォルダが対象になります")))
            }
            q::CwdChangeCheck::Allowed => {}
        }
        self.precheck_space()?;
        self.update_local(&args.chat, true, Duration::ZERO, |f| f.next_cwd = Some(cwd));
        Ok(self.settings_impact(&args.chat))
    }

    pub(super) fn settings_impact(&self, chat: &ChatKey) -> SettingsImpact {
        self.read(|d| SettingsImpact {
            local: d.local_view(chat).expect("update_local created the record"),
            affected_entries: d.queues.get(chat).map(|f| q::affected_by_settings_change(&f.queue)).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_reread_keeps_the_interval_even_when_reads_fail() {
        let t = |ms: i64| UnixMillis(ms);
        let r = 10_000;
        // 既存の記録あり・stale・確認から10秒以上: 失敗歴なしなら再読。
        assert!(terminal_reread_due(true, true, t(0), None, t(10_000), r));
        // 直前に読めなかった（3秒前）: 10秒未満は再読しない。
        assert!(!terminal_reread_due(true, true, t(0), Some(t(27_000)), t(30_000), r));
        // 失敗から10秒以上: 再読する。
        assert!(terminal_reread_due(true, true, t(0), Some(t(20_000)), t(30_000), r));
        // staleでなければ（成功後の通常条件）再読しない。
        assert!(!terminal_reread_due(true, false, t(0), None, t(60_000), r));
        // 記録なし: 初回は読む。読めなかった直後は間隔を空ける。
        assert!(terminal_reread_due(false, true, t(0), None, t(0), r));
        assert!(!terminal_reread_due(false, true, t(0), Some(t(5_000)), t(6_000), r));
    }

    fn root() -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId("r".into()) }
    }

    fn turn_rec(id: &str, end: Option<TurnEnd>) -> TurnRecord {
        TurnRecord {
            key: TurnKey { agent: root(), turn_id: ExternalId(id.into()) },
            end,
            started_at: Known::Missing,
            completed_at: Known::Missing,
            entries: vec![],
            complete: end.is_some(),
        }
    }

    #[test]
    fn recheck_confirms_only_an_explicit_terminal_of_the_last_turn() {
        // 最後のturnに終端がある: 履歴で確認。
        let ended = [turn_rec("t1", None), turn_rec("t2", Some(TurnEnd::Failed))];
        let t2 = ExternalId("t2".into());
        assert!(matches!(recheck_outcome_of(Some(&t2), &history_tail_of(&ended)), RecheckOutcome::TerminalFound));
        assert!(matches!(recheck_outcome_of(None, &history_tail_of(&ended)), RecheckOutcome::TerminalFound));
        // turnなし（記録上も最新turnなし）: 実行されていないと確認できる。
        assert!(matches!(recheck_outcome_of(None, &history_tail_of(&[])), RecheckOutcome::TerminalFound));
        // 最後のturnの終端がない（前のturnが終わっていても、最新が進行中かもしれない）: 状態不明のまま。
        let open = [turn_rec("t1", Some(TurnEnd::Completed)), turn_rec("t2", None)];
        assert!(matches!(recheck_outcome_of(None, &history_tail_of(&open)), RecheckOutcome::StillUnknown));
    }

    #[test]
    fn recheck_does_not_claim_confirmation_when_the_latest_turn_disagrees() {
        // 最新turnがあるのに履歴が空、または別のturnで終わっている: 状態は終端扱いにならないので「確認」と言わない。
        let latest = ExternalId("t9".into());
        assert!(matches!(recheck_outcome_of(Some(&latest), &history_tail_of(&[])), RecheckOutcome::StillUnknown));
        let other = [turn_rec("t2", Some(TurnEnd::Completed))];
        assert!(matches!(recheck_outcome_of(Some(&latest), &history_tail_of(&other)), RecheckOutcome::StillUnknown));
    }

    #[test]
    fn scan_is_not_ready_while_running_or_after_a_new_turn_resets_it() {
        assert!(!scan_ready(None, false), "never scanned");
        assert!(scan_ready(Some(true), false));
        assert!(!scan_ready(Some(true), true), "a running scan is partial");
        assert!(!scan_ready(Some(false), false));
        // 新しいturnの受理（reset_scan）で、前の完了結果は使えなくなり、再走査の完了で戻る。
        let rt = QueueRuntime::default();
        rt.note_scan(&root(), true);
        assert!(scan_ready(rt.scans.lock().unwrap().get(&root()).copied(), false));
        rt.reset_scan(&root());
        assert!(!scan_ready(rt.scans.lock().unwrap().get(&root()).copied(), false));
        rt.note_scan(&root(), true);
        assert!(scan_ready(rt.scans.lock().unwrap().get(&root()).copied(), false));
    }

    #[test]
    fn failed_scan_after_reset_is_restarted_but_not_while_disconnected_or_too_soon() {
        let rt = QueueRuntime::default();
        rt.note_scan(&root(), true);
        rt.reset_scan(&root());
        let started = rt.scan_started.lock().unwrap().get(&root()).copied().unwrap();
        let flag = rt.scans.lock().unwrap().get(&root()).copied();
        // 走査が note_scan 前に失敗して終わった（実行中でもなく、未完了のまま）。
        let ready = scan_ready(flag, false);
        assert!(!ready);
        let later = UnixMillis(started.0 + SCAN_RESTART_INTERVAL_MS);
        assert!(should_start_scan(ready, false, true, Some(started), later), "restart on a later tick");
        assert!(!should_start_scan(ready, false, true, Some(started), UnixMillis(started.0 + 1)), "back off right after a start");
        assert!(!should_start_scan(ready, false, false, Some(started), later), "no restart while disconnected");
        assert!(!should_start_scan(ready, true, true, Some(started), later), "no restart while a scan runs");
        assert!(!should_start_scan(true, false, true, Some(started), later), "no restart once complete");
        assert!(should_start_scan(false, false, true, None, later), "never scanned");
    }
}

#[cfg(test)]
mod history_gate_tests {
    use super::*;

    fn ck() -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId("root".into()) }
    }
    fn ak(id: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn ev(at: i64) -> Evidence {
        Evidence { source: EvidenceSource::HistoryRead, raw_label: Some("thread/read".into()), source_time: None, observed_at: UnixMillis(at) }
    }
    fn view(id: &str, root: bool, state: AgentState, fresh: Freshness) -> AgentView {
        AgentView {
            agent: Agent {
                key: ak(id),
                chat: ck(),
                parent: if root { ParentLink::Root } else { ParentLink::Explicit { parent: ak("root") } },
                forked_from: Known::NotFetched,
                display_name: Known::NotFetched,
                role: Known::NotFetched,
                assignment: Known::NotFetched,
                agent_path: Known::NotFetched,
                // 一覧・走査で作った子は最新turnを持たない。
                latest_turn: None,
            },
            status: AgentStatus { state, raw: RawState { label: "notLoaded".into() }, scope: StateScope::Agent, turn: None, wait: None, evidence: ev(5_000) },
            freshness: fresh,
            current_activity: None,
        }
    }
    fn data() -> HostData {
        let mut d = HostData::default();
        d.sources.push(SourceInfo {
            source: SourceId("s".into()),
            backend: BackendKind::Codex,
            pid: Known::NotFetched,
            started_at: UnixMillis(0),
            version: VersionCheck::Unknown { message: "t".into() },
            capabilities: crate::codex::adapter::codex_capabilities(true),
            connection: ConnectionState::Connected,
        });
        d.agents.push(view("root", true, AgentState::Done, Freshness::Live));
        // 親は終端を観測済み（t0）。
        d.agents[0].agent.latest_turn = Some(ExternalId("t0".into()));
        d.last_end.insert(ak("root"), (ExternalId("t0".into()), TurnEnd::Completed, UnixMillis(3_000)));
        // 親の作業が終わった後に登録された依頼（基準時刻より前の終端は失敗として扱わない）。
        let mut queue = q::new_queue(ck());
        q::enqueue(&mut queue, LocalId("e1".into()), "next".into(), vec![], UnixMillis(2_000));
        queue.baseline_at = Some(UnixMillis(2_500));
        d.queues.insert(ck(), QueueFile { schema_version: SCHEMA_VERSION, queue, unresolved_sends: Vec::new() });
        d.agents.push(view("luna", false, AgentState::Unknown, Freshness::HistoryOnly));
        d
    }
    fn ctx() -> GateContext {
        GateContext { quitting: false, deleting: false, manual: None, op_accept: None, folder_busy: false }
    }
    fn fact_of(turn: Option<&str>, end: Option<TurnEnd>) -> TerminalFact {
        TerminalFact { turn: turn.map(|t| ExternalId(t.into())), end, completed_at: None, checked_at: UnixMillis(6_000) }
    }
    fn decide(d: &HostData, terms: &HashMap<AgentKey, TerminalFact>, confirmed: bool) -> q::GateDecision {
        let f = &d.queues.get(&ck()).unwrap().queue;
        q::evaluate(f, &gate_input(d, &ck(), terms, true, None, ctx(), confirmed))
    }

    #[test]
    fn not_loaded_child_with_a_history_terminal_lets_the_queue_send() {
        let d = data();
        let mut terms = HashMap::new();
        // 履歴の最新turnに終端あり。子の最新turn（走査では未取得）が無くても確認済みとして扱う。
        terms.insert(ak("luna"), fact_of(Some("c1"), Some(TurnEnd::Completed)));
        assert_eq!(decide(&d, &terms, false), q::GateDecision::Send { entry: LocalId("e1".into()) });
    }

    #[test]
    fn not_loaded_child_without_a_history_terminal_stays_on_hold() {
        let d = data();
        let mut terms = HashMap::new();
        // 履歴に終端がない（途中で止まっている）・読めなかった・未確認は、いずれも保留。
        terms.insert(ak("luna"), fact_of(Some("c1"), None));
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(QueueHold::StateUnknown { .. })));
        terms.insert(ak("luna"), fact_of(None, None));
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(QueueHold::StateUnknown { .. })));
        terms.clear();
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(QueueHold::StateUnknown { .. })));
    }

    #[test]
    fn history_terminal_of_an_older_turn_is_not_used_once_the_child_has_a_newer_turn() {
        let mut d = data();
        d.agents[1].agent.latest_turn = Some(ExternalId("c2".into()));
        let mut terms = HashMap::new();
        terms.insert(ak("luna"), fact_of(Some("c1"), Some(TurnEnd::Completed)));
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(_)));
        terms.insert(ak("luna"), fact_of(Some("c2"), Some(TurnEnd::Completed)));
        assert_eq!(decide(&d, &terms, false), q::GateDecision::Send { entry: LocalId("e1".into()) });
    }

    #[test]
    fn failed_history_terminal_after_baseline_stops_the_queue() {
        let d = data();
        let mut terms = HashMap::new();
        let mut f = fact_of(Some("c1"), Some(TurnEnd::Failed));
        f.completed_at = Some(UnixMillis(4_000));
        terms.insert(ak("luna"), f);
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Stop(QueueStopCause::DescendantFailed { .. })));
    }

    #[test]
    fn send_now_confirmation_covers_one_evaluation_and_the_next_entry_returns_to_normal() {
        let mut d = data();
        let terms = HashMap::new();
        // 終端を確認できない状態不明の子。通常は保留、ユーザー確認つきの評価だけ送れる。
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(QueueHold::StateUnknown { .. })));
        assert_eq!(decide(&d, &terms, true), q::GateDecision::Send { entry: LocalId("e1".into()) });
        // 1件目を送った後（送信済み・次の依頼が先頭）。指定は持ち越されず、通常の判定は保留のまま。
        {
            let f = d.queues.get_mut(&ck()).unwrap();
            f.queue.entries[0].state = QueueEntryState::Sent { turn: TurnKey { agent: ak("root"), turn_id: ExternalId("t1".into()) } };
            q::enqueue(&mut f.queue, LocalId("e2".into()), "after".into(), vec![], UnixMillis(2_100));
            f.queue.awaiting = None;
        }
        assert!(matches!(decide(&d, &terms, false), q::GateDecision::Hold(QueueHold::StateUnknown { .. })));
    }
}

#[cfg(test)]
mod manual_tests {
    use super::*;

    #[test]
    fn manual_send_holds_the_queue_until_the_turn_ends_or_the_window_passes() {
        let t1 = ExternalId("t1".into());
        let acc = (t1.clone(), UnixMillis(1_000));
        assert!(!manual_turn_pending(None, None, UnixMillis(1_100)));
        assert!(manual_turn_pending(Some(&acc), None, UnixMillis(1_100)));
        assert!(manual_turn_pending(Some(&acc), Some(&ExternalId("t0".into())), UnixMillis(1_100)), "an older turn's end is not this turn's");
        assert!(!manual_turn_pending(Some(&acc), Some(&t1), UnixMillis(1_100)), "ended");
        assert!(!manual_turn_pending(Some(&acc), None, UnixMillis(1_000 + MANUAL_PENDING_MS)), "never observed: not held forever");
        // フォルダ操作と重なる作業フォルダだけ保留する。
        let active = vec![r"C:\repo".to_string()];
        assert!(folder_op_overlaps(&active, Some("c:/repo/app")));
        assert!(!folder_op_overlaps(&active, Some(r"C:\repo2")));
        assert!(!folder_op_overlaps(&active, None));
        // 操作（レビュー・圧縮）の受付: 受付以後のturnの終端を観測するまで保留、上限で解除。
        let since = Some(UnixMillis(1_000));
        assert!(!op_turn_pending(None, None, UnixMillis(1_100)));
        assert!(op_turn_pending(since, None, UnixMillis(1_100)));
        assert!(op_turn_pending(since, Some(UnixMillis(900)), UnixMillis(1_100)), "an end before the accept is not this op's");
        assert!(!op_turn_pending(since, Some(UnixMillis(1_050)), UnixMillis(1_100)), "ended after the accept");
        assert!(!op_turn_pending(since, None, UnixMillis(1_000 + MANUAL_PENDING_MS)), "never observed: not held forever");
    }
}
