//! 組織実行の開始前の依頼整理（DESIGN_ORG_INTAKE §1）。Tauriコマンド（`commands.rs`）が呼ぶ `Host` のメソッド。
//!
//! 守ること:
//! - **会話中はエンジンもドライバも作らない**。`org_start` を呼ぶのは `org_intake_start`（利用者の「開始」の操作）だけ。
//! - 依頼整理チャットは利用者の普通のチャット（`org` 所有記録なし。Q9 の対象外）。ワークフローへは何も伝わらない。
//! - 「要約を作る」は押した回数だけ送る。切断・受理不明で再送しない（結果の確認は履歴の読取りだけ）。
//! - 要約の結果を確認できなければ成功にしない（`summaryUnconfirmed`）。出す操作は「履歴から読み直す」と、利用者が押す「もう一度要約を作る」だけ。
//! - 開始は要約の版（`rev`）と、確認ダイアログで見せた依頼文が現在と一致するときだけ。`intake.json` を書けなければ始めない。

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::backend::backend::{AiBackend, BackendEvent, ReadOptions, UserConfirmed};
use crate::backend::intake::*;
use crate::backend::ipc::{BlockedReason, HostEvent, IpcError, IpcErrorCode, SendIntent, SendMessageArgs, StartChatArgs};
use crate::backend::local::SaveScope;
use crate::backend::model::{BackendKind, Chat, ChatKey, ChatKind, LocalId, SaveState, SendState, Support, UnixMillis};
use crate::backend::org::{StartOrgArgs, StartOrgResult};
use crate::gitops::prep::repo_status;
use crate::host::state::agent_key_of;
use crate::host::{blocked, err, now_ms, Host, SendExtras};
use crate::rules::diff::parse_status_v2;
use crate::rules::org_intake::*;
use crate::store::atomic::write_atomic;
use crate::store::org::{list_workflow_dirs_newest, INTAKE_FILE};
use crate::store::records::{ConversationRecord, PurposeNote, PurposeSource};

/// `intake.json` の版。
const COPY_SCHEMA_VERSION: u32 = 1;
/// 起動時の照合で見る、新しいワークフローの数。
const RECONCILE_SCAN_MAX: usize = 50;

/// 依頼整理の作業状態（再起動で消えてよいもの）。
#[derive(Default)]
pub struct IntakeRuntime {
    /// 結果の確認（履歴の読取り）を実行中のチャット（二重実行の防止）。
    resolving: Mutex<HashSet<ChatKey>>,
    /// 履歴を読んだが要約の turn が見つからなかったチャット（「確認できません」を出す根拠）。
    not_found: Mutex<HashSet<ChatKey>>,
}

struct ResolveGuard<'a> {
    rt: &'a IntakeRuntime,
    chat: ChatKey,
}

impl Drop for ResolveGuard<'_> {
    fn drop(&mut self) {
        self.rt.resolving.lock().unwrap().remove(&self.chat);
    }
}

impl IntakeRuntime {
    fn begin_resolve(&self, chat: &ChatKey) -> Option<ResolveGuard<'_>> {
        let inserted = self.resolving.lock().unwrap().insert(chat.clone());
        inserted.then(|| ResolveGuard { rt: self, chat: chat.clone() })
    }
}

/// 開始時の要約の写しを保存する（`org\<wf>\intake.json`）。書けなければ Err（開始しない）。
pub fn save_copy(wf_dir: &Path, origin: &IntakeOrigin, at: UnixMillis) -> std::io::Result<()> {
    let copy = IntakeCopy {
        schema_version: COPY_SCHEMA_VERSION,
        chat: origin.chat.clone(),
        intake_id: origin.intake_id.clone(),
        rev: origin.rev,
        summary: origin.summary.clone(),
        composed_request: origin.composed_request.clone(),
        started_at: at,
    };
    let bytes = serde_json::to_vec_pretty(&copy).map_err(std::io::Error::other)?;
    write_atomic(&wf_dir.join(INTAKE_FILE), &bytes)
}

fn io_summary_error(message: impl Into<String>) -> IpcError {
    err(IpcErrorCode::InvalidArgs, message)
}

impl Host {
    fn intake_record(&self, chat: &ChatKey) -> Option<IntakeRecord> {
        self.read(|d| d.locals.get(chat).and_then(|l| l.conversation.as_ref()).and_then(|c| c.intake.clone()))
    }

    fn intake_required(&self, chat: &ChatKey) -> Result<IntakeRecord, IpcError> {
        self.intake_record(chat).ok_or_else(|| err(IpcErrorCode::NotFound, "このチャットには依頼整理がありません"))
    }

    fn emit_intake_updated(&self, chat: &ChatKey) {
        self.mutate(|_| ((), vec![HostEvent::IntakeUpdated { chat: chat.clone() }]));
    }

    /// 記録を会話の記録へ書く（なければ会話の記録を作る）。メモリへ反映して保存を依頼するだけ。
    fn put_intake(self: &Arc<Self>, chat: &ChatKey, rec: Option<IntakeRecord>) {
        let conv_id = self.local_id("conv");
        let now = now_ms();
        self.update_local(chat, true, std::time::Duration::ZERO, |f| match (&mut f.conversation, rec) {
            (Some(c), r) => c.intake = r,
            (None, Some(r)) => f.conversation = Some(new_conversation(conv_id, None, now, Some(r))),
            (None, None) => {}
        });
    }

    /// 記録を保存まで終える。保存できなければメモリを `prev` に戻して Err（送る前の保存に使う）。
    async fn commit_intake(self: &Arc<Self>, chat: &ChatKey, next: IntakeRecord, prev: Option<IntakeRecord>) -> Result<(), IpcError> {
        self.put_intake(chat, Some(next));
        let scope = SaveScope::ChatLocal { chat: chat.clone() };
        let saved = self.save_now(scope.clone()).await;
        if saved.is_some_and(|s| matches!(s.state, SaveState::Saved { .. })) {
            self.emit_intake_updated(chat);
            return Ok(());
        }
        self.put_intake(chat, prev);
        let _ = self.save_now(scope.clone()).await;
        Err(blocked(BlockedReason::SaveFailed { scope }, "依頼整理の記録を保存できなかったため、操作を行っていません"))
    }

    // ───────────── 作成 ─────────────

    /// 依頼整理チャットを作る（普通のチャット。作業フォルダ＝cwd、権限は ReadOnly、目的メモに進め方を入れる）。
    pub async fn org_intake_begin(self: &Arc<Self>, args: IntakeBeginArgs, confirmed: &UserConfirmed) -> Result<IntakeBeginResult, IpcError> {
        let folder = args.folder.trim().to_string();
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Err(io_summary_error(format!("作業フォルダが見つかりません: {}", args.folder)));
        }
        let started = self
            .start_chat(
                StartChatArgs {
                    backend: BackendKind::Codex,
                    cwd: Some(folder.clone()),
                    model: args.model,
                    permission: Some(INTAKE_PERMISSION),
                    first_message: None,
                    worktree: None,
                    extra_folders: Vec::new(),
                    confirmed_strong: Vec::new(),
                },
                confirmed,
            )
            .await?;
        let chat = started.chat.key.clone();
        let mut rec = new_record(self.local_id("intake"), folder);
        rec.write_only = matches!(args.mode, IntakeBeginMode::Write);
        let purpose = PurposeNote { text: INTAKE_PURPOSE_TEXT.to_string(), revision: 1, updated_at: now_ms(), source: PurposeSource::Chat };
        let conv_id = self.local_id("conv");
        let now = now_ms();
        self.update_local(&chat, true, std::time::Duration::ZERO, |f| f.conversation = Some(new_conversation(conv_id, Some(purpose), now, Some(rec))));
        let scope = SaveScope::ChatLocal { chat: chat.clone() };
        let saved = self.save_now(scope.clone()).await;
        if !saved.is_some_and(|s| matches!(s.state, SaveState::Saved { .. })) {
            return Err(blocked(
                BlockedReason::SaveFailed { scope },
                "依頼整理のチャットは作りましたが、依頼整理の記録を保存できませんでした。チャットの「…」メニューから、もう一度依頼整理にしてください",
            ));
        }
        let intake = self.org_intake_view(IntakeChatArgs { chat }).await?;
        Ok(IntakeBeginResult { chat: started.chat, intake })
    }

    /// 既存の自分のチャットを依頼整理にする（目的メモは変えない）。
    pub async fn org_intake_attach(self: &Arc<Self>, args: IntakeChatArgs) -> Result<IntakeView, IpcError> {
        let chat = args.chat;
        let (info, owned): (Option<Chat>, bool) = self.read(|d| (d.chat(&chat).cloned(), d.locals.get(&chat).is_some_and(|l| l.org.is_some())));
        let Some(info) = info else { return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません")) };
        if owned {
            return Err(blocked(BlockedReason::OrgOwned, "組織実行のチャットは、依頼整理にできません"));
        }
        if let Some(existing) = self.intake_record(&chat) {
            if !matches!(existing.state, IntakeState::Abandoned) {
                return self.org_intake_view(IntakeChatArgs { chat }).await;
            }
        }
        if info.kind != ChatKind::Development {
            return Err(io_summary_error("作業フォルダのあるチャットだけ、組織実行の依頼にできます"));
        }
        let Some(folder) = info.cwd.value().cloned() else { return Err(io_summary_error("チャットの作業フォルダを確認できません")) };
        let prev = self.intake_record(&chat);
        let rec = new_record(self.local_id("intake"), folder);
        self.commit_intake(&chat, rec, prev).await?;
        // 承認を出さず書込みもさせない権限にする（保存できなくても、次の送信から効く設定として記録する）。
        self.update_local(&chat, true, std::time::Duration::ZERO, |f| f.permission = Some(INTAKE_PERMISSION));
        self.org_intake_view(IntakeChatArgs { chat }).await
    }

    // ───────────── 読取り ─────────────

    /// 画面用の写し。読取りだけ（Gitの読取りと、入力ファイルの有無の確認）。何も書かず、何も送らない。
    pub async fn org_intake_view(self: &Arc<Self>, args: IntakeChatArgs) -> Result<IntakeView, IpcError> {
        let chat = args.chat;
        let rec = self.intake_required(&chat)?;
        let structured_output = self.backend.capabilities().structured_output == Support::Supported;
        let inputs: Vec<String> = rec.summary.as_ref().map(|s| s.inputs.iter().map(|i| i.path.clone()).filter(|p| !p.trim().is_empty()).collect()).unwrap_or_default();
        let checks = if inputs.is_empty() { Vec::new() } else { self.intake_input_checks(&rec.prefs.folder, &inputs).await };
        let mut blockers: Vec<IntakeBlockerView> = Vec::new();
        let mut composed_preview = None;
        if let Some(summary) = &rec.summary {
            blockers.extend(summary_blockers(summary).into_iter().map(|message| IntakeBlockerView { message }));
            match compose_planner_request(rec.rev, summary, &checks) {
                Ok(text) => composed_preview = Some(text),
                Err(message) => blockers.push(IntakeBlockerView { message }),
            }
        }
        let summary_unconfirmed = matches!(rec.state, IntakeState::Summarizing { .. })
            && (!self.is_connected() || self.unknown_attempt_for_intake(&chat) || self.org_rt.intake.not_found.lock().unwrap().contains(&chat));
        Ok(IntakeView {
            chat,
            state: rec.state,
            summary: rec.summary,
            rev: rec.rev,
            prefs: rec.prefs,
            last_error: rec.last_error,
            started: rec.started,
            input_checks: checks.into_iter().map(|(path, check)| InputCheckView { label: input_label(check).to_string(), path, check }).collect(),
            blockers,
            composed_preview,
            structured_output,
            summary_unconfirmed,
            write_only: rec.write_only,
        })
    }

    fn unknown_attempt_for_intake(&self, chat: &ChatKey) -> bool {
        self.send_unsettled(chat)
    }

    /// 入力ファイルの検査（読取りだけ）。作業フォルダに対して、あるか・中か・コミット済みか。
    async fn intake_input_checks(&self, folder: &str, inputs: &[String]) -> Vec<(String, InputCheck)> {
        let git = self.git_for_worktree();
        // Gitの事実は、作業フォルダ自身がリポジトリのルートのときだけ使う（そうでなければ「検査できていません」）。
        let tracked_status = match repo_status(&git, Path::new(folder)).await {
            crate::backend::git_ops::RepoStatusView::Ready { .. } => git.run(Path::new(folder), &crate::gitops::GitOp::StatusAllUntracked).await.ok().filter(|o| o.success()),
            _ => None,
        };
        let changed: Option<Vec<String>> = tracked_status.map(|o| parse_status_v2(&String::from_utf8_lossy(&o.stdout)).into_iter().map(|e| e.path).collect());
        let mut out = Vec::new();
        for path in inputs {
            let resolved = resolve_input(folder, path);
            let (exists, inside, rel) = match &resolved {
                Some((rel, abs)) => (Path::new(abs).exists(), true, Some(rel.clone())),
                None => {
                    // 作業フォルダの外（または解けない）。存在だけを確かめる（相対パスは作業フォルダ基準）。
                    let p = Path::new(path.trim());
                    let abs = if p.is_absolute() { p.to_path_buf() } else { Path::new(folder).join(p) };
                    (abs.exists(), false, None)
                }
            };
            let mut git_checked = false;
            let (mut tracked, mut is_changed) = (false, false);
            if let (Some(rel), Some(changed)) = (&rel, &changed) {
                if exists {
                    if let Ok(o) = git.run(Path::new(folder), &crate::gitops::GitOp::LsFilesCached { path: rel.clone() }).await {
                        if o.success() {
                            git_checked = true;
                            tracked = !o.stdout.is_empty();
                            let prefix = format!("{rel}/");
                            is_changed = changed.iter().any(|c| c == rel || c.starts_with(&prefix));
                        }
                    }
                }
            }
            out.push((path.clone(), classify_input(&InputFacts { exists, inside, git_checked, tracked, changed: is_changed })));
        }
        out
    }

    // ───────────── 要約 ─────────────

    /// 「要約を作る」。押した回数だけ1回送る。送る前に状態（`Summarizing`）を保存し、保存できなければ送らない。
    pub async fn org_intake_summarize(self: &Arc<Self>, args: IntakeChatArgs, confirmed: &UserConfirmed) -> Result<crate::backend::model::SendAttempt, IpcError> {
        let chat = args.chat;
        let prev = self.intake_required(&chat)?;
        if self.backend.capabilities().structured_output != Support::Supported {
            return Err(err(IpcErrorCode::Unsupported, "このAIは構造化出力に対応していないため、要約を作れません。手で書いてください"));
        }
        let cm = self.local_id("cm").0;
        let next = apply_event(&prev, IntakeEvent::SummaryRequested { client_message_id: cm.clone(), at: now_ms() }).map_err(io_summary_error)?;
        self.org_rt.intake.not_found.lock().unwrap().remove(&chat);
        self.commit_intake(&chat, next, Some(prev.clone())).await?;
        let sent = self
            .send_message_with(
                SendMessageArgs { chat: chat.clone(), text: summary_request_message(), attachments: Vec::new(), intent: SendIntent::NewTurn, steer_entry: None },
                confirmed,
                SendExtras { output_schema: Some(summary_output_schema()), client_message_id: Some(cm) },
            )
            .await;
        match sent {
            Ok(attempt) => {
                // 明確に受理されなかった（送っていない）なら、要約を作る前の状態へ戻す。受理不明はそのまま（照合・読み直しで確認する）。
                if let SendState::Rejected { message } = &attempt.state {
                    self.restore_after_unsent(&chat, &prev, format!("要約の依頼は受理されませんでした: {message}")).await;
                }
                Ok(attempt)
            }
            Err(e) => {
                self.restore_after_unsent(&chat, &prev, format!("要約の依頼を送れませんでした: {}", e.message)).await;
                Err(e)
            }
        }
    }

    /// 送っていないことが確かなので、要約を作る前の記録に戻す（理由だけ残す）。保存に失敗してもメモリは戻す。
    async fn restore_after_unsent(self: &Arc<Self>, chat: &ChatKey, prev: &IntakeRecord, reason: String) {
        let mut r = prev.clone();
        r.last_error = Some(reason);
        self.put_intake(chat, Some(r));
        let _ = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await;
        self.emit_intake_updated(chat);
    }

    /// 「履歴から読み直す」。履歴の読取りだけで、要約の turn の結果を取り込む（送信・再開はしない）。
    pub async fn org_intake_reload_summary(self: &Arc<Self>, args: IntakeChatArgs) -> Result<IntakeView, IpcError> {
        self.intake_resolve(&args.chat).await?;
        self.org_intake_view(args).await
    }

    /// turn の終端を観測したときに呼ぶ（待たない）。要約を待っているチャットなら、履歴を読んで結果を取り込む。
    pub(in crate::host) fn intake_observe_event(self: &Arc<Self>, event: &BackendEvent) {
        let BackendEvent::TurnEnded { turn, .. } = event else { return };
        let chat = ChatKey { backend: turn.agent.backend, id: turn.agent.id.clone() };
        let waiting = self.intake_record(&chat).is_some_and(|r| matches!(r.state, IntakeState::Summarizing { .. }));
        if !waiting {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            if let Err(e) = host.intake_resolve(&chat).await {
                crate::diag::log("org-intake", &format!("resolve failed: {}", e.message));
            }
        });
    }

    /// 要約の turn の結果を履歴から読んで反映する。`Summarizing` 以外では何もしない。
    async fn intake_resolve(self: &Arc<Self>, chat: &ChatKey) -> Result<(), IpcError> {
        let Some(_guard) = self.org_rt.intake.begin_resolve(chat) else { return Ok(()) };
        let rec = self.intake_required(chat)?;
        let IntakeState::Summarizing { sent_at, .. } = &rec.state else { return Ok(()) };
        let history = self.read_history(agent_key_of(chat), ReadOptions { include_turns: true }).await?;
        let facts: Vec<TurnFacts> = history.turns.iter().map(facts_of).collect();
        let event = match pick_summary_turn(&facts, sent_at.0) {
            SummaryTurn::NotFound => {
                self.org_rt.intake.not_found.lock().unwrap().insert(chat.clone());
                self.emit_intake_updated(chat);
                return Ok(());
            }
            SummaryTurn::Running => {
                self.org_rt.intake.not_found.lock().unwrap().remove(chat);
                return Ok(());
            }
            SummaryTurn::Failed(reason) => IntakeEvent::SummaryFailed(reason),
            SummaryTurn::Done(text) => match parse_summary_output(&text) {
                Ok(s) => IntakeEvent::SummaryParsed(s),
                Err(e) => IntakeEvent::SummaryFailed(format!("要約を読み取れませんでした（手で書くこともできます）: {e}")),
            },
        };
        let next = apply_event(&rec, event).map_err(io_summary_error)?;
        self.org_rt.intake.not_found.lock().unwrap().remove(chat);
        // 保存に失敗しても、取り込んだ結果はメモリに残す（保存は再試行される）。
        self.put_intake(chat, Some(next));
        let _ = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await;
        self.emit_intake_updated(chat);
        Ok(())
    }

    // ───────────── 編集・やめる ─────────────

    /// 要約と開始時の設定を保存する（手で書く・編集）。`base_rev` が現在の版と違えば拒否する。
    pub async fn org_intake_save(self: &Arc<Self>, args: IntakeSaveArgs) -> Result<IntakeView, IpcError> {
        let chat = args.chat;
        let prev = self.intake_required(&chat)?;
        if prev.rev != args.base_rev {
            return Err(io_summary_error("要約が別の操作で更新されています。画面を読み直してください"));
        }
        check_summary_limits(&args.summary).map_err(io_summary_error)?;
        let folder = args.prefs.folder.trim().to_string();
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Err(io_summary_error(format!("作業フォルダが見つかりません: {}", args.prefs.folder)));
        }
        let prefs = IntakeStartPrefs { folder, ..args.prefs };
        let next = apply_event(&prev, IntakeEvent::Edited { summary: args.summary, prefs }).map_err(io_summary_error)?;
        self.commit_intake(&chat, next, Some(prev)).await?;
        self.org_intake_view(IntakeChatArgs { chat }).await
    }

    /// 依頼整理をやめる（チャットは消さない）。
    pub async fn org_intake_abandon(self: &Arc<Self>, args: IntakeChatArgs, _confirmed: &UserConfirmed) -> Result<IntakeView, IpcError> {
        let chat = args.chat;
        let prev = self.intake_required(&chat)?;
        let next = apply_event(&prev, IntakeEvent::Abandon).map_err(io_summary_error)?;
        self.commit_intake(&chat, next, Some(prev)).await?;
        self.org_intake_view(IntakeChatArgs { chat }).await
    }

    // ───────────── 開始 ─────────────

    /// 組織実行を開始する。要約の版が `rev` と同じで、確認ダイアログで見せた依頼文が現在と一字一句同じときだけ。
    pub async fn org_intake_start(self: &Arc<Self>, args: IntakeStartArgs, confirmed: UserConfirmed) -> Result<StartOrgResult, IpcError> {
        let chat = args.chat;
        let rec = self.intake_required(&chat)?;
        if !matches!(rec.state, IntakeState::Ready) {
            return Err(io_summary_error("開始できる状態の要約がありません"));
        }
        if rec.rev != args.rev {
            return Err(io_summary_error("要約が更新されています。もう一度内容を確認してください"));
        }
        let Some(summary) = rec.summary.clone() else { return Err(io_summary_error("要約がありません")) };
        if let Some(b) = summary_blockers(&summary).first() {
            return Err(io_summary_error(format!("開始できません: {b}")));
        }
        if !Path::new(&rec.prefs.folder).is_dir() {
            return Err(io_summary_error(format!("作業フォルダが見つかりません: {}", rec.prefs.folder)));
        }
        let inputs: Vec<String> = summary.inputs.iter().map(|i| i.path.clone()).filter(|p| !p.trim().is_empty()).collect();
        let checks = if inputs.is_empty() { Vec::new() } else { self.intake_input_checks(&rec.prefs.folder, &inputs).await };
        let composed = compose_planner_request(rec.rev, &summary, &checks).map_err(io_summary_error)?;
        if composed != args.composed_request {
            return Err(io_summary_error("確認した依頼文と現在の内容が違います（入力ファイルの状態が変わった可能性があります）。もう一度確認してください"));
        }
        let origin = IntakeOrigin { chat: chat.clone(), intake_id: rec.id.clone(), rev: rec.rev, summary, composed_request: composed.clone() };
        let start = StartOrgArgs {
            request: composed,
            repo: rec.prefs.folder.clone(),
            max_writers: rec.prefs.max_writers,
            max_total: rec.prefs.max_total,
            graph_id: rec.prefs.graph_id.clone(),
            max_turns: rec.prefs.max_turns,
            max_retries: None,
        };
        let mut result = self.org_start_checked_with(start, Some(origin), confirmed).await?;
        // 開始の印を保存する。保存できなくてもワークフローは動かしたまま警告を出す（起動時に `intake.json` から直す）。
        let latest = self.intake_record(&chat).unwrap_or(rec);
        let warning = match apply_event(&latest, IntakeEvent::StartRecorded { workflow_id: result.workflow_id.clone(), at: now_ms() }) {
            Ok(next) => match self.commit_intake(&chat, next.clone(), None).await {
                Ok(()) => None,
                Err(_) => {
                    // メモリには残す（保存は再試行される）。
                    self.put_intake(&chat, Some(next));
                    Some("組織実行は始まりましたが、依頼整理の記録に「開始済み」を保存できませんでした。再起動すると自動で直ります".to_string())
                }
            },
            Err(m) => Some(format!("組織実行は始まりましたが、依頼整理の記録を更新できませんでした: {m}")),
        };
        result.intake_warning = warning;
        Ok(result)
    }

    // ───────────── 起動時の照合 ─────────────

    /// ワークフローは動いたのに `Started` の保存が失敗していた依頼整理を、`intake.json` から直す（読取りと記録の更新だけ）。
    pub(in crate::host) fn intake_reconcile_started(self: &Arc<Self>, root: &Path) {
        for dir in list_workflow_dirs_newest(root).into_iter().take(RECONCILE_SCAN_MAX) {
            let Ok(bytes) = std::fs::read(dir.join(INTAKE_FILE)) else { continue };
            let Ok(copy) = serde_json::from_slice::<IntakeCopy>(&bytes) else { continue };
            let Some(workflow_id) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let Some(rec) = self.intake_record(&copy.chat) else { continue };
            let mark = StartedMark { intake_id: copy.intake_id.0.clone(), rev: copy.rev, workflow_id, at: copy.started_at };
            if let Some(fixed) = reconcile_intake_started(&rec, &mark) {
                self.put_intake(&copy.chat, Some(fixed));
            }
        }
    }
}

fn new_record(id: LocalId, folder: String) -> IntakeRecord {
    IntakeRecord { id, state: IntakeState::Drafting, summary: None, rev: 0, prefs: IntakeStartPrefs { folder, ..Default::default() }, last_error: None, started: Vec::new(), write_only: false }
}

fn new_conversation(id: LocalId, purpose: Option<PurposeNote>, _now: UnixMillis, intake: Option<IntakeRecord>) -> ConversationRecord {
    ConversationRecord { id, purpose, threads: Vec::new(), seed: None, purpose_delivered_rev: None, compacted_since_delivery: false, replaces: None, intake }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{ChatKey, ExternalId};

    fn origin() -> IntakeOrigin {
        IntakeOrigin {
            chat: ChatKey { backend: BackendKind::Codex, id: ExternalId("t1".into()) },
            intake_id: LocalId("i1".into()),
            rev: 2,
            summary: IntakeSummary { goal: "g".into(), done_criteria: vec!["d".into()], ..Default::default() },
            composed_request: "# 依頼".into(),
        }
    }

    #[test]
    fn the_copy_is_written_before_start_and_a_write_failure_is_reported() {
        let dir = std::env::temp_dir().join(format!("agentdock-intake-copy-{}-{}", std::process::id(), now_ms().0));
        let wf = dir.join("org").join("wf-1");
        save_copy(&wf, &origin(), UnixMillis(7)).unwrap();
        let copy: IntakeCopy = serde_json::from_slice(&std::fs::read(wf.join(INTAKE_FILE)).unwrap()).unwrap();
        assert_eq!((copy.rev, copy.started_at, copy.composed_request.as_str()), (2, UnixMillis(7), "# 依頼"));
        // a path whose parent is a file cannot be written: the caller refuses to start
        let blocker = dir.join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(save_copy(&blocker.join("wf-2"), &origin(), UnixMillis(7)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_runtime_allows_one_resolve_per_chat() {
        let rt = IntakeRuntime::default();
        let a = ChatKey { backend: BackendKind::Codex, id: ExternalId("a".into()) };
        let g = rt.begin_resolve(&a).expect("first");
        assert!(rt.begin_resolve(&a).is_none());
        drop(g);
        assert!(rt.begin_resolve(&a).is_some());
    }
}
