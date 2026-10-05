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

use super::state::{agent_key_of, HostData};
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

/// キュー実行の作業状態（保存しない）。
#[derive(Default)]
pub struct QueueRuntime {
    notify: Notify,
    /// 評価・送信中のチャット（同じチャットで同時に送らない）。
    busy: Mutex<HashSet<ChatKey>>,
    terminals: Mutex<HashMap<AgentKey, TerminalFact>>,
    /// 照合を回している受理不明の送信。
    reconciling: Mutex<HashSet<LocalId>>,
    /// 子孫の走査が完了したか（ルート別。未走査はキーなし＝未完了）。
    scans: Mutex<HashMap<AgentKey, bool>>,
    awaiting_checked: Mutex<HashMap<ChatKey, UnixMillis>>,
}

impl QueueRuntime {
    pub fn note_scan(&self, root: &AgentKey, complete: bool) {
        self.scans.lock().unwrap().insert(root.clone(), complete);
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

/// 状態（`HostData`）と履歴確認の結果から、純粋な判断の入力を組む。
fn gate_input(
    d: &HostData,
    chat: &ChatKey,
    terminals: &HashMap<AgentKey, TerminalFact>,
    scan_complete: bool,
    unresolved: Option<LocalId>,
) -> q::GateInput {
    let root_key = agent_key_of(chat);
    let fact = |v: &AgentView| -> q::AgentFact {
        let key = &v.agent.key;
        let latest = v.agent.latest_turn.as_ref();
        let live_end = d.last_end.get(key).filter(|(t, _, _)| Some(t) == latest).map(|(_, end, at)| (*end, *at));
        let term = terminals.get(key).filter(|t| t.turn.as_ref() == latest && t.end.is_some());
        let history_end = term.and_then(|t| t.end.map(|e| (e, t.completed_at.unwrap_or(t.checked_at))));
        q::AgentFact {
            agent: key.clone(),
            state: v.status.state,
            freshness: v.freshness,
            latest_turn: v.agent.latest_turn.clone(),
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
        delete_pending: d.locals.get(chat).is_some_and(|l| l.delete_pending.is_some()),
    }
}

/// 送信時点で有効なチャットの設定。
struct Plan {
    text: String,
    model: Option<ModelChoice>,
    permission: Option<PermissionPreset>,
    cwd: Option<String>,
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

    fn is_connected(&self) -> bool {
        self.read(|d| d.sources.first().is_some_and(|s| matches!(s.connection, ConnectionState::Connected)))
    }

    /// 1チャット分: 待っているturnの終端確認 → 判断 → 送信・停止・保留表示。
    async fn drive_chat(self: &Arc<Self>, chat: &ChatKey) {
        self.settle_awaiting(chat).await;
        let (active, pending) = self.read(|d| d.queues.get(chat).map(|f| (f.queue.run == QueueRun::Active, q::has_pending(&f.queue))).unwrap_or((false, false)));
        if !(active && pending) {
            // 評価しない間は保留理由を持ち越さない。
            self.set_hold(chat, None);
            return;
        }
        let root = agent_key_of(chat);
        if !self.queue_rt.scans.lock().unwrap().contains_key(&root) {
            // 子孫の走査が一度もない。読み取りだけの走査を始める（走査完了まで状態不明として保留）。
            self.start_scan(root.clone());
        }
        self.refresh_terminal_facts(chat).await;
        let terminals = self.queue_rt.terminals.lock().unwrap().clone();
        let scan_complete = self.queue_rt.scans.lock().unwrap().get(&root).copied().unwrap_or(false);
        let unresolved = self.unknown_attempt(chat);
        let decision = self.read(|d| d.queues.get(chat).map(|f| q::evaluate(&f.queue, &gate_input(d, chat, &terminals, scan_complete, unresolved))));
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
                let working = self.read(|d| d.view(&turn.agent).is_none_or(|v| is_working(v.status.state)));
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
                match self.backend.read(turn.agent.clone(), ReadOptions { include_turns: true }).await {
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
            self.read(|d| {
                let baseline = d.queues.get(chat).and_then(|f| f.queue.baseline_at);
                let root = agent_key_of(chat);
                d.agents
                    .iter()
                    .filter(|v| &v.agent.chat == chat && v.agent.key != root && v.freshness != Freshness::Live && !is_working(v.status.state))
                    .filter(|v| match terminals.get(&v.agent.key) {
                        None => true,
                        Some(t) => {
                            let stale = t.turn.as_ref() != v.agent.latest_turn.as_ref() || baseline.is_some_and(|b| t.checked_at < b) || t.end.is_none();
                            stale && now.0 - t.checked_at.0 >= TERMINAL_RECHECK.as_millis() as i64
                        }
                    })
                    .map(|v| v.agent.key.clone())
                    .take(TERMINAL_READS_PER_TICK)
                    .collect()
            })
        };
        for agent in cands {
            let fact = match self.backend.read(agent.clone(), ReadOptions { include_turns: true }).await {
                Ok(h) => {
                    let last = h.turns.last();
                    TerminalFact {
                        turn: last.map(|t| t.key.turn_id.clone()),
                        end: last.and_then(|t| t.end),
                        completed_at: last.and_then(|t| t.completed_at.value().copied()),
                        checked_at: now_ms(),
                    }
                }
                // 読めなかった: 確認できていないまま。次の間隔で再試行する（終端があったことにしない）。
                Err(_) => TerminalFact { turn: None, end: None, completed_at: None, checked_at: now_ms() },
            };
            self.queue_rt.terminals.lock().unwrap().insert(agent, fact);
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

    /// 項目を Sending にして、送信時点の設定を記録する。送れない状態の項目なら None。
    fn begin_entry_send(&self, chat: &ChatKey, entry: &LocalId, attempt: SendAttempt) -> Option<Plan> {
        let now = attempt.at;
        self.mutate(|d| {
            let (model, permission, cwd_override) = {
                let l = d.locals.get(chat);
                (d.model_settings.get(chat).and_then(|s| s.selected.clone()), l.and_then(|l| l.permission), l.and_then(|l| l.next_cwd.clone()))
            };
            let cwd_known = match &cwd_override {
                Some(c) => Known::direct(c.clone()),
                None => d.chat(chat).map(|c| c.cwd.clone()).unwrap_or(Known::NotFetched),
            };
            let applied = AppliedSettings { model: model.clone(), permission: permission.unwrap_or(PermissionPreset::WorkspaceWriteOnRequest), cwd: cwd_known, decided_at: now };
            let Some(f) = d.queues.get_mut(chat) else { return (None, vec![]) };
            let Some(text) = f.queue.entries.iter().find(|e| &e.id == entry).map(|e| e.text.clone()) else { return (None, vec![]) };
            if !q::begin_send(&mut f.queue, entry, attempt, applied) {
                return (None, vec![]);
            }
            (Some(Plan { text, model, permission, cwd: cwd_override }), vec![queue_event(f)])
        })
    }

    /// 自動送信（評価が `Send` を返した先頭の項目）。
    async fn send_queue_entry(self: &Arc<Self>, chat: &ChatKey, entry: LocalId) {
        let attempt = SendAttempt { attempt_id: self.local_id("att"), client_message_id: self.local_id("cm").0, at: now_ms(), state: SendState::Sending };
        let Some(plan) = self.begin_entry_send(chat, &entry, attempt.clone()) else { return };
        let request = SendRequest {
            chat: chat.clone(),
            mode: SendMode::NewTurn,
            text: plan.text,
            attachments: Vec::new(),
            model: plan.model,
            permission: plan.permission,
            cwd: plan.cwd,
            client_message_id: attempt.client_message_id.clone(),
        };
        self.run_entry_send(chat, entry, attempt, request).await;
    }

    /// 受理なしが確定した項目の、ユーザー確認つきの再送（`retry_send` から）。
    pub(super) async fn retry_entry_send(self: &Arc<Self>, chat: &ChatKey, entry: LocalId, mut request: SendRequest) -> Result<SendAttempt, IpcError> {
        let attempt = SendAttempt { attempt_id: self.local_id("att"), client_message_id: request.client_message_id.clone(), at: now_ms(), state: SendState::Sending };
        let Some(plan) = self.begin_entry_send(chat, &entry, attempt.clone()) else {
            return Err(err(IpcErrorCode::NotFound, "再送できる依頼がありません"));
        };
        // 送信時点で有効な設定を使う。
        request.model = plan.model;
        request.permission = plan.permission;
        request.cwd = plan.cwd;
        Ok(self.run_entry_send(chat, entry, attempt, request).await)
    }

    /// Sending を保存してから送り、結果を項目へ反映する。保存できなければ送らない。
    async fn run_entry_send(self: &Arc<Self>, chat: &ChatKey, entry: LocalId, attempt: SendAttempt, request: SendRequest) -> SendAttempt {
        let scope = SaveScope::Queue { chat: chat.clone() };
        if let Some(st) = self.save_now(scope).await {
            if !matches!(st.state, SaveState::Saved { .. }) {
                self.mutate(|d| {
                    let Some(f) = d.queues.get_mut(chat) else { return ((), vec![]) };
                    q::abort_send(&mut f.queue, &entry);
                    ((), vec![queue_event(f)])
                });
                self.warn("送信前に送信待ちを保存できなかったため、依頼を送っていません。保存の状態を確認してください。");
                return SendAttempt { state: SendState::Rejected { message: "送信前の保存に失敗したため送っていません".into() }, ..attempt };
            }
        }
        let sent_cwd = request.cwd.clone();
        let outcome = self.backend.send(request).await;
        let now = now_ms();
        let mut done = attempt.clone();
        let result = match outcome {
            SendOutcome::Accepted { turn, .. } => {
                done.state = SendState::Accepted { turn: turn.clone() };
                q::SendResult::Accepted { attempt: done.clone(), turn, turn_started_at: now }
            }
            SendOutcome::Rejected { error, request } => {
                let message = error.to_string();
                done.state = SendState::Rejected { message: message.clone() };
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
            d.queues.entry(chat.clone()).or_insert_with(|| new_file(chat)).unresolved_sends.push(record);
            ((), vec![])
        });
        self.schedule_save(SaveScope::Queue { chat: chat.clone() }, Duration::ZERO);
        self.start_reconcile(attempt.clone());
    }

    fn drop_unresolved_record(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId) {
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
                        None => warnings.push("受理不明の送信の記録（照合に必要なID）が残っていません。履歴で送信の有無を確認してください。".to_string()),
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
                self.apply_reconciled(&chat, attempt_id, |a| q::SendResult::ReconciledAccepted { attempt: a, turn: turn.clone() });
                let a = make(SendState::Accepted { turn });
                self.emit_send(&chat, &a);
                ReconcileStep::Resolved(a)
            }
            ReconcileOutcome::NotFound(na) => {
                self.unresolved.lock().unwrap().resolve(attempt_id);
                self.rejected.lock().unwrap().insert(attempt_id.clone(), na);
                self.drop_unresolved_record(&chat, attempt_id);
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
        if !args.attachments.is_empty() {
            return Err(err(IpcErrorCode::Unsupported, "添付の送信は段階②で対応します"));
        }
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.precheck_space()?;
        let id = self.local_id("q");
        let now = now_ms();
        let entry = self.mutate(|d| {
            let f = d.queues.entry(args.chat.clone()).or_insert_with(|| new_file(&args.chat));
            let e = q::enqueue(&mut f.queue, id, args.text, args.attachments, now);
            (e, vec![queue_event(f)])
        });
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        self.kick_queue();
        Ok(entry)
    }

    pub fn edit_queue_entry(self: &Arc<Self>, args: EditQueueEntryArgs) -> Result<QueueEntry, IpcError> {
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        if !args.attachments.is_empty() {
            return Err(err(IpcErrorCode::Unsupported, "添付の送信は段階②で対応します"));
        }
        self.precheck_space()?;
        let r = self.mutate(|d| {
            let Some(f) = d.queues.get_mut(&args.chat) else { return (Err(q::EditError::NotFound), vec![]) };
            match q::edit(&mut f.queue, &args.entry, args.text, args.attachments) {
                Ok(()) => (Ok(f.queue.entries.iter().find(|e| e.id == args.entry).cloned()), vec![queue_event(f)]),
                Err(e) => (Err(e), vec![]),
            }
        });
        let entry = r.map_err(Self::editable)?.ok_or_else(|| err(IpcErrorCode::NotFound, "依頼が見つかりません"))?;
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        Ok(entry)
    }

    pub fn cancel_queue_entry(self: &Arc<Self>, args: QueueEntryArgs) -> Result<(), IpcError> {
        self.precheck_space()?;
        self.mutate(|d| {
            let Some(f) = d.queues.get_mut(&args.chat) else { return (Err(q::EditError::NotFound), vec![]) };
            match q::cancel(&mut f.queue, &args.entry) {
                Ok(()) => (Ok(()), vec![queue_event(f)]),
                Err(e) => (Err(e), vec![]),
            }
        })
        .map_err(Self::editable)?;
        self.schedule_save(SaveScope::Queue { chat: args.chat }, Duration::ZERO);
        self.kick_queue();
        Ok(())
    }

    /// 「キューを再開」（ユーザー操作）。親がliveでなければ、ここで再開（resume）する。「確認済み」とは別の操作。
    pub async fn resume_queue(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<ChatQueue, IpcError> {
        let state = self.read(|d| d.queues.get(&args.chat).map(|f| (f.queue.run.clone(), q::has_pending(&f.queue))));
        match state {
            None => return Err(err(IpcErrorCode::NotFound, "送信待ちがありません")),
            Some((QueueRun::Active, _)) => return Err(err(IpcErrorCode::InvalidArgs, "キューはすでに有効です")),
            _ => {}
        }
        if let Some(attempt) = self.unknown_attempt(&args.chat) {
            return Err(blocked(BlockedReason::AcceptanceUnknown { attempt }, "受理を確認できるまで、キューは再開できません"));
        }
        self.precheck_space()?;
        if self.read(|d| d.root_view(&args.chat).is_none_or(|v| v.freshness != Freshness::Live)) {
            self.ensure_live(&args.chat, confirmed).await?;
            self.start_scan(agent_key_of(&args.chat));
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

    fn settings_impact(&self, chat: &ChatKey) -> SettingsImpact {
        self.read(|d| SettingsImpact {
            local: d.local_view(chat).expect("update_local created the record"),
            affected_entries: d.queues.get(chat).map(|f| q::affected_by_settings_change(&f.queue)).unwrap_or_default(),
        })
    }
}
