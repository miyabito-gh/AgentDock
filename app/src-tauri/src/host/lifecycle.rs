//! 完全終了・強制終了・sleep/wake・窓の位置の保存（P6、DESIGN_P2 §5、要件§3.4・§3.11）。
//!
//! 規則:
//! - 閉じる操作（トレイ格納）はここを通らない。完全終了だけが `request_quit` から始まる。
//! - 終了の判断は純粋ロジック `rules::manage`（`busy_for_quit`・`next_quit_phase`）。ここは時刻・保存・停止記録をつなぐ。
//! - 停止は証拠（turn終端・OS実体の消滅）で確認する。時間経過だけで確認にしない。保存に失敗したまま正常終了にしない。
//! - 強制終了は `UserConfirmed`（コマンド層）を要する。App Server単位（Job Object）なので、確認画面に影響する全チャットを出す。
//! - wake は鮮度を要照合にして状態を読み直すだけ。通知・再送・resumeはしない。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::state::{agent_key_of, HostData};
use super::{err, now_ms, stop, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::rules::manage::{agents_to_mark_on_resume, busy_for_quit, looks_like_resume, next_quit_phase, ChatWork};
use crate::store::records::{WindowsFile, SCHEMA_VERSION};
use crate::win::power::{PowerEvent, PowerWatcher};

/// アプリを終了する処理（`lib.rs` が `app.exit(0)` を渡す）。
pub type ExitFn = Arc<dyn Fn() + Send + Sync>;

/// 終了手順の進行を確認する間隔。
const QUIT_POLL: Duration = Duration::from_millis(500);
/// sleep検出（壁時計の飛び）の刻み。
const POWER_TICK_MS: i64 = 5_000;
/// 復帰の通知は複数の経路（電源通知2種・壁時計）から重ねて来る。この間隔内の重複は1回として扱う。
const RESUME_DEDUP_MS: i64 = 20_000;
/// 強制終了の後、Job内のプロセスが0になるのを確認する時間（100ms×50）。
const KILL_CONFIRM_POLLS: u32 = 50;
const WINDOW_SAVE_DEBOUNCE_MS: u64 = 500;

struct QuitState {
    phase: QuitPhase,
    /// 手順の世代。取消し・やり直しで進め、古い進行taskを止める。
    gen: u64,
}

pub struct Lifecycle {
    quit: Mutex<QuitState>,
    exit: OnceLock<ExitFn>,
    /// 窓ごとの位置・サイズ（`windows.json`）。
    windows: Mutex<(Option<WindowBounds>, Option<WindowBounds>)>,
    last_resume: Mutex<Option<i64>>,
    power: Mutex<Option<PowerWatcher>>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Lifecycle {
            quit: Mutex::new(QuitState { phase: QuitPhase::Idle, gen: 0 }),
            exit: OnceLock::new(),
            windows: Mutex::new((None, None)),
            last_resume: Mutex::new(None),
            power: Mutex::new(None),
        }
    }
}

/// チャット単位の作業状況（終了確認の材料）。
fn work_of(d: &HostData, chat: &ChatKey) -> ChatWork {
    let has_unfinished = d
        .agents
        .iter()
        .any(|v| &v.agent.chat == chat && matches!(v.status.state, AgentState::Initializing | AgentState::Running | AgentState::Waiting | AgentState::Unknown));
    ChatWork {
        chat: chat.clone(),
        has_unfinished,
        stop_unconfirmed: d.open_stop(chat).map(|r| r.id.clone()),
        ownership_unknown: d.stops.iter().any(|r| &r.chat == chat && r.targets.iter().any(|t| t.ownership == Ownership::Unknown)),
        queue_pending: d.queues.get(chat).is_some_and(|q| {
            // 自動送信して終端を確認していないturn（awaiting）も未完了として数える。
            q.queue.awaiting.is_some()
                || q.queue.entries.iter().any(|e| matches!(e.state, QueueEntryState::Waiting | QueueEntryState::Sending { .. } | QueueEntryState::AcceptanceUnknown { .. }))
        }),
        unresolved_send: false,
    }
}

/// チャットの実行中turn（中断の対象）。
fn running_turns(d: &HostData, chat: &ChatKey) -> Vec<TurnKey> {
    d.agents
        .iter()
        .filter(|v| &v.agent.chat == chat)
        .filter_map(|v| d.running_turn.get(&v.agent.key).map(|t| TurnKey { agent: v.agent.key.clone(), turn_id: t.clone() }))
        .collect()
}

/// 強制終了の結果を停止記録へ反映する。停止記録のないチャットには記録を作る。
/// OS実体の消滅を確認できた（`gone_at`）ときだけ、各対象に「OS実体確認」の証拠を付ける。それ以外は停止未確認のまま。
fn force_kill_records(
    d: &mut HostData,
    new_id: &mut dyn FnMut() -> LocalId,
    affected: &[ChatKey],
    pid: Option<u32>,
    gone_at: Option<UnixMillis>,
    now: UnixMillis,
) -> (Vec<StopRecord>, Vec<HostEvent>) {
    let mut ids: Vec<LocalId> = Vec::new();
    let mut events = Vec::new();
    for chat in affected {
        if d.open_stop(chat).is_none() {
            let mut targets: Vec<StopTarget> = running_turns(d, chat).into_iter().map(|t| stop::new_target(t, None, now)).collect();
            if targets.is_empty() {
                if let Some(pid) = pid {
                    let evidence = StopEvidence::default();
                    let summary = stop::derive_summary(Ownership::Confirmed, &evidence, now);
                    targets.push(StopTarget {
                        target: StopTargetRef::OsProcess { pid, created_at: Known::NotFetched, executable: Known::NotFetched },
                        ownership: Ownership::Confirmed,
                        evidence,
                        summary,
                    });
                }
            }
            if !targets.is_empty() {
                let id = new_id();
                events.extend(d.add_stop_record(StopRecord { id: id.clone(), chat: chat.clone(), started_at: now, targets }, now));
                ids.push(id);
            }
        }
        if let Some(at) = gone_at {
            for r in d.stops.iter_mut().filter(|r| &r.chat == chat && r.blocks_deletion()) {
                for t in &mut r.targets {
                    if t.ownership == Ownership::Confirmed && t.evidence.os_gone_confirmed_at.is_none() {
                        t.evidence.os_gone_confirmed_at = Some(at);
                    }
                }
                stop::refresh(r, now);
                events.push(HostEvent::StopUpdated { record: r.clone() });
                if !ids.contains(&r.id) {
                    ids.push(r.id.clone());
                }
            }
        }
    }
    let records = d.stops.iter().filter(|r| ids.contains(&r.id)).cloned().collect();
    (records, events)
}

impl Host {
    // ── 起動時の接続 ──

    /// アプリの終了処理（`lib.rs` が設定する）。
    pub fn attach_exit(&self, f: ExitFn) {
        let _ = self.lifecycle.exit.set(f);
    }

    // ── 窓の位置・サイズ（M37） ──

    /// 保存済みの窓の位置（復元前に `windows.json` から読んだもの）。
    pub(super) fn restore_windows(&self, file: Option<WindowsFile>) {
        if let Some(f) = file {
            *self.lifecycle.windows.lock().unwrap() = (f.main, f.monitor);
        }
    }

    pub fn saved_bounds(&self, kind: WindowKind) -> Option<WindowBounds> {
        let w = self.lifecycle.windows.lock().unwrap();
        match kind {
            WindowKind::Main => w.0,
            WindowKind::Monitor => w.1,
        }
    }

    /// 窓の位置・サイズを記録して保存を依頼する（移動・リサイズ中は500msまとめ）。
    pub fn set_window_bounds(self: &Arc<Self>, kind: WindowKind, bounds: WindowBounds) {
        {
            let mut w = self.lifecycle.windows.lock().unwrap();
            let slot = match kind {
                WindowKind::Main => &mut w.0,
                WindowKind::Monitor => &mut w.1,
            };
            if *slot == Some(bounds) {
                return;
            }
            *slot = Some(bounds);
        }
        self.schedule_save(SaveScope::WindowBounds, Duration::from_millis(WINDOW_SAVE_DEBOUNCE_MS));
    }

    /// 保存用の内容（`persist::write_scope`）。
    pub(super) fn windows_file(&self) -> WindowsFile {
        let w = self.lifecycle.windows.lock().unwrap();
        WindowsFile { schema_version: SCHEMA_VERSION, main: w.0, monitor: w.1 }
    }

    fn update_settings(self: &Arc<Self>, f: impl FnOnce(&mut AppSettings)) -> Result<(), IpcError> {
        self.precheck_space()?;
        self.mutate(|d| {
            f(&mut d.settings);
            ((), vec![HostEvent::SettingsUpdated { settings: d.settings.clone() }])
        });
        self.schedule_save(SaveScope::AppSettings, Duration::ZERO);
        Ok(())
    }

    /// 最前面の設定を記録する（窓への適用はコマンド層）。窓ごとに独立、初期値オフ。
    pub fn set_always_on_top_pref(self: &Arc<Self>, kind: WindowKind, on: bool) -> Result<(), IpcError> {
        self.update_settings(|s| match kind {
            WindowKind::Main => s.main_window.always_on_top = on,
            WindowKind::Monitor => s.monitor_window.always_on_top = on,
        })
    }

    /// 監視窓の表示範囲（選択中／全チャット）を記録する。表示範囲だけで、送信先・中断・承認は変えない。
    pub fn set_monitor_window_scope(self: &Arc<Self>, scope: MonitorWindowScope) -> Result<(), IpcError> {
        self.update_settings(|s| s.monitor_scope = scope)
    }

    // ── 完全終了（§3.4） ──

    fn chat_works(&self) -> Vec<ChatWork> {
        let mut works = self.read(|d| {
            let mut keys: Vec<ChatKey> = d.chats.iter().map(|c| c.key.clone()).collect();
            for v in &d.agents {
                if !keys.contains(&v.agent.chat) {
                    keys.push(v.agent.chat.clone());
                }
            }
            keys.iter().map(|k| work_of(d, k)).collect::<Vec<_>>()
        });
        // 受理不明の送信は別のロックで持つ（データのロックを保持したまま取らない）。
        let unresolved = self.unresolved.lock().unwrap();
        for w in &mut works {
            w.unresolved_send = unresolved.blocking(&w.chat).is_some();
        }
        works
    }

    pub fn quit_phase(&self) -> QuitPhase {
        self.lifecycle.quit.lock().unwrap().phase.clone()
    }

    /// 手順を新しく始める（古い進行taskを止める）。世代を返す。
    fn start_quit_phase(&self, phase: QuitPhase) -> u64 {
        let mut q = self.lifecycle.quit.lock().unwrap();
        q.gen += 1;
        q.phase = phase.clone();
        let gen = q.gen;
        self.mutate(|_| ((), vec![HostEvent::QuitUpdated { phase }]));
        gen
    }

    /// 同じ手順（世代）の中で段階を進める。取消し・やり直しで世代が変わっていたら何もしない。
    fn advance_quit_phase(&self, gen: u64, phase: QuitPhase) -> bool {
        let mut q = self.lifecycle.quit.lock().unwrap();
        if q.gen != gen {
            return false;
        }
        if q.phase != phase {
            q.phase = phase.clone();
            self.mutate(|_| ((), vec![HostEvent::QuitUpdated { phase }]));
        }
        true
    }

    /// 完全終了の要求。作業中（未終端・停止未確認・受理不明・送信待ち）があれば確認を出す。なければ保存して終了する。
    /// すでに手順の途中なら現在の段階を返すだけ。
    pub fn request_quit(self: &Arc<Self>) -> QuitPhase {
        let current = self.quit_phase();
        if current != QuitPhase::Idle {
            return current;
        }
        let busy = busy_for_quit(&self.chat_works());
        if busy.is_empty() {
            let gen = self.start_quit_phase(QuitPhase::Flushing);
            self.spawn_quit_driver(gen);
            return QuitPhase::Flushing;
        }
        let phase = QuitPhase::Confirming { busy };
        self.start_quit_phase(phase.clone());
        phase
    }

    /// 終了確認への回答。「作業を中断して終了」「中断を再試行」は中断要求（ユーザー操作）を送る。
    pub async fn quit_decision(self: &Arc<Self>, args: QuitDecisionArgs, confirmed: &UserConfirmed) -> Result<QuitPhase, IpcError> {
        let current = self.quit_phase();
        match args.decision {
            QuitDecision::Cancel => {
                if current != QuitPhase::Idle {
                    // 送信済みの中断要求は取り消せない。作業と監視は続く。
                    self.start_quit_phase(QuitPhase::Idle);
                }
                Ok(QuitPhase::Idle)
            }
            QuitDecision::Wait => Ok(current),
            QuitDecision::StopAndQuit => {
                let QuitPhase::Confirming { busy } = current else {
                    return Err(err(IpcErrorCode::InvalidArgs, "終了の確認中ではありません"));
                };
                let ids = self.interrupt_for_quit(&busy, Vec::new(), confirmed).await;
                self.begin_stopping(ids)
            }
            QuitDecision::RetryInterrupt => {
                let QuitPhase::StopUnconfirmed { records } = current else {
                    return Err(err(IpcErrorCode::InvalidArgs, "停止未確認の対象がありません"));
                };
                let chats: Vec<ChatKey> = self.read(|d| {
                    let mut chats = Vec::new();
                    for r in d.stops.iter().filter(|r| records.contains(&r.id)) {
                        if !chats.contains(&r.chat) {
                            chats.push(r.chat.clone());
                        }
                    }
                    chats
                });
                let ids = self.interrupt_for_quit(&chats, records, confirmed).await;
                self.begin_stopping(ids)
            }
        }
    }

    fn begin_stopping(self: &Arc<Self>, records: Vec<LocalId>) -> Result<QuitPhase, IpcError> {
        // 取消しなどで手順が変わっていたら、古い要求の結果で上書きしない。
        if matches!(self.quit_phase(), QuitPhase::Idle) {
            return Ok(QuitPhase::Idle);
        }
        let phase = QuitPhase::Stopping { records, started_at: now_ms() };
        let gen = self.start_quit_phase(phase.clone());
        self.spawn_quit_driver(gen);
        Ok(phase)
    }

    /// 各チャットへ中断要求を送り、監視する停止記録のIDを返す。`keep` は引き続き確認する既存の記録。
    /// 要求を送れなかったチャットでも、実行中のturnが残っていれば「要求なし」の記録を作る（停止を確認できないまま終了へ進めない）。
    async fn interrupt_for_quit(self: &Arc<Self>, chats: &[ChatKey], keep: Vec<LocalId>, confirmed: &UserConfirmed) -> Vec<LocalId> {
        let mut ids = keep;
        for chat in chats {
            if let Some(id) = self.read(|d| d.open_stop(chat).map(|r| r.id.clone())) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            match self.interrupt_chat(InterruptChatArgs { chat: chat.clone() }, confirmed).await {
                Ok(res) => ids.push(res.record.id),
                Err(e) => crate::diag::log("quit", &format!("interrupt failed code={:?}", e.code)),
            }
            let now = now_ms();
            let missing: Vec<TurnKey> = self.read(|d| {
                let covered: Vec<TurnKey> = d
                    .stops
                    .iter()
                    .filter(|r| ids.contains(&r.id))
                    .flat_map(|r| r.targets.iter())
                    .filter_map(|t| match &t.target {
                        StopTargetRef::Turn { turn } => Some(turn.clone()),
                        _ => None,
                    })
                    .collect();
                running_turns(d, chat).into_iter().filter(|t| !covered.contains(t)).collect()
            });
            if !missing.is_empty() {
                let record = StopRecord {
                    id: self.local_id("stop"),
                    chat: chat.clone(),
                    started_at: now,
                    targets: missing.into_iter().map(|t| stop::new_target(t, None, now)).collect(),
                };
                ids.push(record.id.clone());
                self.mutate(|d| ((), d.add_stop_record(record, now)));
            }
        }
        ids
    }

    fn failed_saves(&self) -> Vec<SaveScope> {
        self.read(|d| d.save_status.values().filter(|s| matches!(s.state, SaveState::SaveFailed { .. })).map(|s| s.scope.clone()).collect())
    }

    /// 終了手順を進める（世代が変わったら終わる）。停止と保存を確認でき、保存に失敗がなければ終了する。
    fn spawn_quit_driver(self: &Arc<Self>, gen: u64) {
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                let now = now_ms();
                host.mutate(|d| ((), d.refresh_stops(now)));
                let phase = host.quit_phase();
                let (records, failed) = (host.read(|d| d.stops.clone()), host.failed_saves());
                let next = next_quit_phase(&phase, &records, &failed, now);
                if !host.advance_quit_phase(gen, next.clone()) {
                    return;
                }
                if next == QuitPhase::Flushing {
                    host.flush_pending().await;
                    let failed = host.failed_saves();
                    match next_quit_phase(&QuitPhase::Flushing, &[], &failed, now_ms()) {
                        QuitPhase::Flushing => {
                            // 保存に失敗がないことを確認できた。取消しが入っていなければ終了する。
                            if host.advance_quit_phase(gen, QuitPhase::Flushing) {
                                if let Some(exit) = host.lifecycle.exit.get() {
                                    exit();
                                }
                            }
                            return;
                        }
                        failed_phase => {
                            if !host.advance_quit_phase(gen, failed_phase) {
                                return;
                            }
                        }
                    }
                }
                tokio::time::sleep(QUIT_POLL).await;
            }
        });
    }

    // ── 強制終了（§3.4・合意 2026-10-06） ──

    fn force_target(&self, source: &SourceId) -> Result<std::sync::Arc<crate::win::job::ProcessJob>, IpcError> {
        match self.backend.current_job() {
            Some((current, job)) if &current == source => Ok(job),
            Some(_) => Err(err(IpcErrorCode::InvalidArgs, "指定の監視元は現在の接続ではありません")),
            None => Err(err(
                IpcErrorCode::Unsupported,
                "強制終了できる監視元がありません（このアプリが起動した Codex だけが対象です。切断済み、または起動時に管理対象にできませんでした）",
            )),
        }
    }

    /// 強制終了の確認内容。同じApp Server上の全チャット（作業中・停止未確認など）を挙げる。
    pub fn preview_force_kill(&self, args: ForceKillArgs) -> Result<ForceKillPreview, IpcError> {
        let job = self.force_target(&args.source)?;
        let affected = busy_for_quit(&self.chat_works());
        let active_processes = match job.active_processes() {
            Ok(n) => Known::direct(n),
            Err(_) => Known::NotFetched,
        };
        Ok(ForceKillPreview { source: args.source, affected, active_processes })
    }

    /// 強制終了（ユーザーの確認後）。Job内のプロセスが0になったことを確認できたときだけ「OS実体確認」の証拠を付ける。
    /// 確認できなければ停止未確認のまま（成功と表示しない）。管理外の実行は対象外。
    pub async fn force_kill(self: &Arc<Self>, args: ForceKillArgs, confirmed: &UserConfirmed) -> Result<Vec<StopRecord>, IpcError> {
        let job = self.force_target(&args.source)?;
        let affected = busy_for_quit(&self.chat_works());
        let pid = self.read(|d| d.sources.iter().find(|s| s.source == args.source).and_then(|s| s.pid.value().copied()));
        job.terminate(confirmed).map_err(|e| err(IpcErrorCode::Io, format!("強制終了を要求できませんでした: {e}")))?;
        crate::diag::log("force_kill", &format!("terminated job affected={}", affected.len()));
        let mut gone_at = None;
        for _ in 0..KILL_CONFIRM_POLLS {
            if matches!(job.active_processes(), Ok(0)) {
                gone_at = Some(now_ms());
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if gone_at.is_none() {
            self.warn("強制終了を要求しましたが、プロセスの消滅を確認できていません。停止未確認のまま扱います。");
        }
        let now = now_ms();
        let records = self.mutate(|d| {
            let mut counter = 0u32;
            let mut new_id = || {
                counter += 1;
                LocalId(format!("stop-{}-kill{counter}", now.0))
            };
            let (records, events) = force_kill_records(d, &mut new_id, &affected, pid, gone_at, now);
            (records, events)
        });
        Ok(records)
    }

    // ── sleep / wake（§3.11） ──

    /// 電源通知と壁時計の飛びで復帰を検出する（起動時に1回）。tokioランタイム内から呼ぶ。
    pub fn start_power_watch(self: &Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PowerEvent>();
        match PowerWatcher::start(Box::new(move |ev| {
            let _ = tx.send(ev);
        })) {
            Ok(w) => *self.lifecycle.power.lock().unwrap() = Some(w),
            Err(e) => crate::diag::log("power", &format!("register failed: {e}")),
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut prev = now_ms();
            let mut tick = tokio::time::interval(Duration::from_millis(POWER_TICK_MS as u64));
            let mut open = true;
            loop {
                let resumed = tokio::select! {
                    ev = async { if open { rx.recv().await } else { std::future::pending().await } } => match ev {
                        Some(PowerEvent::Resumed) => true,
                        Some(PowerEvent::Suspending) => false,
                        None => {
                            open = false;
                            false
                        }
                    },
                    _ = tick.tick() => {
                        let now = now_ms();
                        let jumped = looks_like_resume(prev, now, POWER_TICK_MS);
                        prev = now;
                        jumped
                    }
                };
                if resumed {
                    let Some(host) = weak.upgrade() else { return };
                    host.on_resume().await;
                    prev = now_ms();
                }
            }
        });
    }

    /// sleepから復帰した。live の鮮度を要照合にし（状態は変えない）、状態を読み直す。読み取りは live 購読の回復ではないので、結果は履歴のみ（historyOnly）に留める。
    /// live に戻るのはユーザーの送信・再開操作（resume）だけ。通知・再送・resumeはしない。読めなかったものは要照合のまま残す。
    /// 通知・再送・resumeはしない。読めなかったものは要照合のまま残す。
    pub async fn on_resume(self: &Arc<Self>) {
        let at = now_ms();
        {
            let mut last = self.lifecycle.last_resume.lock().unwrap();
            if last.is_some_and(|t| at.0 - t < RESUME_DEDUP_MS) {
                return;
            }
            *last = Some(at.0);
        }
        let marked: Vec<AgentKey> = self.mutate(|d| {
            let keys = agents_to_mark_on_resume(&d.agents);
            let mut events = Vec::new();
            for v in d.agents.iter_mut().filter(|v| keys.contains(&v.agent.key)) {
                v.freshness = Freshness::NeedsReconcile;
                events.push(HostEvent::AgentUpdated { view: v.clone() });
            }
            events.push(HostEvent::SystemResumed { at });
            (keys, events)
        });
        crate::diag::log("wake", &format!("resumed marked={}", marked.len()));
        let mut roots: Vec<AgentKey> = Vec::new();
        for key in marked {
            let Ok(h) = self.backend.read(key.clone(), ReadOptions { include_turns: false }).await else { continue };
            let root = self.mutate(|d| {
                let Some(existing) = d.view(&key).map(|v| v.agent.clone()) else { return (None, Vec::new()) };
                // 照合の結果は通知しない（sleep中の完了を後から通知にしない）。
                let notified = d.notify_inbox.len();
                let events = d.upsert_history_agent(existing.clone(), h.status);
                d.notify_inbox.truncate(notified);
                (Some(agent_key_of(&existing.chat)), events)
            });
            if let Some(r) = root {
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
        }
        // 子孫の対象集合を照合し直す（読み取りのみ）。
        let seen: HashSet<AgentKey> = roots.iter().cloned().collect();
        for r in seen {
            self.start_scan(r);
        }
    }
}
