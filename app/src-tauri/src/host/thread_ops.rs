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
use crate::backend::local::{ChatArgs, SaveScope, SaveStatus};
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::gitops::GitOp;
use crate::rules::thread_ops::{clear_draft_after, fork_name_suffix, judge_op_observation, match_fork, merge_fork_of, ForkInherit, parse_branch_lines, parse_commit_lines, resend_blocker, resend_next, review_target_check, ForkCandidate, ForkMatch, OpObservation, ResendBlock, ResendBlockKind, ResendCtx, ResendNext};
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

/// 実行中の「編集して再送・再生成」の印（チャットごと）。同じチャットの2本目は始められない。ガードを手放すと外れる。
#[derive(Default)]
pub(super) struct ResendInflight {
    set: Arc<std::sync::Mutex<std::collections::HashSet<ChatKey>>>,
}

pub(super) struct ResendGuard {
    set: Arc<std::sync::Mutex<std::collections::HashSet<ChatKey>>>,
    chat: ChatKey,
}

impl ResendInflight {
    /// 実行中でなければ印を付けてガードを返す。実行中なら None。
    pub(super) fn try_begin(&self, chat: &ChatKey) -> Option<ResendGuard> {
        if !self.set.lock().unwrap().insert(chat.clone()) {
            return None;
        }
        Some(ResendGuard { set: self.set.clone(), chat: chat.clone() })
    }
}

impl Drop for ResendGuard {
    fn drop(&mut self) {
        self.set.lock().unwrap().remove(&self.chat);
    }
}

/// 要求を送る前に失敗が確定した（送られていない）エラーか。それ以外は、送ったかどうか分からないので受理不明として扱う。
fn failed_before_dispatch(e: &BackendError) -> bool {
    matches!(e, BackendError::NotConnected | BackendError::Unsupported { .. })
}

impl Host {
    pub(super) fn require_op(&self, op: ParityOp) -> Result<(), IpcError> {
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
    pub(super) fn chat_title(&self, chat: &ChatKey) -> String {
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
        let started = self.start_chat(StartChatArgs { backend: origin.backend, cwd: Some(cwd), model, permission, first_message: None, worktree: None }, confirmed).await?;
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
                if matches!(ack, OpAck::Accepted) {
                    // 応答の時点ではturnの開始通知が未処理のことがある。終端を観測するまで、キューの自動送信を保留する。
                    self.queue_rt.note_op_accept(chat);
                }
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
        self.fork_inner(&args.chat, args.through_turn, ForkPurpose::Fork, "分岐", confirmed).await
    }

    /// 分岐の本体（`fork_chat` と `resend_as_fork` で共用）。元の会話は変えない（再開もしない）。
    /// `name_suffix` は分岐先の名前「{元の名前}（{suffix}）」。拒否・受理不明は分岐先なしで返す（再送しない）。
    async fn fork_inner(self: &Arc<Self>, chat: &ChatKey, through_turn: Option<ExternalId>, purpose: ForkPurpose, name_suffix: &str, confirmed: &UserConfirmed) -> Result<ForkResult, IpcError> {
        self.require_op(ParityOp::Fork)?;
        self.check_op_target(chat)?;
        self.precheck_space()?;
        let root = agent_key_of(chat);
        let (running, running_ids) = self.read(|d| {
            let ids: Vec<ExternalId> = d.agents.iter().filter(|v| &v.agent.chat == chat).filter_map(|v| d.running_turn.get(&v.agent.key).cloned()).collect();
            (d.running_turn.contains_key(&root) || !ids.is_empty(), ids)
        });
        match &through_turn {
            None if running => return Err(err(IpcErrorCode::InvalidArgs, "実行中のチャットは、分岐するturn（終了しているもの）を指定してください")),
            Some(t) if running_ids.contains(t) => return Err(err(IpcErrorCode::InvalidArgs, "実行中のturnは指定できません。終了したturnを指定してください")),
            _ => {}
        }
        let title = self.chat_title(chat);
        let attempted_at = now_ms();
        let params = ForkParams { through_turn: through_turn.clone(), ephemeral: false, read_only: false };
        let out = self.backend.fork_chat(chat.clone(), params, confirmed).await?;
        let Some(new_key) = out.chat.filter(|_| out.ack == OpAck::Accepted) else {
            // 拒否・受理不明。受理不明は再送せず、照合（読取り）を案内する。
            return Ok(ForkResult { ack: out.ack, chat: None, attempted_at, note: None });
        };
        let origin = ForkOrigin { chat: chat.clone(), through_turn, purpose };
        let (found, note) = self.register_forked_chat(&new_key, origin).await;
        self.rename_best_effort(&new_key, format!("{title}（{name_suffix}）"), confirmed).await;
        let found = self.read(|d| d.chat(&new_key).cloned()).or(found);
        Ok(ForkResult { ack: OpAck::Accepted, chat: found, attempted_at, note })
    }

    /// 編集して再送・再生成（M50）。分岐（終点まで）→ 分岐先の下書き保存 → （求められたときだけ）1回送る。
    /// 元の会話は変えず、resumeは送信の経路（分岐先への通常の送信）以外ではしない。自動再送しない:
    /// 分岐が受理不明なら送らず（照合は読取りだけ）、下書きを保存できなければ送らず、送信の受理不明・拒否は再送しない。
    pub async fn resend_as_fork(self: &Arc<Self>, args: ResendAsForkArgs, confirmed: &UserConfirmed) -> Result<ResendAsForkResult, IpcError> {
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼の文が空です"));
        }
        if args.purpose == ForkPurpose::Fork {
            return Err(err(IpcErrorCode::InvalidArgs, "目的は「編集」か「再生成」を指定してください"));
        }
        self.require_chat(&args.chat)?;
        self.check_op_target(&args.chat)?;
        self.precheck_space()?;
        // 同じチャットの実行中は、2本目を通さない（応答待ちにダイアログを閉じて開き直しても、二重に分岐・送信しない）。
        let Some(_inflight) = self.resend_inflight.try_begin(&args.chat) else {
            return Err(blocked(BlockedReason::ChatBusy, "このチャットの「編集して再送・再生成」を実行中です。結果が出るまでお待ちください"));
        };
        // 履歴を読んで（読取りのみ）、無効の条件を検査する。ホストの判定を正とする。
        let history = match self.is_connected() {
            true => Some(self.read_history(agent_key_of(&args.chat), ReadOptions { include_turns: true }).await),
            false => None,
        };
        let turns: &[TurnRecord] = match &history {
            Some(Ok(h)) => &h.turns,
            Some(Err(e)) => return Err(err(IpcErrorCode::Io, format!("会話の履歴を読めないため、分岐できません（{e}）"))),
            None => &[],
        };
        let ctx = self.resend_ctx(&args, turns);
        if let Some(b) = resend_blocker(&ctx) {
            return Err(self.resend_block_error(&args.chat, b));
        }
        let suffix = if args.purpose == ForkPurpose::Regenerate { "再生成" } else { "編集" };
        let fork = self.fork_inner(&args.chat, Some(args.through_turn.clone()), args.purpose, suffix, confirmed).await?;
        let Some(new_chat) = fork.chat.as_ref().filter(|_| fork.ack == OpAck::Accepted).map(|c| c.key.clone()) else {
            // 受理されなかった（拒否・受理不明）。送らず、下書きも保存しない（受理不明は分岐先の確認＝照合を案内する）。
            debug_assert_eq!(resend_next(false, false, args.send), ResendNext::StopAfterFork);
            return Ok(ResendAsForkResult { fork, draft_saved: false, send: None, send_error: None });
        };
        // 分岐先の下書きへ保存し、書き終えるのを待つ。保存できなければ送らない。
        let text = args.text.clone();
        self.update_local(&new_chat, true, std::time::Duration::ZERO, |f| {
            f.draft.text = text;
            f.draft.updated_at = Some(now_ms());
        });
        let saved = matches!(self.save_now(SaveScope::ChatLocal { chat: new_chat.clone() }).await, Some(SaveStatus { state: SaveState::Saved { .. }, .. }));
        if resend_next(true, saved, args.send) != ResendNext::SendOnce {
            return Ok(ResendAsForkResult { fork, draft_saved: saved, send: None, send_error: None });
        }
        // 通常の送信経路で1回だけ送る（送信ロック・clientUserMessageId・Sending保存・受理不明の扱い）。再送しない。
        // 送る前に止まったとき（Err。送信は起きていない）は、分岐先ができていることを伝えるため Ok で理由を返す。下書きは残す。
        let send = match self.send_message(SendMessageArgs { chat: new_chat.clone(), text: args.text, attachments: Vec::new(), intent: SendIntent::NewTurn }, confirmed).await {
            Ok(s) => s,
            Err(e) => return Ok(ResendAsForkResult { fork, draft_saved: true, send: None, send_error: Some(e.message) }),
        };
        if clear_draft_after(&send.state) {
            self.update_local(&new_chat, true, std::time::Duration::ZERO, |f| {
                f.draft.text.clear();
                f.draft.updated_at = Some(now_ms());
            });
        }
        Ok(ResendAsForkResult { fork, draft_saved: true, send: Some(send), send_error: None })
    }

    /// 無効の判断材料（`rules::thread_ops::resend_blocker` へ渡す）。
    fn resend_ctx(&self, args: &ResendAsForkArgs, turns: &[TurnRecord]) -> ResendCtx {
        let idx = turns.iter().position(|t| t.key.turn_id == args.through_turn);
        let target = idx.and_then(|i| turns.get(i + 1)).filter(|t| t.key.turn_id == args.target_turn);
        let through_end_known = !args.through_turn.0.is_empty() && idx.is_some_and(|i| turns[i].end.is_some());
        let target_request_count = target.map(|t| t.entries.iter().filter(|e| matches!(e.kind, ActivityKind::UserMessage)).count()).unwrap_or(0);
        let chat = &args.chat;
        let capability_problem = self.require_op(ParityOp::Fork).err().map(|e| e.message);
        let delete_pending = self.manage_rt.is_deleting(chat) || self.read(|d| d.locals.get(chat).is_some_and(|l| l.delete_pending.is_some()));
        let (running_elsewhere, stop_unconfirmed, agents_busy, root_state_unknown) = self.read(|d| {
            let fresh = d.root_view(chat).map(|v| v.freshness);
            let elsewhere = d.chat(chat).is_some_and(|c| super::state::external_send_locked(c.origin, fresh));
            let busy = d.agents.iter().any(|v| &v.agent.chat == chat && (matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing) || d.running_turn.contains_key(&v.agent.key)));
            let unknown = d.root_view(chat).is_none_or(|v| v.status.state == AgentState::Unknown && !d.unknown_confirmed_idle(v));
            (elsewhere, d.open_stop(chat).is_some(), busy, unknown)
        });
        let busy = agents_busy || self.unknown_attempt(chat).is_some() || self.read(|d| d.locals.get(chat).is_some_and(|l| !l.pending_ops.is_empty()));
        ResendCtx {
            capability_problem,
            connected: self.is_connected(),
            delete_pending,
            running_elsewhere,
            stop_unconfirmed,
            busy,
            root_state_unknown,
            target_follows_through: target.is_some(),
            target_request_count,
            through_end_known,
        }
    }

    fn resend_block_error(&self, chat: &ChatKey, b: ResendBlock) -> IpcError {
        match b.kind {
            ResendBlockKind::Capability => blocked(BlockedReason::CapabilityUnsupported { capability: ParityOp::Fork.name() }, b.message),
            ResendBlockKind::NotConnected => err(IpcErrorCode::NotConnected, b.message),
            ResendBlockKind::DeletePending => blocked(BlockedReason::DeletePending, b.message),
            ResendBlockKind::RunningElsewhere => blocked(BlockedReason::RunningElsewhere, b.message),
            ResendBlockKind::StopUnconfirmed => match self.read(|d| d.open_stop(chat).map(|r| r.id.clone())) {
                Some(record) => blocked(BlockedReason::StopUnconfirmed { record }, b.message),
                None => err(IpcErrorCode::Blocked, b.message),
            },
            // 受理不明の送信が原因のときは、理由をそう示す（実行中とは別。照合で解ける）。
            ResendBlockKind::Busy if self.unknown_attempt(chat).is_some() => blocked(BlockedReason::ChatBusy, "前の送信の受理を確認できていません。「履歴と照合」で確認してから使えます"),
            ResendBlockKind::Busy => blocked(BlockedReason::ChatBusy, b.message),
            ResendBlockKind::StateUnknown | ResendBlockKind::Stale | ResendBlockKind::NoRequest | ResendBlockKind::MultipleRequests | ResendBlockKind::PreviousNotEnded => err(IpcErrorCode::InvalidArgs, b.message),
        }
    }

    /// 分岐先をアプリ管理として記録し、読めれば一覧へ入れる（読取りのみ）。読めないときは記録だけ残し、補足を返す。
    async fn register_forked_chat(self: &Arc<Self>, key: &ChatKey, origin: ForkOrigin) -> (Option<Chat>, Option<String>) {
        let read = self.read_history(agent_key_of(key), ReadOptions { include_turns: false }).await;
        self.adopt_forked(key, origin, read.ok().map(|h| (h.chat, h.agent, h.status)))
    }

    /// 分岐先の記録と一覧への反映。`found` は読めた（チャット・ルートのエージェント・状態）。
    fn adopt_forked(self: &Arc<Self>, key: &ChatKey, origin: ForkOrigin, found: Option<(Option<Chat>, Agent, AgentStatus)>) -> (Option<Chat>, Option<String>) {
        let meta = found.as_ref().and_then(|(c, _, _)| c.as_ref()).map(|c| cached_meta_of(c, now_ms()));
        self.mutate(|d| {
            d.hosted.insert(key.clone());
            ((), vec![])
        });
        // 元の会話の権限・モデル・作業モード・worktreeを、分岐先の未設定の項目へ写す（送る前に。元より広い権限で走らせない）。
        self.inherit_fork_settings(&origin.chat, key);
        let mut conflict = false;
        self.update_local(key, true, std::time::Duration::ZERO, |f| {
            f.hosted = true;
            let (o, c) = merge_fork_of(f.fork_of.take(), origin);
            f.fork_of = Some(o);
            conflict = c;
            if let Some(m) = meta {
                f.cached_meta = Some(m);
            }
        });
        let conflict_note = conflict.then(|| "この会話はすでに別の会話からの分岐として記録されています。記録は変えていません".to_string());
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
        (self.read(|d| d.chat(key).cloned()).or(chat), conflict_note)
    }

    /// 分岐先へ、元のチャットのローカル設定を写す（未設定の項目だけ）。メモリへ即時反映し、保存は呼び出し側が待つ。
    fn inherit_fork_settings(self: &Arc<Self>, src: &ChatKey, dst: &ChatKey) {
        let inherit = self.read(|d| {
            let ms = d.model_settings.get(src);
            ForkInherit::from_source(d.locals.get(src), ms.and_then(|s| s.selected.clone()), ms.and_then(|s| s.work_mode))
        });
        if inherit.model.is_some() || inherit.work_mode.is_some() {
            self.mutate(|d| {
                let s = d.model_settings.entry(dst.clone()).or_insert_with(|| ChatModelSettings::blank(ApplyTiming::NextTurn));
                if s.selected.is_none() {
                    s.selected = inherit.model.clone();
                }
                if s.work_mode.is_none() {
                    s.work_mode = inherit.work_mode;
                }
                let s = s.clone();
                ((), vec![HostEvent::ModelSettingsUpdated { chat: dst.clone(), settings: s }])
            });
        }
        self.update_local(dst, true, std::time::Duration::ZERO, |f| inherit.apply_to_local(f));
    }

    /// 受理不明の分岐の照合（読取りのみ。再送しない）。ちょうど1件に絞れたときだけ分岐先として採用する。
    /// 採用したときだけ、目的つきの記録（まだなければ）と名前の接尾辞（編集・再生成のとき）を通常の経路と同じにする。
    pub async fn reconcile_fork(self: &Arc<Self>, args: ReconcileForkArgs, confirmed: &UserConfirmed) -> Result<ForkReconcile, IpcError> {
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
                // すでに記録済み（通常の経路が登録した）なら、目的・終点・名前は変えない。
                let registered = self.read(|d| d.locals.get(&key).is_some_and(|l| l.fork_of.is_some()));
                let title = self.chat_title(&args.chat);
                let origin = ForkOrigin { chat: args.chat.clone(), through_turn: args.through_turn.clone(), purpose: args.purpose };
                let (chat, _) = self.adopt_forked(&key, origin, Some((Some(s.chat.clone()), s.root.clone(), s.status.clone())));
                if !registered && args.purpose != ForkPurpose::Fork {
                    self.rename_best_effort(&key, format!("{title}（{}）", fork_name_suffix(args.purpose)), confirmed).await;
                }
                let adopted = self.read(|d| d.chat(&key).cloned()).or(chat);
                Ok(ForkReconcile::Adopted { chat: adopted.unwrap_or_else(|| s.chat.clone()) })
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
            .read_history(agent_key_of(&args.chat), ReadOptions { include_turns: true })
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
        let history = match self.read_history(agent_key_of(&args.chat), ReadOptions { include_turns: true }).await {
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
    fn a_second_resend_for_the_same_chat_is_refused_until_the_first_ends() {
        let inflight = ResendInflight::default();
        let key = |id: &str| ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) };
        let first = inflight.try_begin(&key("a")).expect("1本目は始められる");
        assert!(inflight.try_begin(&key("a")).is_none());
        // 別のチャットは妨げない。
        assert!(inflight.try_begin(&key("b")).is_some());
        drop(first);
        assert!(inflight.try_begin(&key("a")).is_some());
    }

    #[test]
    fn pending_records_use_the_same_op_for_both_review_deliveries() {
        assert_eq!(TurnOp::Review(ReviewTarget::UncommittedChanges).parity(), ParityOp::CodeReview);
        assert_eq!(TurnOp::Compact.parity(), ParityOp::Compact);
    }
}
