//! コードレビュー・レビュー結果の置き場所・会話の分岐・文脈の圧縮（P3-2、`app/DESIGN_P3.md` §1 #3・#4・#6・#7）。
//!
//! - すべてユーザーの明示操作（確認つき）。作業中・停止未確認・削除保留・外部実行中・受理不明の記録があるチャットでは、
//!   turnを始める操作（レビュー・圧縮）をしない。分岐は元の会話を変えないので、実行中でも終端したturnを指定すれば可。
//! - turnを始める操作は、送る前に `PendingOp` を保存する（保存できなければ送らない）。受理不明のあいだはキューを保留し、再送しない。
//!   解消できるのは読取り専用の照合だけ（履歴に痕跡があるか。`rules::thread_ops`）。
//! - レビューの別チャットは、新しい会話を作ってから同じ会話の中で実行する（`detached` は使わない）。
//!   会話を作れたがレビューを始められなかったときは、会話を残して区別して返す（自動では再試行しない）。
//! - 圧縮は、送る前に圧縮前の本文を控えとして保存する（保存できなければ送らない。控えは表示専用で自動削除しない）。
//! - 分岐の受理不明は、分岐元が一致し開始以降に作られた会話がちょうど1件のときだけ採用する（読取りのみ。再送しない）。
//! - 完了・結果は、レビュー結果や圧縮の記録（item）を観測したときだけ表示する。ここでは「受け付けた」までしか返さない。

use std::sync::Arc;

use super::persist::{cached_meta_of, save_failure_message};
use super::state::agent_key_of;
use super::{blocked, derive_chat_name, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::changes::ListStatus;
use crate::backend::ipc::*;
use crate::backend::local::ChatArgs;
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::gitops::GitOp;
use crate::rules::thread_ops::{judge_op_observation, match_fork, parse_branch_lines, parse_commit_lines, review_target_check, ForkCandidate, ForkMatch, OpObservation};
use crate::store::records::CompactionFile;
use crate::store::StoreError;

use super::changes::GitCtx;
use super::pending_ops::unresolved_same_op;

/// レビュー対象の候補として出すコミットの上限（`gitops` 側の取得件数と同じ）。
const REVIEW_COMMITS_MAX: usize = 30;

/// turnを始める操作。未解決の記録（`PendingOp`）の種類は、別チャット方式でもレビューは `CodeReview` に揃える。
enum TurnOp {
    Review(ReviewTarget),
    Compact,
}

impl TurnOp {
    fn parity(&self) -> ParityOp {
        match self {
            TurnOp::Review(_) => ParityOp::CodeReview,
            TurnOp::Compact => ParityOp::Compact,
        }
    }
}

/// 要求を送る前に失敗が確定した（送られていない）エラーか。それ以外は、送ったかどうか分からないので受理不明として扱う。
fn failed_before_dispatch(e: &BackendError) -> bool {
    matches!(e, BackendError::NotConnected | BackendError::Unsupported { .. })
}

impl Host {
    fn require_op(&self, op: ParityOp) -> Result<(), IpcError> {
        let cap = self.backend.capabilities().ops.into_iter().find(|c| c.op == op);
        if cap.is_some_and(|c| c.support == Support::Supported) {
            return Ok(());
        }
        Err(IpcError {
            code: IpcErrorCode::Unsupported,
            message: "この操作は、このCodex接続では使えません".into(),
            blocked: Some(BlockedReason::CapabilityUnsupported { capability: op.name() }),
        })
    }

    /// turnを始める操作を止める条件。結果が未確認の操作・作業中・停止未確認・送信の受理不明。
    fn turn_op_blocker(&self, chat: &ChatKey) -> Option<IpcError> {
        if let Some(op) = self.read(|d| d.locals.get(chat).and_then(|l| l.pending_ops.first()).map(|p| p.id.clone())) {
            return Some(blocked(BlockedReason::OperationUnconfirmed { op }, "結果が未確認の操作があります。履歴で確認（読取りのみ）してから実行してください"));
        }
        if self.manage_rt.is_deleting(chat) {
            return Some(blocked(BlockedReason::DeletePending, "このチャットは削除中のため、この操作はできません"));
        }
        let busy = self.read(|d| d.agents.iter().any(|v| &v.agent.chat == chat && d.running_turn.contains_key(&v.agent.key)) || d.open_stop(chat).is_some());
        if busy || self.unknown_attempt(chat).is_some() {
            return Some(blocked(BlockedReason::ChatBusy, "作業中・停止未確認、または送信の受理が未確認のため、この操作はできません。完了または停止の確認後に実行してください"));
        }
        None
    }

    /// チャットの表示名（名前、なければ最初の依頼の先頭）。
    fn chat_title(&self, chat: &ChatKey) -> String {
        self.read(|d| {
            d.chat(chat).and_then(|c| match c.name.value() {
                Some(n) if !n.trim().is_empty() => Some(n.clone()),
                _ => c.preview.value().and_then(|p| derive_chat_name(p)),
            })
        })
        .unwrap_or_else(|| "チャット".to_string())
    }

    /// 名前を付ける（受け付けられなければそのまま。履歴のない会話は名前を付けられないことがある）。
    async fn rename_best_effort(self: &Arc<Self>, chat: &ChatKey, name: String, confirmed: &UserConfirmed) {
        if self.backend.manage_chat(chat.clone(), ManageOp::Rename { name: name.clone() }, confirmed).await.is_err() {
            return;
        }
        let key = chat.clone();
        self.mutate(|d| {
            if let Some(c) = d.chats.iter_mut().find(|c| c.key == key) {
                c.name = Known::direct(name);
                let c = c.clone();
                return ((), vec![HostEvent::ChatUpdated { chat: c }]);
            }
            ((), vec![])
        });
    }

    // ───────────── レビュー（#3・#4） ─────────────

    /// レビュー対象の候補（読取りのみ）。Gitのリポジトリでなければ理由つきで返し、候補は空にする。
    pub async fn get_review_choices(self: &Arc<Self>, args: ChatArgs) -> Result<ReviewChoices, IpcError> {
        self.require_chat(&args.chat)?;
        let (root, git) = match self.git_context(&args.chat).await {
            GitCtx::Ready { root, git } => (root, git),
            GitCtx::NotSupported(message) => return Ok(ReviewChoices { git: ListStatus::NotSupported { message }, branches: vec![], commits: vec![] }),
            GitCtx::NotFetched(message) => return Ok(ReviewChoices { git: ListStatus::NotFetched { message }, branches: vec![], commits: vec![] }),
        };
        let branches = match git.run(&root, &GitOp::Branches).await {
            Ok(o) if o.success() => parse_branch_lines(&o.stdout_text()),
            _ => return Ok(ReviewChoices { git: ListStatus::NotFetched { message: "ブランチの一覧を取得できませんでした".into() }, branches: vec![], commits: vec![] }),
        };
        // コミットがまだない（初回コミット前）などで取れないときは、候補を空にする（ブランチ・未コミットの選択は別に可否が決まる）。
        let commits = match git.run(&root, &GitOp::RecentCommits).await {
            Ok(o) if o.success() => parse_commit_lines(&o.stdout_text()).into_iter().take(REVIEW_COMMITS_MAX).map(|(sha, title)| CommitChoice { sha, title }).collect(),
            _ => Vec::new(),
        };
        Ok(ReviewChoices { git: ListStatus::Ready, branches, commits })
    }

    /// 対象がこの作業フォルダで使えるか（Gitのリポジトリと確認できなければ、指示だけ）。
    async fn check_review_target(self: &Arc<Self>, chat: &ChatKey, target: &ReviewTarget) -> Result<(), IpcError> {
        let (repo_ready, branches) = match target {
            ReviewTarget::Custom { .. } => (false, Vec::new()),
            _ => match self.git_context(chat).await {
                GitCtx::Ready { root, git } => {
                    let branches = match git.run(&root, &GitOp::Branches).await {
                        Ok(o) if o.success() => parse_branch_lines(&o.stdout_text()),
                        _ => Vec::new(),
                    };
                    (true, branches)
                }
                _ => (false, Vec::new()),
            },
        };
        review_target_check(target, repo_ready, &branches).map_err(|m| err(IpcErrorCode::InvalidArgs, m))
    }

    /// 同じ作業フォルダ・モデル・権限で新しい会話を作る（レビューの別チャット）。名前はレビューの開始後に付ける。
    async fn create_review_chat(self: &Arc<Self>, origin: &ChatKey, confirmed: &UserConfirmed) -> Result<ChatKey, IpcError> {
        let (cwd, model, permission) = self.read(|d| {
            (d.chat(origin).and_then(|c| c.cwd.value().cloned()), d.model_settings.get(origin).and_then(|s| s.selected.clone()), d.locals.get(origin).and_then(|l| l.permission))
        });
        let Some(cwd) = cwd else { return Err(err(IpcErrorCode::InvalidArgs, "作業フォルダが分からないため、別チャットを作れません")) };
        let started = self.start_chat(StartChatArgs { backend: origin.backend, cwd: Some(cwd), model, permission, first_message: None }, confirmed).await?;
        let key = started.chat.key;
        self.update_local(&key, true, std::time::Duration::ZERO, |f| f.review_of = Some(origin.clone()));
        Ok(key)
    }

    /// レビュー。現在の会話か、新しい会話で実行する。受け付けた事実だけを返し、結果は会話の記録（レビュー結果）の観測で示す。
    pub async fn start_review(self: &Arc<Self>, args: StartReviewArgs, confirmed: &UserConfirmed) -> Result<ReviewOutcome, IpcError> {
        let cap_op = match args.delivery {
            ReviewDelivery::CurrentChat => ParityOp::CodeReview,
            ReviewDelivery::NewChat => ParityOp::ReviewToNewChat,
        };
        self.require_op(cap_op)?;
        self.check_op_target(&args.chat)?;
        if let Some(e) = self.turn_op_blocker(&args.chat) {
            return Err(e);
        }
        self.precheck_space()?;
        self.check_review_target(&args.chat, &args.target).await?;
        let origin_title = self.chat_title(&args.chat);
        let (run_chat, created) = match args.delivery {
            ReviewDelivery::CurrentChat => (args.chat.clone(), false),
            ReviewDelivery::NewChat => (self.create_review_chat(&args.chat, confirmed).await?, true),
        };
        let result = self.dispatch_turn_op(&run_chat, TurnOp::Review(args.target), confirmed).await;
        let failed = |chat: ChatKey, message: String| ReviewOutcome::NewChatCreatedReviewFailed { chat, message };
        match result {
            Ok(OpAck::Rejected { message }) if created => Ok(failed(run_chat, message)),
            Err(e) if created => Ok(failed(run_chat, e.message)),
            Err(e) => Err(e),
            Ok(ack) => {
                if created && !matches!(ack, OpAck::Rejected { .. }) {
                    self.rename_best_effort(&run_chat, format!("レビュー: {origin_title}"), confirmed).await;
                }
                Ok(ReviewOutcome::Started { chat: run_chat, created_chat: created, ack })
            }
        }
    }

    /// turnを始める共通手順: 再開（必要なら）→ 送る前に未確認の記録を保存 → 送る → 結果に応じて記録を消す。
    /// 受理不明（応答なし・送ったか分からない失敗）は記録を残し、再送しない。
    async fn dispatch_turn_op(self: &Arc<Self>, chat: &ChatKey, op: TurnOp, confirmed: &UserConfirmed) -> Result<OpAck, IpcError> {
        self.ensure_live(chat, confirmed).await?;
        let pending = self.begin_pending_op(chat, op.parity()).await?;
        let sent = match op {
            TurnOp::Review(target) => self.backend.start_review(chat.clone(), target, confirmed).await,
            TurnOp::Compact => self.backend.compact(chat.clone(), confirmed).await,
        };
        match sent {
            Ok(ack @ (OpAck::Accepted | OpAck::Rejected { .. })) => {
                self.resolve_pending_op(chat, &pending.id);
                Ok(ack)
            }
            Ok(unknown) => Ok(unknown),
            Err(e) if failed_before_dispatch(&e) => {
                self.resolve_pending_op(chat, &pending.id);
                Err(e.into())
            }
            Err(e) => Ok(OpAck::Unknown { message: e.to_string() }),
        }
    }

    // ───────────── 分岐（#6） ─────────────

    /// 会話の分岐。元の会話は変えない（再開もしない）。新しい会話はアプリ管理として一覧に入れる。
    pub async fn fork_chat(self: &Arc<Self>, args: ForkChatArgs, confirmed: &UserConfirmed) -> Result<ForkResult, IpcError> {
        self.require_op(ParityOp::Fork)?;
        self.check_op_target(&args.chat)?;
        self.precheck_space()?;
        let root = agent_key_of(&args.chat);
        let (running, running_ids) = self.read(|d| {
            let ids: Vec<ExternalId> = d.agents.iter().filter(|v| v.agent.chat == args.chat).filter_map(|v| d.running_turn.get(&v.agent.key).cloned()).collect();
            (d.running_turn.contains_key(&root) || !ids.is_empty(), ids)
        });
        match &args.through_turn {
            None if running => return Err(err(IpcErrorCode::InvalidArgs, "実行中のチャットは、分岐するturn（終了しているもの）を指定してください")),
            Some(t) if running_ids.contains(t) => return Err(err(IpcErrorCode::InvalidArgs, "実行中のturnは指定できません。終了したturnを指定してください")),
            _ => {}
        }
        let title = self.chat_title(&args.chat);
        let attempted_at = now_ms();
        let params = ForkParams { through_turn: args.through_turn.clone(), ephemeral: false, read_only: false };
        let out = self.backend.fork_chat(args.chat.clone(), params, confirmed).await?;
        let Some(new_key) = out.chat.filter(|_| out.ack == OpAck::Accepted) else {
            // 拒否・受理不明。受理不明は再送せず、照合（読取り）を案内する。
            return Ok(ForkResult { ack: out.ack, chat: None, attempted_at, note: None });
        };
        let origin = ForkOrigin { chat: args.chat.clone(), through_turn: args.through_turn };
        let (chat, note) = self.register_forked_chat(&new_key, origin).await;
        self.rename_best_effort(&new_key, format!("{title}（分岐）"), confirmed).await;
        let chat = self.read(|d| d.chat(&new_key).cloned()).or(chat);
        Ok(ForkResult { ack: OpAck::Accepted, chat, attempted_at, note })
    }

    /// 分岐先をアプリ管理として記録し、読めれば一覧へ入れる（読取りのみ）。読めないときは記録だけ残し、補足を返す。
    async fn register_forked_chat(self: &Arc<Self>, key: &ChatKey, origin: ForkOrigin) -> (Option<Chat>, Option<String>) {
        let read = self.backend.read(agent_key_of(key), ReadOptions { include_turns: false }).await;
        self.adopt_forked(key, origin, read.ok().map(|h| (h.chat, h.agent, h.status)))
    }

    /// 分岐先の記録と一覧への反映。`found` は読めた（チャット・ルートのエージェント・状態）。
    fn adopt_forked(self: &Arc<Self>, key: &ChatKey, origin: ForkOrigin, found: Option<(Option<Chat>, Agent, AgentStatus)>) -> (Option<Chat>, Option<String>) {
        let meta = found.as_ref().and_then(|(c, _, _)| c.as_ref()).map(|c| cached_meta_of(c, now_ms()));
        self.mutate(|d| {
            d.hosted.insert(key.clone());
            ((), vec![])
        });
        self.update_local(key, true, std::time::Duration::ZERO, |f| {
            f.hosted = true;
            f.fork_of = Some(origin);
            if let Some(m) = meta {
                f.cached_meta = Some(m);
            }
        });
        let Some((chat, agent, status)) = found else {
            return (None, Some("分岐は受け付けられましたが、分岐先の内容を読めませんでした。一覧を更新して確認してください".into()));
        };
        self.mutate(|d| {
            let mut ev = Vec::new();
            if let Some(c) = &chat {
                ev.extend(d.upsert_chat(c.clone()));
            }
            ev.extend(d.upsert_history_agent(agent, status));
            ((), ev)
        });
        self.touch_chat_used(key);
        (self.read(|d| d.chat(key).cloned()).or(chat), None)
    }

    /// 受理不明の分岐の照合（読取りのみ。再送しない）。ちょうど1件に絞れたときだけ分岐先として採用する。
    pub async fn reconcile_fork(self: &Arc<Self>, args: ReconcileForkArgs) -> Result<ForkReconcile, IpcError> {
        self.require_chat(&args.chat)?;
        let query = ListChatsQuery { limit: Some(50), include_external: true, ..Default::default() };
        let page = match self.backend.list_chats(query).await {
            Ok(p) => p,
            Err(e) => return Ok(ForkReconcile::Unreadable { message: e.to_string() }),
        };
        let candidates: Vec<ForkCandidate> = page
            .items
            .iter()
            .map(|s| ForkCandidate {
                chat: s.chat.key.clone(),
                forked_from: match &s.root.forked_from {
                    Known::Value { value: Some(k), .. } => Some(k.id.0.clone()),
                    _ => None,
                },
                created_at: s.chat.created_at.value().cloned(),
            })
            .collect();
        match match_fork(&candidates, &args.chat.id.0, args.attempted_at) {
            ForkMatch::One(key) => {
                let Some(s) = page.items.iter().find(|s| s.chat.key == key) else { return Ok(ForkReconcile::NotFound) };
                let origin = ForkOrigin { chat: args.chat.clone(), through_turn: None };
                let (chat, _) = self.adopt_forked(&key, origin, Some((Some(s.chat.clone()), s.root.clone(), s.status.clone())));
                Ok(ForkReconcile::Adopted { chat: chat.unwrap_or_else(|| s.chat.clone()) })
            }
            ForkMatch::None => Ok(ForkReconcile::NotFound),
            ForkMatch::Ambiguous(n) => Ok(ForkReconcile::Ambiguous { count: n as u32 }),
        }
    }

    // ───────────── 圧縮（#7） ─────────────

    /// 文脈の圧縮。送る前に圧縮前の本文を控えとして保存し、保存できなければ送らない。受け付けた事実だけを返す。
    pub async fn compact_chat(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<CompactChatResult, IpcError> {
        self.require_op(ParityOp::Compact)?;
        self.check_op_target(&args.chat)?;
        if let Some(e) = self.turn_op_blocker(&args.chat) {
            return Err(e);
        }
        self.precheck_space()?;
        let Some(store) = self.persist.store().cloned() else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、圧縮前の控えを保存できません。圧縮は送っていません"));
        };
        // 1. 圧縮前の本文を読む（読取りのみ。再開しない）。
        let history = self
            .backend
            .read(agent_key_of(&args.chat), ReadOptions { include_turns: true })
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("圧縮前の本文を取得できないため、圧縮は送っていません（{e}）")))?;
        // 2. 控えとして保存する（領域の書込みの直列化ロックの中で）。
        let at = now_ms();
        let file = CompactionFile { schema_version: 1, chat: args.chat.clone(), created_at: at, turns: history.turns };
        let host = self.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let _g = host.persist.io_guard();
            store.write_compaction(&file)
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("圧縮前の控えの保存に失敗しました: {e}")))?;
        let path = match saved {
            Ok(p) => p,
            Err(StoreError::InsufficientSpace { required, available }) => {
                return Err(blocked(BlockedReason::InsufficientSpace { required, available }, "空き容量が足りず圧縮前の控えを保存できないため、圧縮は送っていません。容量を空けてから再実行してください（保存データは自動では削除しません）"))
            }
            Err(e) => return Err(err(IpcErrorCode::Io, format!("圧縮前の控えを保存できないため、圧縮は送っていません: {}", save_failure_message(&e)))),
        };
        // 3. 送る。
        let ack = self.dispatch_turn_op(&args.chat, TurnOp::Compact, confirmed).await?;
        Ok(CompactChatResult { ack, snapshot: at, snapshot_path: path.to_string_lossy().into_owned() })
    }

    pub async fn list_compaction_snapshots(self: &Arc<Self>, args: ChatArgs) -> Result<Vec<UnixMillis>, IpcError> {
        self.require_chat(&args.chat)?;
        let Some(store) = self.persist.store().cloned() else { return Ok(Vec::new()) };
        let chat = args.chat;
        tokio::task::spawn_blocking(move || store.list_compactions(&chat))
            .await
            .map_err(|e| err(IpcErrorCode::Io, e.to_string()))?
            .map_err(|e| err(IpcErrorCode::Io, save_failure_message(&e)))
    }

    pub async fn read_compaction_snapshot(self: &Arc<Self>, args: ReadCompactionArgs) -> Result<CompactionSnapshot, IpcError> {
        self.require_chat(&args.chat)?;
        let Some(store) = self.persist.store().cloned() else { return Err(err(IpcErrorCode::Io, "保存先が使えません")) };
        let (chat, at) = (args.chat, args.snapshot);
        let file = tokio::task::spawn_blocking(move || store.read_compaction(&chat, at))
            .await
            .map_err(|e| err(IpcErrorCode::Io, e.to_string()))?
            .map_err(|e| err(IpcErrorCode::Io, save_failure_message(&e)))?;
        Ok(CompactionSnapshot { snapshot: file.created_at, turns: file.turns })
    }

    // ───────────── 受理不明の操作の照合（レビュー・圧縮） ─────────────

    /// 結果が未確認の操作を、履歴の痕跡で照合する（読取りのみ。操作は再送しない）。
    /// 痕跡を観測した、または痕跡がないと確定できたときだけ、未確認の記録を消す（キューの保留が解ける）。
    pub async fn reconcile_op(self: &Arc<Self>, args: ReconcileOpArgs) -> Result<OpReconcile, IpcError> {
        self.require_chat(&args.chat)?;
        let pending = self.read(|d| d.locals.get(&args.chat).and_then(|l| unresolved_same_op(&l.pending_ops, args.op).cloned()));
        let Some(pending) = pending else { return Err(err(IpcErrorCode::NotFound, "この操作の未確認の記録はありません")) };
        let history = match self.backend.read(agent_key_of(&args.chat), ReadOptions { include_turns: true }).await {
            Ok(h) => h,
            Err(e) => return Ok(OpReconcile::Unreadable { message: e.to_string() }),
        };
        Ok(match judge_op_observation(args.op, &history.turns, pending.since, now_ms()) {
            OpObservation::Observed => {
                self.resolve_pending_op(&args.chat, &pending.id);
                OpReconcile::Observed { resolved: true }
            }
            OpObservation::NotObserved => {
                self.resolve_pending_op(&args.chat, &pending.id);
                OpReconcile::NotObserved { resolved: true }
            }
            OpObservation::Undetermined { reason } => OpReconcile::Undetermined { reason },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_not_connected_and_unsupported_count_as_not_sent() {
        assert!(failed_before_dispatch(&BackendError::NotConnected));
        assert!(failed_before_dispatch(&BackendError::Unsupported { capability: "compact".into() }));
        // 応答の形の異常・入出力の失敗は、送ったかどうか分からない（受理不明として扱う）。
        assert!(!failed_before_dispatch(&BackendError::Protocol { message: "x".into() }));
        assert!(!failed_before_dispatch(&BackendError::Io { message: "x".into() }));
        assert!(!failed_before_dispatch(&BackendError::OutcomeUnknown { message: "x".into() }));
    }

    #[test]
    fn pending_records_use_the_same_op_for_both_review_deliveries() {
        assert_eq!(TurnOp::Review(ReviewTarget::UncommittedChanges).parity(), ParityOp::CodeReview);
        assert_eq!(TurnOp::Compact.parity(), ParityOp::Compact);
    }
}
