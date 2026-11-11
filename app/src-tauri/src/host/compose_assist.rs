//! 返信・推敲のホスト側（`app/DESIGN_COMPOSE.md`）。
//!
//! - 呼び出しごとに、保存されない一時の会話（ephemeral・読取り専用・承認なし・サブエージェント無効）を新しく作る。
//!   手本は `purpose_draft.rs`（1回の生成と結果の受け取り）と `side.rs`（同じ一時会話への追加の送信）。
//!   一時の会話は一覧に出さず、`HostData::side_threads` に登録して停止の対象には入れる。
//! - 「短く」などの再調整は同じ会話へ追加で送る。別件（取り込み直し・作り直し）は新しい会話。閉じたら会話を閉じる（実行中なら中断する）。
//! - 受理不明・切断で自動再送しない。切断は「結果は不明」で、完了・失敗にはしない。
//! - 取り込んだ文章・結果は、ホストのメモリ上だけに持つ。保存しない・診断ログに書かない・履歴を残さない。同時に1件。
//! - 出力はコピーまで（`compose_result` を呼び出し側がクリップボードへ書く）。自動貼り付け・自動送信はしない。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::{err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::rules::compose_assist as ca;
use crate::rules::side::finalize_agent_text;

/// 最終回答の逐次本文を持つ上限（文字数）。
const BUFFER_MAX_CHARS: usize = 400_000;

// ───────────────────────────── 純粋な部分 ─────────────────────────────

/// turn の終端から結果を決める。Ok＝結果の文、Err＝失敗の理由。
pub(super) fn finish(end: TurnEnd, error: Option<&str>, last_text: Option<&str>) -> Result<String, String> {
    match end {
        TurnEnd::Interrupted => Err("中断されました".into()),
        TurnEnd::Failed => Err(error.map(str::trim).filter(|m| !m.is_empty()).unwrap_or("理由は報告されていません").to_string()),
        TurnEnd::Completed => {
            let text = last_text.map(ca::clean_result).filter(|t| !t.is_empty());
            text.ok_or_else(|| "返答を受け取れませんでした（空の返答でした）".to_string())
        }
    }
}

/// 作成中か（結果を待っている）。
pub(super) fn is_generating(state: &ComposeState) -> bool {
    matches!(state, ComposeState::Generating)
}

/// 接続が切れたときの状態。作成中は「結果は不明」にする（完了・失敗にしない）。終端の状態はそのまま。
pub(super) fn after_disconnect(state: &ComposeState) -> ComposeState {
    if is_generating(state) {
        ComposeState::Unknown { message: "接続が切れたため、結果は不明です（完了したとは限りません）。必要なら「作り直す」を押してください".into() }
    } else {
        state.clone()
    }
}

/// 失敗の表示文。前の結果が残っているときは、その旨を添える。
pub(super) fn failure_message(reason: &str, has_previous_result: bool) -> String {
    if has_previous_result {
        format!("{reason}。前の結果は残っています")
    } else {
        reason.to_string()
    }
}

/// 追加で再調整できるか（会話が生きていて、作成中・結果不明でないとき）。
pub(super) fn adjustable(thread_alive: bool, state: &ComposeState) -> bool {
    thread_alive && matches!(state, ComposeState::Done | ComposeState::Failed { .. })
}

fn thread_key(agent: &AgentKey) -> ChatKey {
    ChatKey { backend: agent.backend, id: agent.id.clone() }
}

// ───────────────────────────── 作業状態 ─────────────────────────────

#[derive(Default)]
struct ComposeLive {
    view: ComposeView,
    /// 一時の会話（開始できたら）。
    thread: Option<ChatKey>,
    /// 最後に確定したエージェント発言。
    last_text: Option<String>,
    buffers: HashMap<ExternalId, String>,
    clipped: HashSet<ExternalId>,
    /// 閉じる・作り直すたびに進める。古い非同期処理の結果を捨てる印。
    session: u64,
    /// 取り込みの通し番号（閉じても戻さない）。
    captures: u32,
}

impl ComposeLive {
    fn refresh(&mut self) {
        self.view.can_adjust = adjustable(self.thread.is_some(), &self.view.state);
    }

    fn clear_turn_buffers(&mut self) {
        self.last_text = None;
        self.buffers.clear();
        self.clipped.clear();
    }
}

/// 返信・推敲の作業状態（同時に1件）。
#[derive(Default)]
pub struct ComposeRuntime {
    live: Mutex<ComposeLive>,
}

impl Host {
    fn emit_compose(&self) {
        self.mutate(|_| ((), vec![HostEvent::ComposeUpdated]));
    }

    /// 設定を保存する（ショートカットの再登録はコマンド層）。
    pub fn set_compose_settings(self: &Arc<Self>, compose: ComposeSettings) -> Result<ComposeSettings, IpcError> {
        let current = self.read(|d| d.settings.compose.clone());
        let compose = ca::sanitize_settings(compose, &current).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        self.update_settings(|s| s.compose = compose.clone())?;
        Ok(compose)
    }

    /// 現在の表示内容。
    pub fn compose_view(&self) -> ComposeView {
        self.compose_rt.live.lock().unwrap().view.clone()
    }

    /// 結果の文（コピー用）。なければ None。
    pub fn compose_result(&self) -> Option<String> {
        self.compose_rt.live.lock().unwrap().view.result.clone()
    }

    /// 一時の会話の状態を一覧外の登録から外す。
    fn retire_compose_thread(&self, thread: &ChatKey) {
        self.mutate(|d| {
            d.forget_side_thread(thread);
            ((), vec![])
        });
    }

    /// 今の会話を閉じて、表示内容を空にする（実行中なら中断を要求する）。世代を進める。
    async fn compose_teardown(self: &Arc<Self>, confirmed: &UserConfirmed) {
        let (thread, generating) = {
            let mut g = self.compose_rt.live.lock().unwrap();
            g.session += 1;
            let generating = is_generating(&g.view.state);
            g.clear_turn_buffers();
            g.view = ComposeView::default();
            (g.thread.take(), generating)
        };
        if let Some(t) = thread {
            if generating {
                // 中止＝閉じる。中断の要求だけで、停止の確認は既存の停止記録が続ける。要求できなくても閉じる。
                let _ = self.interrupt_chat(InterruptChatArgs { chat: t.clone() }, confirmed).await;
            }
            self.retire_compose_thread(&t);
        }
    }

    /// パネルを開く（新しい取り込み。取り込み直しを含む）。前の会話は閉じる。`mode` が None なら前回使った方。
    pub async fn compose_begin(self: &Arc<Self>, mode: Option<ComposeMode>, source: String, origin: ComposeOrigin, notice: Option<String>, confirmed: &UserConfirmed) -> ComposeView {
        let mode = mode.unwrap_or_else(|| self.read(|d| d.settings.compose.last_mode));
        self.compose_teardown(confirmed).await;
        {
            let mut g = self.compose_rt.live.lock().unwrap();
            g.captures += 1;
            g.view = ComposeView { open: true, mode, source, origin, notice, capture: g.captures, ..ComposeView::default() };
            g.refresh();
        }
        self.emit_compose();
        self.compose_view()
    }

    /// パネルを閉じる（会話を閉じ、取り込んだ文章も結果も消す）。
    pub async fn compose_close(self: &Arc<Self>, confirmed: &UserConfirmed) -> ComposeView {
        self.compose_teardown(confirmed).await;
        self.emit_compose();
        self.compose_view()
    }

    /// 世代が同じときだけ、表示内容を更新して通知する。
    fn compose_update(&self, session: u64, f: impl FnOnce(&mut ComposeLive)) -> bool {
        {
            let mut g = self.compose_rt.live.lock().unwrap();
            if g.session != session {
                return false;
            }
            f(&mut g);
            g.refresh();
        }
        self.emit_compose();
        true
    }

    fn compose_fail(&self, session: u64, reason: String) {
        self.compose_update(session, |l| {
            if is_generating(&l.view.state) {
                l.view.state = ComposeState::Failed { message: failure_message(&reason, l.view.result.is_some()) };
            }
        });
    }

    fn compose_workdir(&self) -> Result<String, IpcError> {
        let dir = self.app_data_dir.join("compose");
        std::fs::create_dir_all(&dir).map_err(|e| err(IpcErrorCode::Io, format!("作業用のフォルダを作れません: {e}")))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    // ───────────── 作成 ─────────────

    /// 作成する（利用者が押したときの1回だけ）。一時の会話を新しく開始して、最初の文を1回だけ送る。
    pub async fn compose_start(self: &Arc<Self>, args: ComposeStartArgs, _confirmed: &UserConfirmed) -> Result<ComposeView, IpcError> {
        ca::check_input(&args.source, &args.instruction).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        if !self.is_connected() {
            return Err(err(IpcErrorCode::InvalidArgs, "Codex に接続していないため、作成できません"));
        }
        let settings = self.read(|d| d.settings.compose.clone());
        let cwd = self.compose_workdir()?;
        let (previous, session) = {
            let mut g = self.compose_rt.live.lock().unwrap();
            if is_generating(&g.view.state) {
                return Err(err(IpcErrorCode::InvalidArgs, "作成中です。終わるまでお待ちください（閉じると中止します）"));
            }
            g.session += 1;
            g.clear_turn_buffers();
            let previous = g.thread.take();
            let (origin, notice) = (g.view.origin, g.view.notice.clone());
            g.view = ComposeView { open: true, mode: args.mode, source: args.source.clone(), origin, notice, state: ComposeState::Generating, round: 1, capture: g.view.capture, ..ComposeView::default() };
            g.refresh();
            (previous, g.session)
        };
        if let Some(t) = previous {
            self.retire_compose_thread(&t);
        }
        if settings.last_mode != args.mode {
            // 前回使った方の記録。保存できなくても作成は続ける。
            let _ = self.update_settings(|s| s.compose.last_mode = args.mode);
        }
        self.emit_compose();

        let models = self.models_cache.lock().unwrap().clone();
        let choice = ca::resolve_choice(&settings, &models);
        let params = StartChatParams {
            kind: ChatKind::General,
            cwd,
            model: choice.clone(),
            permission: PermissionPreset::ReadOnlyNoPrompt,
            extra_folders: Vec::new(),
            ephemeral: true,
            config: Some(crate::codex::thread_config::no_subagents_config()),
            developer_instructions: Some(ca::developer_instructions(args.mode, &settings)),
        };
        let started = match self.backend.start_chat(params).await {
            Ok(s) => s,
            Err(e) => {
                // 文はまだ送っていない。作れたか確認できなくても、一覧には出ない（再送はしない）。
                self.compose_fail(session, format!("一時の会話を開始できませんでした（{e}）"));
                return Ok(self.compose_view());
            }
        };
        let thread = started.chat.key.clone();
        let at = now_ms();
        let status = AgentStatus {
            state: AgentState::Idle,
            raw: RawState { label: "thread/start".into() },
            scope: StateScope::Agent,
            turn: None,
            wait: None,
            evidence: Evidence { source: EvidenceSource::Response, raw_label: Some("thread/start".into()), source_time: None, observed_at: at },
        };
        // 一覧には出さない会話として登録する（状態の追跡・停止の確認のため、エージェントとしては持つ）。主会話はないので自分自身を指す。
        self.mutate(|d| {
            d.side_threads.insert(thread.clone(), thread.clone());
            ((), d.set_live(started.root.clone(), status))
        });
        if !self.compose_update(session, |l| l.thread = Some(thread.clone())) {
            // 開始を待つあいだに閉じられた。作った会話は片付ける（文は送らない）。
            self.retire_compose_thread(&thread);
            return Ok(self.compose_view());
        }
        self.compose_send(&thread, ca::first_prompt(args.mode, &args.source, &args.instruction), choice, session).await;
        Ok(self.compose_view())
    }

    /// 同じ会話へ追加で再調整を送る（「短く」など）。
    pub async fn compose_adjust(self: &Arc<Self>, args: ComposeAdjustArgs, _confirmed: &UserConfirmed) -> Result<ComposeView, IpcError> {
        ca::check_instruction(&args.instruction).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        if args.instruction.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "指示が空です"));
        }
        if !self.is_connected() {
            return Err(err(IpcErrorCode::InvalidArgs, "Codex に接続していないため、再調整できません"));
        }
        let settings = self.read(|d| d.settings.compose.clone());
        let (thread, session) = {
            let mut g = self.compose_rt.live.lock().unwrap();
            let thread = match g.thread.clone() {
                Some(t) if g.view.open && adjustable(true, &g.view.state) => t,
                _ => return Err(err(IpcErrorCode::InvalidArgs, "この会話には追加で送れません。「作り直す」を押してください")),
            };
            g.clear_turn_buffers();
            g.view.state = ComposeState::Generating;
            g.view.round += 1;
            g.refresh();
            (thread, g.session)
        };
        self.emit_compose();
        let models = self.models_cache.lock().unwrap().clone();
        let choice = ca::resolve_choice(&settings, &models);
        self.compose_send(&thread, ca::adjust_prompt(&args.instruction), choice, session).await;
        Ok(self.compose_view())
    }

    /// 一時の会話へ1回送る。再送しない（受理不明は「結果は不明」と示すだけ）。
    async fn compose_send(self: &Arc<Self>, thread: &ChatKey, text: String, choice: Option<ModelChoice>, session: u64) {
        let request = SendRequest {
            chat: thread.clone(),
            mode: SendMode::NewTurn,
            text,
            attachments: Vec::new(),
            speed_tier: choice.as_ref().and_then(|m| m.speed_tier.clone()),
            model: choice,
            permission: None,
            cwd: None,
            work_folders: None,
            work_mode: None,
            client_message_id: self.local_id("cm").0,
            output_schema: None,
        };
        match self.backend.send(request).await {
            // 状態はすでに作成中。受理の応答より先に終端が観測されていたら、その結果を上書きしない。
            SendOutcome::Accepted { .. } => {}
            SendOutcome::Rejected { error, .. } => self.compose_fail(session, format!("作成を始められませんでした（{error}）")),
            SendOutcome::AcceptanceUnknown(_) => {
                self.compose_update(session, |l| {
                    if is_generating(&l.view.state) {
                        l.view.state = ComposeState::Unknown { message: "送信が受け付けられたか確認できません。結果は不明です。自動では再送しません。必要なら「作り直す」を押してください".into() };
                    }
                });
            }
        }
    }

    // ───────────── イベントの観測 ─────────────

    /// 一時の会話のイベントを観測する（状態の判定には使わない）。確定した最終回答を集め、turn の終端で結果にする。
    /// 接続が切れたら「結果は不明」にする（完了・失敗にしない）。待たない・送らない。
    pub(super) fn compose_observe(self: &Arc<Self>, ev: &BackendEvent) {
        let mut g = self.compose_rt.live.lock().unwrap();
        let Some(thread) = g.thread.clone() else { return };
        match ev {
            BackendEvent::ActivityDelta { item, delta } if thread_key(&item.agent) == thread => {
                let buf = g.buffers.entry(item.item_id.clone()).or_default();
                if buf.chars().count() + delta.chars().count() <= BUFFER_MAX_CHARS {
                    buf.push_str(delta);
                } else {
                    g.clipped.insert(item.item_id.clone());
                }
            }
            BackendEvent::Activity { activity } if activity.phase == ActivityPhase::Completed && thread_key(&activity.key.agent) == thread => {
                let buffer = g.buffers.remove(&activity.key.item_id);
                let clipped = g.clipped.remove(&activity.key.item_id);
                if activity.kind != ActivityKind::AgentMessage {
                    return;
                }
                // 切り捨てた逐次本文・要約だけの本文は全文ではないので、結果にしない。
                if let Some((text, summary_only)) = finalize_agent_text(buffer.as_deref(), &activity.summary) {
                    if !(clipped && summary_only) {
                        g.last_text = Some(text);
                    }
                }
            }
            BackendEvent::TurnEnded { turn, end, error, .. } if thread_key(&turn.agent) == thread => {
                // 作成中か、受理不明のまま終端を観測したときだけ結果にする（観測であって、再送ではない）。
                if !matches!(g.view.state, ComposeState::Generating | ComposeState::Unknown { .. }) {
                    return;
                }
                match finish(*end, error.as_deref(), g.last_text.as_deref()) {
                    Ok(text) => {
                        g.view.result = Some(text);
                        g.view.state = ComposeState::Done;
                    }
                    Err(reason) => g.view.state = ComposeState::Failed { message: failure_message(&reason, g.view.result.is_some()) },
                }
                g.clear_turn_buffers();
                g.refresh();
                drop(g);
                self.emit_compose();
            }
            BackendEvent::Connection { state: ConnectionState::Disconnected { .. } } => {
                // 一時の会話は接続とともに消える。作成中なら結果は不明。結果が出ていればそのまま残す（追加の再調整はできなくなる）。
                g.thread = None;
                g.view.state = after_disconnect(&g.view.state);
                g.clear_turn_buffers();
                g.refresh();
                drop(g);
                self.retire_compose_thread(&thread);
                self.emit_compose();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completed_turn_becomes_the_trimmed_result() {
        assert_eq!(finish(TurnEnd::Completed, None, Some("\n  お世話になっております。\n")), Ok("お世話になっております。".into()));
    }

    #[test]
    fn an_empty_or_missing_reply_is_a_failure_not_an_empty_result() {
        for none in [None, Some("  \n")] {
            assert!(finish(TurnEnd::Completed, None, none).is_err());
        }
    }

    #[test]
    fn failed_and_interrupted_turns_report_their_reason() {
        assert_eq!(finish(TurnEnd::Failed, Some("上限に達しました"), Some("x")), Err("上限に達しました".into()));
        assert!(finish(TurnEnd::Failed, None, None).unwrap_err().contains("報告されていません"));
        assert_eq!(finish(TurnEnd::Interrupted, None, Some("途中")), Err("中断されました".into()));
    }

    #[test]
    fn a_disconnect_makes_a_running_compose_unknown_and_leaves_finished_ones() {
        let ComposeState::Unknown { message } = after_disconnect(&ComposeState::Generating) else { panic!("must be unknown") };
        assert!(message.contains("結果は不明"));
        for s in [ComposeState::Idle, ComposeState::Done, ComposeState::Failed { message: "x".into() }, ComposeState::Unknown { message: "y".into() }] {
            assert_eq!(after_disconnect(&s), s);
        }
    }

    #[test]
    fn adjusting_needs_a_live_conversation_and_a_finished_turn() {
        let failed = ComposeState::Failed { message: "x".into() };
        assert!(adjustable(true, &ComposeState::Done) && adjustable(true, &failed));
        assert!(!adjustable(false, &ComposeState::Done), "the conversation is gone after a disconnect");
        for s in [ComposeState::Idle, ComposeState::Generating, ComposeState::Unknown { message: "u".into() }] {
            assert!(!adjustable(true, &s), "{s:?}");
        }
    }

    #[test]
    fn a_failure_after_a_result_says_the_previous_result_remains() {
        assert_eq!(failure_message("中断されました", false), "中断されました");
        assert!(failure_message("中断されました", true).ends_with("前の結果は残っています"));
    }

    #[test]
    fn the_view_debug_never_shows_the_text() {
        let v = ComposeView { source: "秘密の本文".into(), result: Some("秘密の結果".into()), ..ComposeView::default() };
        let s = format!("{v:?}");
        assert!(!s.contains("秘密"));
    }
}
