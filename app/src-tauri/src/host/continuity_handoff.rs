//! 継続チャット最小版: 「引き継いで送信」（`app/DESIGN_CONT.md` §6.3〜6.6）。
//!
//! - 利用者の操作（`handoff_and_send`）の中だけで動く。引き継ぎ元の thread（P）には resume・送信・アーカイブ・削除をしない。
//!   P に対して行うのは読取り（`read_history`）と AgentDock 自身の保存（`supersededBy`・下書き）だけ。
//! - 流れは設計の C0〜C7: 前提確認 → P の履歴を読む → 資料を組み立てる → 新しい thread（N）を開始 → N の chat.json を保存
//!   → P の chat.json を保存 → 添付をコピー → N へ通常の送信経路で1回だけ送る。保存は `save_now` で書き終えるのを待ってから次へ進む。
//! - 資料（seed）は N の chat.json に組み立てた本文のまま持ち、受理を確認するまで `Pending`。受理不明・拒否は再送しない
//!   （既存の照合と、利用者の次の手動送信に任せる）。
//! - ロックの順序は常に P の送信ロック → N の送信ロック。二重実行は引き継ぎ元ごとの印（`HandoffInflight`）と P の送信ロックで止める。
//! - 一般チャットの作業領域は P と同じものを N も使う（N のチャット領域には作業領域を作らない）。チャット領域の削除は
//!   チャットごとの領域（`chats\<dirId>`）だけが対象なので、N を削除しても P の作業領域は消えない（単体テストで固定）。

use std::sync::Arc;
use std::time::Duration;

use super::state::agent_key_of;
use super::thread_ops::ResendInflight;
use super::work_folders::compose_with_primary;
use super::{err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::{SaveScope, SaveStatus};
use crate::backend::model::*;
use crate::rules::continuity as cont;
use crate::rules::queue::AgentFact;
use crate::rules::thread_ops::clear_draft_after;
use crate::store::records::{ConversationRecord, HandoffSeed, PurposeNote, SeedState, Superseded, ThreadSegment};

/// 実行中の「引き継いで送信」の印（引き継ぎ元ごと）。同じ元の2本目は始められない。ガードを手放すと外れる。
pub(super) type HandoffInflight = ResendInflight;

// ───────────────────────────── 純粋な判断 ─────────────────────────────

/// N の開始のしかた。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StartPlan {
    /// 一般チャット: P と同じ作業領域のパスを作業フォルダとして使う（新しい作業領域は作らない）。
    ReuseWorkspace { workspace: String },
    /// 開発チャット: P の作業フォルダ（プライマリ＋受理済みの追加フォルダ）で開始する。
    Folders { cwd: String, extras: Vec<String> },
}

/// N の開始のしかたを決める。`cwd` は P の作業フォルダ、`accepted` は P の受理済みの作業フォルダの記録（先頭がプライマリ）。
pub(super) fn plan_start(kind: ChatKind, cwd: Option<&str>, accepted: Option<&[String]>) -> Result<StartPlan, String> {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return Err("引き継ぎ元の作業フォルダを確認できないため、引き継げません".into());
    };
    Ok(match kind {
        ChatKind::General => StartPlan::ReuseWorkspace { workspace: cwd.to_string() },
        _ => StartPlan::Folders { cwd: cwd.to_string(), extras: compose_with_primary(cwd, accepted.unwrap_or(&[])).into_iter().skip(1).collect() },
    })
}

/// 子孫の材料から、(実行中・入力待ちがある, 状態不明の件数) を出す。状態不明は「live でも終端確認済みでもない」もの。
fn descendant_summary(facts: &[AgentFact]) -> (bool, u32) {
    let working = |s: AgentState| matches!(s, AgentState::Running | AgentState::Waiting | AgentState::Initializing);
    let busy = facts.iter().any(|f| working(f.state));
    let unknown = facts.iter().filter(|f| !working(f.state) && !f.terminal_explicit && (f.state == AgentState::Unknown || f.freshness != Freshness::Live)).count();
    (busy, unknown.min(u32::MAX as usize) as u32)
}

/// 新しい thread が作れたか確認できない失敗か（送ったかどうか分からない）。拒否・未接続・非対応は「作られていない」と確定している。
fn start_unknown(code: IpcErrorCode) -> bool {
    matches!(code, IpcErrorCode::OutcomeUnknown | IpcErrorCode::Io | IpcErrorCode::Protocol)
}

/// 空の先頭のやり直し（§6.6）か。資料が `Pending` のまま、Codex に履歴がない（発話前に落ちた・受理されていない）先頭。
fn is_redo_source(rec: Option<&ConversationRecord>, no_history: bool) -> bool {
    no_history && rec.and_then(|r| r.seed.as_ref()).is_some_and(|s| s.state == SeedState::Pending)
}

/// 引き継ぎ先 N の会話の記録。P の記録（目的メモ・以前の thread の列・会話ID）を写し、P を列の末尾に足す。資料は `Pending`。
/// 目的メモを届けた記録は引き継がない（新しい thread では未送付から始める）。
pub(super) fn handoff_conversation(src: &ChatKey, src_rec: Option<&ConversationRecord>, title: Option<String>, seed: HandoffSeed, new_id: LocalId, now: UnixMillis) -> ConversationRecord {
    let mut threads = src_rec.map(|r| r.threads.clone()).unwrap_or_default();
    threads.push(ThreadSegment { chat: src.clone(), handed_off_at: now, title });
    ConversationRecord {
        id: src_rec.map(|r| r.id.clone()).unwrap_or(new_id),
        purpose: src_rec.and_then(|r| r.purpose.clone()),
        threads,
        seed: Some(seed),
        purpose_delivered_rev: None,
        compacted_since_delivery: false,
        replaces: None,
        // 依頼整理の記録（要約・状態）は会話の記録の一部。新しい先頭へ写す。
        intake: src_rec.and_then(|r| r.intake.clone()),
    }
}

/// 空の先頭 N のやり直し先 N2 の記録。資料は N の `seed.material` をそのまま使う（P を読み直さない）。
/// 列は N と同じ（空の N は列に入れない）。N の資料が `Pending` でなければ None。
pub(super) fn redo_conversation(src: &ChatKey, src_rec: &ConversationRecord, now: UnixMillis) -> Option<ConversationRecord> {
    let old = src_rec.seed.as_ref().filter(|s| s.state == SeedState::Pending)?;
    Some(ConversationRecord {
        id: src_rec.id.clone(),
        purpose: src_rec.purpose.clone(),
        threads: src_rec.threads.clone(),
        seed: Some(HandoffSeed { created_at: now, state: SeedState::Pending, ..old.clone() }),
        purpose_delivered_rev: None,
        compacted_since_delivery: false,
        replaces: Some(src.clone()),
        intake: src_rec.intake.clone(),
    })
}

/// 分岐先へ写す目的メモ（空白だけのメモは写さない）。
pub(super) fn purpose_for_fork(rec: Option<&ConversationRecord>) -> Option<PurposeNote> {
    rec.and_then(|r| r.purpose.clone()).filter(|p| !p.text.trim().is_empty())
}

/// 分岐先の会話の記録（別の会話。id は新規、以前の thread・引き継ぎ資料・届けた記録は持たない）。
pub(super) fn fork_conversation(id: LocalId, purpose: PurposeNote) -> ConversationRecord {
    ConversationRecord { id, purpose: Some(purpose), threads: Vec::new(), seed: None, purpose_delivered_rev: None, compacted_since_delivery: false, replaces: None, intake: None }
}

/// チャット領域の作業領域（`workspace`）を作業フォルダとして使っている、別のチャット（`candidates` は (チャット, 作業フォルダ)）。
/// 引き継ぎ先の N や、鎖の先頭が P0 の領域を共有しているときに、領域ごとの削除でデータを失わないための判断。
pub(super) fn workspace_users(workspace: &std::path::Path, owner: &ChatKey, candidates: &[(ChatKey, String)]) -> Vec<ChatKey> {
    candidates.iter().filter(|(k, cwd)| k != owner && crate::store::layout::is_inside(workspace, std::path::Path::new(cwd))).map(|(k, _)| k.clone()).collect()
}

/// 添付の事前検査（読取りのみ）。引き継ぎ元の台帳にあり、使える状態で、コピーが実在すること。だめなら理由（P を隠す前に止める）。
pub(super) fn attachment_blocker(ledger: &[crate::backend::local::AttachmentEntry], ids: &[LocalId], exists: &dyn Fn(&str) -> bool) -> Option<String> {
    use crate::backend::local::{AttachmentSource, AttachmentState};
    for id in ids {
        let Some(a) = ledger.iter().find(|a| &a.id == id) else {
            return Some("指定した添付が元のチャットの台帳に見つかりません。付け直してください".into());
        };
        if a.state != AttachmentState::Ready {
            return Some(format!("添付「{}」は使えない状態です（{}）", a.display_name, super::attachments::not_ready_message(&a.state)));
        }
        if !matches!(a.source, AttachmentSource::Skill { .. }) && !a.copy_path.as_deref().is_some_and(exists) {
            return Some(format!("添付「{}」のコピーが見つかりません。取り外して、もう一度添付してください", a.display_name));
        }
    }
    None
}

// ───────────────────────────── ホスト ─────────────────────────────

/// 前提の判断材料と、プレビュー・送信が共有する付随情報。
pub(super) struct HandoffFacts {
    pub ctx: cont::HandoffCtx,
    /// 空の先頭のやり直し（資料は先頭の `seed` をそのまま使う）。
    pub redo: bool,
    /// 子孫の走査が未完了（まだ見つかっていない子孫があるかもしれない）。
    pub scan_incomplete: bool,
}

/// 止めない注記に、子孫の走査が未完了であることを足す（接続中だけ。未接続は別の理由で止まる）。
pub(super) fn with_scan_note(mut notes: Vec<String>, scan_incomplete: bool) -> Vec<String> {
    if scan_incomplete {
        notes.push("まだ見つかっていない子孫があるかもしれません。".to_string());
    }
    notes
}

/// `CreatedNotSent` の文に足す添付の説明（指定した件数と、新しいチャットの入力欄に付いた件数）。添付の指定がなければ空。
fn attach_note(requested: usize, in_new: usize) -> String {
    if requested == 0 {
        return String::new();
    }
    format!("（添付: 新しいチャットの入力欄に{in_new}件、指定は{requested}件）")
}

/// P から集める事実（ロックの中で1回に取る）。
struct Source {
    kind: ChatKind,
    cwd: Option<String>,
    title: String,
    pinned: bool,
    model: Option<ModelChoice>,
    permission: Option<PermissionPreset>,
    accepted_folders: Option<Vec<String>>,
    conversation: Option<ConversationRecord>,
}

impl Host {
    /// 引き継ぎの前提を判断する材料を集める（`handoff_blocker` へ渡す。読取りのみ。プレビューも同じ材料を使える）。
    /// 空の先頭のやり直し（`redo`）かどうかはここで判定し、まだ履歴のない先頭は「状態不明」とみなさない。
    /// プレビュー（`preview_handoff`）と送信（`handoff_and_send`）が共有する唯一の材料集め。
    pub(super) async fn handoff_send_ctx(self: &Arc<Self>, chat: &ChatKey) -> HandoffFacts {
        let connected = self.is_connected();
        let redo = self.read(|d| is_redo_source(d.locals.get(chat).and_then(|l| l.conversation.as_ref()), d.chat(chat).is_some_and(|c| c.no_history)));
        // 履歴に依頼を含む turn がないと確かめられたときだけやり直しにする（読めない・依頼があるなら通常の引き継ぎ）。
        let redo = redo && connected && self.head_is_empty(chat).await;
        let gate = if connected { Some(self.queue_gate_input(chat).await) } else { None };
        let (descendants_busy, unknown_descendants) = gate.as_ref().map(|g| descendant_summary(&g.descendants)).unwrap_or((false, 0));
        let scan_incomplete = gate.as_ref().is_some_and(|g| !g.descendant_scan_complete);
        let org_owned = self.check_not_org_owned(chat).is_err();
        let deleting = self.manage_rt.is_deleting(chat);
        let unresolved_send = self.send_unsettled(chat);
        let folder_op_running = self.folder_op_active(chat);
        let root = agent_key_of(chat);
        let ctx = self.read(|d| {
            let local = d.locals.get(chat);
            let view = d.root_view(chat);
            let working = view.is_some_and(|v| matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing));
            cont::HandoffCtx {
                connected,
                superseded: local.is_some_and(|l| l.superseded_by.is_some()),
                org_owned,
                deleting: deleting || local.is_some_and(|l| l.delete_pending.is_some()),
                has_worktree: local.is_some_and(|l| l.worktree.is_some()),
                root_busy: working || d.running_turn.contains_key(&root),
                stop_unconfirmed: d.open_stop(chat).is_some(),
                unresolved_send,
                pending_ops: local.is_some_and(|l| !l.pending_ops.is_empty()),
                folder_op_running,
                queue_nonempty: d.queues.get(chat).is_some_and(|f| crate::rules::queue::has_pending(&f.queue)),
                descendants_busy,
                unknown_descendants,
                // 履歴のない先頭（やり直し）はルートの状態を持たないことがある。それ以外で見えないものは不明として止める。
                root_state_unknown: !redo && view.is_none_or(|v| v.status.state == AgentState::Unknown),
                unknown_confirmed_idle: view.is_some_and(|v| d.unknown_confirmed_idle(v)),
                external_send_locked: d.chat(chat).is_some_and(|c| super::state::external_send_locked(c.origin, view.map(|v| v.freshness))),
            }
        });
        HandoffFacts { ctx, redo, scan_incomplete }
    }

    /// このチャットの作業領域を、別のチャットが作業フォルダとして使っているときの理由（削除を止める。領域は消さない）。
    /// 保存先がない・領域が別のチャットのものでないときは None。
    pub(super) fn workspace_in_use_message(&self, chat: &ChatKey) -> Option<String> {
        let store = self.persist.store()?;
        let workspace = crate::store::layout::workspace_dir(&crate::store::layout::chat_dir(store.root(), &self.persist.dir_id_for(chat)));
        let candidates: Vec<(ChatKey, String)> = self.read(|d| {
            let mut out: Vec<(ChatKey, String)> = d.chats.iter().filter_map(|c| c.cwd.value().map(|v| (c.key.clone(), v.clone()))).collect();
            for (k, l) in &d.locals {
                if !out.iter().any(|(o, _)| o == k) {
                    if let Some(cwd) = l.cached_meta.as_ref().and_then(|m| m.cwd.value().cloned()) {
                        out.push((k.clone(), cwd));
                    }
                }
            }
            out
        });
        let users = workspace_users(&workspace, chat, &candidates);
        if users.is_empty() {
            return None;
        }
        let names: Vec<String> = users.iter().take(3).map(|k| format!("「{}」", self.chat_title(k))).collect();
        let more = if users.len() > 3 { format!("ほか{}件", users.len() - 3) } else { String::new() };
        Some(format!("この thread の作業フォルダを、引き継ぎ先の thread（{}{}）が使っています。先に今の thread を削除するか、そのまま残してください", names.join("、"), more))
    }

    /// 先頭に依頼を含む turn が1つもないか（読取りのみ）。Codex に履歴がない（`no rollout found`）ときも空。それ以外の読取り失敗は空と確かめられない。
    async fn head_is_empty(self: &Arc<Self>, chat: &ChatKey) -> bool {
        match self.read_history(agent_key_of(chat), ReadOptions { include_turns: true }).await {
            Ok(h) => !h.turns.iter().any(|t| t.entries.iter().any(|e| e.kind == ActivityKind::UserMessage)),
            Err(e) => crate::rules::manage::backend_delete_target_missing(&e),
        }
    }

    fn handoff_source(&self, chat: &ChatKey) -> Option<Source> {
        let title = self.chat_title(chat);
        self.read(|d| {
            let c = d.chat(chat)?;
            let local = d.locals.get(chat);
            Some(Source {
                kind: c.kind,
                cwd: c.cwd.value().cloned(),
                title,
                pinned: c.pinned || local.is_some_and(|l| l.pinned),
                model: d.model_settings.get(chat).and_then(|s| s.selected.clone()).or_else(|| local.and_then(|l| l.model.clone())),
                permission: local.and_then(|l| l.permission),
                accepted_folders: local.and_then(|l| l.accepted_folders.as_ref().map(|a| a.folders.folders.clone())),
                conversation: local.and_then(|l| l.conversation.clone()),
            })
        })
    }

    /// 受理を確認した送信に含まれていた資料を「届いた」と記録する（目的メモの版と、引き継ぎ資料）。受理不明・拒否では呼ばない。
    pub(super) fn note_envelope_delivered(self: &Arc<Self>, chat: &ChatKey, env: cont::DeliveredEnvelope) {
        if let Some(rev) = env.purpose_rev {
            self.note_purpose_delivered(chat, rev);
        }
        if env.handoff {
            // 受理済みなのに Pending のまま落ちると、次の送信で同じ資料をもう一度付けてしまうので、まとめず書く。
            self.update_local(chat, true, Duration::ZERO, |f| {
                if let Some(s) = f.conversation.as_mut().and_then(|c| c.seed.as_mut()) {
                    s.state = SeedState::Delivered;
                }
            });
        }
    }

    /// 「引き継いで送信」（DESIGN_CONT §6.3）。新しい thread を作り、資料つきで1回だけ送る。
    /// 前提が満たされない・読めない・開始できない間は何も作らず `NotStarted`。送信の拒否・受理不明は再送しない。
    pub async fn handoff_and_send(self: &Arc<Self>, args: HandoffAndSendArgs, confirmed: &UserConfirmed) -> Result<HandoffResult, IpcError> {
        let p = args.chat.clone();
        if args.text.trim().is_empty() && args.attachments.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "引き継いで送る依頼の文が空です"));
        }
        if self.read(|d| d.chat(&p).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let stop = |message: String| Ok(HandoffResult::NotStarted { message, start_unknown: false });
        // 二重実行の防止。同じ元の2本目と、同じ元の「編集して再送・再生成」を通さない。
        let Some(_inflight) = self.handoff_inflight.try_begin(&p) else {
            return stop("このチャットの引き継ぎを実行中です。結果が出るまでお待ちください".into());
        };
        let Some(_resend) = self.resend_inflight.try_begin(&p) else {
            return stop("このチャットの「編集して再送・再生成」を実行中です。結果が出るまでお待ちください".into());
        };
        // P の送信ロックを全体で持つ（P への手動送信・キュー送信と交わらない）。N への送信は N のロックなので、順序は常に P → N。
        let p_lock = self.queue_rt.send_lock(&p);
        let _p_guard = p_lock.lock().await;
        // C0 前提確認。送信ロックを取った後で読み直す（ロック待ちの間に送信が受理されて資料が Delivered に変わる競合を避ける）。
        // 古い画面からの二重押下は superseded で止まる。
        let Some(src) = self.handoff_source(&p) else {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        };
        let HandoffFacts { ctx, redo, scan_incomplete } = self.handoff_send_ctx(&p).await;
        let notes = match cont::handoff_blocker(&ctx) {
            Ok(notes) => with_scan_note(notes, scan_incomplete),
            Err(b) => return stop(b.message),
        };
        // 添付の検査は P を隠す前に済ませる（予測できる失敗で「作ったが送れない」にしない）。
        let ledger = self.read(|d| d.locals.get(&p).map(|l| l.attachments.clone()).unwrap_or_default());
        if let Some(m) = attachment_blocker(&ledger, &args.attachments, &|path| std::path::Path::new(path).exists()) {
            return stop(m);
        }
        if let Err(e) = self.precheck_space() {
            return stop(e.message);
        }
        let plan = match plan_start(src.kind, src.cwd.as_deref(), src.accepted_folders.as_deref()) {
            Ok(plan) => plan,
            Err(m) => return stop(m),
        };
        let now = now_ms();
        // C1・C2 資料を用意する。やり直しは N の資料をそのまま使う（読取りもしない）。通常は P の履歴を読む（読取りのみ）。
        let seed = match (&src.conversation, redo) {
            (Some(rec), true) => match redo_conversation(&p, rec, now).and_then(|r| r.seed) {
                Some(s) => s,
                None => return stop("引き継ぎ資料が見つからないため、やり直せません".into()),
            },
            _ => {
                let history = match self.read_history(agent_key_of(&p), ReadOptions { include_turns: true }).await {
                    Ok(h) => h,
                    Err(e) => return stop(format!("引き継ぎ元の履歴を読めないため、引き継げません（{e}）。何も作っていません")),
                };
                let rounds = self.read(|d| d.settings.continuation.normalized().rounds) as usize;
                let m = cont::build_material(&history.turns, rounds, crate::win::clock::local_utc_offset_min());
                HandoffSeed { from: p.clone(), material: m.text, rounds_included: m.rounds_included, truncated: m.truncated, created_at: now, state: SeedState::Pending }
            }
        };
        // C3 新しい thread を開始する（P と同じ作業場所・モデル・権限）。失敗・受理不明は何も変えずに止める。
        let (cwd, extras, reuse) = match plan {
            StartPlan::ReuseWorkspace { workspace } => {
                if !std::path::Path::new(&workspace).is_dir() {
                    return stop(format!("引き継ぎ元の作業領域が見つかりません: {workspace}"));
                }
                (None, Vec::new(), Some(workspace))
            }
            StartPlan::Folders { cwd, extras } => (Some(cwd), extras, None),
        };
        let start = StartChatArgs {
            backend: p.backend,
            cwd,
            model: src.model.clone(),
            permission: src.permission,
            first_message: None,
            worktree: None,
            extra_folders: extras,
            confirmed_strong: Vec::new(),
        };
        let started = match self.start_chat_in(start, reuse, confirmed).await {
            Ok(s) => s,
            Err(e) if start_unknown(e.code) => {
                return Ok(HandoffResult::NotStarted {
                    message: format!("新しい thread を作れたか確認できません（{}）。元のチャットは変わっていません。もう一度試すときは、操作をやり直してください", e.message),
                    start_unknown: true,
                })
            }
            Err(e) => return stop(format!("新しい thread を開始できませんでした（{}）。元のチャットは変わっていません", e.message)),
        };
        let n = started.chat.key.clone();
        let requested = args.attachments.len();
        let attach_msg = |host: &Host| attach_note(requested, host.read(|d| d.locals.get(&n).map_or(0, |l| l.draft.attachments.len())));
        let n_chat = |host: &Host| host.read(|d| d.chat(&n).cloned()).unwrap_or_else(|| started.chat.clone());
        // 以降は N ができている。送れなかったときは CreatedNotSent（下書きは N にある）。P へ戻さない。
        self.inherit_fork_settings(&p, &n);
        if src.pinned {
            let _ = self.set_pinned(SetPinnedArgs { chat: n.clone(), pinned: true });
        }
        // C4 N の chat.json を保存して待つ: 会話の記録（目的メモの写し・以前の thread の列・資料 Pending）と下書き。
        let record = if redo {
            src.conversation.as_ref().and_then(|r| redo_conversation(&p, r, now))
        } else {
            Some(handoff_conversation(&p, src.conversation.as_ref(), Some(src.title.clone()), seed, self.local_id("conv"), now))
        };
        let Some(record) = record else {
            return Ok(HandoffResult::CreatedNotSent { chat: n_chat(self), message: format!("引き継ぎの記録を作れませんでした。新しい空のチャットだけができています{}", attach_msg(self)) });
        };
        let text = args.text.clone();
        self.update_local(&n, true, Duration::ZERO, |f| {
            f.conversation = Some(record);
            f.draft.text = text;
            f.draft.updated_at = Some(now_ms());
        });
        if !self.saved_now(&n).await {
            // 記録を残せないなら、引き継いだことにしない。N は引き継ぎの記録のないただの新しいチャット（P は変えていない）。
            self.update_local(&n, true, Duration::ZERO, |f| f.conversation = None);
            return Ok(HandoffResult::CreatedNotSent {
                chat: n_chat(self),
                message: format!("引き継ぎの記録を保存できなかったため、引き継いでいません。新しい空のチャットができています（元のチャットは変わっていません）。入力した文は新しいチャットの入力欄にあります{}", attach_msg(self)),
            });
        }
        // C5 P の chat.json を保存して待つ: supersededBy=N、下書きの文を空にする。ここで先頭が替わる。
        // 削除が始まっていないか確かめ直す（P の領域を N が使うため、削除中の P を隠して先へ進まない）。
        if self.manage_rt.is_deleting(&p) || self.read(|d| d.locals.get(&p).is_some_and(|l| l.delete_pending.is_some())) {
            return Ok(HandoffResult::CreatedNotSent {
                chat: n_chat(self),
                message: format!("元のチャットの削除が始まっているため、送っていません。入力した文は新しいチャットの入力欄にあります{}", attach_msg(self)),
            });
        }
        let cleared_attachments = args.attachments.clone();
        self.update_local(&p, true, Duration::ZERO, |f| {
            f.superseded_by = Some(Superseded { by: n.clone(), at: now });
            f.draft.text.clear();
            f.draft.updated_at = Some(now_ms());
        });
        if !self.saved_now(&p).await {
            // 起動時の照合（resolve_chains）が、N の列に P があって P の印がないことから修復する。送らない。
            return Ok(HandoffResult::CreatedNotSent {
                chat: n_chat(self),
                message: format!("元のチャットの引き継ぎ済みの印を保存できなかったため、送っていません。入力した文は新しいチャットの入力欄にあります。保存の状態を確認してください{}", attach_msg(self)),
            });
        }
        // C6 添付を P → N へコピーする（新しいコピーが N の送信前の添付＝下書きに入る）。引き継げなければ送らない。
        let mut carried: Vec<LocalId> = Vec::new();
        if !args.attachments.is_empty() {
            match self.carry_attachments(&p, &n, &args.attachments).await {
                Ok(es) => carried = es.into_iter().map(|e| e.id).collect(),
                Err(m) => {
                    return Ok(HandoffResult::CreatedNotSent {
                        chat: n_chat(self),
                        message: format!("添付を引き継げなかったため、送っていません（{m}）。新しいチャットの入力欄で添付を確認してください{}", attach_msg(self)),
                    })
                }
            }
            self.update_local(&p, true, Duration::ZERO, |f| f.draft.attachments.retain(|a| !cleared_attachments.contains(a)));
        }
        // C7 N へ通常の送信経路で1回だけ送る（送信ロック・送信前記録・受理不明の照合・再起動後の照合が効く）。資料は send_message の中で付く。
        let send = match self.send_message(SendMessageArgs { chat: n.clone(), text: args.text, attachments: carried, intent: SendIntent::NewTurn, steer_entry: None }, confirmed).await {
            Ok(s) => s,
            Err(e) => return Ok(HandoffResult::CreatedNotSent { chat: n_chat(self), message: format!("{}{}", e.message, attach_msg(self)) }),
        };
        if clear_draft_after(&send.state) {
            self.update_local(&n, true, Duration::ZERO, |f| {
                f.draft.text.clear();
                f.draft.updated_at = Some(now_ms());
            });
        }
        if matches!(send.state, SendState::Accepted { .. }) {
            // 受理後なら履歴ができているので名前を付けられる（付けられなければそのまま）。
            self.rename_best_effort(&n, src.title.clone(), confirmed).await;
        }
        Ok(HandoffResult::Sent { chat: n_chat(self), send, notes })
    }

    /// チャット別の保存を書き終えるまで待ち、保存できたか返す（保存先が使えない場合は保存できていないものとして扱う）。
    async fn saved_now(self: &Arc<Self>, chat: &ChatKey) -> bool {
        matches!(self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await, Some(SaveStatus { state: SaveState::Saved { .. }, .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::layout;
    use crate::store::records::ChatLocalFile;
    use crate::store::Store;

    fn key(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn seed(state: SeedState) -> HandoffSeed {
        HandoffSeed { from: key("p"), material: "■ 以前の会話からの引き継ぎ\n本文".into(), rounds_included: 2, truncated: false, created_at: UnixMillis(1), state }
    }
    fn purpose(text: &str, rev: u32) -> PurposeNote {
        PurposeNote { text: text.into(), revision: rev, updated_at: UnixMillis(1), source: crate::store::records::PurposeSource::Chat }
    }
    fn record(purpose: Option<PurposeNote>, threads: Vec<ThreadSegment>) -> ConversationRecord {
        ConversationRecord { id: LocalId("c0".into()), purpose, threads, seed: None, purpose_delivered_rev: Some(7), compacted_since_delivery: true, replaces: None, intake: None }
    }
    fn fact(state: AgentState, freshness: Freshness, terminal_explicit: bool) -> AgentFact {
        AgentFact {
            agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("a".into()) },
            state,
            freshness,
            latest_turn: None,
            latest_end: None,
            terminal_explicit,
            observed_at: UnixMillis(1),
        }
    }
    fn temp_root(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("agentdock-handoff-{name}-{}-{}", std::process::id(), now_ms().0));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    // ── 二重実行の防止（A7） ──

    #[test]
    fn a_second_handoff_from_the_same_chat_is_refused_until_the_first_ends() {
        let inflight = HandoffInflight::default();
        let first = inflight.try_begin(&key("p")).expect("1本目は始められる");
        assert!(inflight.try_begin(&key("p")).is_none(), "同じ元の2本目は止める");
        assert!(inflight.try_begin(&key("other")).is_some(), "別のチャットは妨げない");
        drop(first);
        assert!(inflight.try_begin(&key("p")).is_some(), "終われば再び始められる");
    }

    // ── 記録の組立て ──

    #[test]
    fn handoff_and_redo_carry_the_intake_record_to_the_new_head_but_a_fork_does_not() {
        use crate::backend::intake::{IntakeRecord, IntakeStartPrefs, IntakeState};
        let intake = IntakeRecord { id: LocalId("i1".into()), state: IntakeState::Ready, summary: None, rev: 3, prefs: IntakeStartPrefs::default(), last_error: None, started: vec![] };
        let mut src = record(Some(purpose("目的", 1)), vec![]);
        src.intake = Some(intake.clone());
        let rec = handoff_conversation(&key("p"), Some(&src), None, seed(SeedState::Pending), LocalId("new".into()), UnixMillis(9));
        assert_eq!(rec.intake, Some(intake.clone()));
        // 空の先頭のやり直しでも写る
        src.seed = Some(seed(SeedState::Pending));
        assert_eq!(redo_conversation(&key("n"), &src, UnixMillis(10)).unwrap().intake, Some(intake));
        // 分岐は別の会話なので持たない
        assert!(fork_conversation(LocalId("f".into()), purpose("目的", 1)).intake.is_none());
    }

    #[test]
    fn new_head_copies_purpose_and_chain_but_starts_undelivered_with_pending_seed() {
        let older = ThreadSegment { chat: key("p0"), handed_off_at: UnixMillis(5), title: Some("古い".into()) };
        let src = record(Some(purpose("目的", 3)), vec![older.clone()]);
        let rec = handoff_conversation(&key("p"), Some(&src), Some("元の名前".into()), seed(SeedState::Pending), LocalId("new".into()), UnixMillis(9));
        // 会話のIDは引き継いでも変えない。目的メモは写し、届けた記録・圧縮の印は持ち越さない。
        assert_eq!(rec.id, LocalId("c0".into()));
        assert_eq!(rec.purpose, src.purpose);
        assert_eq!((rec.purpose_delivered_rev, rec.compacted_since_delivery), (None, false));
        // 列は P の列＋P（古い順）。
        assert_eq!(rec.threads.len(), 2);
        assert_eq!(rec.threads[0], older);
        assert_eq!(rec.threads[1], ThreadSegment { chat: key("p"), handed_off_at: UnixMillis(9), title: Some("元の名前".into()) });
        assert_eq!(rec.seed.as_ref().map(|s| s.state), Some(SeedState::Pending));
        // P に記録がなければ、新しい会話ID・目的メモなし・列は P だけ。
        let fresh = handoff_conversation(&key("p"), None, None, seed(SeedState::Pending), LocalId("new".into()), UnixMillis(9));
        assert_eq!((fresh.id, fresh.purpose, fresh.threads.len()), (LocalId("new".into()), None, 1));
        // 引き継ぎ元の記録は変えない（呼び出し側が写しを残す）。
        assert_eq!(src.threads.len(), 1);
    }

    #[test]
    fn redo_reuses_the_empty_heads_material_and_does_not_list_the_empty_head() {
        let mut empty_head = record(Some(purpose("目的", 2)), vec![ThreadSegment { chat: key("p"), handed_off_at: UnixMillis(5), title: None }]);
        empty_head.seed = Some(seed(SeedState::Pending));
        assert!(is_redo_source(Some(&empty_head), true));
        assert!(!is_redo_source(Some(&empty_head), false), "履歴がある先頭はやり直しではない");
        assert!(!is_redo_source(None, true));
        let redo = redo_conversation(&key("n"), &empty_head, UnixMillis(20)).expect("Pending なら作れる");
        let s = redo.seed.as_ref().unwrap();
        // 資料の本文・元の引き継ぎ元は N のものをそのまま。時刻だけ新しく、受理は未確認。
        assert_eq!((s.material.as_str(), &s.from, s.created_at, s.state), ("■ 以前の会話からの引き継ぎ\n本文", &key("p"), UnixMillis(20), SeedState::Pending));
        // 列は N と同じ（空の N は入れない）。
        assert_eq!(redo.threads, empty_head.threads);
        // 受理を確認済みの資料はやり直せない（二重に送らない）。
        empty_head.seed = Some(seed(SeedState::Delivered));
        assert!(!is_redo_source(Some(&empty_head), true));
        assert!(redo_conversation(&key("n"), &empty_head, UnixMillis(20)).is_none());
    }

    #[test]
    fn fork_copies_only_a_non_empty_purpose_as_a_new_conversation() {
        let src = record(Some(purpose("目的", 3)), vec![ThreadSegment { chat: key("p"), handed_off_at: UnixMillis(5), title: None }]);
        let p = purpose_for_fork(Some(&src)).expect("写す");
        let f = fork_conversation(LocalId("f1".into()), p.clone());
        assert_eq!((f.id, f.purpose), (LocalId("f1".into()), Some(p)));
        assert!(f.threads.is_empty() && f.seed.is_none() && f.purpose_delivered_rev.is_none() && !f.compacted_since_delivery);
        assert!(purpose_for_fork(Some(&record(Some(purpose("  \n", 1)), vec![]))).is_none());
        assert!(purpose_for_fork(Some(&record(None, vec![]))).is_none());
        assert!(purpose_for_fork(None).is_none());
    }

    // ── 作業領域・作業フォルダ ──

    #[test]
    fn general_chat_reuses_the_sources_workspace_and_development_chat_keeps_its_folders() {
        let ws = r"C:\Users\x\AppData\Local\com.agentdock.app\chats\dir-1\workspace";
        assert_eq!(plan_start(ChatKind::General, Some(ws), None), Ok(StartPlan::ReuseWorkspace { workspace: ws.into() }));
        // 一般チャットに追加フォルダはない。記録があっても使わない。
        assert_eq!(plan_start(ChatKind::General, Some(ws), Some(&[ws.into(), r"D:\extra".into()])), Ok(StartPlan::ReuseWorkspace { workspace: ws.into() }));
        assert_eq!(plan_start(ChatKind::Development, Some(r"D:\proj"), None), Ok(StartPlan::Folders { cwd: r"D:\proj".into(), extras: vec![] }));
        let accepted = vec![r"D:\proj".to_string(), r"E:\lib".to_string()];
        assert_eq!(plan_start(ChatKind::Development, Some(r"D:\proj"), Some(&accepted)), Ok(StartPlan::Folders { cwd: r"D:\proj".into(), extras: vec![r"E:\lib".into()] }));
        // 作業フォルダが分からないものは、推測せず引き継がない。
        assert!(plan_start(ChatKind::General, None, None).is_err());
        assert!(plan_start(ChatKind::Development, Some("  "), None).is_err());
    }

    #[test]
    fn deleting_the_new_chat_never_removes_the_sources_workspace() {
        let root = temp_root("delete");
        let store = Store::open(root.clone()).unwrap();
        // P: 一般チャットの専用領域と作業領域の中のファイル。
        let (p_id, p_dir) = store.create_chat_dir().unwrap();
        let p = key("p");
        store.register_dir(&p, &p_id);
        store.save_chat_local(&ChatLocalFile::new(p_id.clone(), Some(p.clone()))).unwrap();
        let file = layout::workspace_dir(&p_dir).join("out.txt");
        std::fs::write(&file, b"keep").unwrap();
        // N: ホストと同じやり方（`dir_id_for` ＝ `ensure_dir_id`）で自分のチャット領域を持つ。作業領域は作らない。
        let n = key("n");
        let n_id = store.ensure_dir_id(&n);
        assert_ne!(n_id, p_id, "N は P と別のチャット領域を持つ");
        store.save_chat_local(&ChatLocalFile::new(n_id.clone(), Some(n.clone()))).unwrap();
        let n_dir = layout::chat_dir(&root, &n_id);
        assert!(n_dir.join(layout::CHAT_FILE).exists() && !layout::workspace_dir(&n_dir).exists());
        // N の削除（ホストの削除が最後に呼ぶ `remove_chat_dir`）は N の領域だけを消す。
        store.remove_chat_dir(&n).unwrap();
        assert!(!n_dir.exists(), "N の領域は消える");
        assert_eq!(std::fs::read(&file).unwrap(), b"keep", "P の作業領域のファイルは残る");
        assert!(p_dir.join(layout::CHAT_FILE).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_failed_start_in_the_sources_workspace_creates_and_removes_nothing() {
        let root = temp_root("start-fail");
        let store = Arc::new(Store::open(root.clone()).unwrap());
        let (p_id, p_dir) = store.create_chat_dir().unwrap();
        let ws = layout::workspace_dir(&p_dir);
        std::fs::write(ws.join("out.txt"), b"keep").unwrap();
        let chats_before = std::fs::read_dir(root.join(layout::CHATS_DIR)).unwrap().count();
        let host = Arc::new(Host::with_store(root.clone(), store));
        let args = StartChatArgs { backend: BackendKind::Codex, cwd: None, model: None, permission: None, first_message: None, worktree: None, extra_folders: vec![], confirmed_strong: vec![] };
        // 未接続なので開始は失敗する。新しい作業領域を作らず、失敗時の後始末も P の領域に触れない。
        let r = host.start_chat_in(args, Some(ws.to_string_lossy().into_owned()), &UserConfirmed::from_user_command()).await;
        assert!(r.is_err());
        assert_eq!(std::fs::read_dir(root.join(layout::CHATS_DIR)).unwrap().count(), chats_before, "チャット領域は増えも減りもしない");
        assert_eq!(std::fs::read(ws.join("out.txt")).unwrap(), b"keep");
        let _ = p_id;
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── 子孫・開始失敗の分類 ──

    #[test]
    fn descendant_summary_separates_busy_from_unconfirmed() {
        use AgentState::*;
        assert_eq!(descendant_summary(&[]), (false, 0));
        // 実行中・入力待ちがあれば止める側。
        assert_eq!(descendant_summary(&[fact(Running, Freshness::Live, false)]), (true, 0));
        assert_eq!(descendant_summary(&[fact(Waiting, Freshness::Live, false)]).0, true);
        // live の完了、または履歴で終端を確認済みのものは、不明に数えない。
        assert_eq!(descendant_summary(&[fact(Done, Freshness::Live, false), fact(Done, Freshness::HistoryOnly, true)]), (false, 0));
        // live でも終端確認済みでもないもの（読込み済みの完了、状態不明）は数えて注記する（止めない）。
        assert_eq!(descendant_summary(&[fact(Done, Freshness::HistoryOnly, false), fact(Unknown, Freshness::Live, false), fact(Done, Freshness::Live, false)]), (false, 2));
    }

    #[test]
    fn only_unknown_outcomes_are_reported_as_start_unknown() {
        assert!(start_unknown(IpcErrorCode::OutcomeUnknown));
        assert!(start_unknown(IpcErrorCode::Io));
        assert!(!start_unknown(IpcErrorCode::NotConnected));
        assert!(!start_unknown(IpcErrorCode::Rejected));
        assert!(!start_unknown(IpcErrorCode::InvalidArgs));
    }

    #[test]
    fn notes_and_attachment_text_are_explicit_about_what_happened() {
        assert_eq!(attach_note(0, 0), "");
        assert_eq!(attach_note(2, 1), "（添付: 新しいチャットの入力欄に1件、指定は2件）");
        assert_eq!(with_scan_note(vec!["a".into()], false), vec!["a".to_string()]);
        assert_eq!(with_scan_note(vec![], true).len(), 1);
    }

    #[test]
    fn deleting_the_new_head_returns_the_previous_thread_to_the_list_in_the_same_session() {
        let mut d = super::super::state::HostData::default();
        let (p, n) = (key("p"), key("n"));
        let mut pl = ChatLocalFile::new(LocalId("dp".into()), Some(p.clone()));
        pl.superseded_by = Some(Superseded { by: n.clone(), at: UnixMillis(1) });
        let other = ChatLocalFile::new(LocalId("do".into()), Some(key("o")));
        d.locals.insert(p.clone(), pl);
        d.locals.insert(n.clone(), ChatLocalFile::new(LocalId("dn".into()), Some(n.clone())));
        d.locals.insert(key("o"), other);
        d.forget_deleted_chat(&n);
        assert!(d.locals.get(&p).unwrap().superseded_by.is_none(), "P の印が外れる");
        assert!(d.locals.get(&n).is_none());
        assert!(d.locals.contains_key(&key("o")));
    }

    #[test]
    fn workspace_users_are_found_only_inside_the_owners_workspace() {
        let ws = std::path::Path::new(r"C:\app\chats\dir-p\workspace");
        let c = vec![
            (key("p"), r"C:\app\chats\dir-p\workspace".to_string()),
            (key("n"), r"C:\app\chats\dir-p\workspace".to_string()),
            (key("sub"), r"c:\APP\chats\dir-p\workspace\inner".to_string()),
            (key("other"), r"C:\app\chats\dir-x\workspace".to_string()),
            (key("dev"), r"D:\proj".to_string()),
            (key("rel"), "workspace".to_string()),
        ];
        assert_eq!(workspace_users(ws, &key("p"), &c), vec![key("n"), key("sub")]);
        assert!(workspace_users(ws, &key("p"), &c[3..]).is_empty(), "使う thread がなければ空");
    }

    #[tokio::test]
    async fn deleting_the_source_is_refused_while_another_thread_uses_its_workspace_and_allowed_otherwise() {
        let root = temp_root("h1");
        let store = Arc::new(Store::open(root.clone()).unwrap());
        let (p_id, p_dir) = store.create_chat_dir().unwrap();
        let (p, n) = (key("p"), key("n"));
        store.register_dir(&p, &p_id);
        let ws = layout::workspace_dir(&p_dir);
        std::fs::write(ws.join("out.txt"), b"keep").unwrap();
        let host = Arc::new(Host::with_store(root.clone(), store.clone()));
        let mk = |k: &ChatKey, cwd: String| Chat {
            key: k.clone(), kind: ChatKind::General, cwd: Known::direct(cwd), name: Known::NotFetched, preview: Known::NotFetched, pinned: false, archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged, draft: None, created_at: Known::NotFetched, last_used_at: None, no_history: false, project_id: None,
        };
        let ws_text = ws.to_string_lossy().into_owned();
        host.mutate(|d| ((), d.upsert_chat(mk(&p, ws_text.clone()))));
        // N がまだ無い: 使う thread がないので止めない（従来どおり、領域ごと消せる）。
        assert!(host.workspace_in_use_message(&p).is_none());
        // N が P の作業領域を使う: 削除は止まり、理由を返す。領域は消えない。
        // 削除が使用確認を通過した後に N が作られた場合: 領域を消す直前の確認が止める。
        assert!(host.check_area_removable(&p).is_ok());
        host.mutate(|d| ((), d.upsert_chat(mk(&n, ws_text.clone()))));
        assert!(host.check_area_removable(&p).is_err(), "N ができた後は領域を消さない");
        assert!(ws.join("out.txt").exists());
        // 削除が引き継ぎのガードを持っている間、引き継ぎは始まらない。
        let g = host.handoff_inflight.try_begin(&p).unwrap();
        assert!(matches!(host.handoff_and_send(HandoffAndSendArgs { chat: p.clone(), text: "x".into(), attachments: vec![] }, &UserConfirmed::from_user_command()).await.unwrap(), HandoffResult::NotStarted { .. }));
        drop(g);
        let m = host.workspace_in_use_message(&p).expect("止める");
        assert!(m.contains("引き継ぎ先"), "{m}");
        let e = host.delete_chat(crate::backend::local::ChatArgs { chat: p.clone() }, &UserConfirmed::from_user_command()).await.unwrap_err();
        assert_eq!(e.code, IpcErrorCode::Blocked);
        assert_eq!(std::fs::read(ws.join("out.txt")).unwrap(), b"keep");
        assert!(!host.manage_rt.is_deleting(&p), "削除中の印は残らない");
        // 使う thread が無くなれば（N を削除した後）、従来どおり P の領域は消せる。
        host.mutate(|d| ((), d.forget_deleted_chat(&n)));
        assert!(host.workspace_in_use_message(&p).is_none());
        store.remove_chat_dir(&p).unwrap();
        assert!(!p_dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn attachment_blocker_stops_unready_missing_and_unknown_attachments_before_anything_is_created() {
        use crate::backend::local::{AttachmentEntry, AttachmentSource, AttachmentState};
        use crate::backend::model::AttachmentKind;
        let mk = |id: &str, state: AttachmentState, path: Option<&str>| AttachmentEntry {
            id: LocalId(id.into()), chat: key("p"), kind: AttachmentKind::File, display_name: format!("{id}.txt"), source: AttachmentSource::ClipboardImage,
            copy_path: path.map(Into::into), size: Known::NotFetched, attached_at: UnixMillis(1), state, used_by: vec![],
        };
        let ledger = vec![mk("ok", AttachmentState::Ready, Some("a")), mk("busy", AttachmentState::Copying, None), mk("gone", AttachmentState::Ready, Some("zz")), mk("nopath", AttachmentState::Ready, None)];
        let exists = |p: &str| p == "a";
        let ids = |v: &[&str]| v.iter().map(|s| LocalId((*s).into())).collect::<Vec<_>>();
        assert_eq!(attachment_blocker(&ledger, &ids(&["ok"]), &exists), None);
        assert_eq!(attachment_blocker(&ledger, &[], &exists), None);
        assert!(attachment_blocker(&ledger, &ids(&["ok", "busy"]), &exists).unwrap().contains("コピー中"));
        assert!(attachment_blocker(&ledger, &ids(&["gone"]), &exists).unwrap().contains("見つかりません"));
        assert!(attachment_blocker(&ledger, &ids(&["nopath"]), &exists).is_some());
        assert!(attachment_blocker(&ledger, &ids(&["nothing"]), &exists).unwrap().contains("台帳"));
    }

    // ── 引き継ぎが始まらない経路（P を変えない） ──

    async fn host_with_chat(id: &str) -> (Arc<Host>, ChatKey) {
        let host = Arc::new(Host::new(std::env::temp_dir()));
        let k = key(id);
        let chat = Chat {
            key: k.clone(),
            kind: ChatKind::General,
            cwd: Known::direct(r"C:\x\workspace".into()),
            name: Known::NotFetched,
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
            no_history: false,
            project_id: None,
        };
        host.mutate(|d| ((), d.upsert_chat(chat)));
        (host, k)
    }

    fn args(chat: &ChatKey, text: &str) -> HandoffAndSendArgs {
        HandoffAndSendArgs { chat: chat.clone(), text: text.into(), attachments: vec![] }
    }

    #[tokio::test]
    async fn empty_request_and_unknown_chat_are_errors_and_disconnected_chat_does_not_start() {
        let (host, p) = host_with_chat("p").await;
        let confirmed = UserConfirmed::from_user_command();
        assert_eq!(host.handoff_and_send(args(&p, "  "), &confirmed).await.unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert_eq!(host.handoff_and_send(args(&key("missing"), "x"), &confirmed).await.unwrap_err().code, IpcErrorCode::NotFound);
        // 未接続: 何も作らず NotStarted。P の記録（引き継ぎ済みの印）も下書きも変えない。
        match host.handoff_and_send(args(&p, "続きをお願いします"), &confirmed).await.unwrap() {
            HandoffResult::NotStarted { message, start_unknown } => {
                assert!(!start_unknown);
                assert!(message.contains("接続"), "{message}");
            }
            other => panic!("開始しないはず: {other:?}"),
        }
        assert!(host.read(|d| d.locals.get(&p).is_none_or(|l| l.superseded_by.is_none() && l.conversation.is_none())));
        assert_eq!(host.read(|d| d.chats.len()), 1, "新しいチャットは作られない");
    }

    #[tokio::test]
    async fn a_running_handoff_blocks_the_second_one_and_the_mark_is_released_afterwards() {
        let (host, p) = host_with_chat("p").await;
        let confirmed = UserConfirmed::from_user_command();
        let guard = host.handoff_inflight.try_begin(&p).expect("1本目");
        match host.handoff_and_send(args(&p, "x"), &confirmed).await.unwrap() {
            HandoffResult::NotStarted { message, .. } => assert!(message.contains("実行中"), "{message}"),
            other => panic!("止まるはず: {other:?}"),
        }
        drop(guard);
        // 印が外れたあとは、前提の判断（ここでは未接続）まで進む＝「実行中」では止まらない。
        match host.handoff_and_send(args(&p, "x"), &confirmed).await.unwrap() {
            HandoffResult::NotStarted { message, .. } => assert!(!message.contains("実行中"), "{message}"),
            other => panic!("開始しないはず: {other:?}"),
        }
        // 「編集して再送・再生成」の実行中も、同じ元からの引き継ぎを通さない。
        let _resend = host.resend_inflight.try_begin(&p).expect("再送");
        match host.handoff_and_send(args(&p, "x"), &confirmed).await.unwrap() {
            HandoffResult::NotStarted { message, .. } => assert!(message.contains("再生成"), "{message}"),
            other => panic!("止まるはず: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_already_handed_off_chat_is_refused_by_the_precondition() {
        let (host, p) = host_with_chat("p").await;
        host.update_local(&p, false, Duration::ZERO, |f| f.superseded_by = Some(Superseded { by: key("n"), at: UnixMillis(1) }));
        let confirmed = UserConfirmed::from_user_command();
        assert!(host.handoff_send_ctx(&p).await.ctx.superseded);
        // 古い画面からの2回目は、新しい thread を作らずに止まる（接続していればブロッカーは「引き継ぎ済み」。ここでは未接続の理由が先に出る）。
        assert!(matches!(host.handoff_and_send(args(&p, "x"), &confirmed).await.unwrap(), HandoffResult::NotStarted { .. }));
        assert_eq!(host.read(|d| d.chats.len()), 1);
    }
}
