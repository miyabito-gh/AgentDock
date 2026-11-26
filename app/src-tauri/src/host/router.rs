//! 受付係（依頼の入口と振り分け）のホスト側（`app/DESIGN_ROUTER.md` §5〜§9）。Tauriコマンド（`commands.rs`）が呼ぶ `Host` のメソッド。
//!
//! 守ること:
//! - **AIは決めるだけ、実行はアプリ**。受付係の返答（構造化出力）は純関数（`rules::router`）で検査し、通ったものだけを
//!   カード（`Proposed`）にする。自動送信の条件は利用者指定の既存先と分類結果が一致すること。ルール帳の編集は利用者操作で行う。
//! - 受付係への資料の「届いた記録」は、送信が `Accepted` になったときだけ書く。受理不明・拒否・送れなかったときは書かない。
//!   切断・受理不明で自動再送しない（振り分けも `Proposed` のまま、または `Unconfirmed` で止める）。
//! - 利用者の送信と同じ経路（`send_message_with`、`SendAuth::User`）で送る。組織実行の所有チャット・アーカイブ・引き継ぎ済みへは送らない。
//! - ルール帳（`<app_data>/router/rules.json`）は `write_atomic` で書く。読めなければ `rules.json.bad` へ退避し、空から始めて警告を出す。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::backend::backend::{AiBackend, BackendEvent, PermissionPreset, ReadOptions, UserConfirmed};
use crate::backend::ipc::{BlockedReason, ListSkillsArgs, HostEvent, IpcError, IpcErrorCode, SendIntent, SendMessageArgs, StartChatArgs};
use crate::backend::local::{EnqueueArgs, ListVisibility, SaveScope};
use crate::backend::model::*;
use crate::backend::router::*;
use crate::host::state::agent_key_of;
use crate::host::{blocked, err, now_ms, Host, SendExtras};
use crate::host::queue_driver::check_agy_text_length;
use crate::rules::org_intake::facts_of;
use crate::rules::router::*;
use crate::store::atomic::write_atomic;
use crate::store::records::{ConversationRecord, Dispatch, DispatchProposal, DispatchState, DispatchTarget, ExecutionStatus, NewChatProposal, RouterRecord, RouterTurnRecord, RuleNoticeRecord, SentCatalog};

/// 受付係のチャットに付ける名前。
const ROUTER_TITLE: &str = "受付";
const ROUTER_SKILL: &str = "reception-triage";
/// 送り先の解決で引き継ぎ先を辿る上限。
const SUPERSEDE_HOPS: usize = 20;
/// 自動送信の送り先が live でなかったとき、受付の返答に出す理由（「自動送信を保留しました: 」に続ける）。
const AUTO_SEND_NOT_LIVE: &str = "送り先のチャットが今回の起動で再開されていない（または状態を確認できない）ため、自動では送りません。カードの［送る］で送れます（送ると再開してから送ります）";
/// 受理待ちの送信を覚えておく件数の上限（メモリ上だけ）。
const PENDING_MAX: usize = 20;

// ───────────────────────────── 作業状態 ─────────────────────────────

/// 受付係へ送った送信（メモリ上だけ。返答の turn と一覧の対応づけ・利用者の本文の取り出しに使う）。
#[derive(Debug, Clone)]
struct PendingSend {
    chat: ChatKey,
    client_message_id: String,
    /// 送った本文全体（資料＋利用者の本文）。
    full_text: String,
    /// 受理された turn（受理を確認できたとき）。
    turn: Option<String>,
}

#[derive(Default)]
pub struct RouterRuntime {
    /// ルール帳の読み書きの直列化（`rules.json` の読込み→書込みの間）。
    rules_lock: Mutex<()>,
    /// ルール帳を読めずに退避したときの警告（このプロセスの間は残す）。
    warning: Mutex<Option<String>>,
    pending: Mutex<Vec<PendingSend>>,
    /// 返答を処理中の（受付係, turn）。二重処理を防ぐ。
    processing: Mutex<HashSet<(ChatKey, String)>>,
    /// 送信処理中の振り分け（カードの二重押下を防ぐ）。
    dispatching: Mutex<HashSet<LocalId>>,
    creating: Mutex<bool>,
}

struct CreatingGuard<'a>(&'a RouterRuntime);

impl Drop for CreatingGuard<'_> {
    fn drop(&mut self) {
        *self.0.creating.lock().unwrap() = false;
    }
}

struct DispatchGuard<'a> {
    rt: &'a RouterRuntime,
    id: LocalId,
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        self.rt.dispatching.lock().unwrap().remove(&self.id);
    }
}

struct ProcessingGuard<'a> {
    rt: &'a RouterRuntime,
    key: (ChatKey, String),
}

impl Drop for ProcessingGuard<'_> {
    fn drop(&mut self) {
        self.rt.processing.lock().unwrap().remove(&self.key);
    }
}

// ───────────────────────────── 純粋な部分 ─────────────────────────────

/// 送信が受理されたとき、資料に含めた節を「届いた記録」へ反映する（§5.3）。受理以外（受理不明・拒否・未確定）は何も変えない。
/// 反映したときだけ true。
pub(super) fn apply_delivery(rec: &mut RouterRecord, inc: &EnvelopeSections, composed_epoch: u32, state: &SendState) -> bool {
    if !matches!(state, SendState::Accepted { .. }) {
        return false;
    }
    if let Some(rev) = inc.instructions {
        rec.instructions_delivered_rev = Some(rev);
        // 圧縮を観測したあとは指示・ルール・一覧を全部添え直すので、この送信で印を外す。
        // ただし資料を組み立てた後に新しい圧縮を観測していたら、その分は届いていないので印を残す。
        if rec.compaction_epoch == composed_epoch {
            rec.compacted_since_delivery = false;
        }
    }
    if let Some(rev) = inc.rules {
        rec.rules_delivered_rev = Some(rev);
    }
    if let Some(d) = &inc.catalog_digest {
        rec.catalog_delivered_digest = Some(d.clone());
    }
    for d in rec.dispatches.iter_mut().filter(|d| inc.reported.contains(&d.id)) {
        d.reported = true;
    }
    true
}

/// 振り分けの送信の結果。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum DispatchOutcome {
    /// 受理を確認した。
    Sent(DispatchState),
    /// 受理不明（送れたか確認できない）。自動再送しない。
    Unconfirmed(DispatchState),
    /// 送っていない（受理されなかった）。カードは「提案」のまま。
    NotSent(String),
}

pub(super) fn dispatch_outcome(state: &SendState, client_message_id: &str, at: UnixMillis) -> DispatchOutcome {
    let cm = || client_message_id.to_string();
    match state {
        SendState::Accepted { .. } => DispatchOutcome::Sent(DispatchState::Sent { at, client_message_id: cm() }),
        SendState::Rejected { message } => DispatchOutcome::NotSent(format!("送り先に受理されませんでした: {message}")),
        SendState::NotFoundAfterReconcile => DispatchOutcome::NotSent("送り先に届いた形跡がありません".into()),
        // 確定していない（送信中）も、受理不明と同じく「確認できない」として扱う。
        SendState::AcceptanceUnknown { .. } | SendState::Sending => DispatchOutcome::Unconfirmed(DispatchState::Unconfirmed { at, client_message_id: cm() }),
    }
}

/// 返答の解析・検査の結果。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Interpreted {
    pub dispatches: Vec<Dispatch>,
    pub route_rejects: Vec<String>,
    /// ルール帳を書き換えたときの新しい版。
    pub rules: Option<RulesFile>,
    pub notices: Vec<RuleNoticeRecord>,
}

fn confidence_str(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

/// 解析済みの返答から、カード（`Proposed`）とルール帳の更新を作る（§6.2・§6.3）。書込み・送信はしない。
/// 振り分け案は `sent`（その送信の一覧）で検査し、ルール変更は利用者の本文（資料を除く）の引用と照合する。
/// ルール変更は順に適用し、1つ落ちても他は進める（落ちた分は「ルールは更新していません（理由）」）。
#[allow(clippy::too_many_arguments)]
pub(super) fn interpret_reply(
    reply: &RouterReply,
    source_turn: &str,
    sent: &SentCatalog,
    user_text: &str,
    rules: &RulesFile,
    state_of: &dyn Fn(&ChatKey) -> ChatRouteState,
    title_of: &dyn Fn(&ChatKey) -> String,
    folders_with_existing_chat: &[String],
    new_id: &mut dyn FnMut() -> LocalId,
    now: UnixMillis,
) -> Interpreted {
    let mut dispatches = Vec::new();
    let mut route_rejects = Vec::new();
    for (i, r) in reply.routes.iter().enumerate() {
        match check_route(r, sent, state_of) {
            Ok(v) => {
                let (target, new_chat) = match &v.target {
                    RouteTarget::Existing { chat } => (DispatchTarget::Existing { chat: chat.clone() }, None),
                    RouteTarget::New { title, folder, purpose, provider } => (DispatchTarget::New { chat: None }, Some(NewChatProposal { title: title.clone(), folder: folder.clone(), purpose: purpose.clone(), provider: *provider })),
                };
                dispatches.push(Dispatch {
                    id: new_id(),
                    source_turn: source_turn.to_string(),
                    route_index: i as u32,
                    target: target.clone(),
                    suggested: target,
                    state: DispatchState::Proposed,
                    reported: false,
                    proposal: Some(DispatchProposal { text: v.text, text_is_original: v.text_is_original, reason: v.reason, confidence: confidence_str(v.confidence).to_string(), new_chat }),
                });
            }
            Err(e) => route_rejects.push(format!("振り分け案{}を出しませんでした（{}）", i + 1, e.message())),
        }
    }

    let mut current = rules.clone();
    let mut notices = Vec::new();
    for c in &reply.rule_changes {
        match check_rule_change(c, user_text, &current, sent, folders_with_existing_chat) {
            Ok(v) => {
                let next = apply_rule_change(&current, &v, Some(source_turn), title_of, now);
                // 書いた変更の要約（履歴の先頭がこの変更）。
                let (revision, summary) = next.history.first().map(|h| (Some(h.revision), change_summary(h))).unwrap_or((None, String::new()));
                notices.push(RuleNoticeRecord { revision, text: summary });
                current = next;
            }
            Err(e) => notices.push(RuleNoticeRecord { revision: None, text: format!("ルールは更新していません（{}）", e.message()) }),
        }
    }
    let rules = (current.revision != rules.revision).then_some(current);
    Interpreted { dispatches, route_rejects, rules, notices }
}

/// ルール帳を書けなかったとき、「更新した」と見せた通知を「更新していません」へ直す。
pub(super) fn downgrade_notices(notices: &mut [RuleNoticeRecord], reason: &str) {
    for n in notices.iter_mut().filter(|n| n.revision.is_some()) {
        n.revision = None;
        n.text = format!("ルールは更新していません（{reason}）");
    }
}

/// 自動送信の判断（利用者が本文の先頭行で `自動送信: <ID>` と指定したときだけ意味を持つ）。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum AutoSend {
    /// 自動送信の指定なし。
    NotRequested,
    /// 条件をすべて満たした（振り分けID・送り先・送る本文）。
    Send { id: LocalId, target: ChatKey, text: String },
    /// 指定されたが条件を満たさない（理由。受付の返答の `route_rejects` へ出す）。
    Skip(&'static str),
}

/// 自動送信の条件（利用者が先頭行で既存の送り先を指定し、分類が単一案・ID一致・確信度 high・整えた文、返答の処理が送信時の照合を伴うこと）を判断する。
/// 指定があるのに満たさないときは理由を返す（返さないと、カードが提案のまま残るだけで、なぜ自動で送られないのか利用者に分からない）。
pub(super) fn auto_send_decision(user_text: &str, allow_auto: bool, is_head: bool, dispatches: &[Dispatch], route_rejects_empty: bool, rule_changes_empty: bool) -> AutoSend {
    let Some(requested) = automatic_target_id(user_text) else { return AutoSend::NotRequested };
    if !allow_auto {
        return AutoSend::Skip("この返答は送信時の記録と照合できない経路（再起動後の取り込みなど）で受け取ったため");
    }
    if !is_head {
        return AutoSend::Skip("この受付係は今の受付ではない（アーカイブまたは引き継ぎ済み）ため");
    }
    let [d] = dispatches else {
        return AutoSend::Skip(if dispatches.is_empty() { "振り分け案が出なかったため" } else { "振り分け案が複数あるため" });
    };
    if !route_rejects_empty {
        return AutoSend::Skip("出せなかった案があるため");
    }
    if !rule_changes_empty {
        return AutoSend::Skip("ルールの変更を含む返答のため");
    }
    let (DispatchTarget::Existing { chat: target }, Some(p)) = (&d.target, &d.proposal) else {
        return AutoSend::Skip("送り先が既存のチャットではない（新規チャットの案）ため");
    };
    if requested != target.id.0.as_str() {
        return AutoSend::Skip("指定したチャットIDと、分類が選んだ送り先が一致しないため");
    }
    if p.confidence != "high" {
        return AutoSend::Skip("分類の確信度が high ではないため");
    }
    if p.text_is_original {
        return AutoSend::Skip("本文が整えられていない（原文のまま）ため");
    }
    AutoSend::Send { id: d.id.clone(), target: target.clone(), text: p.text.clone() }
}

// ───────────────────────────── Host ─────────────────────────────

impl Host {
    fn router_dir(&self) -> PathBuf {
        self.app_data_dir.join("router")
    }

    fn rules_path(&self) -> PathBuf {
        self.router_dir().join("rules.json")
    }

    fn set_rules_warning(&self, message: String) {
        *self.router_rt.warning.lock().unwrap() = Some(message);
    }

    fn rules_warning(&self) -> Option<String> {
        self.router_rt.warning.lock().unwrap().clone()
    }

    /// ルール帳を読む。読めない・壊れている・新しい版のときは `.bad`（あれば時刻つき）へ退避して空から始め、警告を残す。
    /// 戻り値の bool は書けるか（退避できなかったときは、壊れた中身を上書きしないよう false）。
    fn load_rules_unlocked(&self) -> (RulesFile, bool) {
        let path = self.rules_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (RulesFile::default(), true),
            Err(e) => {
                self.set_rules_warning(format!("ルール帳（rules.json）を読めませんでした: {e}。書込みは止めています"));
                return (RulesFile::default(), false);
            }
        };
        match parse_rules_file(&bytes) {
            Ok(f) => (f, true),
            Err(reason) => {
                let mut bad = path.with_file_name("rules.json.bad");
                if bad.exists() {
                    bad = path.with_file_name(format!("rules.json.bad.{}", now_ms().0));
                }
                match std::fs::rename(&path, &bad) {
                    Ok(()) => {
                        self.set_rules_warning(format!("ルール帳（rules.json）を読めなかったため、{} へ退避し、空から始めました（{reason}）", bad.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                        (RulesFile::default(), true)
                    }
                    Err(e) => {
                        self.set_rules_warning(format!("ルール帳（rules.json）を読めず、退避もできませんでした（{reason}／{e}）。書込みは止めています"));
                        (RulesFile::default(), false)
                    }
                }
            }
        }
    }

    /// ルール帳を直列化して読む。書くときは同じ guard を持ったまま `write_rules` する。
    fn lock_rules(&self) -> (MutexGuard<'_, ()>, RulesFile, bool) {
        let g = self.router_rt.rules_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (f, writable) = self.load_rules_unlocked();
        (g, f, writable)
    }

    fn write_rules(&self, f: &RulesFile) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
        write_atomic(&self.rules_path(), &bytes).map_err(|e| format!("ルール帳を保存できませんでした: {e}"))
    }

    fn emit_router(&self, chat: &ChatKey) {
        self.mutate(|_| ((), vec![HostEvent::RouterUpdated { chat: chat.clone() }]));
    }

    /// 受付係のチャット（アーカイブ・引き継ぎ済みでない、先頭のもの）。
    fn router_head(&self) -> Option<ChatKey> {
        self.read(|d| {
            d.locals
                .iter()
                .find(|(k, l)| l.conversation.as_ref().is_some_and(|c| c.router.is_some()) && l.superseded_by.is_none() && !chat_archived(d, k))
                .map(|(k, _)| k.clone())
        })
    }

    fn router_record(&self, chat: &ChatKey) -> Option<RouterRecord> {
        self.read(|d| d.locals.get(chat).and_then(|l| l.conversation.as_ref()).and_then(|c| c.router.clone()))
    }

    /// 記録を更新する（メモリへ反映して保存を依頼するだけ）。受付係でなければ何もしない。
    fn update_router(self: &Arc<Self>, chat: &ChatKey, f: impl FnOnce(&mut RouterRecord)) {
        self.update_local(chat, false, std::time::Duration::ZERO, |file| {
            if let Some(r) = file.conversation.as_mut().and_then(|c| c.router.as_mut()) {
                f(r);
            }
        });
    }

    /// 保存まで終える。保存できなかったときだけ false（保存先がない場合は保存の対象外なので true）。
    async fn save_router(self: &Arc<Self>, chat: &ChatKey) -> bool {
        match self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await {
            Some(s) => matches!(s.state, SaveState::Saved { .. }),
            None => true,
        }
    }

    fn router_title_of(c: &Chat) -> String {
        match c.name.value() {
            Some(n) if !n.trim().is_empty() => n.clone(),
            _ => c.preview.value().and_then(|p| crate::host::derive_chat_name(p)).unwrap_or_else(|| "チャット".to_string()),
        }
    }

    /// 送り先チャットの、検査時点の状態。
    pub(super) fn route_state(&self, chat: &ChatKey) -> ChatRouteState {
        let org = self.check_not_org_owned(chat).is_err();
        self.read(|d| {
            if d.chat(chat).is_none() {
                return ChatRouteState::Missing;
            }
            let l = d.locals.get(chat);
            if l.and_then(|l| l.conversation.as_ref()).is_some_and(|c| c.router.is_some()) {
                ChatRouteState::Router
            } else if l.is_some_and(|l| l.superseded_by.is_some()) {
                ChatRouteState::Superseded
            } else if chat_archived(d, chat) {
                ChatRouteState::Archived
            } else if org {
                ChatRouteState::OrgOwned
            } else {
                ChatRouteState::Available
            }
        })
    }

    /// 引き継ぎ先を辿った、今の先頭。
    fn resolve_head(&self, chat: &ChatKey) -> ChatKey {
        self.read(|d| {
            let mut cur = chat.clone();
            for _ in 0..SUPERSEDE_HOPS {
                match d.locals.get(&cur).and_then(|l| l.superseded_by.as_ref()) {
                    Some(s) => cur = s.by.clone(),
                    None => break,
                }
            }
            cur
        })
    }

    /// 一覧の候補にするチャットの要約（全チャット。除外は `build_catalog` が行う）。
    fn chat_summaries(&self) -> Vec<ChatSummary> {
        let keys: Vec<ChatKey> = self.read(|d| d.chats.iter().map(|c| c.key.clone()).collect());
        let org: HashSet<ChatKey> = keys.into_iter().filter(|k| self.check_not_org_owned(k).is_err()).collect();
        self.read(|d| {
            d.chats
                .iter()
                .map(|c| {
                    let l = d.locals.get(&c.key);
                    let conv = l.and_then(|l| l.conversation.as_ref());
                    ChatSummary {
                        key: c.key.clone(),
                        title: Self::router_title_of(c),
                        purpose: conv.and_then(|c| c.purpose.as_ref()).map(|p| p.text.clone()).filter(|t| !t.trim().is_empty()),
                        cwd: c.cwd.value().cloned(),
                        last_used: c.last_used_at.or(l.and_then(|l| l.last_used_at)),
                        running: d.root_view(&c.key).is_some_and(|v| matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)),
                        pinned: c.pinned || l.is_some_and(|l| l.pinned),
                        archived: chat_archived(d, &c.key),
                        superseded_by: l.and_then(|l| l.superseded_by.as_ref()).map(|s| s.by.clone()),
                        org_owned: org.contains(&c.key),
                        intake_active: conv.and_then(|c| c.intake.as_ref()).is_some_and(|i| crate::rules::org_intake::intake_guard_active(Some(&i.state))),
                        is_router: conv.is_some_and(|c| c.router.is_some()),
                    }
                })
                .collect()
        })
    }

    /// 新規チャットが既に作られている作業フォルダ（受付係のルールの `newFolder` の根拠）。
    fn existing_chat_folders(&self) -> Vec<String> {
        self.read(|d| d.chats.iter().filter(|c| c.kind == ChatKind::Development).filter_map(|c| c.cwd.value().cloned()).collect())
    }

    // ───────────── 作成・やめる ─────────────

    /// 受付係を作る（普通のチャット。読み取り専用、作業フォルダは `<app_data>/router/`）。既にあればそれを返す（2つ目は作らない）。
    /// モデルは指定しない（既定。軽量モデルという選択は今のところ持たない。チャットのモデル表示から変えられる）。
    pub async fn router_create(self: &Arc<Self>, confirmed: &UserConfirmed) -> Result<Chat, IpcError> {
        if let Some(chat) = self.router_head().and_then(|k| self.read(|d| d.chat(&k).cloned())) {
            return Ok(chat);
        }
        {
            let mut creating = self.router_rt.creating.lock().unwrap();
            if *creating {
                return Err(err(IpcErrorCode::InvalidArgs, "受付係を作成中です"));
            }
            *creating = true;
        }
        let _release = CreatingGuard(&self.router_rt);
        let dir = self.router_dir();
        std::fs::create_dir_all(&dir).map_err(|e| err(IpcErrorCode::Io, format!("受付係の作業フォルダを作れませんでした: {e}")))?;
        let started = self
            .start_chat_in(
                StartChatArgs {
                    backend: BackendKind::Codex,
                    cwd: None,
                    model: None,
                    permission: Some(PermissionPreset::ReadOnly),
                    first_message: None,
                    worktree: None,
                    extra_folders: Vec::new(),
                    confirmed_strong: Vec::new(),
                },
                Some(dir.to_string_lossy().into_owned()),
                confirmed,
            )
            .await?;
        let key = started.chat.key.clone();
        let conv_id = self.local_id("conv");
        self.update_local(&key, true, std::time::Duration::ZERO, |f| match f.conversation.as_mut() {
            Some(c) => c.router = Some(RouterRecord::default()),
            None => {
                f.conversation = Some(ConversationRecord {
                    id: conv_id,
                    purpose: None,
                    threads: Vec::new(),
                    seed: None,
                    purpose_delivered_rev: None,
                    compacted_since_delivery: false,
                    replaces: None,
                    intake: None,
                    router: Some(RouterRecord::default()),
                })
            }
        });
        self.rename_best_effort(&key, ROUTER_TITLE.to_string(), confirmed).await;
        if !self.save_router(&key).await {
            return Err(blocked(
                BlockedReason::SaveFailed { scope: SaveScope::ChatLocal { chat: key.clone() } },
                "受付係のチャットは作りましたが、受付係の印を保存できませんでした。もう一度「受付係を作る」を押してください",
            ));
        }
        self.emit_router(&key);
        Ok(self.read(|d| d.chat(&key).cloned()).unwrap_or(started.chat))
    }

    /// 受付係をやめる（印を外すだけ。チャットもルール帳も消さない）。
    pub async fn router_retire(self: &Arc<Self>, chat: ChatKey) -> Result<(), IpcError> {
        if self.router_record(&chat).is_none() {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットは受付係ではありません"));
        }
        self.update_local(&chat, true, std::time::Duration::ZERO, |f| {
            if let Some(c) = f.conversation.as_mut() {
                c.router = None;
            }
        });
        if !self.save_router(&chat).await {
            return Err(blocked(BlockedReason::SaveFailed { scope: SaveScope::ChatLocal { chat: chat.clone() } }, "受付係をやめた記録を保存できませんでした"));
        }
        self.emit_router(&chat);
        Ok(())
    }

    // ───────────── 読取り ─────────────

    /// カード1枚分の写し（送り先の今の名前・実行中・使えない理由を添える）。
    fn dispatch_view(&self, rec: &RouterRecord, d: &Dispatch) -> DispatchView {
        let title_of = |t: &DispatchTarget| -> Option<String> {
            let chat = match t {
                DispatchTarget::Existing { chat } => chat.clone(),
                DispatchTarget::New { chat: Some(c) } => c.clone(),
                DispatchTarget::New { chat: None } => return None,
            };
            self.read(|data| data.chat(&chat).map(Self::router_title_of))
        };
        let (target_running, target_problem) = match &d.target {
            DispatchTarget::Existing { chat } => {
                // 順番待ちになる条件（live かつ実行中）と同じ判定で表示する（再開されていない送り先は、実行中と保存されていても直接送るので「順番待ち」と見せない）。
                let running = self.target_is_running(chat);
                let problem = match self.route_state(chat) {
                    ChatRouteState::Available => None,
                    s => Some(RouteReject::Unavailable(s).message()),
                };
                (running, problem)
            }
            DispatchTarget::New { .. } => (false, None),
        };
        let (queue_turn, queue_status) = self.dispatch_queue_status(d);
        // 結果の状態は値で返す（画面は文言から判定しない）。保存した結果を優先し、状態を持たない旧記録だけ文言から読み替える。
        let (execution_status, execution_result) = match rec.dispatch_results.get(&d.id.0) {
            Some(text) => {
                let status = rec.dispatch_result_status.get(&d.id.0).copied().or_else(|| ExecutionStatus::from_legacy_text(text));
                (status, Some(status.map_or_else(|| text.clone(), |s| s.view_text(text))))
            }
            None => match queue_status {
                Some((status, text)) => (Some(status), Some(text)),
                None => (None, None),
            },
        };
        DispatchView {
            request_id: rec.request_ids.get(&d.source_turn).cloned(),
            execution_turn: queue_turn.as_ref().or_else(|| rec.dispatch_turns.get(&d.id.0)).map(|t| t.turn_id.0.clone()),
            execution_status,
            execution_result,
            id: d.id.clone(),
            source_turn: d.source_turn.clone(),
            route_index: d.route_index,
            target: d.target.clone(),
            suggested: d.suggested.clone(),
            state: d.state.clone(),
            reported: d.reported,
            target_title: title_of(&d.target),
            suggested_title: title_of(&d.suggested),
            target_running,
            target_problem,
            proposal: d.proposal.clone(),
        }
    }

    /// 受付係の画面の写し（読取りだけ。何も送らない）。受付係がなければ None。
    pub fn router_get(self: &Arc<Self>) -> Result<Option<RouterView>, IpcError> {
        let Some(chat) = self.router_head() else { return Ok(None) };
        let Some(rec) = self.router_record(&chat) else { return Ok(None) };
        let (guard, rules, _) = self.lock_rules();
        drop(guard);
        let turns = rec
            .turns
            .iter()
            .map(|t| RouterTurnView {
                turn: t.turn.clone(),
                reply: t.reply.clone(),
                parse_error: t.parse_error.clone(),
                route_rejects: t.route_rejects.clone(),
                notices: t
                    .notices
                    .iter()
                    .map(|n| RuleNoticeView {
                        revision: n.revision,
                        text: n.text.clone(),
                        undone: n.revision.is_some_and(|r| rules.history.iter().find(|h| h.revision == r).is_some_and(|h| h.undone)),
                    })
                    .collect(),
            })
            .collect();
        Ok(Some(RouterView { chat, rules_warning: self.rules_warning(), rules_revision: rules.revision, dispatches: rec.dispatches.iter().map(|d| self.dispatch_view(&rec, d)).collect(), turns }))
    }

    fn dispatch_queue_status(&self, d: &Dispatch) -> (Option<TurnKey>, Option<(ExecutionStatus, String)>) {
        use crate::backend::local::{QueueEntryState, QueueRun};
        let (DispatchState::Queued { queue_entry, .. }, DispatchTarget::Existing { chat }) = (&d.state, &d.target) else {
            return (None, None);
        };
        self.read(|data| {
            let queue = data.queues.get(chat).map(|f| &f.queue);
            let Some((queue, entry)) = queue.and_then(|q| q.entries.iter().find(|e| &e.id == queue_entry).map(|e| (q, e))) else {
                return (None, Some((ExecutionStatus::QueueMissing, "キュー記録を照合できません".into())));
            };
            // 状態は値（ExecutionStatus）で返す。文は人が読む説明で、画面は文言から状態を判定しない。
            match &entry.state {
                QueueEntryState::Sent { turn } => (Some(turn.clone()), Some((ExecutionStatus::Accepted, "担当が受理しました。完了を照合中です".into()))),
                QueueEntryState::NotAccepted { message, .. } => (None, Some((ExecutionStatus::SendFailed, format!("送信失敗: {message}")))),
                QueueEntryState::Cancelled => (None, Some((ExecutionStatus::Cancelled, "キューで取り消されました".into()))),
                QueueEntryState::AcceptanceUnknown { .. } => (None, Some((ExecutionStatus::Unconfirmed, "担当への受理を照合中です".into()))),
                QueueEntryState::Sending { .. } => (None, Some((ExecutionStatus::Sending, "担当へ送信中です".into()))),
                QueueEntryState::Waiting if queue.run != QueueRun::Active => (None, Some((ExecutionStatus::QueuePaused, "キューは停止中です。担当チャットで詳細を確認してください".into()))),
                // 動いているキューで送信待ちのまま保留理由があるとき（親が今回の起動で再開されていない等）は、その理由を返す。
                // 通常の待ち（理由なし・親が作業中）は従来どおり「送信待ち」だけ。
                QueueEntryState::Waiting => {
                    let root = agent_key_of(chat);
                    let name_of = |a: &AgentKey| {
                        if a == &root {
                            data.chat(chat).map(Self::router_title_of).unwrap_or_else(|| a.id.0.clone())
                        } else {
                            data.view(a).and_then(|v| v.agent.display_name.value().cloned()).unwrap_or_else(|| a.id.0.clone())
                        }
                    };
                    match queue.hold.as_ref().and_then(|h| queue_hold_text(h, &name_of, &root)) {
                        Some(text) => (None, Some((ExecutionStatus::QueueHeld, text))),
                        None => (None, None),
                    }
                }
            }
        })
    }

    /// 保存済みの対応IDだけで履歴を照合する。受付の復旧から新しい送信はしない。
    pub async fn router_refresh(self: &Arc<Self>) -> Result<Option<RouterView>, IpcError> {
        let Some(chat) = self.router_head() else { return Ok(None) };
        let Some(rec) = self.router_record(&chat) else { return Ok(None) };
        if self.is_connected() && rec.catalogs.iter().any(|c| !rec.request_ids.values().any(|id| id == &c.client_message_id) && !self.router_rt.pending.lock().unwrap().iter().any(|p| p.client_message_id == c.client_message_id)) {
            if let Ok(history) = self.read_history(agent_key_of(&chat), ReadOptions { include_turns: true }).await {
                for turn in history.turns.iter().filter(|t| t.complete && t.end == Some(TurnEnd::Completed)) {
                    let facts = facts_of(turn);
                    if let (Some(input), Some(output)) = (
                        facts.user_texts.iter().find(|text| router_request_id(text).is_some_and(|id| rec.catalogs.iter().any(|c| c.client_message_id == id) && !self.router_rt.pending.lock().unwrap().iter().any(|p| p.client_message_id == id))),
                        facts.final_text.as_deref(),
                    ) {
                        let key = (chat.clone(), turn.key.turn_id.0.clone());
                        if self.router_rt.processing.lock().unwrap().insert(key.clone()) {
                            let _guard = ProcessingGuard { rt: &self.router_rt, key };
                            self.router_apply_reply(&chat, &turn.key.turn_id.0, input, output, false).await;
                        }
                    }
                }
            }
        }
        let mut changed = false;
        let mut histories = HashMap::new();
        for d in &rec.dispatches {
            if rec.dispatch_results.contains_key(&d.id.0) {
                continue;
            }
            let (queue_turn, _) = self.dispatch_queue_status(d);
            let Some(turn) = queue_turn.or_else(|| rec.dispatch_turns.get(&d.id.0).cloned()) else { continue };
            let history = match histories.entry(turn.agent.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let fetched = self.read_history(turn.agent.clone(), ReadOptions { include_turns: true }).await;
                    e.insert(fetched)
                }
            };
            let Ok(history) = history else { continue };
            let Some(record) = history.turns.iter().find(|r| r.key == turn && r.complete) else { continue };
            // 状態は値で保存し、文は人が読む説明。完了の文は先頭行「担当完了」＋最終回答（画面へは見出し行を省いて返す。ExecutionStatus::view_text）。
            let (status, result) = match record.end {
                Some(TurnEnd::Completed) => match facts_of(record).final_text {
                    Some(text) => (ExecutionStatus::Done, format!("担当完了\n{text}")),
                    None => (ExecutionStatus::Done, "担当完了（最終回答の記録なし）".into()),
                },
                Some(TurnEnd::Failed) => (ExecutionStatus::Failed, "担当の実行が失敗しました".into()),
                Some(TurnEnd::Interrupted) => (ExecutionStatus::Interrupted, "担当の実行が中断されました".into()),
                None => continue,
            };
            self.update_router(&chat, |r| {
                r.dispatch_turns.insert(d.id.0.clone(), turn);
                r.dispatch_results.insert(d.id.0.clone(), result);
                r.dispatch_result_status.insert(d.id.0.clone(), status);
            });
            changed = true;
        }
        if changed && !self.save_router(&chat).await {
            self.warn("担当の結果を保存できませんでした");
        }
        self.router_get()
    }

    async fn router_skill(self: &Arc<Self>, chat: &ChatKey) -> Result<Option<Attachment>, IpcError> {
        if self.require_op(ParityOp::Skills).is_err() {
            return Ok(None);
        }
        let list = self.list_skills(ListSkillsArgs { chat: chat.clone(), force_reload: false }).await?;
        Ok(list.skills.into_iter()
            .find(|s| s.name == ROUTER_SKILL && s.enabled.value() != Some(&false) && s.errors.is_empty())
            .map(|s| Attachment {
                id: self.local_id("att"),
                kind: AttachmentKind::Skill,
                original_path: s.path,
                copy_path: None,
                attached_at: now_ms(),
                exists: Known::NotFetched,
                owner_chat: chat.clone(),
                name: Some(s.name),
            }))
    }

    // ───────────── 受付係への送信 ─────────────

    /// 受付係へ送る（通常の `send_message` の代わり）。資料と構造化出力の指定を付け、一覧の対応を送る前に保存する。
    /// 受理を確認したときだけ「届いた記録」を書く。受理不明・拒否・切断で自動再送しない。
    pub async fn router_send(self: &Arc<Self>, args: SendMessageArgs, confirmed: &UserConfirmed) -> Result<SendAttempt, IpcError> {
        let chat = args.chat.clone();
        let Some(rec) = self.router_record(&chat) else {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットは受付係ではありません。通常の送信を使ってください"));
        };
        if self.router_head().as_ref() != Some(&chat) {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットは今の受付係ではありません（アーカイブまたは引き継ぎ済みです）"));
        }
        if !matches!(args.intent, SendIntent::NewTurn) {
            return Err(err(IpcErrorCode::InvalidArgs, "受付係への送信は新しい依頼としてだけ行えます（返答が終わってから送ってください）"));
        }
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        // 受付係は Codex のチャット。Codex が起動していないと送れない（理由を返す。送信の記録も作らない）。
        if !self.is_connected() {
            return Err(err(IpcErrorCode::NotConnected, CODEX_NOT_RUNNING_SEND));
        }
        if self.backend.capabilities().structured_output != Support::Supported {
            return Err(err(IpcErrorCode::Unsupported, "このAIは構造化出力に対応していないため、受付係を使えません"));
        }
        let skill = self.router_skill(&chat).await?;
        let now = now_ms();
        let (guard, rules, _) = self.lock_rules();
        drop(guard);
        let summaries = self.chat_summaries();
        let catalog = build_catalog(&summaries, &rules, now);
        let titles: HashMap<ChatKey, String> = summaries.iter().map(|s| (s.key.clone(), s.title.clone())).collect();
        let label = |t: &DispatchTarget| -> String {
            match t {
                DispatchTarget::Existing { chat } | DispatchTarget::New { chat: Some(chat) } => titles.get(chat).cloned().unwrap_or_else(|| "（不明なチャット）".to_string()),
                DispatchTarget::New { chat: None } => "新規チャット".to_string(),
            }
        };
        let mut env = compose_router_envelope(&rec, &catalog, &rules, &label, &args.text);
        let cm = self.local_id("reception").0;
        env.text = stamp_router_request(&env.text, &cm);
        // 一覧の連番とチャットの対応を、送る前に保存する（返答の解析に使う）。保存できなければ送らない。
        let sent_catalog = catalog.to_sent(&cm, now);
        self.update_router(&chat, |r| r.push_catalog(sent_catalog));
        if !self.save_router(&chat).await {
            return Err(blocked(BlockedReason::SaveFailed { scope: SaveScope::ChatLocal { chat: chat.clone() } }, "受付係の記録を保存できなかったため、送信していません"));
        }
        self.remember_pending(PendingSend { chat: chat.clone(), client_message_id: cm.clone(), full_text: env.text.clone(), turn: None });
        let sent = self
            .send_message_with(
                SendMessageArgs { chat: chat.clone(), text: env.text.clone(), attachments: args.attachments, intent: SendIntent::NewTurn, steer_entry: None },
                confirmed,
                SendExtras { output_schema: Some(router_output_schema()), client_message_id: Some(cm.clone()), skill },
            )
            .await;
        match sent {
            Ok(attempt) => {
                self.router_note_send(&chat, &cm, &env.included, rec.compaction_epoch, &attempt);
                Ok(attempt)
            }
            Err(e) => {
                // 送っていない。届いた記録は書かない（再送もしない）。
                self.forget_pending(&cm);
                Err(e)
            }
        }
    }

    fn remember_pending(&self, p: PendingSend) {
        let mut g = self.router_rt.pending.lock().unwrap();
        g.push(p);
        let over = g.len().saturating_sub(PENDING_MAX);
        g.drain(..over);
    }

    fn forget_pending(&self, cm: &str) {
        self.router_rt.pending.lock().unwrap().retain(|p| p.client_message_id != cm);
    }

    /// 送信の結果を反映する。受理を確認したときだけ、届いた記録と turn の対応づけを書く。
    pub(super) fn router_note_send(self: &Arc<Self>, chat: &ChatKey, cm: &str, inc: &EnvelopeSections, composed_epoch: u32, attempt: &SendAttempt) {
        match &attempt.state {
            SendState::Accepted { turn } => {
                if let Some(p) = self.router_rt.pending.lock().unwrap().iter_mut().find(|p| p.client_message_id == cm) {
                    p.turn = Some(turn.turn_id.0.clone());
                }
                let state = attempt.state.clone();
                self.update_router(chat, |r| {
                    apply_delivery(r, inc, composed_epoch, &state);
                });
                self.emit_router(chat);
            }
            SendState::Rejected { .. } | SendState::NotFoundAfterReconcile => self.forget_pending(cm),
            // 受理不明・未確定は、届いた記録を書かない。返答の対応づけのため送信の記録は残す（本文の照合で使う）。
            SendState::AcceptanceUnknown { .. } | SendState::Sending => {}
        }
    }

    // ───────────── 返答の処理（turn 完了時） ─────────────

    /// turn の終端を観測したとき（待たない・送らない）。受付係の完了した turn なら、履歴を読んで返答を処理する。
    pub(in crate::host) fn router_observe_event(self: &Arc<Self>, event: &BackendEvent) {
        let BackendEvent::TurnEnded { turn, end, .. } = event else { return };
        let target = ChatKey { backend: turn.agent.backend, id: turn.agent.id.clone() };
        if let Some(router) = self.router_head() {
            if self.router_record(&router).is_some_and(|r| r.dispatches.iter().any(|d| match &d.target {
                DispatchTarget::Existing { chat } | DispatchTarget::New { chat: Some(chat) } => chat == &target,
                _ => false,
            })) {
                self.emit_router(&router);
            }
        }
        if *end != TurnEnd::Completed {
            return;
        }
        let chat = ChatKey { backend: turn.agent.backend, id: turn.agent.id.clone() };
        if self.router_record(&chat).is_none() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let host = self.clone();
        let turn_id = turn.turn_id.0.clone();
        tokio::spawn(async move {
            host.router_process_turn(chat, turn_id).await;
        });
    }

    async fn router_process_turn(self: &Arc<Self>, chat: ChatKey, turn_id: String) {
        let key = (chat.clone(), turn_id.clone());
        if !self.router_rt.processing.lock().unwrap().insert(key.clone()) {
            return;
        }
        let _guard = ProcessingGuard { rt: &self.router_rt, key };
        if self.router_record(&chat).is_some_and(|r| r.turns.iter().any(|t| t.turn == turn_id)) {
            return;
        }
        let history = match self.read_history(agent_key_of(&chat), ReadOptions { include_turns: true }).await {
            Ok(h) => h,
            Err(e) => {
                crate::diag::log("router", &format!("history read failed: {e}"));
                self.router_hand_to_refresh(&chat, &turn_id);
                return;
            }
        };
        let Some(turn) = history.turns.iter().find(|t| t.key.turn_id.0 == turn_id) else { return self.router_hand_to_refresh(&chat, &turn_id) };
        let facts = facts_of(turn);
        let Some(sent_text) = facts.user_texts.iter().find(|u| router_turn_user_text(u).is_some()) else { return self.router_hand_to_refresh(&chat, &turn_id) };
        let Some(final_text) = facts.final_text.as_deref() else { return self.router_hand_to_refresh(&chat, &turn_id) };
        let live_request = router_request_id(sent_text).is_some_and(|id| {
            self.router_rt.pending.lock().unwrap().iter().any(|p| p.chat == chat && p.client_message_id == id && p.full_text.trim() == sent_text.trim())
        });
        self.router_apply_reply(&chat, &turn_id, sent_text, final_text, live_request).await;
    }

    /// 返答を読み取れなかったとき（履歴の読み取り失敗・turn が見つからない・最終回答がない）、この turn の送信の記録（メモリ上）を外し、画面の取り直しを促す。
    /// 送信の記録が残っていると、復旧（`router_refresh`）が「処理中の送信」として除外し、再起動するまで返答が処理されないままになる。
    /// 復旧は自動送信をしない（送信時の照合ができない経路）。
    fn router_hand_to_refresh(self: &Arc<Self>, chat: &ChatKey, turn_id: &str) {
        self.router_rt.pending.lock().unwrap().retain(|p| !(&p.chat == chat && p.turn.as_deref() == Some(turn_id)));
        self.emit_router(chat);
    }

    /// 受付係の返答（最終回答）を処理する。`sent_text` はその turn の利用者の発言（資料つきの送信全体）。
    /// 送信時の受付依頼IDに一致する候補一覧を使う。旧形式は同じプロセス内の対応記録だけを使う。
    pub(super) async fn router_apply_reply(self: &Arc<Self>, chat: &ChatKey, turn_id: &str, sent_text: &str, final_text: &str, allow_auto: bool) {
        let Some(rec) = self.router_record(chat) else { return };
        if rec.turns.iter().any(|t| t.turn == turn_id)
            || router_request_id(sent_text).is_some_and(|id| rec.request_ids.values().any(|used| used == id))
        {
            return;
        }
        let user_text = router_turn_user_text(sent_text).unwrap_or_default().to_string();
        let cm = router_request_id(sent_text).map(str::to_owned).or_else(|| self
            .router_rt
            .pending
            .lock()
            .unwrap()
            .iter()
            .find(|p| &p.chat == chat && (p.turn.as_deref() == Some(turn_id) || p.full_text.trim() == sent_text.trim()))
            .map(|p| p.client_message_id.clone()));
        let sent = cm
            .as_deref()
            .and_then(|cm| rec.catalogs.iter().find(|c| c.client_message_id == cm))
            .cloned()
            .unwrap_or(SentCatalog { client_message_id: String::new(), entries: Vec::new(), at: now_ms() });
        let now = now_ms();
        let mut auto = None;
        let matched = !sent.client_message_id.is_empty();

        let turn_rec = match parse_router_reply(final_text) {
            Err(e) => RouterTurnRecord { turn: turn_id.to_string(), reply: None, parse_error: Some(e.message()), route_rejects: Vec::new(), notices: Vec::new() },
            Ok(reply) => {
                let folders = self.existing_chat_folders();
                let mut interpreted = {
                    let (guard, rules, _) = self.lock_rules();
                    let mut new_id = || self.local_id("disp");
                    let state_of = |k: &ChatKey| self.route_state(k);
                    let title_of = |k: &ChatKey| self.chat_title(k);
                    let mut out = interpret_reply(&reply, turn_id, &sent, &user_text, &rules, &state_of, &title_of, &folders, &mut new_id, now);
                    if out.rules.take().is_some() {
                        downgrade_notices(&mut out.notices, "ルール帳で明示的に登録してください");
                    }
                    if !matched {
                        out.dispatches.clear();
                        out.route_rejects.push("受付依頼IDと候補一覧を照合できませんでした".into());
                    }
                    drop(guard);
                    out
                };
                let dispatches = std::mem::take(&mut interpreted.dispatches);
                // 自動送信は利用者が本文の先頭で指定した既存先と分類が一致した場合だけ。指定したのに送られないときは、理由を返答へ出す。
                match auto_send_decision(&user_text, allow_auto, self.router_head().as_ref() == Some(chat), &dispatches, interpreted.route_rejects.is_empty(), reply.rule_changes.is_empty()) {
                    AutoSend::Send { id, target, text } => auto = Some((id, target, text)),
                    AutoSend::Skip(reason) => {
                        let hint = if dispatches.is_empty() { "" } else { "。送る場合はカードの［送る］を押してください" };
                        interpreted.route_rejects.push(format!("自動送信はしませんでした（{reason}）{hint}"));
                    }
                    AutoSend::NotRequested => {}
                }
                // 新しい順に並ぶよう、案の並びの逆から足す。
                self.update_router(chat, |r| {
                    for d in dispatches.into_iter().rev() {
                        r.push_dispatch(d);
                    }
                });
                RouterTurnRecord { turn: turn_id.to_string(), reply: Some(reply.reply), parse_error: None, route_rejects: interpreted.route_rejects, notices: interpreted.notices }
            }
        };
        self.update_router(chat, |r| {
            if matched {
                r.request_ids.insert(turn_id.to_string(), sent.client_message_id.clone());
            }
            r.push_turn(turn_rec);
        });
        if !self.save_router(chat).await {
            self.warn("受付係の返答の解析結果を保存できませんでした（画面には残っています）");
        } else if let Some((id, target, text)) = auto {
            let acquired = self.router_rt.dispatching.lock().unwrap().insert(id.clone());
            if acquired {
                let _guard = DispatchGuard { rt: &self.router_rt, id: id.clone() };
                if self.find_dispatch(&id).is_some_and(|(_, d)| matches!(d.state, DispatchState::Proposed)) {
                    // 送り先が live（今回の起動で再開済み）でなければ登録しない。キューの駆動側は再開しないため、登録すると永久に保留になる。
                    // 自動で再開もしない（再開は利用者の操作の中だけ）。カードは「提案」のまま残し、［送る］（直接送信＝再開つき）へ案内する。
                    let rejected = if self.target_is_live(&target) {
                        self.router_enqueue(chat, &id, target, text).await.err().map(|e| e.message)
                    } else {
                        Some(AUTO_SEND_NOT_LIVE.to_string())
                    };
                    if let Some(message) = rejected {
                        self.update_router(chat, |rec| {
                            if let Some(turn) = rec.turns.iter_mut().find(|t| t.turn == turn_id) {
                                turn.route_rejects.push(format!("自動送信を保留しました: {message}"));
                            }
                        });
                        self.save_router(chat).await;
                    }
                }
            }
        }
        if let Some(cm) = cm { self.forget_pending(&cm); }
        self.emit_router(chat);
    }

    // ───────────── 振り分けの実行 ─────────────

    /// 送り先が今回の起動で再開済み（live購読中）で、状態も取れているか。キューの自動送信は親が live（かつ状態が Unknown・Closed でない）ことが条件で、
    /// 駆動側は再開しない。子孫の状態による保留は含めない（子孫が原因の保留は「状態を再確認」で解ける）。
    fn target_is_live(&self, chat: &ChatKey) -> bool {
        self.read(|d| d.root_view(chat).is_some_and(|v| v.freshness == Freshness::Live && !matches!(v.status.state, AgentState::Unknown | AgentState::Closed)))
    }

    /// 送り先が今回の起動で再開済み（live）で、実行中か。受付の［送る］が順番待ちになる条件で、カードの「順番待ちとして送ります」の表示も同じ判定を使う。
    fn target_is_running(&self, chat: &ChatKey) -> bool {
        self.read(|d| d.root_view(chat).is_some_and(|v| v.freshness == Freshness::Live && matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)))
    }

    fn find_dispatch(&self, id: &LocalId) -> Option<(ChatKey, Dispatch)> {
        self.read(|d| {
            d.locals.iter().find_map(|(k, l)| {
                let r = l.conversation.as_ref()?.router.as_ref()?;
                r.dispatches.iter().find(|x| &x.id == id).map(|x| (k.clone(), x.clone()))
            })
        })
    }

    fn set_dispatch(self: &Arc<Self>, router: &ChatKey, id: &LocalId, f: impl FnOnce(&mut Dispatch)) {
        self.update_router(router, |r| {
            if let Some(d) = r.dispatches.iter_mut().find(|d| &d.id == id) {
                f(d);
            }
        });
    }

    fn dispatch_view_of(&self, id: &LocalId) -> Result<DispatchView, IpcError> {
        self.find_dispatch(id).and_then(|(chat, d)| self.router_record(&chat).map(|rec| self.dispatch_view(&rec, &d))).ok_or_else(|| err(IpcErrorCode::NotFound, "振り分けの記録が見つかりません"))
    }

    /// カードの［送る］。送り先を送る直前に再検査し、利用者の送信（`SendAuth::User`）として1回だけ送る。
    /// 送り先が実行中で `intent` が None のときだけ、完了後に送る順番待ち（`Queued`）に入れる。それ以外（実行中でない送り先、または `intent` 指定あり）は
    /// 利用者の送信として直接送る（保存履歴だけの送り先は `ensure_live` で再開する）。結果は `Sent`／`Unconfirmed`。
    /// 受理されなかったときはカードを「提案」のまま残してエラーを返す。自動で再送しない（`Sent`・`Unconfirmed` のカードは再送できない）。
    pub async fn router_dispatch(self: &Arc<Self>, args: RouterDispatchArgs, confirmed: &UserConfirmed) -> Result<DispatchView, IpcError> {
        let Some((router, d)) = self.find_dispatch(&args.dispatch_id) else {
            return Err(err(IpcErrorCode::NotFound, "振り分けの記録が見つかりません"));
        };
        if !self.router_rt.dispatching.lock().unwrap().insert(args.dispatch_id.clone()) {
            return Err(err(IpcErrorCode::InvalidArgs, "この振り分けは送信中です"));
        }
        let _guard = DispatchGuard { rt: &self.router_rt, id: args.dispatch_id.clone() };
        // 排他を取った後の状態で判断する（二重押下で2回送らない）。
        let d = self.find_dispatch(&args.dispatch_id).map(|(_, d)| d).unwrap_or(d);
        match &d.state {
            DispatchState::Proposed => {}
            DispatchState::Sent { .. } => return Err(err(IpcErrorCode::InvalidArgs, "すでに送っています")),
            DispatchState::Queued { .. } => return Err(err(IpcErrorCode::InvalidArgs, "すでに送り先の順番待ちに入れています")),
            DispatchState::Unconfirmed { .. } => return Err(err(IpcErrorCode::InvalidArgs, "送れたか確認できていません。再送はしません（送り先の会話を開いて確認してください）")),
            DispatchState::Dismissed { .. } => return Err(err(IpcErrorCode::InvalidArgs, "見送った振り分けです")),
        }
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "送る本文が空です"));
        }
        let target = match (args.target_override.clone(), &d.target) {
            (Some(t), _) => t,
            (None, DispatchTarget::Existing { chat }) => chat.clone(),
            (None, DispatchTarget::New { .. }) => return Err(err(IpcErrorCode::InvalidArgs, "新規チャットの案です。新規チャットの作成から始めてください")),
        };
        match self.route_state(&target) {
            ChatRouteState::Available => {}
            ChatRouteState::Missing => return Err(err(IpcErrorCode::NotFound, "送り先のチャットが見つかりません")),
            ChatRouteState::Superseded => {
                let head = self.resolve_head(&target);
                let name = self.read(|data| data.chat(&head).map(Self::router_title_of)).filter(|_| head != target);
                let hint = name.map(|n| format!("（引き継ぎ先: 『{n}』。送り先を変えて送ってください）")).unwrap_or_default();
                return Err(err(IpcErrorCode::InvalidArgs, format!("{}{hint}", RouteReject::Unavailable(ChatRouteState::Superseded).message())));
            }
            ChatRouteState::OrgOwned => return Err(blocked(BlockedReason::OrgOwned, RouteReject::Unavailable(ChatRouteState::OrgOwned).message())),
            s => return Err(err(IpcErrorCode::InvalidArgs, RouteReject::Unavailable(s).message())),
        }
        // Antigravity 宛で本文が agy のコマンドライン長の上限を超えるなら、順番待ちへ入れる前・送る前に止める（Codex 宛は検査しない）。
        check_agy_text_length(&target, &args.text)?;
        // 実行中の送り先へ intent 指定なしで送るときだけ順番待ち（完了後に送る）に入れる。別件の依頼を実行中の turn へ差し込まない。
        // 実行中でなければ、利用者の送信として直接送る（`send_message_with` が `ensure_live` で再開してから新しい turn にする）。
        // 「実行中でない」送り先をキューへ入れると、今回の起動で未再開（live でない）の送り先は駆動側が再開しないため永久に保留される。
        // 実行中の判定は live を必須にする（live でない送り先の保存済みの状態が実行中でも、キューの駆動側は再開しないため永久に保留される）。
        // live でなければ直接送信に回り、再開した結果が本当に実行中なら送信がエラーになり、カードは提案のまま（もう一度押せば live・実行中として順番待ちに入る）。
        let running = self.target_is_running(&target);
        if running && args.intent.is_none() {
            if !self.is_connected() {
                return Err(err(IpcErrorCode::InvalidArgs, CODEX_NOT_RUNNING_ENQUEUE));
            }
            return self.router_enqueue(&router, &args.dispatch_id, target, args.text).await;
        }
        let intent = args.intent.unwrap_or(SendIntent::NewTurn);
        let cm = self.local_id("cm").0;
        let sent = self
            .send_message_with(
                SendMessageArgs { chat: target.clone(), text: args.text, attachments: Vec::new(), intent, steer_entry: None },
                confirmed,
                SendExtras { client_message_id: Some(cm.clone()), ..Default::default() },
            )
            .await?;
        self.router_settle_dispatch(&router, &args.dispatch_id, DispatchTarget::Existing { chat: target }, &sent).await
    }

    async fn router_enqueue(self: &Arc<Self>, router: &ChatKey, id: &LocalId, target: ChatKey, text: String) -> Result<DispatchView, IpcError> {
        if self.route_state(&target) != ChatRouteState::Available {
            return Err(err(IpcErrorCode::InvalidArgs, "送り先を利用できません"));
        }
        let entry = self.local_id("q");
        // 登録に失敗したときに元へ戻すための、登録前の送り先と状態。
        let before = self.find_dispatch(id).map(|(_, d)| (d.target, d.state));
        self.set_dispatch(router, id, |d| {
            d.target = DispatchTarget::Existing { chat: target.clone() };
            d.state = DispatchState::Queued { at: now_ms(), queue_entry: entry.clone() };
        });
        if !self.save_router(router).await {
            // 保存できなかった「順番待ち」は記録に残らない。メモリ上もカードを提案へ戻す（戻さないと、キュー項目のない順番待ちのカードが出口なく残る）。
            self.restore_dispatch(router, id, before);
            self.emit_router(router);
            return Err(err(IpcErrorCode::Io, "振り分けを保存できないためキューに登録していません"));
        }
        let result = if self.route_state(&target) == ChatRouteState::Available {
            self.enqueue_with_id(EnqueueArgs { chat: target, text, attachments: Vec::new() }, entry)
        } else {
            Err(err(IpcErrorCode::InvalidArgs, "送り先を利用できなくなりました"))
        };
        if result.is_err() {
            // キューへ登録できなかった（容量不足など）。順番待ちのカードは出口がない（［送る］も［見送る］も出ない）ので、提案へ戻して
            // 失敗はエラー（手動）または受付の返答の理由（自動送信）として利用者へ返す。送ってはいない。
            self.restore_dispatch(router, id, before);
            if !self.save_router(router).await {
                self.warn("キュー登録に失敗した振り分けを提案へ戻した記録を保存できませんでした");
            }
        }
        self.emit_router(router);
        result?;
        self.dispatch_view_of(id)
    }

    /// 振り分けの送り先と状態を、登録前（`before`）へ戻す。
    fn restore_dispatch(self: &Arc<Self>, router: &ChatKey, id: &LocalId, before: Option<(DispatchTarget, DispatchState)>) {
        if let Some((target, state)) = before {
            self.set_dispatch(router, id, |d| {
                d.target = target;
                d.state = state;
            });
        }
    }

    /// 送信の結果をカードへ反映する。受理を確認したら `Sent`、受理不明なら `Unconfirmed`、受理されなければ「提案」のままエラー。
    pub(super) async fn router_settle_dispatch(self: &Arc<Self>, router: &ChatKey, id: &LocalId, target: DispatchTarget, attempt: &SendAttempt) -> Result<DispatchView, IpcError> {
        match dispatch_outcome(&attempt.state, &attempt.client_message_id, attempt.at) {
            DispatchOutcome::NotSent(message) => Err(err(IpcErrorCode::Rejected, message)),
            DispatchOutcome::Sent(state) | DispatchOutcome::Unconfirmed(state) => {
                if let SendState::Accepted { turn } = &attempt.state {
                    self.update_router(router, |rec| {
                        rec.dispatch_turns.insert(id.0.clone(), turn.clone());
                    });
                }
                self.set_dispatch(router, id, |d| {
                    d.target = target;
                    d.state = state;
                });
                if !self.save_router(router).await {
                    self.warn("振り分けの結果を保存できませんでした（画面には残っています）");
                }
                self.emit_router(router);
                self.dispatch_view_of(id)
            }
        }
    }

    /// 受理不明だった直接送信（受付カードの［送る］）の履歴照合の結果を、カードへ反映する（自動・手動の照合のどちらでも呼ばれる。読み取りだけで再送しない）。
    /// 受理と確定（`turn` あり）なら `Sent`（担当 turn を記録するので、完了後の結果も回収される）。受理なしと確定（`turn` なし）なら送っていないので、
    /// カードを提案へ戻す（利用者が押し直せる。自動では再送しない）。該当するカードがなければ何もしない。
    pub(super) fn router_note_reconciled(self: &Arc<Self>, chat: &ChatKey, client_message_id: &str, turn: Option<&TurnKey>) {
        let found = self.read(|d| {
            d.locals.iter().find_map(|(k, l)| {
                let r = l.conversation.as_ref()?.router.as_ref()?;
                r.dispatches.iter().find_map(|x| match (&x.state, &x.target) {
                    (DispatchState::Unconfirmed { at, client_message_id: cm }, DispatchTarget::Existing { chat: c } | DispatchTarget::New { chat: Some(c) }) if cm == client_message_id && c == chat => Some((k.clone(), x.id.clone(), *at)),
                    _ => None,
                })
            })
        });
        let Some((router, id, at)) = found else { return };
        match turn {
            Some(turn) => {
                self.update_router(&router, |rec| {
                    rec.dispatch_turns.insert(id.0.clone(), turn.clone());
                });
                self.set_dispatch(&router, &id, |d| d.state = DispatchState::Sent { at, client_message_id: client_message_id.to_string() });
            }
            None => self.set_dispatch(&router, &id, |d| d.state = DispatchState::Proposed),
        }
        self.emit_router(&router);
    }

    /// 順番待ちの項目が追加指示として送られ、受理されたとき、その受付カードを `Sent`（担当 turn つき）にする。
    /// 反映しないと、届いているのに項目の取消し（追加指示の後始末）が「キューで取り消されました」と表示され、結果も回収されない。
    pub(super) fn router_note_steered(self: &Arc<Self>, chat: &ChatKey, entry: &LocalId, turn: &TurnKey, client_message_id: &str) {
        let found = self.read(|d| {
            d.locals.iter().find_map(|(k, l)| {
                let r = l.conversation.as_ref()?.router.as_ref()?;
                r.dispatches.iter().find_map(|x| match (&x.state, &x.target) {
                    (DispatchState::Queued { at, queue_entry }, DispatchTarget::Existing { chat: c }) if queue_entry == entry && c == chat => Some((k.clone(), x.id.clone(), *at)),
                    _ => None,
                })
            })
        });
        let Some((router, id, at)) = found else { return };
        self.update_router(&router, |rec| {
            rec.dispatch_turns.insert(id.0.clone(), turn.clone());
        });
        self.set_dispatch(&router, &id, |d| d.state = DispatchState::Sent { at, client_message_id: client_message_id.to_string() });
        self.emit_router(&router);
    }

    /// 新規チャットの案を、UIが既存の新規チャット作成の経路で作った後に呼ぶ。送り先を作ったチャットにして、最初の依頼の結果を記録する。
    /// `first_send` が受理済みなら `Sent`、受理不明・未確定なら `Unconfirmed`、受理されなかった・送っていないなら案は「提案」のまま（送り先だけ記録する）。
    /// 受理不明（未確定）のカードは、確定した `first_send` で呼び直すと更新できる。
    pub async fn router_dispatch_created(self: &Arc<Self>, args: RouterDispatchCreatedArgs) -> Result<DispatchView, IpcError> {
        let Some((router, d)) = self.find_dispatch(&args.dispatch_id) else {
            return Err(err(IpcErrorCode::NotFound, "振り分けの記録が見つかりません"));
        };
        if !matches!(d.suggested, DispatchTarget::New { .. }) && !matches!(d.target, DispatchTarget::New { .. }) {
            return Err(err(IpcErrorCode::InvalidArgs, "新規チャットの案ではありません"));
        }
        let linked = matches!(&d.target, DispatchTarget::New { chat: Some(c) } if c == &args.chat);
        match &d.state {
            DispatchState::Proposed => {}
            DispatchState::Unconfirmed { .. } if linked => {}
            DispatchState::Queued { .. } => return Err(err(IpcErrorCode::InvalidArgs, "すでに順番待ちに入れています（再送はしません）")),
            DispatchState::Dismissed { .. } => return Err(err(IpcErrorCode::InvalidArgs, "見送った振り分けです")),
            _ => return Err(err(IpcErrorCode::InvalidArgs, "すでに送っています（再送はしません）")),
        }
        if self.read(|data| data.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "作成したチャットが見つかりません"));
        }
        let target = DispatchTarget::New { chat: Some(args.chat.clone()) };
        match args.first_send {
            None => {
                self.set_dispatch(&router, &args.dispatch_id, |d| d.target = target);
                self.save_router(&router).await;
                self.emit_router(&router);
                self.dispatch_view_of(&args.dispatch_id)
            }
            Some(attempt) => match dispatch_outcome(&attempt.state, &attempt.client_message_id, attempt.at) {
                DispatchOutcome::NotSent(_) => {
                    self.set_dispatch(&router, &args.dispatch_id, |d| d.target = target);
                    self.save_router(&router).await;
                    self.emit_router(&router);
                    self.dispatch_view_of(&args.dispatch_id)
                }
                _ => self.router_settle_dispatch(&router, &args.dispatch_id, target, &attempt).await,
            },
        }
    }

    /// ［見送る］。送っていない案だけ。
    pub async fn router_dismiss(self: &Arc<Self>, args: RouterDispatchIdArgs) -> Result<DispatchView, IpcError> {
        if !self.router_rt.dispatching.lock().unwrap().insert(args.dispatch_id.clone()) {
            return Err(err(IpcErrorCode::InvalidArgs, "この振り分けは送信中です"));
        }
        let _guard = DispatchGuard { rt: &self.router_rt, id: args.dispatch_id.clone() };
        let Some((router, d)) = self.find_dispatch(&args.dispatch_id) else {
            return Err(err(IpcErrorCode::NotFound, "振り分けの記録が見つかりません"));
        };
        if !matches!(d.state, DispatchState::Proposed) {
            return Err(err(IpcErrorCode::InvalidArgs, "提案のままの振り分けだけ見送れます"));
        }
        let at = now_ms();
        self.set_dispatch(&router, &args.dispatch_id, |d| d.state = DispatchState::Dismissed { at });
        self.save_router(&router).await;
        self.emit_router(&router);
        self.dispatch_view_of(&args.dispatch_id)
    }

    // ───────────── ルール帳 ─────────────

    fn rules_view(&self, rules: &RulesFile) -> RulesView {
        RulesView {
            revision: rules.revision,
            warning: self.rules_warning(),
            rules: rules
                .rules
                .iter()
                .map(|r| RuleView {
                    rule: r.clone(),
                    target_title: match &r.target {
                        Some(RuleTarget::Chat { chat, .. }) => {
                            let head = self.resolve_head(chat);
                            self.read(|d| d.chat(&head).filter(|_| !chat_archived(d, &head)).map(Self::router_title_of))
                        }
                        _ => None,
                    },
                })
                .collect(),
            history: rules.history.iter().map(|c| RuleChangeView { change: c.clone(), summary: change_summary(c) }).collect(),
        }
    }

    /// ルール帳の写し（読取りだけ）。
    pub fn router_rules(self: &Arc<Self>) -> Result<RulesView, IpcError> {
        let (guard, rules, _) = self.lock_rules();
        drop(guard);
        Ok(self.rules_view(&rules))
    }

    fn emit_router_head(&self) {
        if let Some(chat) = self.router_head() {
            self.emit_router(&chat);
        }
    }

    /// 手での編集（出所は手動）。上限・禁止語・ルールIDの存在は会話からの変更と同じに守る。
    pub async fn router_rule_edit(self: &Arc<Self>, args: RuleEditArgs) -> Result<RulesView, IpcError> {
        let target = match args.target {
            None => None,
            Some(RuleTargetInput::NewIn { folder }) => Some(RuleTargetSpec::NewIn(folder.trim().to_string())),
            Some(RuleTargetInput::Chat { chat }) => {
                match self.route_state(&chat) {
                    ChatRouteState::Available => {}
                    s => return Err(err(IpcErrorCode::InvalidArgs, RouteReject::Unavailable(s).message())),
                }
                Some(RuleTargetSpec::Chat(chat))
            }
        };
        let (guard, rules, writable) = self.lock_rules();
        if !writable {
            return Err(err(IpcErrorCode::Io, "ルール帳を書ける状態ではありません（rules.json を確認してください）"));
        }
        let valid = check_manual_rule_change(args.op, args.rule_id.as_deref(), &args.text, target, &rules).map_err(|e| err(IpcErrorCode::InvalidArgs, e.message()))?;
        let title_of = |k: &ChatKey| self.chat_title(k);
        let next = apply_rule_change(&rules, &valid, None, &title_of, now_ms());
        self.write_rules(&next).map_err(|m| err(IpcErrorCode::Io, m))?;
        drop(guard);
        self.emit_router_head();
        Ok(self.rules_view(&next))
    }

    /// 変更の取り消し。その後に同じルールを変えた変更があれば `RouterRuleUndoNeedsConfirm`（確認のうえ `force=true` で再実行）。
    pub async fn router_rule_undo(self: &Arc<Self>, args: RuleUndoArgs) -> Result<RulesView, IpcError> {
        let (guard, rules, writable) = self.lock_rules();
        if !writable {
            return Err(err(IpcErrorCode::Io, "ルール帳を書ける状態ではありません（rules.json を確認してください）"));
        }
        let next = undo_change(&rules, args.revision, args.force, now_ms()).map_err(|e| match e {
            UndoErr::NeedsConfirm => blocked(BlockedReason::RouterRuleUndoNeedsConfirm, "その後に同じルールを変えています。それでも取り消しますか"),
            UndoErr::NotFound => err(IpcErrorCode::NotFound, "取り消せる変更が見つかりません"),
            UndoErr::AlreadyUndone => err(IpcErrorCode::InvalidArgs, "すでに取り消しています"),
            UndoErr::LimitExceeded => err(IpcErrorCode::InvalidArgs, "戻すとルールの件数・字数の上限を超えるため取り消せません"),
        })?;
        self.write_rules(&next).map_err(|m| err(IpcErrorCode::Io, m))?;
        drop(guard);
        self.emit_router_head();
        Ok(self.rules_view(&next))
    }
}

/// チャットがアプリ上でアーカイブ（または一覧から外した）されているか。
fn chat_archived(d: &crate::host::state::HostData, chat: &ChatKey) -> bool {
    d.locals.get(chat).is_some_and(|l| !matches!(l.visibility, ListVisibility::Visible)) || d.chat(chat).is_some_and(|c| c.archived.value() == Some(&true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    fn accepted() -> SendState {
        SendState::Accepted { turn: TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("r".into()) }, turn_id: ExternalId("t1".into()) } }
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let d = crate::test_support::unique_temp_dir(&format!("agentdock-router-{tag}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn chat_of(k: &ChatKey, kind: ChatKind, cwd: &str, name: &str) -> Chat {
        Chat {
            key: k.clone(),
            kind,
            cwd: Known::direct(cwd.into()),
            name: Known::direct(name.into()),
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
            no_history: false,
            project_id: None,
        }
    }

    /// 受付係（r）と、通常のチャット（a）を持つホスト。
    fn host_with_router(tag: &str) -> (Arc<Host>, ChatKey, ChatKey) {
        let host = Arc::new(Host::new(unique_dir(tag)));
        let (r, a) = (key("r"), key("a"));
        host.mutate(|d| {
            let mut ev = d.upsert_chat(chat_of(&r, ChatKind::General, r"C:\x\router", "受付"));
            ev.extend(d.upsert_chat(chat_of(&a, ChatKind::Development, r"C:\work\a", "週次議事録")));
            ((), ev)
        });
        host.update_local(&r, false, std::time::Duration::ZERO, |f| {
            f.conversation = Some(ConversationRecord {
                id: LocalId("conv".into()),
                purpose: None,
                threads: vec![],
                seed: None,
                purpose_delivered_rev: None,
                compacted_since_delivery: false,
                replaces: None,
                intake: None,
                router: Some(RouterRecord::default()),
            })
        });
        (host, r, a)
    }

    fn sent_catalog(cm: &str, entries: &[(&str, &ChatKey)]) -> SentCatalog {
        SentCatalog { client_message_id: cm.into(), entries: entries.iter().map(|(i, k)| ((*i).to_string(), (*k).clone())).collect(), at: UnixMillis(1) }
    }

    fn reply_json(routes: &str, rule_changes: &str) -> String {
        format!(r#"{{"reply":"了解です","routes":[{routes}],"ruleChanges":[{rule_changes}]}}"#)
    }

    fn route(target: &str, text: &str) -> String {
        format!(r#"{{"target":"{target}","newChat":null,"text":"{text}","textIsOriginal":true,"reason":"理由","confidence":"high"}}"#)
    }

    fn envelope_for(user: &str) -> String {
        format!("[AgentDock 受付係の資料]\n## 候補チャット\nc1 『週次議事録』\n[ここまで資料。以下が利用者の依頼]\n{user}")
    }

    fn dispatch(id: &str, target: DispatchTarget, state: DispatchState) -> Dispatch {
        Dispatch { id: LocalId(id.into()), source_turn: "t".into(), route_index: 0, suggested: target.clone(), target, state, reported: false, proposal: None }
    }

    // ── 届いた記録（Accepted だけ） ──

    #[test]
    fn delivery_marks_are_written_only_when_the_send_is_accepted() {
        let inc = EnvelopeSections { instructions: Some(1), rules: Some(3), catalog_digest: Some("dg".into()), reported: vec![LocalId("d1".into())] };
        let base = || RouterRecord {
            compacted_since_delivery: true,
            dispatches: vec![dispatch("d1", DispatchTarget::Existing { chat: key("a") }, DispatchState::Sent { at: UnixMillis(1), client_message_id: "m".into() }), dispatch("d2", DispatchTarget::New { chat: None }, DispatchState::Proposed)],
            ..Default::default()
        };
        for not_accepted in [SendState::Sending, SendState::AcceptanceUnknown { since: UnixMillis(1) }, SendState::Rejected { message: "x".into() }, SendState::NotFoundAfterReconcile] {
            let mut r = base();
            assert!(!apply_delivery(&mut r, &inc, 0, &not_accepted), "{not_accepted:?}");
            assert_eq!(r, base(), "受理を確認できない送信は記録を変えない: {not_accepted:?}");
        }
        let mut r = base();
        assert!(apply_delivery(&mut r, &inc, 0, &accepted()));
        assert_eq!((r.instructions_delivered_rev, r.rules_delivered_rev, r.catalog_delivered_digest.as_deref(), r.compacted_since_delivery), (Some(1), Some(3), Some("dg"), false));
        assert!(r.dispatches[0].reported && !r.dispatches[1].reported, "資料に含めた結果だけ報告済みにする");
        // 資料に含めなかった節は触らない
        let mut r = RouterRecord { rules_delivered_rev: Some(2), ..Default::default() };
        apply_delivery(&mut r, &EnvelopeSections { catalog_digest: Some("x".into()), ..Default::default() }, 0, &accepted());
        assert_eq!((r.instructions_delivered_rev, r.rules_delivered_rev), (None, Some(2)));
    }

    #[test]
    fn dispatch_outcomes_never_turn_unknown_into_sent_or_rejected_into_sent() {
        let at = UnixMillis(5);
        assert_eq!(dispatch_outcome(&accepted(), "m", at), DispatchOutcome::Sent(DispatchState::Sent { at, client_message_id: "m".into() }));
        for s in [SendState::AcceptanceUnknown { since: at }, SendState::Sending] {
            assert_eq!(dispatch_outcome(&s, "m", at), DispatchOutcome::Unconfirmed(DispatchState::Unconfirmed { at, client_message_id: "m".into() }));
        }
        for s in [SendState::Rejected { message: "no".into() }, SendState::NotFoundAfterReconcile] {
            assert!(matches!(dispatch_outcome(&s, "m", at), DispatchOutcome::NotSent(_)));
        }
    }

    // ── 返答の処理 ──

    #[tokio::test]
    async fn a_completed_reply_makes_proposed_cards_and_requires_explicit_rule_edit() {
        let (host, r, a) = host_with_router("reply");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        let user = "定例会議の議事録は、今後は週次議事録へ送って";
        let json = reply_json(
            &format!("{},{}", route("c1", "議事録の依頼です"), route("c9", "存在しない")),
            r#"{"op":"add","ruleId":null,"text":"定例会議の議事録 → 週次議事録","target":"c1","newFolder":null,"quote":"今後は週次議事録へ送って"},{"op":"add","ruleId":null,"text":"x","target":null,"newFolder":null,"quote":"本文にない引用です"}"#,
        );
        host.router_apply_reply(&r, "t1", &stamp_router_request(&envelope_for(user), "cm1"), &json, false).await;

        let rec = host.router_record(&r).unwrap();
        assert_eq!(rec.dispatches.len(), 1, "連番c9は案にしない");
        let d = &rec.dispatches[0];
        assert_eq!((&d.state, &d.target, &d.source_turn, d.route_index), (&DispatchState::Proposed, &DispatchTarget::Existing { chat: a.clone() }, &"t1".to_string(), 0));
        assert_eq!(d.proposal.as_ref().unwrap().text, "議事録の依頼です");
        let t = &rec.turns[0];
        assert_eq!((t.reply.as_deref(), t.route_rejects.len()), (Some("了解です"), 1));
        assert_eq!(t.notices.len(), 2);
        assert_eq!(t.notices[0].revision, None);
        assert!(t.notices[0].text.contains("明示的に登録"));
        assert!(t.notices[1].revision.is_none() && t.notices[1].text.starts_with("ルールは更新していません"), "{:?}", t.notices[1]);

        let rules = host.router_rules().unwrap();
        assert_eq!((rules.revision, rules.rules.len()), (0, 0));
        assert!(!host.rules_path().exists());

        // 同じ turn をもう一度処理しても増えない
        host.router_apply_reply(&r, "t1", &stamp_router_request(&envelope_for(user), "cm1"), &json, false).await;
        assert_eq!(host.router_record(&r).unwrap().dispatches.len(), 1);
        let view = host.router_get().unwrap().unwrap();
        assert_eq!((view.dispatches.len(), view.turns.len(), view.turns[0].notices[0].undone), (1, 1, false));
        assert_eq!(view.dispatches[0].target_title.as_deref(), Some("週次議事録"));
    }

    #[tokio::test]
    async fn an_unreadable_reply_makes_no_cards_and_no_rule_writes() {
        let (host, r, a) = host_with_router("badreply");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        host.router_apply_reply(&r, "t1", &envelope_for("x"), "これはJSONではありません", false).await;
        let rec = host.router_record(&r).unwrap();
        assert!(rec.dispatches.is_empty());
        assert!(rec.turns[0].reply.is_none() && rec.turns[0].parse_error.is_some());
        assert!(!host.rules_path().exists());
    }

    #[tokio::test]
    async fn the_catalog_of_the_matching_send_is_used_not_a_later_one() {
        let (host, r, a) = host_with_router("catalog");
        let b = key("b");
        host.mutate(|d| ((), d.upsert_chat(chat_of(&b, ChatKind::Development, r"C:\work\b", "別件"))));
        // 古い送信 cm1 では c1=a、新しい送信 cm2 では c1=b。cm1 の返答は a へ向く。
        host.update_router(&r, |rec| {
            rec.push_catalog(sent_catalog("cm1", &[("c1", &a)]));
            rec.push_catalog(sent_catalog("cm2", &[("c1", &b)]));
        });
        let text = envelope_for("依頼");
        host.remember_pending(PendingSend { chat: r.clone(), client_message_id: "cm1".into(), full_text: text.clone(), turn: None });
        host.router_apply_reply(&r, "t1", &text, &reply_json(&route("c1", "本文"), ""), false).await;
        assert_eq!(host.router_record(&r).unwrap().dispatches[0].target, DispatchTarget::Existing { chat: a });
    }

    /// 送り先を今回の起動で再開済み（live）にする（作成直後・再開後と同じ状態）。
    fn make_live(host: &Arc<Host>, chat: &ChatKey, state: AgentState) {
        let ak = AgentKey { backend: chat.backend, id: chat.id.clone() };
        let agent = Agent {
            key: ak,
            chat: chat.clone(),
            parent: ParentLink::Root,
            forked_from: Known::NotFetched,
            display_name: Known::NotFetched,
            role: Known::NotFetched,
            assignment: Known::NotFetched,
            agent_path: Known::NotFetched,
            model: Known::NotFetched,
            effort: Known::NotFetched,
            spawn_spec: Known::NotFetched,
            latest_turn: None,
        };
        let status = AgentStatus {
            state,
            raw: RawState { label: "test".into() },
            scope: StateScope::Agent,
            turn: None,
            wait: None,
            evidence: Evidence { source: EvidenceSource::Response, raw_label: Some("test".into()), source_time: None, observed_at: now_ms() },
        };
        host.mutate(|d| ((), d.set_live(agent, status)));
    }

    /// 接続済み（Codex の監視元が Connected）にする。
    fn make_connected(host: &Arc<Host>) {
        host.mutate(|d| {
            d.sources.push(SourceInfo {
                source: SourceId("s".into()),
                backend: BackendKind::Codex,
                pid: Known::NotFetched,
                started_at: UnixMillis(0),
                version: VersionCheck::Unknown { message: "t".into() },
                capabilities: crate::codex::adapter::codex_capabilities(true),
                connection: ConnectionState::Connected,
                sandbox_workaround: false,
            });
            ((), Vec::new())
        });
    }

    fn go(id: &str) -> RouterDispatchArgs {
        RouterDispatchArgs { dispatch_id: LocalId(id.into()), target_override: None, text: "週次の議事録をまとめて".into(), intent: None }
    }

    fn queue_len(host: &Arc<Host>, chat: &ChatKey) -> usize {
        host.read(|d| d.queues.get(chat).map_or(0, |f| f.queue.entries.len()))
    }

    // ── 修正A: 手動［送る］は、実行中の送り先だけキュー。それ以外は利用者の送信（ensure_live を含む）で直接送る ──

    /// 履歴だけ（HistoryOnly・非実行）の送り先へ intent なしで送る。ensure_live（再開）を通って送られ、カードは Sent・キュー登録なし。
    /// 修正前はキューに入り、live でない送り先は永久に保留された（`dispatch-not-starting-diagnosis-2026-11-26.md`）。
    #[cfg(windows)]
    #[tokio::test]
    async fn a_manual_send_to_a_history_only_idle_target_resumes_and_sends_directly() {
        let dir = unique_dir("manual-direct");
        let (host, r, _a) = host_with_router("manual-direct-host");
        let confirmed = UserConfirmed::from_user_command();
        // 偽の agy（受け取った引数を記録し、固定の出力を返す）。実機の agy は使わない。
        let out = [
            r#"{"event":"init","conversation_id":"9db82f11-0000","init":{"model":"gemini-test-1","cwd":"C:\\work","permission_mode":"request-review"}}"#,
            r#"{"event":"result","result":{"conversation_id":"9db82f11-0000","status":"SUCCESS","response":"PONG\n","usage":{"total_tokens":1}}}"#,
        ];
        std::fs::write(dir.join("out.ndjson"), out.join("\r\n") + "\r\n").unwrap();
        let script = dir.join("fake-agy.cmd");
        std::fs::write(&script, "@echo off\r\necho %* > \"%~dp0args.txt\"\r\ntype \"%~dp0out.ndjson\"\r\nexit /b 0\r\n").unwrap();
        host.backend.attach_antigravity_for_test(&script.to_string_lossy(), &host.app_data_dir.to_string_lossy());
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let started = host
            .start_chat(StartChatArgs { backend: BackendKind::Antigravity, cwd: Some(work.to_string_lossy().into_owned()), model: Some(ModelChoice { model: "gemini-test-1".into(), effort: None, speed_tier: None }), permission: None, first_message: None, worktree: None, extra_folders: vec![], confirmed_strong: vec![] }, &confirmed)
            .await
            .unwrap();
        let target = started.chat.key.clone();
        // 今回の起動で未再開（再起動後の既存チャットと同じ HistoryOnly）にする。
        host.mutate(|d| ((), d.set_agent_freshness(&AgentKey { backend: target.backend, id: target.id.clone() }, Freshness::HistoryOnly, UnixMillis(1))));
        assert_eq!(host.read(|d| d.root_view(&target).map(|v| (v.freshness, v.status.state))), Some((Freshness::HistoryOnly, AgentState::Idle)));
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: target.clone() }, DispatchState::Proposed)));

        let v = host.router_dispatch(go("d1"), &confirmed).await.unwrap();
        assert!(matches!(v.state, DispatchState::Sent { .. }), "直接送信で Sent: {:?}", v.state);
        assert_eq!(queue_len(&host, &target), 0, "キューには入れない");
        assert_eq!(host.read(|d| d.root_view(&target).map(|v| v.freshness)), Some(Freshness::Live), "ensure_live で再開済みになる");
        assert!(host.router_record(&r).unwrap().dispatch_turns.contains_key("d1"), "結果の追跡用に担当 turn を記録する");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 総点検の判断待ち: 親が live のまま状態が不明（StateUnknown が親だけを指す）の保留に、画面の出口（再開で状態を取り直す） ──

    /// 親が live のまま状態が不明の保留は、`resume_queue`（「このチャットを再開して送る」）で状態を取り直せる。従来は「キューはすでに有効です」で断られ、
    /// 画面にも出口がなかった。親が原因でない保留（親が作業中・子孫が原因）は、従来どおり「すでに有効」で断る。
    #[cfg(windows)]
    #[tokio::test]
    async fn resume_queue_refreshes_a_live_parent_whose_state_is_unknown_and_still_refuses_other_holds() {
        use crate::backend::local::{ChatArgs, HoldTarget, QueueHold};
        let dir = unique_dir("refresh-live");
        let (host, _r, _a) = host_with_router("refresh-live-host");
        let confirmed = UserConfirmed::from_user_command();
        host.backend.attach_antigravity_for_test("fake-agy-is-never-run.cmd", &host.app_data_dir.to_string_lossy());
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let started = host
            .start_chat(StartChatArgs { backend: BackendKind::Antigravity, cwd: Some(work.to_string_lossy().into_owned()), model: Some(ModelChoice { model: "gemini-test-1".into(), effort: None, speed_tier: None }), permission: None, first_message: None, worktree: None, extra_folders: vec![], confirmed_strong: vec![] }, &confirmed)
            .await
            .unwrap();
        let target = started.chat.key.clone();
        let root = AgentKey { backend: target.backend, id: target.id.clone() };
        // live のまま、状態が不明（切断・再起動をまたいだ古い状態ではなく、いまの接続で状態が取れていない）。
        make_live(&host, &target, AgentState::Unknown);
        assert_eq!(host.read(|d| d.root_view(&target).map(|v| (v.freshness, v.status.state))), Some((Freshness::Live, AgentState::Unknown)));
        let hold_on = |hold: QueueHold| {
            host.mutate(|d| {
                let mut queue = crate::rules::queue::new_queue(target.clone());
                crate::rules::queue::enqueue(&mut queue, LocalId("e1".into()), "次の依頼".into(), vec![], UnixMillis(1));
                queue.hold = Some(hold);
                d.queues.insert(target.clone(), crate::store::records::QueueFile { schema_version: crate::store::records::SCHEMA_VERSION, queue, unresolved_sends: Vec::new() });
                ((), vec![])
            });
        };
        let unknown_root = |t: &AgentKey| HoldTarget { agent: t.clone(), state: AgentState::Unknown, freshness: Freshness::Live };
        // 親だけが原因でない保留は、従来どおり「すでに有効」で断る（再開・取り直しをしない）。
        for hold in [
            QueueHold::ParentWorking,
            QueueHold::StateUnknown { targets: vec![unknown_root(&AgentKey { backend: target.backend, id: ExternalId("child".into()) })], descendant_scan_incomplete: false },
            QueueHold::StateUnknown { targets: vec![unknown_root(&root)], descendant_scan_incomplete: true },
        ] {
            hold_on(hold.clone());
            let e = host.resume_queue(ChatArgs { chat: target.clone() }, &confirmed).await.unwrap_err();
            assert!(e.message.contains("すでに有効"), "{hold:?}: {}", e.message);
            assert_eq!(host.read(|d| d.root_view(&target).map(|v| v.status.state)), Some(AgentState::Unknown), "取り直していない: {hold:?}");
        }
        // 親だけが原因の保留（live のまま不明）は、再開で状態を取り直す。応答の状態（待機中）になり、続く評価で保留が解ける。
        hold_on(QueueHold::StateUnknown { targets: vec![unknown_root(&root)], descendant_scan_incomplete: false });
        host.resume_queue(ChatArgs { chat: target.clone() }, &confirmed).await.expect("取り直せる");
        assert_eq!(host.read(|d| d.root_view(&target).map(|v| (v.freshness, v.status.state))), Some((Freshness::Live, AgentState::Idle)), "状態が取り直されている");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 実行中の送り先へ intent なしで送るときは従来どおり順番待ち（Queued）。直接は送らない。
    #[tokio::test]
    async fn a_manual_send_to_a_running_target_is_still_queued() {
        let (host, r, a) = host_with_router("manual-running");
        let confirmed = UserConfirmed::from_user_command();
        make_connected(&host);
        make_live(&host, &a, AgentState::Running);
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        let v = host.router_dispatch(go("d1"), &confirmed).await.unwrap();
        let DispatchState::Queued { queue_entry, .. } = &v.state else { panic!("実行中の送り先は順番待ち: {:?}", v.state) };
        assert_eq!(queue_len(&host, &a), 1);
        host.read(|d| assert_eq!(&d.queues.get(&a).unwrap().queue.entries[0].id, queue_entry));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    fn agy_key(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Antigravity, id: ExternalId(id.into()) }
    }

    /// Antigravity 宛で本文が agy のコマンドライン長の上限を超えるなら、順番待ちへ入れる前にも、直接送る前にも、分かる理由で止める。
    /// カードは提案のまま。Codex 宛には影響しない（同じ長さでも順番待ちに入る）。
    #[tokio::test]
    async fn a_dispatch_over_the_agy_command_line_limit_is_refused_before_queueing_and_does_not_touch_codex_targets() {
        let (host, r, a) = host_with_router("agy-too-long");
        let confirmed = UserConfirmed::from_user_command();
        make_connected(&host);
        let agy = agy_key("agy-1");
        host.mutate(|d| ((), d.upsert_chat(chat_of(&agy, ChatKind::Development, r"C:workagy", "Antigravity担当"))));
        make_live(&host, &agy, AgentState::Running);
        make_live(&host, &a, AgentState::Running);
        let long = "あ".repeat(crate::antigravity::cmd::WINDOWS_COMMAND_LINE_MAX_UNITS);
        let args = |id: &str| RouterDispatchArgs { dispatch_id: LocalId(id.into()), target_override: None, text: long.clone(), intent: None };
        host.update_router(&r, |rec| {
            rec.push_dispatch(dispatch("d-agy", DispatchTarget::Existing { chat: agy.clone() }, DispatchState::Proposed));
            rec.push_dispatch(dispatch("d-codex", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed));
        });
        // 実行中の Antigravity 宛（順番待ちになる経路）。
        let e = host.router_dispatch(args("d-agy"), &confirmed).await.unwrap_err();
        assert_eq!(e.code, IpcErrorCode::InvalidArgs);
        assert!(e.message.contains("本文が長すぎるため送っていません") && !e.message.contains("os error"), "{}", e.message);
        assert_eq!(queue_len(&host, &agy), 0, "送れない本文は順番待ちへ入れない");
        let state_of = |id: &str| host.router_record(&r).unwrap().dispatches.iter().find(|d| d.id.0 == id).unwrap().state.clone();
        assert_eq!(state_of("d-agy"), DispatchState::Proposed);
        assert!(host.router_rt.dispatching.lock().unwrap().is_empty(), "送信中の印は外れる");
        // 実行中でない Antigravity 宛（直接送る経路）も、再開や起動の前に止める。
        make_live(&host, &agy, AgentState::Idle);
        let e = host.router_dispatch(args("d-agy"), &confirmed).await.unwrap_err();
        assert!(e.message.contains("本文が長すぎるため送っていません"), "{}", e.message);
        assert_eq!(state_of("d-agy"), DispatchState::Proposed);
        // 収まる本文は止めない（Codex 宛の経路と同じく順番待ちに入る）。
        make_live(&host, &agy, AgentState::Running);
        let ok = RouterDispatchArgs { dispatch_id: LocalId("d-agy".into()), target_override: None, text: "短い依頼".into(), intent: None };
        assert!(matches!(host.router_dispatch(ok, &confirmed).await.unwrap().state, DispatchState::Queued { .. }));
        // Codex 宛は同じ長さでも検査しない。
        let v = host.router_dispatch(args("d-codex"), &confirmed).await.unwrap();
        assert!(matches!(v.state, DispatchState::Queued { .. }), "{:?}", v.state);
        assert_eq!(queue_len(&host, &a), 1);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 通常のチャットの「完了後に送る依頼」（enqueue・編集）も、Antigravity 宛で長すぎる本文は登録しない。Codex 宛は登録できる。
    #[tokio::test]
    async fn the_queue_refuses_an_agy_text_over_the_limit_and_accepts_it_for_codex() {
        let (host, _r, a) = host_with_router("agy-queue-too-long");
        let agy = agy_key("agy-1");
        host.mutate(|d| ((), d.upsert_chat(chat_of(&agy, ChatKind::Development, r"C:workagy", "Antigravity担当"))));
        let long = "あ".repeat(crate::antigravity::cmd::WINDOWS_COMMAND_LINE_MAX_UNITS);
        let e = host.enqueue(EnqueueArgs { chat: agy.clone(), text: long.clone(), attachments: Vec::new() }).unwrap_err();
        assert_eq!(e.code, IpcErrorCode::InvalidArgs);
        assert!(e.message.contains("本文が長すぎるため送っていません"), "{}", e.message);
        assert_eq!(queue_len(&host, &agy), 0);
        let entry = host.enqueue(EnqueueArgs { chat: agy.clone(), text: "短い依頼".into(), attachments: Vec::new() }).unwrap();
        // 登録済みの項目を、長すぎる本文へ編集することも止める（項目は元の本文のまま）。
        let e = host.edit_queue_entry(crate::backend::local::EditQueueEntryArgs { chat: agy.clone(), entry: entry.id.clone(), text: long.clone(), attachments: Vec::new() }).unwrap_err();
        assert!(e.message.contains("本文が長すぎるため送っていません"), "{}", e.message);
        host.read(|d| assert_eq!(d.queues.get(&agy).unwrap().queue.entries[0].text, "短い依頼"));
        // Codex 宛は検査しない。
        assert!(host.enqueue(EnqueueArgs { chat: a.clone(), text: long, attachments: Vec::new() }).is_ok());
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// Codex が起動していない（接続されていない）とき、順番待ちになる送信は、その理由で断る（「AIに接続してから」だけの分かりにくい文にしない）。
    #[tokio::test]
    async fn a_manual_send_that_would_queue_says_codex_is_not_running_when_it_is_not_connected() {
        let (host, r, a) = host_with_router("codex-down-enqueue");
        let confirmed = UserConfirmed::from_user_command();
        make_live(&host, &a, AgentState::Running);
        assert!(!host.is_connected());
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        let e = host.router_dispatch(go("d1"), &confirmed).await.unwrap_err();
        assert_eq!(e.message, CODEX_NOT_RUNNING_ENQUEUE);
        assert!(e.message.starts_with("Codex が起動していない") && e.message.contains("Antigravity 宛も"), "{}", e.message);
        assert_eq!(queue_len(&host, &a), 0);
        assert_eq!(host.router_record(&r).unwrap().dispatches[0].state, DispatchState::Proposed);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 保存済みの状態が実行中でも live でない送り先（切断後・履歴の読み取りだけ）は、キューに入れない（駆動側は再開せず永久に保留されるため）。直接送信に回る。
    #[tokio::test]
    async fn a_manual_send_to_a_not_live_target_whose_stored_state_is_running_is_not_queued() {
        let (host, r, a) = host_with_router("manual-stale-running");
        let confirmed = UserConfirmed::from_user_command();
        make_connected(&host);
        host.mutate(|d| {
            let agent = Agent { key: AgentKey { backend: a.backend, id: a.id.clone() }, chat: a.clone(), parent: ParentLink::Root, forked_from: Known::NotFetched, display_name: Known::NotFetched, role: Known::NotFetched, assignment: Known::NotFetched, agent_path: Known::NotFetched, model: Known::NotFetched, effort: Known::NotFetched, spawn_spec: Known::NotFetched, latest_turn: None };
            let status = AgentStatus { state: AgentState::Running, raw: RawState { label: "t".into() }, scope: StateScope::Agent, turn: None, wait: None, evidence: Evidence { source: EvidenceSource::HistoryRead, raw_label: None, source_time: None, observed_at: UnixMillis(1) } };
            ((), d.upsert_history_agent(agent, status))
        });
        assert_eq!(host.read(|d| d.root_view(&a).map(|v| (v.freshness, v.status.state))), Some((Freshness::HistoryOnly, AgentState::Running)));
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        // 直接送信に回る（この試験では再開できず送れないのでエラー）。キューには入らず、カードは提案のまま。
        assert!(host.router_dispatch(go("d1"), &confirmed).await.is_err());
        assert_eq!(queue_len(&host, &a), 0);
        assert_eq!(host.router_record(&r).unwrap().dispatches[0].state, DispatchState::Proposed);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 履歴がなく再開できない送り先（発話前のチャットの再起動後など）は、エラーを返し、カードは Proposed のまま・キューにも入れない。
    #[tokio::test]
    async fn a_manual_send_to_a_target_that_cannot_be_resumed_errors_and_stays_proposed() {
        let (host, r, a) = host_with_router("manual-noresume");
        let confirmed = UserConfirmed::from_user_command();
        make_connected(&host);
        host.mutate(|d| {
            let mut c = chat_of(&a, ChatKind::Development, r"C:\work\a", "週次議事録");
            c.no_history = true;
            ((), d.upsert_chat(c))
        });
        assert!(host.read(|d| d.root_view(&a).is_none()), "エージェントの状態を持たない（再開もできない）");
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        let e = host.router_dispatch(go("d1"), &confirmed).await.unwrap_err();
        assert!(e.message.contains("履歴がない"), "{}", e.message);
        let rec = host.router_record(&r).unwrap();
        assert_eq!(rec.dispatches.iter().find(|d| d.id.0 == "d1").unwrap().state, DispatchState::Proposed);
        assert_eq!(queue_len(&host, &a), 0);
        assert!(host.router_rt.dispatching.lock().unwrap().is_empty(), "送信中の印は外れる");
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 修正B: 自動送信は、送り先が live でなければキューに登録せず、カードを Proposed のまま理由を出す ──

    const AUTO_ROUTE: &str = r#"{"target":"c1","newChat":null,"text":"週次議事録の要約を作成してください。","textIsOriginal":false,"reason":"週次議事録担当","confidence":"high"}"#;

    #[tokio::test]
    async fn an_automatic_send_to_a_target_that_is_not_live_is_not_queued_and_says_why() {
        let (host, r, a) = host_with_router("auto-notlive");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("live", &[("c1", &a)])));
        // 履歴だけ（HistoryOnly）の送り先
        host.mutate(|d| {
            let agent = Agent { key: AgentKey { backend: a.backend, id: a.id.clone() }, chat: a.clone(), parent: ParentLink::Root, forked_from: Known::NotFetched, display_name: Known::NotFetched, role: Known::NotFetched, assignment: Known::NotFetched, agent_path: Known::NotFetched, model: Known::NotFetched, effort: Known::NotFetched, spawn_spec: Known::NotFetched, latest_turn: None };
            let status = AgentStatus { state: AgentState::Idle, raw: RawState { label: "t".into() }, scope: StateScope::Agent, turn: None, wait: None, evidence: Evidence { source: EvidenceSource::HistoryRead, raw_label: None, source_time: None, observed_at: UnixMillis(1) } };
            ((), d.upsert_history_agent(agent, status))
        });
        assert_eq!(host.read(|d| d.root_view(&a).map(|v| v.freshness)), Some(Freshness::HistoryOnly));
        let live = stamp_router_request(&envelope_for("自動送信: a\n週次の議事録をまとめて"), "live");
        host.router_apply_reply(&r, "t-live", &live, &reply_json(AUTO_ROUTE, ""), true).await;
        let view = host.router_get().unwrap().unwrap();
        assert_eq!(view.dispatches.len(), 1);
        assert!(matches!(view.dispatches[0].state, DispatchState::Proposed), "カードは提案のまま: {:?}", view.dispatches[0].state);
        assert_eq!(queue_len(&host, &a), 0, "キューに登録しない");
        let rejects = &view.turns[0].route_rejects;
        assert_eq!(rejects.len(), 1);
        assert!(rejects[0].contains("自動送信を保留しました") && rejects[0].contains("再開されていない") && rejects[0].contains("［送る］"), "{}", rejects[0]);
        assert_eq!(host.read(|d| d.root_view(&a).map(|v| v.freshness)), Some(Freshness::HistoryOnly), "自動で再開しない");
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// live でも状態が取れていない（Closed／Unknown）送り先は、キューが StateUnknown で止めるため、自動送信を登録しない。
    #[tokio::test]
    async fn an_automatic_send_to_a_live_target_whose_state_is_closed_is_not_queued() {
        let (host, r, a) = host_with_router("auto-closed");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("live", &[("c1", &a)])));
        make_live(&host, &a, AgentState::Closed);
        let live = stamp_router_request(&envelope_for("自動送信: a
週次の議事録をまとめて"), "live");
        host.router_apply_reply(&r, "t-live", &live, &reply_json(AUTO_ROUTE, ""), true).await;
        let view = host.router_get().unwrap().unwrap();
        assert!(matches!(view.dispatches[0].state, DispatchState::Proposed));
        assert_eq!(queue_len(&host, &a), 0);
        assert!(view.turns[0].route_rejects[0].contains("自動送信を保留しました"));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 総点検: 自動送信を指定したのに送られないときは理由を出す ──

    fn auto_dispatch(target: DispatchTarget, confidence: &str, original: bool) -> Dispatch {
        Dispatch {
            proposal: Some(DispatchProposal { text: "整えた文".into(), text_is_original: original, reason: "理由".into(), confidence: confidence.into(), new_chat: None }),
            ..dispatch("d1", target, DispatchState::Proposed)
        }
    }

    #[test]
    fn the_automatic_send_decision_names_the_first_unmet_condition() {
        let a = DispatchTarget::Existing { chat: key("a") };
        let ok = || vec![auto_dispatch(a.clone(), "high", false)];
        let user = "自動送信: a\n依頼";
        let decide = |user: &str, allow: bool, head: bool, ds: &[Dispatch], rejects_empty: bool, rules_empty: bool| auto_send_decision(user, allow, head, ds, rejects_empty, rules_empty);
        assert_eq!(decide("依頼だけ", true, true, &ok(), true, true), AutoSend::NotRequested, "指定がなければ理由も出さない");
        assert!(matches!(decide(user, true, true, &ok(), true, true), AutoSend::Send { .. }));
        let skip = |d: AutoSend| match d {
            AutoSend::Skip(m) => m,
            other => panic!("見送りのはず: {other:?}"),
        };
        assert!(skip(decide(user, false, true, &ok(), true, true)).contains("照合できない経路"));
        assert!(skip(decide(user, true, false, &ok(), true, true)).contains("今の受付ではない"));
        assert!(skip(decide(user, true, true, &[], true, true)).contains("出なかった"));
        let two = vec![auto_dispatch(a.clone(), "high", false), auto_dispatch(a.clone(), "high", false)];
        assert!(skip(decide(user, true, true, &two, true, true)).contains("複数"));
        assert!(skip(decide(user, true, true, &ok(), false, true)).contains("出せなかった案"));
        assert!(skip(decide(user, true, true, &ok(), true, false)).contains("ルールの変更"));
        assert!(skip(decide(user, true, true, &[auto_dispatch(DispatchTarget::New { chat: None }, "high", false)], true, true)).contains("新規"));
        assert!(skip(decide("自動送信: zzz\n依頼", true, true, &ok(), true, true)).contains("一致しない"));
        assert!(skip(decide(user, true, true, &[auto_dispatch(a.clone(), "medium", false)], true, true)).contains("確信度"));
        assert!(skip(decide(user, true, true, &[auto_dispatch(a.clone(), "high", true)], true, true)).contains("原文のまま"));
    }

    /// 自動送信を指定したのに条件を満たさなかった返答は、カードを提案のまま残すだけでなく、その理由を受付の返答（route_rejects）へ出す。
    /// 指定のない通常の依頼には出さない。
    #[tokio::test]
    async fn an_unmet_automatic_send_says_why_on_the_reply_and_an_ordinary_request_does_not() {
        let (host, r, a) = host_with_router("auto-skip-reason");
        make_live(&host, &a, AgentState::Idle);
        host.update_router(&r, |rec| {
            rec.push_catalog(sent_catalog("orig", &[("c1", &a)]));
            rec.push_catalog(sent_catalog("recovered", &[("c1", &a)]));
            rec.push_catalog(sent_catalog("plain", &[("c1", &a)]));
        });
        // 本文が原文のまま（整えていない）。
        let orig = stamp_router_request(&envelope_for("自動送信: a\n週次の議事録をまとめて"), "orig");
        host.router_apply_reply(&r, "t-orig", &orig, &reply_json(&route("c1", "週次の議事録をまとめて"), ""), true).await;
        // 再起動後の取り込み（送信時の記録と照合できない経路。allow_auto = false）。
        let recovered = stamp_router_request(&envelope_for("自動送信: a\n週次の議事録をまとめて"), "recovered");
        host.router_apply_reply(&r, "t-recovered", &recovered, &reply_json(AUTO_ROUTE, ""), false).await;
        // 自動送信の指定なしの通常の依頼。
        let plain = stamp_router_request(&envelope_for("週次の議事録をまとめて"), "plain");
        host.router_apply_reply(&r, "t-plain", &plain, &reply_json(AUTO_ROUTE, ""), true).await;
        let view = host.router_get().unwrap().unwrap();
        let rejects = |turn: &str| view.turns.iter().find(|t| t.turn == turn).unwrap().route_rejects.clone();
        let r_orig = rejects("t-orig");
        assert_eq!(r_orig.len(), 1, "{r_orig:?}");
        assert!(r_orig[0].contains("自動送信はしませんでした") && r_orig[0].contains("原文のまま") && r_orig[0].contains("［送る］"), "{}", r_orig[0]);
        let r_rec = rejects("t-recovered");
        assert!(r_rec.len() == 1 && r_rec[0].contains("照合できない経路"), "{r_rec:?}");
        assert!(rejects("t-plain").is_empty(), "指定のない依頼には出さない");
        assert_eq!(queue_len(&host, &a), 0);
        assert!(view.dispatches.iter().all(|d| matches!(d.state, DispatchState::Proposed)));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 総点検: キュー登録に失敗したカードは提案へ戻す（順番待ちのまま出口がなくならない） ──

    /// 登録に失敗した（ここでは空の本文）カードを「順番待ち」のまま残すと、［送る］も［見送る］も出ず、結果欄に失敗が出るだけで止まる。
    /// 提案へ戻し、エラーを返し、もう一度［送る］で順番待ちに入れられる。
    #[tokio::test]
    async fn a_failed_queue_registration_returns_the_card_to_proposed_so_it_can_be_sent_again() {
        let (host, r, a) = host_with_router("enqueue-failed");
        let confirmed = UserConfirmed::from_user_command();
        make_connected(&host);
        make_live(&host, &a, AgentState::Running);
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        let e = host.router_enqueue(&r, &LocalId("d1".into()), a.clone(), "  ".into()).await.unwrap_err();
        assert!(e.message.contains("空"), "{}", e.message);
        let rec = host.router_record(&r).unwrap();
        assert_eq!(rec.dispatches[0].state, DispatchState::Proposed, "順番待ちのまま残さない");
        assert!(rec.dispatch_results.is_empty() && rec.dispatch_result_status.is_empty(), "失敗は結果欄ではなくエラーで返す");
        assert_eq!(queue_len(&host, &a), 0);
        let v = host.router_dispatch(go("d1"), &confirmed).await.unwrap();
        assert!(matches!(v.state, DispatchState::Queued { .. }), "押し直せば順番待ちに入る: {:?}", v.state);
        assert_eq!(queue_len(&host, &a), 1);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 総点検: 受理不明だった直接送信の照合結果をカードへ反映する ──

    /// 受理不明（Unconfirmed）のカードは、履歴照合（自動・手動）で受理と確定したら Sent（担当 turn を記録）、受理なしと確定したら提案へ戻す。
    /// 反映しないと、受理されていても「送れたか確認できません」のまま結果が回収されず、受理なしでも再び送れない。別の送信の照合は無関係。
    #[tokio::test]
    async fn an_unconfirmed_card_follows_the_reconciliation_of_its_send() {
        let (host, r, a) = host_with_router("reconciled");
        let unconfirmed = |id: &str, cm: &str, target: &ChatKey| dispatch(id, DispatchTarget::Existing { chat: target.clone() }, DispatchState::Unconfirmed { at: UnixMillis(5), client_message_id: cm.into() });
        host.update_router(&r, |rec| {
            rec.push_dispatch(unconfirmed("d-ok", "cm-ok", &a));
            rec.push_dispatch(unconfirmed("d-none", "cm-none", &a));
            rec.push_dispatch(unconfirmed("d-other", "cm-other", &a));
        });
        let state_of = |host: &Arc<Host>, id: &str| host.router_record(&r).unwrap().dispatches.into_iter().find(|d| d.id.0 == id).unwrap().state;
        let turn = TurnKey { agent: AgentKey { backend: a.backend, id: a.id.clone() }, turn_id: ExternalId("turn-9".into()) };
        // 別のチャット・別の送信の照合は、このカードに触れない。
        host.router_note_reconciled(&key("zzz"), "cm-ok", Some(&turn));
        host.router_note_reconciled(&a, "cm-unrelated", Some(&turn));
        assert!(matches!(state_of(&host, "d-ok"), DispatchState::Unconfirmed { .. }));
        host.router_note_reconciled(&a, "cm-ok", Some(&turn));
        assert_eq!(state_of(&host, "d-ok"), DispatchState::Sent { at: UnixMillis(5), client_message_id: "cm-ok".into() });
        assert_eq!(host.router_record(&r).unwrap().dispatch_turns.get("d-ok"), Some(&turn), "結果の回収のため担当 turn を記録する");
        host.router_note_reconciled(&a, "cm-none", None);
        assert_eq!(state_of(&host, "d-none"), DispatchState::Proposed, "受理なしが確定したら押し直せる");
        assert!(matches!(state_of(&host, "d-other"), DispatchState::Unconfirmed { .. }));
        // 確定後の再照合・Sent のカードは変えない。
        host.router_note_reconciled(&a, "cm-ok", None);
        assert!(matches!(state_of(&host, "d-ok"), DispatchState::Sent { .. }));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 追加指示として送られて受理された順番待ちの項目は、カードを「取り消し」にせず Sent（担当 turn つき）にする。別の項目のカードは変えない。
    #[tokio::test]
    async fn a_queued_card_whose_entry_was_sent_as_a_steer_becomes_sent() {
        let (host, r, a) = host_with_router("steered");
        let queued = |id: &str, entry: &str| dispatch(id, DispatchTarget::Existing { chat: a.clone() }, DispatchState::Queued { at: UnixMillis(7), queue_entry: LocalId(entry.into()) });
        host.update_router(&r, |rec| {
            rec.push_dispatch(queued("d1", "q1"));
            rec.push_dispatch(queued("d2", "q2"));
        });
        let turn = TurnKey { agent: AgentKey { backend: a.backend, id: a.id.clone() }, turn_id: ExternalId("turn-3".into()) };
        host.router_note_steered(&a, &LocalId("q1".into()), &turn, "cm-steer");
        let rec = host.router_record(&r).unwrap();
        let st = |id: &str| rec.dispatches.iter().find(|d| d.id.0 == id).unwrap().state.clone();
        assert_eq!(st("d1"), DispatchState::Sent { at: UnixMillis(7), client_message_id: "cm-steer".into() });
        assert!(matches!(st("d2"), DispatchState::Queued { .. }));
        assert_eq!(rec.dispatch_turns.get("d1"), Some(&turn));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 「実行中です。順番待ちとして送ります」の表示は、［送る］が順番待ちにする条件（live かつ実行中）と同じ。
    /// 再開されていない送り先は、実行中と保存されていても直接送る（再開して新しい turn）ので、順番待ちとは見せない。
    #[tokio::test]
    async fn the_card_shows_the_target_as_running_only_when_a_manual_send_would_queue() {
        let (host, r, a) = host_with_router("running-view");
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Proposed)));
        host.mutate(|d| {
            let agent = Agent { key: AgentKey { backend: a.backend, id: a.id.clone() }, chat: a.clone(), parent: ParentLink::Root, forked_from: Known::NotFetched, display_name: Known::NotFetched, role: Known::NotFetched, assignment: Known::NotFetched, agent_path: Known::NotFetched, model: Known::NotFetched, effort: Known::NotFetched, spawn_spec: Known::NotFetched, latest_turn: None };
            let status = AgentStatus { state: AgentState::Running, raw: RawState { label: "t".into() }, scope: StateScope::Agent, turn: None, wait: None, evidence: Evidence { source: EvidenceSource::HistoryRead, raw_label: None, source_time: None, observed_at: UnixMillis(1) } };
            ((), d.upsert_history_agent(agent, status))
        });
        assert!(!host.router_get().unwrap().unwrap().dispatches[0].target_running, "保存済みの状態が実行中でも live でなければ順番待ちにならない");
        make_live(&host, &a, AgentState::Running);
        assert!(host.router_get().unwrap().unwrap().dispatches[0].target_running);
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    /// 返答を読み取れなかった（履歴の読み取り失敗など）とき、その turn の送信の記録をメモリから外す。残すと、復旧（router_refresh）が
    /// 処理中の送信として除外し、再起動するまで返答が処理されないままになる。
    #[tokio::test]
    async fn a_reply_that_could_not_be_read_is_handed_to_the_recovery_path() {
        let (host, r, _a) = host_with_router("hand-to-refresh");
        host.remember_pending(PendingSend { chat: r.clone(), client_message_id: "cm1".into(), full_text: "本文".into(), turn: Some("t1".into()) });
        host.remember_pending(PendingSend { chat: r.clone(), client_message_id: "cm2".into(), full_text: "別の本文".into(), turn: Some("t2".into()) });
        host.router_process_turn(r.clone(), "t1".into()).await;
        let pending: Vec<String> = host.router_rt.pending.lock().unwrap().iter().map(|p| p.client_message_id.clone()).collect();
        assert_eq!(pending, vec!["cm2".to_string()], "読み取れなかった turn の分だけ外す");
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    // ── 修正D: 受付カードに、送信待ちの保留理由を出す ──

    #[tokio::test]
    async fn a_waiting_entry_in_an_active_queue_with_a_hold_shows_the_reason_on_the_card() {
        let (host, r, a) = host_with_router("queue-held");
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("d1", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Queued { at: UnixMillis(1), queue_entry: LocalId("q1".into()) })));
        let mut queue = crate::rules::queue::new_queue(a.clone());
        crate::rules::queue::enqueue(&mut queue, LocalId("q1".into()), "本文".into(), vec![], UnixMillis(1));
        host.mutate(|d| {
            d.queues.insert(a.clone(), crate::store::records::QueueFile { schema_version: crate::store::records::SCHEMA_VERSION, queue, unresolved_sends: Vec::new() });
            ((), Vec::new())
        });
        let status_of = |host: &Arc<Host>| {
            let v = host.router_get().unwrap().unwrap();
            (v.dispatches[0].execution_status, v.dispatches[0].execution_result.clone())
        };
        // 保留理由なし・親が作業中（通常の待ち）は従来どおり何も返さない（画面は「送信待ち」）。
        assert_eq!(status_of(&host), (None, None));
        let target = || crate::backend::local::HoldTarget { agent: AgentKey { backend: a.backend, id: a.id.clone() }, state: AgentState::Idle, freshness: Freshness::HistoryOnly };
        let set_hold = |hold: Option<crate::backend::local::QueueHold>| {
            host.mutate(|d| {
                d.queues.get_mut(&a).unwrap().queue.hold = hold;
                ((), Vec::new())
            })
        };
        set_hold(Some(crate::backend::local::QueueHold::ParentWorking));
        assert_eq!(status_of(&host), (None, None));
        // 親が今回の起動で再開されていない（NotLive）
        set_hold(Some(crate::backend::local::QueueHold::NotLive { targets: vec![target()] }));
        let (st, text) = status_of(&host);
        assert_eq!(st, Some(ExecutionStatus::QueueHeld));
        let text = text.unwrap();
        assert!(text.contains("受信（ライブ監視）が戻っていないため保留") && text.contains("週次議事録") && text.contains("このチャットを再開して送る"), "{text}");
        // 親の状態を確認できない
        set_hold(Some(crate::backend::local::QueueHold::StateUnknown { targets: vec![], descendant_scan_incomplete: false }));
        let (st, text) = status_of(&host);
        assert_eq!((st, text.as_deref()), (Some(ExecutionStatus::QueueHeld), Some("親の状態を確認できないため、送らずに待っています")));
        // Codex が起動していない（接続が切れている）。Antigravity 宛の順番待ちも止まるので、その理由をカードに出す（無言の停滞にしない）。
        set_hold(Some(crate::backend::local::QueueHold::Disconnected));
        let (st, text) = status_of(&host);
        assert_eq!((st, text.as_deref()), (Some(ExecutionStatus::QueueHeld), Some(CODEX_NOT_RUNNING_HOLD)));
        assert!(text.unwrap().starts_with("Codex が起動していない"));
        // キューが停止中（再起動後の停止を含む）なら従来どおり QueuePaused（保留理由より優先）
        host.mutate(|d| {
            d.queues.get_mut(&a).unwrap().queue.run = crate::backend::local::QueueRun::PausedAfterRestart;
            ((), Vec::new())
        });
        assert_eq!(status_of(&host).0, Some(ExecutionStatus::QueuePaused));
        let _ = std::fs::remove_dir_all(&host.app_data_dir);
    }

    #[tokio::test]
    async fn explicitly_addressed_reception_queues_once_and_recovery_does_not_send() {
        let (host, r, a) = host_with_router("auto-dispatch");
        // 自動送信は、送り先が live（今回の起動で再開済み）のときだけキューに登録する。
        make_live(&host, &a, AgentState::Idle);
        let root = host.app_data_dir.clone();
        host.update_router(&r, |rec| {
            rec.push_catalog(sent_catalog("live", &[("c1", &a)]));
            rec.push_catalog(sent_catalog("recovered", &[("c1", &a)]));
        });
        let input = envelope_for("自動送信: a\n週次の議事録をまとめて");
        let live = stamp_router_request(&input, "live");
        let recovered = stamp_router_request(&input, "recovered");
        let route = r#"{"target":"c1","newChat":null,"text":"週次議事録の要約を作成してください。","textIsOriginal":false,"reason":"週次議事録担当","confidence":"high"}"#;
        let reply = reply_json(route, "");
        host.router_apply_reply(&r, "t-live", &live, &reply, true).await;
        host.router_apply_reply(&r, "t-live", &live, &reply, true).await;
        host.router_apply_reply(&r, "t-recovered", &recovered, &reply, false).await;
        let view = host.router_get().unwrap().unwrap();
        assert_eq!(view.dispatches.len(), 2);
        assert!(matches!(view.dispatches[0].state, DispatchState::Proposed));
        assert_eq!(view.dispatches[0].request_id.as_deref(), Some("recovered"));
        let DispatchState::Queued { queue_entry, .. } = &view.dispatches[1].state else { panic!("自動送信はキューに登録される") };
        assert_eq!(view.dispatches[1].request_id.as_deref(), Some("live"));
        host.read(|data| {
            let entries = &data.queues.get(&a).unwrap().queue.entries;
            assert_eq!(entries.len(), 1);
            assert_eq!(&entries[0].id, queue_entry);
            assert_eq!(entries[0].text, "週次議事録の要約を作成してください。");
        });
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_route_to_a_chat_that_became_archived_is_not_offered() {
        let (host, r, a) = host_with_router("archived");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        host.update_local(&a, false, std::time::Duration::ZERO, |f| f.visibility = ListVisibility::Archived { at: UnixMillis(1), sync: crate::backend::local::ArchiveSync::Unsupported });
        host.router_apply_reply(&r, "t1", &stamp_router_request(&envelope_for("x"), "cm1"), &reply_json(&route("c1", "本文"), ""), false).await;
        let rec = host.router_record(&r).unwrap();
        assert!(rec.dispatches.is_empty() && rec.turns[0].route_rejects[0].contains("アーカイブ"), "{:?}", rec.turns[0]);
    }

    // ── 送信・振り分け（接続なしのホストで、記録が変わらないことを確かめる） ──

    #[tokio::test]
    async fn router_send_rejects_a_chat_that_is_not_the_router() {
        let (host, _r, a) = host_with_router("notrouter");
        let confirmed = UserConfirmed::from_user_command();
        let e = host.router_send(SendMessageArgs { chat: a.clone(), text: "依頼".into(), attachments: vec![], intent: SendIntent::NewTurn, steer_entry: None }, &confirmed).await.unwrap_err();
        assert_eq!(e.code, IpcErrorCode::InvalidArgs);
        assert!(host.router_rt.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_send_that_cannot_be_made_writes_no_delivery_marks_and_does_not_retry() {
        let (host, r, _a) = host_with_router("unsent");
        let confirmed = UserConfirmed::from_user_command();
        let before = host.router_record(&r).unwrap();
        // 未接続（構造化出力に対応しない状態も同じ）: どちらでも送らず、届いた記録は変えない。
        let res = host.router_send(SendMessageArgs { chat: r.clone(), text: "議事録をお願い".into(), attachments: vec![], intent: SendIntent::NewTurn, steer_entry: None }, &confirmed).await;
        // Codex が起動していないための断りは、その理由を返す（無言・分かりにくい文にしない）。
        let e = res.unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (IpcErrorCode::NotConnected, CODEX_NOT_RUNNING_SEND));
        assert!(e.message.starts_with("Codex が起動していない") && e.message.contains("受付係へは送れません"), "{}", e.message);
        let after = host.router_record(&r).unwrap();
        assert_eq!((after.instructions_delivered_rev, after.rules_delivered_rev, after.catalog_delivered_digest.clone()), (None, None, None));
        assert_eq!(after.dispatches, before.dispatches);
        assert!(host.router_rt.pending.lock().unwrap().is_empty(), "送っていない送信は覚えない");
        // 受付係以外の意図（追加指示）も断る
        let e = host.router_send(SendMessageArgs { chat: r.clone(), text: "x".into(), attachments: vec![], intent: SendIntent::Steer, steer_entry: None }, &confirmed).await.unwrap_err();
        assert_eq!(e.code, IpcErrorCode::InvalidArgs);
    }

    #[tokio::test]
    async fn a_dispatch_to_an_unavailable_target_stays_proposed_and_sent_or_unconfirmed_cards_are_never_resent() {
        let (host, r, a) = host_with_router("dispatch");
        let confirmed = UserConfirmed::from_user_command();
        let missing = key("gone");
        host.update_router(&r, |rec| {
            rec.push_dispatch(dispatch("d-missing", DispatchTarget::Existing { chat: missing.clone() }, DispatchState::Proposed));
            rec.push_dispatch(dispatch("d-new", DispatchTarget::New { chat: None }, DispatchState::Proposed));
            rec.push_dispatch(dispatch("d-sent", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Sent { at: UnixMillis(1), client_message_id: "m".into() }));
            rec.push_dispatch(dispatch("d-unk", DispatchTarget::Existing { chat: a.clone() }, DispatchState::Unconfirmed { at: UnixMillis(1), client_message_id: "m".into() }));
        });
        let go = |id: &str, over: Option<ChatKey>| RouterDispatchArgs { dispatch_id: LocalId(id.into()), target_override: over, text: "本文".into(), intent: None };
        assert_eq!(host.router_dispatch(go("d-missing", None), &confirmed).await.unwrap_err().code, IpcErrorCode::NotFound);
        assert_eq!(host.router_dispatch(go("d-new", None), &confirmed).await.unwrap_err().code, IpcErrorCode::InvalidArgs, "新規は作成の経路から");
        assert_eq!(host.router_dispatch(go("d-sent", None), &confirmed).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        let e = host.router_dispatch(go("d-unk", None), &confirmed).await.unwrap_err();
        assert!(e.message.contains("再送はしません"), "{}", e.message);
        // 受付係自身へは送れない
        assert_eq!(host.router_dispatch(go("d-missing", Some(r.clone())), &confirmed).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        // 送れなかった案は提案のまま（接続なしで a への送信は通らない）
        assert!(host.router_dispatch(go("d-missing", Some(a.clone())), &confirmed).await.is_err());
        let rec = host.router_record(&r).unwrap();
        let st = |id: &str| rec.dispatches.iter().find(|d| d.id.0 == id).unwrap().state.clone();
        assert_eq!((st("d-missing"), st("d-new")), (DispatchState::Proposed, DispatchState::Proposed));
        assert!(host.router_rt.dispatching.lock().unwrap().is_empty(), "送信中の印は外れる");
    }

    #[tokio::test]
    async fn settling_a_dispatch_records_sent_or_unconfirmed_and_leaves_a_rejected_one_proposed() {
        let (host, r, a) = host_with_router("settle");
        host.update_router(&r, |rec| {
            for id in ["d1", "d2", "d3"] {
                rec.push_dispatch(dispatch(id, DispatchTarget::Existing { chat: key("zzz") }, DispatchState::Proposed));
            }
        });
        let attempt = |state: SendState| SendAttempt { attempt_id: LocalId("att".into()), client_message_id: "cmx".into(), at: UnixMillis(9), state };
        let to = DispatchTarget::Existing { chat: a.clone() };
        let v = host.router_settle_dispatch(&r, &LocalId("d1".into()), to.clone(), &attempt(accepted())).await.unwrap();
        assert_eq!((v.state, v.target, v.suggested), (DispatchState::Sent { at: UnixMillis(9), client_message_id: "cmx".into() }, to.clone(), DispatchTarget::Existing { chat: key("zzz") }));
        let v = host.router_settle_dispatch(&r, &LocalId("d2".into()), to.clone(), &attempt(SendState::AcceptanceUnknown { since: UnixMillis(9) })).await.unwrap();
        assert!(matches!(v.state, DispatchState::Unconfirmed { .. }));
        let e = host.router_settle_dispatch(&r, &LocalId("d3".into()), to, &attempt(SendState::Rejected { message: "x".into() })).await.unwrap_err();
        assert_eq!(e.code, IpcErrorCode::Rejected);
        let rec = host.router_record(&r).unwrap();
        assert_eq!(rec.dispatches.iter().find(|d| d.id.0 == "d3").unwrap().state, DispatchState::Proposed);
        // 見送りは提案のものだけ
        assert!(host.router_dismiss(RouterDispatchIdArgs { dispatch_id: LocalId("d1".into()) }).await.is_err());
        let v = host.router_dismiss(RouterDispatchIdArgs { dispatch_id: LocalId("d3".into()) }).await.unwrap();
        assert!(matches!(v.state, DispatchState::Dismissed { .. }));
    }

    #[tokio::test]
    async fn a_created_chat_links_the_new_chat_card_and_an_unconfirmed_first_send_is_not_called_sent() {
        let (host, r, a) = host_with_router("created");
        host.update_router(&r, |rec| rec.push_dispatch(dispatch("dn", DispatchTarget::New { chat: None }, DispatchState::Proposed)));
        let att = |state: SendState| SendAttempt { attempt_id: LocalId("att".into()), client_message_id: "cm1".into(), at: UnixMillis(3), state };
        let id = LocalId("dn".into());
        let v = host.router_dispatch_created(RouterDispatchCreatedArgs { dispatch_id: id.clone(), chat: a.clone(), first_send: Some(att(SendState::AcceptanceUnknown { since: UnixMillis(3) })) }).await.unwrap();
        assert_eq!((v.target.clone(), matches!(v.state, DispatchState::Unconfirmed { .. })), (DispatchTarget::New { chat: Some(a.clone()) }, true));
        // 確定した結果で呼び直すと更新できる
        let v = host.router_dispatch_created(RouterDispatchCreatedArgs { dispatch_id: id.clone(), chat: a.clone(), first_send: Some(att(accepted())) }).await.unwrap();
        assert!(matches!(v.state, DispatchState::Sent { .. }));
        // 送った後は再送しない
        assert!(host.router_dispatch_created(RouterDispatchCreatedArgs { dispatch_id: id, chat: a, first_send: None }).await.is_err());
    }

    // ── ルール帳 ──

    #[tokio::test]
    async fn an_unreadable_rules_file_is_set_aside_and_a_warning_is_exposed() {
        let (host, r, _a) = host_with_router("badrules");
        std::fs::create_dir_all(host.router_dir()).unwrap();
        std::fs::write(host.rules_path(), b"{ not json").unwrap();
        let view = host.router_rules().unwrap();
        assert_eq!(view.revision, 0);
        assert!(view.warning.as_deref().is_some_and(|w| w.contains("rules.json.bad")), "{:?}", view.warning);
        assert!(host.router_dir().join("rules.json.bad").exists() && !host.rules_path().exists());
        assert!(host.router_get().unwrap().unwrap().rules_warning.is_some());
        // 2回目に壊れたときは時刻つきで残す（前の退避を消さない）
        std::fs::write(host.rules_path(), b"also bad").unwrap();
        host.router_rules().unwrap();
        let bads = std::fs::read_dir(host.router_dir()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("rules.json.bad")).count();
        assert_eq!(bads, 2);
        let _ = r;
    }

    #[tokio::test]
    async fn manual_edits_enforce_limits_and_undo_asks_for_confirmation_when_a_later_change_exists() {
        let (host, _r, a) = host_with_router("undo");
        let add = |text: &str| RuleEditArgs { op: RuleOp::Add, rule_id: None, text: text.into(), target: Some(RuleTargetInput::Chat { chat: a.clone() }) };
        // 禁止語・空・存在しない対象は書かない
        assert_eq!(host.router_rule_edit(add("確認なしで自動送信する")).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert_eq!(host.router_rule_edit(add("  ")).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert_eq!(host.router_rule_edit(RuleEditArgs { op: RuleOp::Update, rule_id: Some("r99".into()), text: "x".into(), target: None }).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert_eq!(host.router_rule_edit(add(&"あ".repeat(RULE_TEXT_MAX_CHARS + 1))).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert!(!host.rules_path().exists());

        let v = host.router_rule_edit(add("議事録の依頼 → 週次議事録")).await.unwrap();
        assert_eq!((v.revision, v.rules[0].rule.id.as_str(), v.rules[0].target_title.as_deref()), (1, "r1", Some("週次議事録")));
        assert!(matches!(v.rules[0].rule.source, RuleSource::Manual));
        let v = host.router_rule_edit(RuleEditArgs { op: RuleOp::Update, rule_id: Some("r1".into()), text: "議事録と報告書の依頼 → 週次議事録".into(), target: None }).await.unwrap();
        assert_eq!(v.revision, 2);

        // 版1（追加）の取り消しは、その後の更新があるので確認が要る
        let e = host.router_rule_undo(RuleUndoArgs { revision: 1, force: false }).await.unwrap_err();
        assert_eq!((e.code, e.blocked), (IpcErrorCode::Blocked, Some(BlockedReason::RouterRuleUndoNeedsConfirm)));
        assert_eq!(host.router_rules().unwrap().revision, 2, "確認前は書き換えない");
        // 直近の更新は確認なしで戻せる
        let v = host.router_rule_undo(RuleUndoArgs { revision: 2, force: false }).await.unwrap();
        assert_eq!((v.revision, v.rules[0].rule.text.as_str()), (3, "議事録の依頼 → 週次議事録"));
        assert_eq!(host.router_rule_undo(RuleUndoArgs { revision: 2, force: false }).await.unwrap_err().code, IpcErrorCode::InvalidArgs, "二重の取り消しはしない");
        // 版1は、版2を取り消した後は確認なしで戻せる（追加の取り消し＝削除）
        let v = host.router_rule_undo(RuleUndoArgs { revision: 1, force: false }).await.unwrap();
        assert!(v.rules.is_empty());
        assert!(v.history.iter().filter(|h| h.change.undoes.is_some()).count() == 2, "取り消しも履歴に残る");
    }

    #[test]
    fn a_compaction_observed_after_composing_is_not_cleared_by_the_acceptance() {
        let inc = EnvelopeSections { instructions: Some(1), ..Default::default() };
        let mut r = RouterRecord { compacted_since_delivery: true, compaction_epoch: 3, ..Default::default() };
        // 組み立て時の世代3で受理 → 印を外す
        apply_delivery(&mut r, &inc, 3, &accepted());
        assert!(!r.compacted_since_delivery);
        // 組み立て後（世代3）に圧縮を観測（世代4）してから受理 → 印を残す
        let mut r = RouterRecord { compacted_since_delivery: true, compaction_epoch: 4, ..Default::default() };
        apply_delivery(&mut r, &inc, 3, &accepted());
        assert!(r.compacted_since_delivery);
        assert_eq!(r.instructions_delivered_rev, Some(1));
    }

    #[tokio::test]
    async fn router_send_refuses_an_archived_or_superseded_router() {
        let (host, r, a) = host_with_router("stale");
        let confirmed = UserConfirmed::from_user_command();
        let send = |host: Arc<Host>, r: ChatKey| async move { host.router_send(SendMessageArgs { chat: r, text: "依頼".into(), attachments: vec![], intent: SendIntent::NewTurn, steer_entry: None }, &UserConfirmed::from_user_command()).await.unwrap_err() };
        let _ = &confirmed;
        host.update_local(&r, false, std::time::Duration::ZERO, |f| f.visibility = ListVisibility::Archived { at: UnixMillis(1), sync: crate::backend::local::ArchiveSync::Unsupported });
        let e = send(host.clone(), r.clone()).await;
        assert!(e.code == IpcErrorCode::InvalidArgs && e.message.contains("アーカイブまたは引き継ぎ済み"), "{}", e.message);
        host.update_local(&r, false, std::time::Duration::ZERO, |f| {
            f.visibility = ListVisibility::Visible;
            f.superseded_by = Some(crate::store::records::Superseded { by: a, at: UnixMillis(1) });
        });
        let e = send(host.clone(), r).await;
        assert!(e.message.contains("アーカイブまたは引き継ぎ済み"), "{}", e.message);
    }

    #[tokio::test]
    async fn retiring_removes_only_the_mark() {
        let (host, r, _a) = host_with_router("retire");
        assert_eq!(host.router_head(), Some(r.clone()));
        host.router_retire(r.clone()).await.unwrap();
        assert_eq!(host.router_head(), None);
        assert!(host.router_get().unwrap().is_none());
        assert!(host.read(|d| d.chat(&r).is_some()), "チャットは消さない");
        assert_eq!(host.router_retire(r).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
    }
}
