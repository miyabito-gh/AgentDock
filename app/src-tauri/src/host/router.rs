//! 受付係（依頼の入口と振り分け）のホスト側（`app/DESIGN_ROUTER.md` §5〜§9）。Tauriコマンド（`commands.rs`）が呼ぶ `Host` のメソッド。
//!
//! 守ること:
//! - **AIは決めるだけ、実行はアプリ**。受付係の返答（構造化出力）は純関数（`rules::router`）で検査し、通ったものだけを
//!   カード（`Proposed`）とルール帳の更新にする。振り分けの送信は利用者が［送る］を押したとき（`router_dispatch`）だけ。
//! - 受付係への資料の「届いた記録」は、送信が `Accepted` になったときだけ書く。受理不明・拒否・送れなかったときは書かない。
//!   切断・受理不明で自動再送しない（振り分けも `Proposed` のまま、または `Unconfirmed` で止める）。
//! - 利用者の送信と同じ経路（`send_message_with`、`SendAuth::User`）で送る。組織実行の所有チャット・アーカイブ・引き継ぎ済みへは送らない。
//! - ルール帳（`<app_data>/router/rules.json`）は `write_atomic` で書く。読めなければ `rules.json.bad` へ退避し、空から始めて警告を出す。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::backend::backend::{AiBackend, BackendEvent, PermissionPreset, ReadOptions, UserConfirmed};
use crate::backend::ipc::{BlockedReason, HostEvent, IpcError, IpcErrorCode, SendIntent, SendMessageArgs, StartChatArgs};
use crate::backend::local::{ListVisibility, SaveScope};
use crate::backend::model::*;
use crate::backend::router::*;
use crate::host::state::agent_key_of;
use crate::host::{blocked, err, now_ms, Host, SendExtras};
use crate::rules::org_intake::facts_of;
use crate::rules::router::*;
use crate::store::atomic::write_atomic;
use crate::store::records::{ConversationRecord, Dispatch, DispatchProposal, DispatchState, DispatchTarget, NewChatProposal, RouterRecord, RouterTurnRecord, RuleNoticeRecord, SentCatalog};

/// 受付係のチャットに付ける名前。
const ROUTER_TITLE: &str = "受付";
/// 送り先の解決で引き継ぎ先を辿る上限。
const SUPERSEDE_HOPS: usize = 20;
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
pub(super) fn apply_delivery(rec: &mut RouterRecord, inc: &EnvelopeSections, state: &SendState) -> bool {
    if !matches!(state, SendState::Accepted { .. }) {
        return false;
    }
    if let Some(rev) = inc.instructions {
        rec.instructions_delivered_rev = Some(rev);
        // 圧縮を観測したあとは指示・ルール・一覧を全部添え直すので、この送信で印を外す。
        rec.compacted_since_delivery = false;
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
                    RouteTarget::New { title, folder, purpose } => (DispatchTarget::New { chat: None }, Some(NewChatProposal { title: title.clone(), folder: folder.clone(), purpose: purpose.clone() })),
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
    fn dispatch_view(&self, d: &Dispatch) -> DispatchView {
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
                let running = self.read(|data| data.root_view(chat).is_some_and(|v| matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)));
                let problem = match self.route_state(chat) {
                    ChatRouteState::Available => None,
                    s => Some(RouteReject::Unavailable(s).message()),
                };
                (running, problem)
            }
            DispatchTarget::New { .. } => (false, None),
        };
        DispatchView {
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
        Ok(Some(RouterView { chat, rules_warning: self.rules_warning(), rules_revision: rules.revision, dispatches: rec.dispatches.iter().map(|d| self.dispatch_view(d)).collect(), turns }))
    }

    // ───────────── 受付係への送信 ─────────────

    /// 受付係へ送る（通常の `send_message` の代わり）。資料と構造化出力の指定を付け、一覧の対応を送る前に保存する。
    /// 受理を確認したときだけ「届いた記録」を書く。受理不明・拒否・切断で自動再送しない。
    pub async fn router_send(self: &Arc<Self>, args: SendMessageArgs, confirmed: &UserConfirmed) -> Result<SendAttempt, IpcError> {
        let chat = args.chat.clone();
        let Some(rec) = self.router_record(&chat) else {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットは受付係ではありません。通常の送信を使ってください"));
        };
        if self.route_state(&chat) != ChatRouteState::Router {
            return Err(err(IpcErrorCode::InvalidArgs, "このチャットは今の受付係ではありません（アーカイブまたは引き継ぎ済みです）"));
        }
        if !matches!(args.intent, SendIntent::NewTurn) {
            return Err(err(IpcErrorCode::InvalidArgs, "受付係への送信は新しい依頼としてだけ行えます（返答が終わってから送ってください）"));
        }
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        if self.backend.capabilities().structured_output != Support::Supported {
            return Err(err(IpcErrorCode::Unsupported, "このAIは構造化出力に対応していないため、受付係を使えません"));
        }
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
        let env = compose_router_envelope(&rec, &catalog, &rules, &label, &args.text);
        let cm = self.local_id("cm").0;
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
                SendExtras { output_schema: Some(router_output_schema()), client_message_id: Some(cm.clone()) },
            )
            .await;
        match sent {
            Ok(attempt) => {
                self.router_note_send(&chat, &cm, &env.included, &attempt);
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
    pub(super) fn router_note_send(self: &Arc<Self>, chat: &ChatKey, cm: &str, inc: &EnvelopeSections, attempt: &SendAttempt) {
        match &attempt.state {
            SendState::Accepted { turn } => {
                if let Some(p) = self.router_rt.pending.lock().unwrap().iter_mut().find(|p| p.client_message_id == cm) {
                    p.turn = Some(turn.turn_id.0.clone());
                }
                let state = attempt.state.clone();
                self.update_router(chat, |r| {
                    apply_delivery(r, inc, &state);
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
                return;
            }
        };
        let Some(turn) = history.turns.iter().find(|t| t.key.turn_id.0 == turn_id) else { return };
        let facts = facts_of(turn);
        let Some(sent_text) = facts.user_texts.iter().find(|u| router_turn_user_text(u).is_some()) else { return };
        let Some(final_text) = facts.final_text.as_deref() else { return };
        self.router_apply_reply(&chat, &turn_id, sent_text, final_text).await;
    }

    /// 受付係の返答（最終回答）を処理する。`sent_text` はその turn の利用者の発言（資料つきの送信全体）。
    /// 一覧の対応は、turn または本文が一致する送信のもの。見つからなければ最も新しい送信の一覧を使う。
    pub(super) async fn router_apply_reply(self: &Arc<Self>, chat: &ChatKey, turn_id: &str, sent_text: &str, final_text: &str) {
        let Some(rec) = self.router_record(chat) else { return };
        if rec.turns.iter().any(|t| t.turn == turn_id) {
            return;
        }
        let user_text = router_turn_user_text(sent_text).unwrap_or_default().to_string();
        let cm = self
            .router_rt
            .pending
            .lock()
            .unwrap()
            .iter()
            .find(|p| &p.chat == chat && (p.turn.as_deref() == Some(turn_id) || p.full_text.trim() == sent_text.trim()))
            .map(|p| p.client_message_id.clone());
        let sent = cm
            .as_deref()
            .and_then(|cm| rec.catalogs.iter().find(|c| c.client_message_id == cm))
            .or_else(|| rec.catalogs.first())
            .cloned()
            .unwrap_or(SentCatalog { client_message_id: String::new(), entries: Vec::new(), at: now_ms() });
        let now = now_ms();

        let turn_rec = match parse_router_reply(final_text) {
            Err(e) => RouterTurnRecord { turn: turn_id.to_string(), reply: None, parse_error: Some(e.message()), route_rejects: Vec::new(), notices: Vec::new() },
            Ok(reply) => {
                let folders = self.existing_chat_folders();
                let mut interpreted = {
                    let (guard, rules, writable) = self.lock_rules();
                    let mut new_id = || self.local_id("disp");
                    let state_of = |k: &ChatKey| self.route_state(k);
                    let title_of = |k: &ChatKey| self.chat_title(k);
                    let mut out = interpret_reply(&reply, turn_id, &sent, &user_text, &rules, &state_of, &title_of, &folders, &mut new_id, now);
                    if let Some(next) = &out.rules {
                        let written = if writable { self.write_rules(next) } else { Err("ルール帳を書ける状態ではありません".to_string()) };
                        if let Err(reason) = written {
                            out.rules = None;
                            downgrade_notices(&mut out.notices, &reason);
                        }
                    }
                    drop(guard);
                    out
                };
                let dispatches = std::mem::take(&mut interpreted.dispatches);
                // 新しい順に並ぶよう、案の並びの逆から足す。
                self.update_router(chat, |r| {
                    for d in dispatches.into_iter().rev() {
                        r.push_dispatch(d);
                    }
                });
                RouterTurnRecord { turn: turn_id.to_string(), reply: Some(reply.reply), parse_error: None, route_rejects: interpreted.route_rejects, notices: interpreted.notices }
            }
        };
        self.update_router(chat, |r| r.push_turn(turn_rec));
        if !self.save_router(chat).await {
            self.warn("受付係の返答の解析結果を保存できませんでした（画面には残っています）");
        }
        self.emit_router(chat);
    }

    // ───────────── 振り分けの実行 ─────────────

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
        self.find_dispatch(id).map(|(_, d)| self.dispatch_view(&d)).ok_or_else(|| err(IpcErrorCode::NotFound, "振り分けの記録が見つかりません"))
    }

    /// カードの［送る］。送り先を送る直前に再検査し、利用者の送信（`SendAuth::User`）として1回だけ送る。
    /// 実行中の送り先には、`intent` が None なら追加指示として送る（通常の送信と同じ規則）。結果は `Sent`／`Unconfirmed`。
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
        let running = self.read(|data| data.root_view(&target).is_some_and(|v| matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)));
        let intent = args.intent.unwrap_or(if running { SendIntent::Steer } else { SendIntent::NewTurn });
        let cm = self.local_id("cm").0;
        let sent = self
            .send_message_with(
                SendMessageArgs { chat: target.clone(), text: args.text, attachments: Vec::new(), intent, steer_entry: None },
                confirmed,
                SendExtras { output_schema: None, client_message_id: Some(cm.clone()) },
            )
            .await?;
        self.router_settle_dispatch(&router, &args.dispatch_id, DispatchTarget::Existing { chat: target }, &sent).await
    }

    /// 送信の結果をカードへ反映する。受理を確認したら `Sent`、受理不明なら `Unconfirmed`、受理されなければ「提案」のままエラー。
    pub(super) async fn router_settle_dispatch(self: &Arc<Self>, router: &ChatKey, id: &LocalId, target: DispatchTarget, attempt: &SendAttempt) -> Result<DispatchView, IpcError> {
        match dispatch_outcome(&attempt.state, &attempt.client_message_id, attempt.at) {
            DispatchOutcome::NotSent(message) => Err(err(IpcErrorCode::Rejected, message)),
            DispatchOutcome::Sent(state) | DispatchOutcome::Unconfirmed(state) => {
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
        let d = std::env::temp_dir().join(format!("agentdock-router-{tag}-{}-{}", std::process::id(), now_ms().0));
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
            assert!(!apply_delivery(&mut r, &inc, &not_accepted), "{not_accepted:?}");
            assert_eq!(r, base(), "受理を確認できない送信は記録を変えない: {not_accepted:?}");
        }
        let mut r = base();
        assert!(apply_delivery(&mut r, &inc, &accepted()));
        assert_eq!((r.instructions_delivered_rev, r.rules_delivered_rev, r.catalog_delivered_digest.as_deref(), r.compacted_since_delivery), (Some(1), Some(3), Some("dg"), false));
        assert!(r.dispatches[0].reported && !r.dispatches[1].reported, "資料に含めた結果だけ報告済みにする");
        // 資料に含めなかった節は触らない
        let mut r = RouterRecord { rules_delivered_rev: Some(2), ..Default::default() };
        apply_delivery(&mut r, &EnvelopeSections { catalog_digest: Some("x".into()), ..Default::default() }, &accepted());
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
    async fn a_completed_reply_makes_proposed_cards_and_writes_rules_with_a_notice() {
        let (host, r, a) = host_with_router("reply");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        let user = "定例会議の議事録は、今後は週次議事録へ送って";
        let json = reply_json(
            &format!("{},{}", route("c1", "議事録の依頼です"), route("c9", "存在しない")),
            r#"{"op":"add","ruleId":null,"text":"定例会議の議事録 → 週次議事録","target":"c1","newFolder":null,"quote":"今後は週次議事録へ送って"},{"op":"add","ruleId":null,"text":"x","target":null,"newFolder":null,"quote":"本文にない引用です"}"#,
        );
        host.router_apply_reply(&r, "t1", &envelope_for(user), &json).await;

        let rec = host.router_record(&r).unwrap();
        assert_eq!(rec.dispatches.len(), 1, "連番c9は案にしない");
        let d = &rec.dispatches[0];
        assert_eq!((&d.state, &d.target, &d.source_turn, d.route_index), (&DispatchState::Proposed, &DispatchTarget::Existing { chat: a.clone() }, &"t1".to_string(), 0));
        assert_eq!(d.proposal.as_ref().unwrap().text, "議事録の依頼です");
        let t = &rec.turns[0];
        assert_eq!((t.reply.as_deref(), t.route_rejects.len()), (Some("了解です"), 1));
        assert_eq!(t.notices.len(), 2);
        assert_eq!(t.notices[0].revision, Some(1));
        assert!(t.notices[0].text.starts_with("ルールを更新"));
        assert!(t.notices[1].revision.is_none() && t.notices[1].text.starts_with("ルールは更新していません"), "{:?}", t.notices[1]);

        let rules = host.router_rules().unwrap();
        assert_eq!((rules.revision, rules.rules.len()), (1, 1));
        let bytes = std::fs::read(host.rules_path()).unwrap();
        assert_eq!(parse_rules_file(&bytes).unwrap().rules[0].text, "定例会議の議事録 → 週次議事録");

        // 同じ turn をもう一度処理しても増えない
        host.router_apply_reply(&r, "t1", &envelope_for(user), &json).await;
        assert_eq!(host.router_record(&r).unwrap().dispatches.len(), 1);
        let view = host.router_get().unwrap().unwrap();
        assert_eq!((view.dispatches.len(), view.turns.len(), view.turns[0].notices[0].undone), (1, 1, false));
        assert_eq!(view.dispatches[0].target_title.as_deref(), Some("週次議事録"));
    }

    #[tokio::test]
    async fn an_unreadable_reply_makes_no_cards_and_no_rule_writes() {
        let (host, r, a) = host_with_router("badreply");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        host.router_apply_reply(&r, "t1", &envelope_for("x"), "これはJSONではありません").await;
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
        host.router_apply_reply(&r, "t1", &text, &reply_json(&route("c1", "本文"), "")).await;
        assert_eq!(host.router_record(&r).unwrap().dispatches[0].target, DispatchTarget::Existing { chat: a });
    }

    #[tokio::test]
    async fn a_route_to_a_chat_that_became_archived_is_not_offered() {
        let (host, r, a) = host_with_router("archived");
        host.update_router(&r, |rec| rec.push_catalog(sent_catalog("cm1", &[("c1", &a)])));
        host.update_local(&a, false, std::time::Duration::ZERO, |f| f.visibility = ListVisibility::Archived { at: UnixMillis(1), sync: crate::backend::local::ArchiveSync::Unsupported });
        host.router_apply_reply(&r, "t1", &envelope_for("x"), &reply_json(&route("c1", "本文"), "")).await;
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
        assert!(res.is_err());
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
