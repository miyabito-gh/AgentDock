//! side相談（P3-4、`app/DESIGN_P3.md` §1 #10）。主会話の最後の終端turnまでを、読取り専用の一時の分岐（ephemeral）として開く。
//!
//! - 一時の分岐は一覧に出さず、主会話の子孫に数えず、キューの自動送信の条件にも入れない。主会話の作業・キューはそのまま続く。
//!   停止対象には入れる（完全終了・強制終了の確認、主会話の削除の停止照合。`HostData::stop_scope`）。
//! - sideへの送信は専用の入口（`send_side`）。再送しない（受理不明は照合できないので「確認できない」と示すだけ）。
//!   承認・質問はsideのパネルで回答する（通知は主会話の宛先に「承認待ち」「質問待ち」として出す）。
//! - 発言の記録は、確定した本文だけを `chats\<dirId>\side\<id>.json` に残す（表示専用。閉じても・再起動後も主会話の削除まで残す）。
//!   一時の分岐はプロセス終了・切断で消えるので、その記録は「終了（再開不可）」で開く。状態を完了扱いにはしない。
//! - 引渡しは、選んだ発言を引用ブロックにして返すだけ。自動送信しない。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use super::state::agent_key_of;
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::ChatArgs;
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::rules::side::{check_sendable, finalize_agent_text, handoff_text, last_terminal_turn};
use crate::store::records::SideFile;

/// 逐次本文を1つの発言として持つ上限（文字数）。超えた分は持たず、短縮として扱う。
const BUFFER_MAX_CHARS: usize = 400_000;

/// 開いているsideの会話の、メモリ上の記録。
struct SideLive {
    id: LocalId,
    main: ChatKey,
    opened_at: UnixMillis,
    entries: Vec<SideEntry>,
    /// 項目ごとの逐次本文（確定したら全文として記録する）。
    buffers: HashMap<ExternalId, String>,
    /// 記録済みの項目（同じ項目の確定通知を二重に記録しない）。
    recorded: HashSet<ExternalId>,
    /// 逐次本文が上限を超えて切り捨てられた項目。
    clipped: HashSet<ExternalId>,
}

/// 開いているsideの会話（side の会話 → 記録）。
#[derive(Default)]
pub struct SideRuntime {
    live: std::sync::Mutex<HashMap<ChatKey, SideLive>>,
}

fn thread_key(agent: &AgentKey) -> ChatKey {
    ChatKey { backend: agent.backend, id: agent.id.clone() }
}

impl Host {
    fn find_side(&self, id: &LocalId) -> Option<SideSessionMeta> {
        self.read(|d| d.locals.values().flat_map(|f| f.side_sessions.iter()).find(|s| &s.id == id).cloned())
    }

    fn side_running(&self, thread: &ChatKey) -> bool {
        self.read(|d| d.running_turn.contains_key(&agent_key_of(thread)))
    }

    fn emit_side(&self, side: &SideSessionMeta) {
        let side = side.clone();
        self.mutate(|_| ((), vec![HostEvent::SideUpdated { side }]));
    }

    /// 記録ファイルの内容（開いているものはメモリ上の最新）。
    fn side_file_of(&self, thread: &ChatKey) -> Option<SideFile> {
        let g = self.side_rt.live.lock().unwrap();
        let l = g.get(thread)?;
        Some(SideFile { schema_version: crate::store::records::SCHEMA_VERSION, id: l.id.clone(), main: l.main.clone(), thread: thread.clone(), opened_at: l.opened_at, entries: l.entries.clone() })
    }

    /// 記録を書く（別taskで原子的に置き換える。失敗は警告として示し、メモリ上の記録は残す）。
    fn persist_side(self: &Arc<Self>, file: SideFile) {
        let Some(store) = self.persist.store().cloned() else { return };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let host = self.clone();
        tokio::task::spawn_blocking(move || {
            let res = {
                let _g = host.persist.io_guard();
                store.write_side(&file)
            };
            if let Err(e) = res {
                host.warn(format!("side相談の記録を保存できませんでした（相談の画面には残っています）: {}", super::persist::save_failure_message(&e)));
            }
        });
    }

    fn set_side_state(self: &Arc<Self>, side: &SideSessionMeta, state: SideState) -> SideSessionMeta {
        let (id, main) = (side.id.clone(), side.main.clone());
        let next = state.clone();
        self.update_local(&main, true, Duration::ZERO, |f| {
            if let Some(s) = f.side_sessions.iter_mut().find(|s| s.id == id) {
                s.state = next;
            }
        });
        let meta = SideSessionMeta { state, ..side.clone() };
        self.emit_side(&meta);
        meta
    }

    /// 開いている相談を終了にする（観測をやめ、最後の記録を書く）。
    fn end_side(self: &Arc<Self>, side: &SideSessionMeta, reason: &str) -> SideSessionMeta {
        let file = self.side_file_of(&side.thread);
        self.side_rt.live.lock().unwrap().remove(&side.thread);
        if let Some(f) = file {
            self.persist_side(f);
        }
        self.set_side_state(side, SideState::Ended { reason: reason.to_string() })
    }

    // ───────────── 開く ─────────────

    /// side相談を開く。主会話の最後の終端turnまでを、読取り専用・保存されない分岐として開く。
    /// 開けたか不明（受理不明）のときは再送しない。主会話は変更せず、再開もしない。
    pub async fn open_side(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<OpenSideResult, IpcError> {
        self.require_op(ParityOp::SideChat)?;
        self.check_op_target(&args.chat)?;
        self.precheck_space()?;
        if self.persist.store().is_none() {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、side相談を開けません（相談の記録を残せません）"));
        }
        let main = args.chat;
        // 分岐点は履歴の最後の終端turn（進行中のturnは指定できない）。履歴は読取りだけで確かめる。
        let history = self
            .read_history(agent_key_of(&main), ReadOptions { include_turns: true })
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("履歴を取得できないため、side相談を開けません（{e}）")))?;
        let through = last_terminal_turn(&history.turns);
        if through.is_none() && !history.turns.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "終了したturnがまだないため、side相談を開けません。最初のturnが終わってから開いてください"));
        }
        let params = ForkParams { through_turn: through, ephemeral: true, read_only: true };
        let out = self.backend.fork_chat(main.clone(), params, confirmed).await?;
        let Some(thread) = out.chat.filter(|_| out.ack == OpAck::Accepted) else {
            // 拒否・受理不明（一覧で照合できない一時の会話なので、開けたか不明のまま示す。再送しない）。
            return Ok(OpenSideResult { ack: out.ack, side: None });
        };
        let at = now_ms();
        let meta = SideSessionMeta { id: self.local_id("side"), main: main.clone(), thread: thread.clone(), state: SideState::Open, opened_at: Some(at) };
        // 一覧には出さない会話として登録する（状態の追跡・承認・停止の確認のため、エージェントとしては持つ）。
        let agent = Agent {
            key: agent_key_of(&thread),
            chat: thread.clone(),
            parent: ParentLink::Root,
            forked_from: Known::direct(Some(agent_key_of(&main))),
            display_name: Known::NotFetched,
            role: Known::NotFetched,
            assignment: Known::NotFetched,
            agent_path: Known::NotFetched,
            latest_turn: None,
        };
        let status = AgentStatus {
            state: AgentState::Idle,
            raw: RawState { label: "thread/fork".into() },
            scope: StateScope::Agent,
            turn: None,
            wait: None,
            evidence: Evidence { source: EvidenceSource::Response, raw_label: Some("thread/fork".into()), source_time: None, observed_at: at },
        };
        self.mutate(|d| {
            d.side_threads.insert(thread.clone(), main.clone());
            ((), d.set_live(agent, status))
        });
        self.side_rt.live.lock().unwrap().insert(
            thread.clone(),
            SideLive { id: meta.id.clone(), main: main.clone(), opened_at: at, entries: Vec::new(), buffers: HashMap::new(), recorded: HashSet::new(), clipped: HashSet::new() },
        );
        let saved = meta.clone();
        self.update_local(&main, true, Duration::ZERO, |f| f.side_sessions.push(saved));
        self.emit_side(&meta);
        if let Some(f) = self.side_file_of(&thread) {
            self.persist_side(f);
        }
        Ok(OpenSideResult { ack: OpAck::Accepted, side: Some(meta) })
    }

    // ───────────── 送信 ─────────────

    /// sideへ送る。再送しない（受理不明は照合できないので、確認できないと示すだけ）。主会話の送信・キューとは独立。
    pub async fn send_side(self: &Arc<Self>, args: SendSideArgs, _confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        let Some(side) = self.find_side(&args.side) else { return Err(err(IpcErrorCode::NotFound, "side相談が見つかりません")) };
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "メッセージが空です"));
        }
        self.check_not_delete_pending(&side.main)?;
        let live = self.side_rt.live.lock().unwrap().contains_key(&side.thread);
        if !live {
            return Err(err(IpcErrorCode::InvalidArgs, "この相談は終了しています（一時的な相談は再開できません）"));
        }
        check_sendable(&side.state, self.side_running(&side.thread)).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        // 設定（モデル・権限・作業フォルダ）は上書きしない。読取り専用の権限は分岐のときのまま。
        let request = SendRequest {
            chat: side.thread.clone(),
            mode: SendMode::NewTurn,
            text: args.text.clone(),
            attachments: Vec::new(),
            model: None,
            permission: None,
            cwd: None,
            work_mode: None,
            speed_tier: None,
            client_message_id: self.local_id("cm").0,
        };
        match self.backend.send(request).await {
            SendOutcome::Accepted { .. } => {
                self.push_side_entry(&side.thread, SideEntry { role: SideRole::User, text: args.text, at: now_ms(), truncated: false });
                Ok(OpAck::Accepted)
            }
            SendOutcome::Rejected { error, .. } => Ok(OpAck::Rejected { message: error.to_string() }),
            SendOutcome::AcceptanceUnknown(_) => {
                Ok(OpAck::Unknown { message: "送信が受け付けられたか確認できません。side相談では照合できないため、再送しません。相談の内容を見て、必要なら送り直してください".into() })
            }
        }
    }

    fn push_side_entry(self: &Arc<Self>, thread: &ChatKey, entry: SideEntry) {
        let file = {
            let mut g = self.side_rt.live.lock().unwrap();
            let Some(l) = g.get_mut(thread) else { return };
            l.entries.push(entry);
            SideFile { schema_version: crate::store::records::SCHEMA_VERSION, id: l.id.clone(), main: l.main.clone(), thread: thread.clone(), opened_at: l.opened_at, entries: l.entries.clone() }
        };
        self.persist_side(file);
    }

    // ───────────── 閉じる・読む・引渡し ─────────────

    /// 相談を閉じる。実行中は、中断を確認したうえ（`interrupt`）でだけ閉じる（中断の停止確認は停止記録で続く）。閉じても記録は主会話の削除まで残る。
    pub async fn close_side(self: &Arc<Self>, args: CloseSideArgs, confirmed: &UserConfirmed) -> Result<SideSessionMeta, IpcError> {
        let Some(side) = self.find_side(&args.side) else { return Err(err(IpcErrorCode::NotFound, "side相談が見つかりません")) };
        if matches!(side.state, SideState::Ended { .. }) {
            return Ok(side);
        }
        let running = self.side_running(&side.thread);
        if running && !args.interrupt {
            return Err(blocked(BlockedReason::ChatBusy, "相談が実行中です。中断して閉じるか、完了を待ってください"));
        }
        let reason = if running {
            // 中断の要求は受付で、停止の確認は停止記録（10秒で未確認の案内）が続ける。要求できなければ閉じない。
            self.interrupt_chat(InterruptChatArgs { chat: side.thread.clone() }, confirmed).await?;
            "閉じました（中断を要求しました。停止は確認中です）"
        } else {
            "閉じました"
        };
        Ok(self.end_side(&side, reason))
    }

    async fn side_entries(self: &Arc<Self>, side: &SideSessionMeta) -> Result<Vec<SideEntry>, IpcError> {
        if let Some(f) = self.side_file_of(&side.thread) {
            return Ok(f.entries);
        }
        let Some(store) = self.persist.store().cloned() else { return Err(err(IpcErrorCode::Io, "保存先が使えません")) };
        let (main, id) = (side.main.clone(), side.id.clone());
        match tokio::task::spawn_blocking(move || store.read_side(&main, &id)).await {
            Ok(Ok(f)) => Ok(f.entries),
            // 一度も発言がなく記録が書かれていない場合は、空として扱う（読めなかったのとは区別する）。
            Ok(Err(crate::store::StoreError::Io(e))) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Ok(Err(e)) => Err(err(IpcErrorCode::Io, super::persist::save_failure_message(&e))),
            Err(e) => Err(err(IpcErrorCode::Io, e.to_string())),
        }
    }

    /// 相談の記録（確定した発言）。閉じた後・再起動後は、保存した記録を読み取り専用で返す。
    pub async fn read_side(self: &Arc<Self>, args: SideIdArgs) -> Result<SideTranscript, IpcError> {
        let Some(side) = self.find_side(&args.side) else { return Err(err(IpcErrorCode::NotFound, "side相談が見つかりません")) };
        let entries = self.side_entries(&side).await?;
        let running = matches!(side.state, SideState::Open) && self.side_running(&side.thread);
        Ok(SideTranscript { side, entries, running })
    }

    /// 選んだ発言を、主会話の入力欄へ入れる引用ブロックにする（自動送信しない。入力欄への挿入はUI）。
    pub async fn handoff_side(self: &Arc<Self>, args: HandoffSideArgs) -> Result<HandoffText, IpcError> {
        let Some(side) = self.find_side(&args.side) else { return Err(err(IpcErrorCode::NotFound, "side相談が見つかりません")) };
        let entries = self.side_entries(&side).await?;
        let text = handoff_text(&self.chat_title(&side.main), &entries, &args.entries).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        let chars = text.chars().count() as u32;
        Ok(HandoffText { text, chars })
    }

    // ───────────── イベントの観測 ─────────────

    /// 開いている相談のイベントを記録する（状態の判定には使わない）。逐次本文を集め、確定したエージェントの発言を記録する。
    /// 切断では、一時の会話は消えるので終了にする（完了・失敗には変えない）。
    pub(super) fn side_observe(self: &Arc<Self>, ev: &BackendEvent) {
        if self.side_rt.live.lock().unwrap().is_empty() {
            return;
        }
        match ev {
            BackendEvent::ActivityDelta { item, delta } => {
                let mut g = self.side_rt.live.lock().unwrap();
                let Some(l) = g.get_mut(&thread_key(&item.agent)) else { return };
                let buf = l.buffers.entry(item.item_id.clone()).or_default();
                if buf.chars().count() + delta.chars().count() <= BUFFER_MAX_CHARS {
                    buf.push_str(delta);
                } else {
                    l.clipped.insert(item.item_id.clone());
                }
            }
            BackendEvent::Activity { activity } if activity.phase == ActivityPhase::Completed => {
                let thread = thread_key(&activity.key.agent);
                let entry = {
                    let mut g = self.side_rt.live.lock().unwrap();
                    let Some(l) = g.get_mut(&thread) else { return };
                    let buffer = l.buffers.remove(&activity.key.item_id);
                    let clipped = l.clipped.remove(&activity.key.item_id);
                    if activity.kind != ActivityKind::AgentMessage || !l.recorded.insert(activity.key.item_id.clone()) {
                        return;
                    }
                    // 切り捨てた逐次本文は全文ではないので、短縮として扱う。
                    finalize_agent_text(buffer.as_deref(), &activity.summary).map(|(text, summary_only)| SideEntry { role: SideRole::Agent, text, at: now_ms(), truncated: summary_only || clipped })
                };
                if let Some(e) = entry {
                    self.push_side_entry(&thread, e);
                }
            }
            BackendEvent::Connection { state: ConnectionState::Disconnected { .. } } => {
                let threads: Vec<ChatKey> = self.side_rt.live.lock().unwrap().keys().cloned().collect();
                for t in threads {
                    if let Some(side) = self.read(|d| d.locals.values().flat_map(|f| f.side_sessions.iter()).find(|s| s.thread == t).cloned()) {
                        self.end_side(&side, "接続が切れたため終了しました（一時的な相談は再開できません）");
                    }
                }
            }
            _ => {}
        }
    }
}
