//! 目的メモの下書き支援のホスト側（`app/DESIGN_CONT_DRAFT.md` §2〜§6）。
//!
//! - 利用者が押したときだけ、会話本体とは別の一時 thread（保存されない・読取り専用・承認を出さない）を新しく開始し、
//!   現在の目的メモ＋直近の往復＋目次＋下書き用の台帳のパスを1回だけ送る。会話本体の thread には何も送らず、resume もしない
//!   （履歴は読取りだけ）。受理不明・切断で再送しない。
//! - 一時 thread は side相談と同じ扱い: 一覧に出さず、子孫・キュー条件に数えず、停止対象には入れる（`HostData::side_threads`）。
//!   接続が切れたら「結果は不明（Lost）」にする。完了・失敗にはしない。
//! - 結果は構造化出力（JSON）を `rules::purpose_draft::check_draft` で検査する。保存はしない。目的メモの変更は
//!   `set_purpose_note`（版の照合つき）だけ。
//! - 最後の結果は `chats\<dirId>\purpose_draft\last.json` に残す。再起動後に作成中だったものは「結果は不明」で読み出す。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::state::agent_key_of;
use super::{err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::draft::*;
use crate::backend::ipc::*;
use crate::backend::local::ChatArgs;
use crate::backend::model::*;
use crate::rules::continuity as cont;
use crate::rules::purpose_draft as pd;
use crate::rules::side::finalize_agent_text;

/// 最終回答の逐次本文を持つ上限（文字数）。
const BUFFER_MAX_CHARS: usize = 400_000;

// ───────────────────────────── 純粋な部分 ─────────────────────────────

/// 作成中（結果を待っている）の状態か。2つ目の作成を止める根拠。
pub(super) fn is_active(state: &DraftState) -> bool {
    matches!(state, DraftState::Preparing | DraftState::Running { .. } | DraftState::AcceptanceUnknown)
}

/// 接続が切れたときの状態。作成中は「結果は不明」にする（完了・失敗にしない）。終端の状態はそのまま。
pub(super) fn after_disconnect(state: &DraftState) -> DraftState {
    if is_active(state) {
        DraftState::Lost
    } else {
        state.clone()
    }
}

/// turn の終端から状態を決める。完了なら最終回答を検査する（検査を通っても「確認待ち」。成功扱いにはしない）。
pub(super) fn finish_state(end: TurnEnd, error: Option<&str>, last_text: Option<&str>, cite: &pd::CiteSet, memo: &str) -> DraftState {
    match end {
        TurnEnd::Interrupted => DraftState::Interrupted,
        TurnEnd::Failed => DraftState::Failed { message: error.map(str::trim).filter(|m| !m.is_empty()).unwrap_or("理由は報告されていません").to_string() },
        TurnEnd::Completed => {
            let Some(raw) = last_text.filter(|t| !t.trim().is_empty()) else {
                return DraftState::Unreadable { raw: String::new(), reason: "最終回答を受け取れませんでした".into() };
            };
            match pd::check_draft(raw, cite, memo) {
                pd::Checked::Unreadable { reason } => DraftState::Unreadable { raw: raw.to_string(), reason },
                c @ pd::Checked::Ready { .. } => {
                    let review_count = c.review_count();
                    let pd::Checked::Ready { proposals, omitted, dropped } = c else { unreachable!() };
                    DraftState::Ready { proposals, omitted, dropped, review_count }
                }
            }
        }
    }
}

/// 保存してあった結果を読み直したときの状態。再起動後は一時 thread が残っていないので、作成中だったものは「結果は不明」にする。
pub(super) fn demote_loaded(mut v: PurposeDraftView) -> PurposeDraftView {
    v.state = after_disconnect(&v.state);
    v
}

/// 提案の出典になった往復の抜粋だけを残す（画面へ運ぶ量を抑える）。
pub(super) fn cited_glances(all: &[RoundGlance], state: &DraftState) -> Vec<RoundGlance> {
    let DraftState::Ready { proposals, .. } = state else { return Vec::new() };
    let cited = pd::cited_numbers(proposals);
    all.iter().filter(|g| cited.contains(&g.number)).cloned().collect()
}

/// 組み立てた入力（資料・指示文・出典番号の集合・抜粋）。
pub(super) struct Assembled {
    pub material: cont::Material,
    pub prompt: String,
    pub cite: pd::CiteSet,
    pub glances: Vec<RoundGlance>,
}

/// 連鎖の turn と台帳の扱いから、送る指示文を組み立てる。同じ入力なら同じ文字列。
/// 番号は `Global`＝台帳の通し番号、`HeadOnly`（台帳の扱いが `Incomplete`）＝資料内の連番。
pub(super) fn assemble(turns: &[TurnRecord], note: &cont::LedgerNote, mode: DraftMode, rev: Option<u32>, purpose: &str, utc_offset_min: i32) -> Assembled {
    let material = cont::build_material_with(turns, pd::DRAFT_ROUNDS, utc_offset_min, note);
    let ledger_written = matches!(note, cont::LedgerNote::Path(_));
    let cite = pd::CiteSet::new(mode, material.rounds_available, material.rounds_included, ledger_written && material.ledger);
    let glances = cont::round_glances(turns, pd::DRAFT_ROUNDS, utc_offset_min, mode == DraftMode::Global);
    let prompt = pd::build_prompt(rev, purpose, &material.text, mode);
    Assembled { material, prompt, cite, glances }
}

/// 終端のない（実行中の）依頼を含む往復があるか。あれば「実行中の往復は含めません」と示す。
fn has_running_round(turns: &[TurnRecord]) -> bool {
    turns.iter().any(|t| t.end.is_none() && t.entries.iter().any(|e| e.kind == ActivityKind::UserMessage))
}

fn thread_key(agent: &AgentKey) -> ChatKey {
    ChatKey { backend: agent.backend, id: agent.id.clone() }
}

// ───────────────────────────── 作業状態 ─────────────────────────────

/// 1つの会話の下書き（メモリ上）。
struct DraftLive {
    view: PurposeDraftView,
    /// 一時 thread（開始できたら）。
    thread: Option<ChatKey>,
    /// 最後に確定したエージェント発言（構造化出力の最終回答）。
    last_text: Option<String>,
    buffers: HashMap<ExternalId, String>,
    clipped: HashSet<ExternalId>,
    cite: pd::CiteSet,
    /// 押した時点の目的メモの本文（置換対象の検査に使う）。
    memo: String,
    glances: Vec<RoundGlance>,
}

/// 下書きの作業状態（会話本体の先頭チャットごと）。再起動で消えてよい（最後の結果は `last.json`）。
#[derive(Default)]
pub struct DraftRuntime {
    live: Mutex<HashMap<ChatKey, DraftLive>>,
}

/// 材料集めの結果。
struct Gathered {
    turns: Vec<TurnRecord>,
    note: cont::LedgerNote,
    /// 書き出す台帳（書けるときだけ）。
    ledger: Option<cont::Ledger>,
    mode: DraftMode,
    /// 以前の thread を読めなかった理由（`HeadOnly` へ切り替える選択肢を出す）。
    incomplete: Option<String>,
    excluded_running: bool,
}

impl Host {
    fn draft_ctx(&self, chat: &ChatKey) -> pd::DraftCtx {
        let org_owned = self.check_not_org_owned(chat).is_err();
        let deleting = self.manage_rt.is_deleting(chat);
        let inflight = self.draft_rt.live.lock().unwrap().get(chat).is_some_and(|l| is_active(&l.view.state));
        let (superseded, delete_pending, enabled) =
            self.read(|d| (d.locals.get(chat).is_some_and(|l| l.superseded_by.is_some()), d.locals.get(chat).is_some_and(|l| l.delete_pending.is_some()), d.settings.continuation.enabled));
        pd::DraftCtx {
            connected: self.is_connected(),
            org_owned,
            superseded,
            delete_pending: delete_pending || deleting,
            has_store: self.persist.store().is_some(),
            structured_output: self.backend.capabilities().structured_output == Support::Supported,
            inflight,
            enabled,
        }
    }

    fn draft_purpose(&self, chat: &ChatKey) -> (Option<u32>, String) {
        self.read(|d| {
            let p = d.locals.get(chat).and_then(|l| l.conversation.as_ref()).and_then(|c| c.purpose.as_ref());
            (p.map(|p| p.revision), p.map(|p| p.text.clone()).unwrap_or_default())
        })
    }

    /// 材料を集める（履歴の読取りだけ。書込み・送信・resume をしない）。`head_only` は利用者が「今の thread だけで作る」を選んだとき。
    async fn gather_draft(self: &Arc<Self>, chat: &ChatKey, head_only: bool) -> Result<Gathered, IpcError> {
        let history = self
            .read_history(agent_key_of(chat), ReadOptions { include_turns: true })
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("履歴を取得できないため、更新案を作れません（{e}）")))?;
        let excluded_running = has_running_round(&history.turns);
        if head_only {
            return Ok(Gathered { turns: history.turns, note: cont::LedgerNote::Incomplete("今の thread だけで作成します".into()), ledger: None, mode: DraftMode::HeadOnly, incomplete: None, excluded_running });
        }
        let earlier = self.read(|d| d.locals.get(chat).and_then(|l| l.conversation.as_ref()).map(|c| c.threads.clone()).unwrap_or_default());
        let chain = self.chain_material(chat, &earlier, history.turns).await;
        Ok(match chain.note {
            cont::LedgerNote::Incomplete(why) => Gathered { turns: chain.turns, note: cont::LedgerNote::Incomplete(why.clone()), ledger: None, mode: DraftMode::HeadOnly, incomplete: Some(why), excluded_running },
            cont::LedgerNote::Path(_) => {
                // 引き継ぎ用の台帳とは別の、下書き用の台帳を案内する。
                let path = self.persist.store().map(|s| s.draft_ledger_path(chat).to_string_lossy().into_owned());
                match path {
                    Some(p) => Gathered { turns: chain.turns, note: cont::LedgerNote::Path(p), ledger: chain.ledger.map(|(_, l)| l), mode: DraftMode::Global, incomplete: None, excluded_running },
                    None => Gathered { turns: chain.turns, note: cont::LedgerNote::WriteFailed("保存先を使えません".into()), ledger: None, mode: DraftMode::Global, incomplete: None, excluded_running },
                }
            }
            other => Gathered { turns: chain.turns, note: other, ledger: None, mode: DraftMode::Global, incomplete: None, excluded_running },
        })
    }

    /// 作成前の見積り（読取りのみ。何も作らず・送らない）。
    pub async fn preview_purpose_draft(self: &Arc<Self>, args: StartPurposeDraftArgs) -> Result<PurposeDraftEstimate, IpcError> {
        let chat = args.chat;
        if self.read(|d| d.chat(&chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let mut blockers: Vec<String> = pd::draft_blocker(&self.draft_ctx(&chat)).into_iter().collect();
        let (rev, purpose) = self.draft_purpose(&chat);
        let mut est = PurposeDraftEstimate { blockers: Vec::new(), head_only_available: false, rounds_included: 0, rounds_available: 0, toc_rounds: 0, input_chars: 0, purpose_revision: rev, excluded_running: false, notes: Vec::new() };
        if !self.is_connected() {
            est.blockers = blockers;
            return Ok(est);
        }
        let g = self.gather_draft(&chat, args.head_only).await?;
        if let Some(why) = &g.incomplete {
            blockers.push(format!("以前の thread を読めないため、全体の台帳を作れません（{why}）。今の thread だけで作ることはできます（出典は [今#n] になります）"));
            est.head_only_available = true;
        }
        let a = assemble(&g.turns, &g.note, g.mode, rev, &purpose, crate::win::clock::local_utc_offset_min());
        if a.material.rounds_available == 0 {
            blockers.push("往復がまだないため、更新案を作れません".into());
        }
        if let cont::LedgerNote::WriteFailed(why) = &g.note {
            est.notes.push(format!("会話台帳を書けません（{why}）。目次だけの往復は根拠を確かめられません"));
        }
        est.blockers = blockers;
        est.rounds_included = a.material.rounds_included;
        est.rounds_available = a.material.rounds_available;
        est.toc_rounds = a.material.toc_rounds.unwrap_or(0);
        est.input_chars = a.prompt.chars().count().min(u32::MAX as usize) as u32;
        est.excluded_running = g.excluded_running;
        Ok(est)
    }

    // ───────────── 状態の更新・保存 ─────────────

    fn emit_draft(&self, chat: &ChatKey) {
        self.mutate(|_| ((), vec![HostEvent::PurposeDraftUpdated { chat: chat.clone() }]));
    }

    /// 状態の変更を通知し、最後の結果として保存する（別 task。作成中の準備段階は保存しない。失敗しても画面の状態は残す）。
    fn publish_draft(self: &Arc<Self>, chat: &ChatKey) {
        self.emit_draft(chat);
        let view = self.draft_rt.live.lock().unwrap().get(chat).map(|l| l.view.clone());
        let Some(view) = view.filter(|v| v.state != DraftState::Preparing) else { return };
        let Some(store) = self.persist.store().cloned() else { return };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&view) else { return };
        let host = self.clone();
        let chat = chat.clone();
        tokio::task::spawn_blocking(move || {
            let res = {
                let _g = host.persist.io_guard();
                store.write_draft_last(&chat, &bytes)
            };
            if let Err(e) = res {
                host.warn(format!("更新案の結果を保存できませんでした（画面には残っています）: {}", super::persist::save_failure_message(&e)));
            }
        });
    }

    fn with_draft<R>(&self, chat: &ChatKey, f: impl FnOnce(&mut DraftLive) -> R) -> Option<R> {
        self.draft_rt.live.lock().unwrap().get_mut(chat).map(f)
    }

    /// 一時 thread の状態を一覧外の登録から外す（一覧には元から出ていない）。
    fn retire_draft_thread(&self, thread: &ChatKey) {
        self.mutate(|d| {
            d.forget_side_thread(thread);
            ((), vec![])
        });
    }

    async fn write_draft_ledger_file(self: &Arc<Self>, chat: &ChatKey, text: String) -> Result<(), String> {
        let Some(store) = self.persist.store().cloned() else { return Err("保存先を使えません".into()) };
        let host = self.clone();
        let chat = chat.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let _g = host.persist.io_guard();
            store.write_draft_ledger(&chat, &text)
        })
        .await
        .map_err(|e| format!("書込みに失敗しました: {e}"))?;
        saved.map(|_| ()).map_err(|e| super::persist::save_failure_message(&e))
    }

    // ───────────── 作成 ─────────────

    /// 更新案を作る（利用者が押したときの1回だけ）。一時 thread を新しく開始して、指示文を1回だけ送る。
    pub async fn start_purpose_draft(self: &Arc<Self>, args: StartPurposeDraftArgs, _confirmed: &UserConfirmed) -> Result<PurposeDraftView, IpcError> {
        let chat = args.chat.clone();
        self.check_not_org_owned(&chat)?;
        let Some((kind, cwd)) = self.read(|d| d.chat(&chat).map(|c| (c.kind, c.cwd.value().cloned()))) else {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        };
        if let Some(m) = pd::draft_blocker(&self.draft_ctx(&chat)) {
            return Err(err(IpcErrorCode::InvalidArgs, m));
        }
        let (rev, purpose) = self.draft_purpose(&chat);
        if rev != args.expected_revision {
            return Err(err(IpcErrorCode::InvalidArgs, "目的メモが画面の表示と違います。開き直してから作ってください"));
        }
        let Some(cwd) = cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットの作業フォルダを確認できないため、更新案を作れません"));
        };
        let id = self.local_id("draft");
        let started_view = PurposeDraftView {
            id: id.clone(),
            chat: chat.clone(),
            base_revision: rev,
            mode: if args.head_only { DraftMode::HeadOnly } else { DraftMode::Global },
            input_chars: 0,
            rounds_included: 0,
            rounds_available: 0,
            toc_rounds: 0,
            excluded_running: false,
            usage_tokens: None,
            state: DraftState::Preparing,
            glances: Vec::new(),
        };
        // 作成中の印を先に置く（同じ会話の2本目を通さない）。前回の結果の一時 thread は片付ける。
        let previous = {
            let mut g = self.draft_rt.live.lock().unwrap();
            if g.get(&chat).is_some_and(|l| is_active(&l.view.state)) {
                return Err(err(IpcErrorCode::InvalidArgs, "このチャットの更新案を作成中です。結果が出るまでお待ちください"));
            }
            let prev = g.remove(&chat).and_then(|l| l.thread);
            g.insert(
                chat.clone(),
                DraftLive {
                    view: started_view,
                    thread: None,
                    last_text: None,
                    buffers: HashMap::new(),
                    clipped: HashSet::new(),
                    cite: pd::CiteSet { total: 0, first_raw: 1, ledger: false },
                    memo: purpose.clone(),
                    glances: Vec::new(),
                },
            );
            prev
        };
        if let Some(t) = previous {
            self.retire_draft_thread(&t);
        }
        // 材料集め以降の「始められない」は何も作らずに印を外して返す。
        let abort = |host: &Arc<Host>, e: IpcError| -> IpcError {
            host.draft_rt.live.lock().unwrap().remove(&chat);
            host.emit_draft(&chat);
            e
        };
        let g = match self.gather_draft(&chat, args.head_only).await {
            Ok(g) => g,
            Err(e) => return Err(abort(self, e)),
        };
        if let (Some(why), false) = (&g.incomplete, args.head_only) {
            let e = err(IpcErrorCode::InvalidArgs, format!("以前の thread を読めないため、更新案を作れません（{why}）。「今の thread だけで作る」を選ぶと、今の thread の往復だけで作れます"));
            return Err(abort(self, e));
        }
        let mut note = g.note.clone();
        if let Some(ledger) = &g.ledger {
            // 下書き用の台帳は一時 thread へ送る前に書く。書けなくても止めず、目次だけの往復は根拠を確かめられない扱いにする。
            if let Err(why) = self.write_draft_ledger_file(&chat, ledger.text.clone()).await {
                note = cont::LedgerNote::WriteFailed(why);
            }
        }
        let a = assemble(&g.turns, &note, g.mode, rev, &purpose, crate::win::clock::local_utc_offset_min());
        if a.material.rounds_available == 0 {
            return Err(abort(self, err(IpcErrorCode::InvalidArgs, "往復がまだないため、更新案を作れません")));
        }
        if let Err(e) = self.precheck_space() {
            return Err(abort(self, e));
        }
        let input_chars = a.prompt.chars().count().min(u32::MAX as usize) as u32;
        self.with_draft(&chat, |l| {
            l.cite = a.cite;
            l.glances = a.glances.clone();
            l.view.mode = g.mode;
            l.view.input_chars = input_chars;
            l.view.rounds_included = a.material.rounds_included;
            l.view.rounds_available = a.material.rounds_available;
            l.view.toc_rounds = a.material.toc_rounds.unwrap_or(0);
            l.view.excluded_running = g.excluded_running;
        });
        // 一時 thread を開始する（会話と同じ作業フォルダ・モデル。読取り専用・承認なし・保存されない）。
        let model = self.read(|d| d.model_settings.get(&chat).and_then(|s| s.accepted.value().cloned()));
        let params = StartChatParams { kind, cwd, model: model.clone(), permission: PermissionPreset::ReadOnlyNoPrompt, extra_folders: Vec::new(), ephemeral: true };
        let started = match self.backend.start_chat(params).await {
            Ok(s) => s,
            Err(e) => {
                // 一時 thread を作れたか確認できないときは、その旨を添える（作れていても一覧には出ない。再送はしない）。
                let unknown = matches!(e, BackendError::OutcomeUnknown { .. } | BackendError::Io { .. } | BackendError::Protocol { .. });
                let suffix = if unknown { "（一時 thread を作れたか確認できません。作れていても一覧には出ません）" } else { "" };
                self.with_draft(&chat, |l| l.view.state = DraftState::Rejected { message: format!("一時 thread を開始できませんでした: {e}{suffix}") });
                self.publish_draft(&chat);
                return Ok(self.draft_view_of(&chat));
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
        // 一覧には出さない会話として登録する（状態の追跡・承認・停止の確認のため、エージェントとしては持つ）。
        self.mutate(|d| {
            d.side_threads.insert(thread.clone(), chat.clone());
            ((), d.set_live(started.root.clone(), status))
        });
        self.with_draft(&chat, |l| l.thread = Some(thread.clone()));
        let request = SendRequest {
            chat: thread.clone(),
            mode: SendMode::NewTurn,
            text: a.prompt,
            attachments: Vec::new(),
            speed_tier: model.as_ref().and_then(|m| m.speed_tier.clone()),
            model,
            permission: None,
            cwd: None,
            work_folders: None,
            work_mode: None,
            client_message_id: self.local_id("cm").0,
            output_schema: Some(pd::draft_output_schema()),
        };
        match self.backend.send(request).await {
            SendOutcome::Accepted { .. } => {
                // 受理の応答より先に終端が観測されていたら、その結果を上書きしない。
                self.with_draft(&chat, |l| {
                    if l.view.state == DraftState::Preparing {
                        l.view.state = DraftState::Running { started_at: now_ms() };
                    }
                });
            }
            SendOutcome::Rejected { error, .. } => {
                self.with_draft(&chat, |l| {
                    if l.view.state == DraftState::Preparing {
                        l.view.state = DraftState::Rejected { message: format!("作成を始められませんでした（{error}）") };
                    }
                });
                self.retire_draft_thread(&thread);
            }
            SendOutcome::AcceptanceUnknown(_) => {
                self.with_draft(&chat, |l| {
                    if l.view.state == DraftState::Preparing {
                        l.view.state = DraftState::AcceptanceUnknown;
                    }
                });
            }
        }
        self.publish_draft(&chat);
        Ok(self.draft_view_of(&chat))
    }

    fn draft_view_of(&self, chat: &ChatKey) -> PurposeDraftView {
        self.draft_rt.live.lock().unwrap().get(chat).map(|l| l.view.clone()).expect("下書きの作業状態")
    }

    // ───────────── 読む・中止・破棄 ─────────────

    /// 最後の結果を返す（メモリ上のもの、なければ保存してあるもの。なければ None）。保存から読んだ作成中のものは「結果は不明」にする。
    pub async fn read_purpose_draft(self: &Arc<Self>, args: ChatArgs) -> Result<Option<PurposeDraftView>, IpcError> {
        let chat = args.chat;
        if let Some(l) = self.draft_rt.live.lock().unwrap().get(&chat) {
            return Ok(Some(l.view.clone()));
        }
        let Some(store) = self.persist.store().cloned() else { return Ok(None) };
        let key = chat.clone();
        let bytes = match tokio::task::spawn_blocking(move || store.read_draft_last(&key)).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => return Err(err(IpcErrorCode::Io, super::persist::save_failure_message(&e))),
            Err(e) => return Err(err(IpcErrorCode::Io, e.to_string())),
        };
        // 読めない・別のチャットの記録は、結果なしとして扱う（推測で補わない）。
        Ok(bytes.and_then(|b| serde_json::from_slice::<PurposeDraftView>(&b).ok()).filter(|v| v.chat == chat).map(demote_loaded))
    }

    /// 作成の中止を要求する（会話本体ではなく一時 thread の中断）。停止の確認は既存の停止記録が続ける。
    pub async fn cancel_purpose_draft(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<PurposeDraftView, IpcError> {
        let chat = args.chat;
        let found = self.draft_rt.live.lock().unwrap().get(&chat).map(|l| (l.thread.clone(), l.view.state.clone()));
        let Some((thread, state)) = found else { return Err(err(IpcErrorCode::NotFound, "作成中の更新案がありません")) };
        let Some(thread) = thread.filter(|_| is_active(&state) && state != DraftState::Preparing) else {
            return Err(err(IpcErrorCode::InvalidArgs, "中止できる状態ではありません"));
        };
        self.interrupt_chat(InterruptChatArgs { chat: thread }, confirmed).await?;
        Ok(self.draft_view_of(&chat))
    }

    /// 結果を破棄する（保存してある最後の結果も消す）。作成中は破棄できない（先に中止する）。
    pub async fn discard_purpose_draft(self: &Arc<Self>, args: ChatArgs) -> Result<(), IpcError> {
        let chat = args.chat;
        let thread = {
            let mut g = self.draft_rt.live.lock().unwrap();
            if g.get(&chat).is_some_and(|l| matches!(l.view.state, DraftState::Preparing | DraftState::Running { .. })) {
                return Err(err(IpcErrorCode::InvalidArgs, "作成中です。先に中止してください"));
            }
            g.remove(&chat).and_then(|l| l.thread)
        };
        if let Some(t) = thread {
            self.retire_draft_thread(&t);
        }
        if let Some(store) = self.persist.store().cloned() {
            let host = self.clone();
            let key = chat.clone();
            let res = tokio::task::spawn_blocking(move || {
                let _g = host.persist.io_guard();
                store.remove_draft_last(&key)
            })
            .await;
            if let Ok(Err(e)) = res {
                self.warn(format!("更新案の保存済みの結果を消せませんでした: {}", super::persist::save_failure_message(&e)));
            }
        }
        self.emit_draft(&chat);
        Ok(())
    }

    // ───────────── イベントの観測 ─────────────

    /// 一時 thread のイベントを観測する（状態の判定には使わない）。確定した最終回答を集め、turn の終端で検査する。
    /// 接続が切れたら「結果は不明」にする（完了・失敗にしない）。待たない・送らない。
    pub(super) fn draft_observe(self: &Arc<Self>, ev: &BackendEvent) {
        if self.draft_rt.live.lock().unwrap().is_empty() {
            return;
        }
        match ev {
            BackendEvent::ActivityDelta { item, delta } => {
                let t = thread_key(&item.agent);
                let mut g = self.draft_rt.live.lock().unwrap();
                let Some(l) = g.values_mut().find(|l| l.thread.as_ref() == Some(&t)) else { return };
                let buf = l.buffers.entry(item.item_id.clone()).or_default();
                if buf.chars().count() + delta.chars().count() <= BUFFER_MAX_CHARS {
                    buf.push_str(delta);
                } else {
                    l.clipped.insert(item.item_id.clone());
                }
            }
            BackendEvent::Activity { activity } if activity.phase == ActivityPhase::Completed => {
                let t = thread_key(&activity.key.agent);
                let mut g = self.draft_rt.live.lock().unwrap();
                let Some(l) = g.values_mut().find(|l| l.thread.as_ref() == Some(&t)) else { return };
                let buffer = l.buffers.remove(&activity.key.item_id);
                let clipped = l.clipped.remove(&activity.key.item_id);
                if activity.kind != ActivityKind::AgentMessage {
                    return;
                }
                // 切り捨てた逐次本文・要約だけの本文は全文ではない。JSON として読めなければ「読めない」として本文を示す。
                if let Some((text, summary_only)) = finalize_agent_text(buffer.as_deref(), &activity.summary) {
                    if !(clipped && summary_only) {
                        l.last_text = Some(text);
                    }
                }
            }
            BackendEvent::TokenUsage { agent, thread_total, .. } => {
                let t = thread_key(agent);
                let head = {
                    let mut g = self.draft_rt.live.lock().unwrap();
                    let Some((head, l)) = g.iter_mut().find(|(_, l)| l.thread.as_ref() == Some(&t)) else { return };
                    if thread_total.is_none() || l.view.usage_tokens == *thread_total {
                        return;
                    }
                    l.view.usage_tokens = *thread_total;
                    head.clone()
                };
                self.publish_draft(&head);
            }
            BackendEvent::TurnEnded { turn, end, error, .. } => {
                let t = thread_key(&turn.agent);
                let head = {
                    let mut g = self.draft_rt.live.lock().unwrap();
                    let Some((head, l)) = g.iter_mut().find(|(_, l)| l.thread.as_ref() == Some(&t)) else { return };
                    if !is_active(&l.view.state) {
                        return;
                    }
                    let state = finish_state(*end, error.as_deref(), l.last_text.as_deref(), &l.cite, &l.memo);
                    l.view.glances = cited_glances(&l.glances, &state);
                    l.view.state = state;
                    l.buffers.clear();
                    l.clipped.clear();
                    head.clone()
                };
                self.publish_draft(&head);
            }
            BackendEvent::Connection { state: ConnectionState::Disconnected { .. } } => {
                let (heads, threads): (Vec<ChatKey>, Vec<ChatKey>) = {
                    let mut g = self.draft_rt.live.lock().unwrap();
                    let mut heads = Vec::new();
                    let mut threads = Vec::new();
                    for (head, l) in g.iter_mut() {
                        if is_active(&l.view.state) {
                            l.view.state = after_disconnect(&l.view.state);
                            heads.push(head.clone());
                            threads.extend(l.thread.take());
                        }
                    }
                    (heads, threads)
                };
                for t in &threads {
                    self.retire_draft_thread(t);
                }
                for h in &heads {
                    self.publish_draft(h);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::purpose_draft::CiteSet;

    fn agent() -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId("m".into()) }
    }

    fn entry(kind: ActivityKind, text: &str, n: u32) -> TranscriptEntry {
        TranscriptEntry {
            key: ItemKey { agent: agent(), turn_id: Some(ExternalId("t".into())), item_id: ExternalId(format!("i{n}")) },
            kind,
            text: Known::direct(text.to_string()),
            phase: ActivityPhase::Completed,
            message_phase: MessagePhase::default(),
            attachments: Vec::new(),
        }
    }

    fn turn(id: &str, end: Option<TurnEnd>, user: &str, ai: &str) -> TurnRecord {
        TurnRecord {
            key: TurnKey { agent: agent(), turn_id: ExternalId(id.into()) },
            end,
            started_at: Known::direct(UnixMillis(1_700_000_000_000)),
            completed_at: Known::NotFetched,
            entries: vec![entry(ActivityKind::UserMessage, user, 1), entry(ActivityKind::AgentMessage, ai, 2)],
            complete: true,
            failure: Known::NotFetched,
        }
    }

    fn cite() -> CiteSet {
        CiteSet { total: 5, first_raw: 1, ledger: true }
    }

    const OK_JSON: &str = r#"{"items":[{"kind":"decision","action":"add","text":"締切は10/30","replaces":"","sources":[2],"noEvidence":false,"note":""}],"omitted":""}"#;

    #[test]
    fn a_completed_turn_is_checked_and_stays_pending_confirmation() {
        let s = finish_state(TurnEnd::Completed, None, Some(OK_JSON), &cite(), "");
        let DraftState::Ready { proposals, review_count, dropped, .. } = s else { panic!("{s:?}") };
        assert_eq!((proposals.len(), review_count, dropped), (1, 0, 0));
        // 存在しない出典は要確認として数える（成功扱いにしない）
        let bad = OK_JSON.replace("[2]", "[9]");
        let DraftState::Ready { review_count, .. } = finish_state(TurnEnd::Completed, None, Some(&bad), &cite(), "") else { panic!() };
        assert_eq!(review_count, 1);
    }

    #[test]
    fn unreadable_or_missing_output_is_shown_as_unreadable_not_empty_or_failed() {
        let s = finish_state(TurnEnd::Completed, None, Some("文章で返ってきた"), &cite(), "");
        assert!(matches!(&s, DraftState::Unreadable { raw, .. } if raw == "文章で返ってきた"), "{s:?}");
        for none in [None, Some("  \n")] {
            assert!(matches!(finish_state(TurnEnd::Completed, None, none, &cite(), ""), DraftState::Unreadable { .. }));
        }
    }

    #[test]
    fn interrupted_and_failed_turns_are_distinct_states() {
        assert_eq!(finish_state(TurnEnd::Interrupted, None, Some(OK_JSON), &cite(), ""), DraftState::Interrupted);
        assert_eq!(finish_state(TurnEnd::Failed, Some("上限に達しました"), None, &cite(), ""), DraftState::Failed { message: "上限に達しました".into() });
        assert!(matches!(finish_state(TurnEnd::Failed, None, None, &cite(), ""), DraftState::Failed { message } if message.contains("報告されていません")));
    }

    #[test]
    fn a_disconnect_makes_active_drafts_unknown_and_leaves_finished_ones() {
        for s in [DraftState::Preparing, DraftState::Running { started_at: UnixMillis(1) }, DraftState::AcceptanceUnknown] {
            assert!(is_active(&s));
            assert_eq!(after_disconnect(&s), DraftState::Lost, "never done/failed");
        }
        for s in [DraftState::Interrupted, DraftState::Lost, DraftState::Failed { message: "x".into() }, DraftState::Rejected { message: "y".into() }] {
            assert!(!is_active(&s));
            assert_eq!(after_disconnect(&s), s);
        }
    }

    #[test]
    fn a_restored_running_draft_is_shown_as_lost() {
        let v = PurposeDraftView {
            id: LocalId("d".into()),
            chat: ChatKey { backend: BackendKind::Codex, id: ExternalId("c".into()) },
            base_revision: Some(2),
            mode: DraftMode::Global,
            input_chars: 10,
            rounds_included: 1,
            rounds_available: 1,
            toc_rounds: 1,
            excluded_running: false,
            usage_tokens: None,
            state: DraftState::Running { started_at: UnixMillis(1) },
            glances: Vec::new(),
        };
        assert_eq!(demote_loaded(v.clone()).state, DraftState::Lost);
        let ready = PurposeDraftView { state: DraftState::Ready { proposals: vec![], omitted: String::new(), dropped: 0, review_count: 0 }, ..v };
        assert!(matches!(demote_loaded(ready).state, DraftState::Ready { .. }));
    }

    #[test]
    fn assemble_numbers_by_ledger_or_by_the_material_and_is_deterministic() {
        let turns: Vec<TurnRecord> = (1..=12).map(|i| turn(&format!("t{i}"), Some(TurnEnd::Completed), &format!("依頼{i}"), &format!("応答{i}"))).collect();
        let note = cont::LedgerNote::Path(r"C:\x\purpose_draft\会話台帳.md".into());
        let a = assemble(&turns, &note, DraftMode::Global, Some(3), "目的", 0);
        assert_eq!(a.prompt, assemble(&turns, &note, DraftMode::Global, Some(3), "目的", 0).prompt);
        assert_eq!((a.material.rounds_available, a.material.rounds_included), (12, 10));
        assert_eq!(a.cite, CiteSet { total: 12, first_raw: 3, ledger: true });
        assert_eq!(a.glances.first().map(|g| g.number), Some(1), "global numbers cover every round");
        assert_eq!(a.glances.len(), 12);
        assert!(a.prompt.contains("往復12（") && a.prompt.contains("#1 ") && a.prompt.contains(r"C:\x\purpose_draft\会話台帳.md"));
        // 台帳を書けなかったときは、目次だけの往復の根拠を確かめられない
        let failed = assemble(&turns, &cont::LedgerNote::WriteFailed("x".into()), DraftMode::Global, None, "", 0);
        assert_eq!(failed.cite, CiteSet { total: 12, first_raw: 3, ledger: false });
        // 今の thread だけ: 資料内の連番（直近10往復が1..=10）
        let head = assemble(&turns, &cont::LedgerNote::Incomplete("x".into()), DraftMode::HeadOnly, None, "", 0);
        assert_eq!(head.cite, CiteSet { total: 10, first_raw: 1, ledger: false });
        assert_eq!((head.glances.first().map(|g| g.number), head.glances.len()), (Some(1), 10));
        assert!(head.glances[0].user_head.contains("依頼3"), "numbering restarts at the first included round");
    }

    #[test]
    fn only_cited_rounds_travel_with_a_ready_draft() {
        let all: Vec<RoundGlance> = (1..=5).map(|n| RoundGlance { number: n, time: None, user_head: format!("u{n}"), ai_head: format!("a{n}") }).collect();
        let DraftState::Ready { .. } = finish_state(TurnEnd::Completed, None, Some(OK_JSON), &cite(), "") else { panic!() };
        let state = finish_state(TurnEnd::Completed, None, Some(OK_JSON), &cite(), "");
        assert_eq!(cited_glances(&all, &state).iter().map(|g| g.number).collect::<Vec<_>>(), vec![2]);
        assert!(cited_glances(&all, &DraftState::Lost).is_empty());
    }

    #[test]
    fn a_running_round_is_detected_so_it_can_be_left_out_and_said_so() {
        let done = turn("t1", Some(TurnEnd::Completed), "a", "b");
        let running = turn("t2", None, "c", "");
        assert!(!has_running_round(&[done.clone()]));
        assert!(has_running_round(&[done, running]));
    }
}
