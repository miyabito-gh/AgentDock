//! 組織ワークフローの削除・アーカイブ（DESIGN_ORG_DELETE）。
//!
//! 守ること:
//! - 終わったワークフローだけが対象（可否は `rules::org_manage::verdict`）。削除のために中断せず、停止を確認できない役割チャットがあれば
//!   削除保留も作らずに断る（停止を確認してから、利用者が改めて選ぶ）。
//! - 実行順: `delete.json` を書く（書けなければ何もしない）→ 役割チャットを1件ずつ（既存の削除の排他・停止確認を通す）→ 記録フォルダ。
//!   途中で失敗したら完了と偽らず `Partial` を返し、続きは同じ操作で（済んだ段は飛ばす）。
//! - worktree・ブランチ・台帳は消さない（正本§3.15.4）。アーカイブはアプリ内だけの印で、Codex の `thread/archive` は送らない。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use org_engine::{restore, HaltReason};

use super::driver::{snapshot_of, OrgSnapshot};
use crate::backend::backend::UserConfirmed;
use crate::backend::intake::{IntakeRecord, IntakeState};use crate::backend::ipc::{BlockedReason, HostEvent, IpcError, IpcErrorCode};
use crate::backend::local::DeleteOutcome;
use crate::backend::model::{Basis, ChatKey, Known};
use crate::backend::org::*;
use crate::gitops::worktree as gw;
use crate::host::manage::{DeleteMode, SetGuard};
use crate::host::{blocked, err, now_ms, Host};
use crate::rules::manage::save_blocks_chat;
use crate::rules::org::chat_key_of_ref;
use crate::rules::org_manage::{
    chat_block_reason, delete_progress_counts, delete_targets, intake_delete_block, intake_link, intake_state_finished, next_steps, step_label, verdict, Binding, ChatFact, DeleteRecord, IntakeDeleteFact, ManageKind, Step, TaskFact,
    VerdictInput, FILES_LABEL,
};
use crate::store::org::{
    dir_bytes, read_archived_at, read_delete_record, remove_archived, remove_workflow_files, workflow_dir, write_archived, write_delete_record, ReadOnlyOrgStore, INTAKE_FILE,
};

/// 判定と実行の元になる、1つのワークフローの事実。
struct Facts {
    workflow_id: String,
    dir: PathBuf,
    /// 削除が途中ならその記録。
    record: Option<DeleteRecord>,
    /// 状態の写し。読めなければ None（削除の途中のものだけ。最初の判定では必ずある）。
    snapshot: Option<OrgSnapshot>,
    /// 消す（または「残す」を選ぶと解放する）未解放の役割チャット。途中の削除なら、まだ済んでいないもの。
    targets: Vec<ChatKey>,
    /// 手動に切り替え済みで残るチャット。
    kept_released: Vec<ChatKey>,
}

fn snapshot_from_disk(dir: &Path) -> Option<OrgSnapshot> {
    let (state, _) = restore(&ReadOnlyOrgStore::new(dir.to_path_buf())).ok()?;
    Some(snapshot_of(&state, None))
}

/// 依頼整理の記録が「開始した」と持つワークフローID（現在の状態の分と履歴の分、重複なし）。
fn intake_workflow_ids(i: &IntakeRecord) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let IntakeState::Started { workflow_id, .. } = &i.state {
        ids.push(workflow_id.clone());
    }
    for s in &i.started {
        if !ids.contains(&s.workflow_id) {
            ids.push(s.workflow_id.clone());
        }
    }
    ids
}

fn intake_state_text(i: &IntakeRecord) -> &'static str {
    match i.state {
        IntakeState::Started { .. } => "開始済み",
        IntakeState::Abandoned => "やめたもの",
        IntakeState::Drafting => "下書き",
        IntakeState::Summarizing { .. } => "要約中",
        IntakeState::Ready => "確認待ち",
    }
}

fn io_err(what: &str, e: impl std::fmt::Display) -> IpcError {
    err(IpcErrorCode::Io, format!("{what}: {e}"))
}

fn join_err(e: tokio::task::JoinError) -> IpcError {
    err(IpcErrorCode::Io, format!("処理が異常終了しました: {e}"))
}

impl Host {
    fn org_store_root(&self) -> Result<PathBuf, IpcError> {
        self.persist.store().map(|s| s.root().to_path_buf()).ok_or_else(|| err(IpcErrorCode::Io, "保存先が使えないため、ワークフローの記録を扱えません"))
    }

    /// 司令taskがこのワークフローで動いているか（終わった・止まったtaskは含めない）。
    fn org_driver_on(&self, workflow_id: &str) -> bool {
        self.org_rt.driver.lock().unwrap().as_ref().is_some_and(|d| d.workflow.0 == workflow_id && !d.tx.is_closed())
    }

    async fn org_manage_facts(self: &Arc<Self>, workflow_id: &str) -> Result<Facts, IpcError> {
        let root = self.org_store_root()?;
        let dir = workflow_dir(&root, workflow_id);
        if dir.file_name().is_none_or(|n| n.to_string_lossy() != workflow_id) {
            return Err(err(IpcErrorCode::NotFound, "ワークフローが見つかりません"));
        }
        let d = dir.clone();
        let (exists, record) = tokio::task::spawn_blocking(move || (d.is_dir(), read_delete_record(&d))).await.map_err(join_err)?;
        if !exists {
            return Err(err(IpcErrorCode::NotFound, "ワークフローが見つかりません"));
        }
        let record = record.map_err(|m| err(IpcErrorCode::Io, m))?;
        let snapshot = match self.org_snapshot().filter(|s| s.workflow.0 == workflow_id) {
            Some(s) => Some(s),
            None => {
                let d = dir.clone();
                tokio::task::spawn_blocking(move || snapshot_from_disk(&d)).await.map_err(join_err)?
            }
        };
        if snapshot.is_none() && record.is_none() {
            return Err(err(IpcErrorCode::Io, "ワークフローの記録を読めないため、扱えません"));
        }
        let bindings: Vec<Binding> = self.read(|d| {
            d.locals
                .iter()
                .filter_map(|(k, l)| l.org.as_ref().map(|o| Binding { chat: k.clone(), workflow_id: o.workflow_id.clone(), released: o.released_at.is_some() }))
                .collect()
        });
        let task_chats: Vec<ChatKey> = snapshot.as_ref().map(|s| s.tasks.iter().filter_map(|t| t.chat.as_ref().and_then(chat_key_of_ref)).collect()).unwrap_or_default();
        let (mut targets, kept_released) = delete_targets(workflow_id, &task_chats, &bindings);
        if let Some(rec) = &record {
            // 途中の削除は、始めたときに確認された集合のうち、まだ済んでいないものを続ける。
            targets = rec.chats.iter().filter(|c| !rec.done.contains(&step_label(&Step::Chat((*c).clone())))).cloned().collect();
        }
        Ok(Facts { workflow_id: workflow_id.to_string(), dir, record, snapshot, targets, kept_released })
    }

    fn org_chat_fact(&self, chat: &ChatKey) -> ChatFact {
        let work = self.chat_work(chat);
        let (name, pending, save_blocked) = self.read(|d| {
            (
                d.chat(chat).and_then(|c| c.name.value().cloned()),
                d.locals.get(chat).and_then(|l| l.delete_pending.clone()),
                save_blocks_chat(&d.save_status.values().cloned().collect::<Vec<_>>(), chat).is_some(),
            )
        });
        // 引き継ぎ中、または作業領域を別の thread が使っているチャットは削除できない（領域ごと消えるため）。
        let in_use = self
            .workspace_in_use_message(chat)
            .or_else(|| self.handoff_inflight.try_begin(chat).is_none().then(|| "引き継ぎを実行中です".to_string()));
        ChatFact { chat: chat.clone(), label: name.unwrap_or_else(|| chat.id.0.clone()), work, pending, save_blocked, in_use }
    }

    /// このワークフローを開始した依頼整理のチャット。第一は `intake.json`、なければ依頼整理の記録の「開始済み」から（1つに決まるときだけ）。
    async fn org_intake_link(self: &Arc<Self>, dir: &Path, workflow_id: &str) -> Option<ChatKey> {
        let d = dir.to_path_buf();
        let copy = tokio::task::spawn_blocking(move || {
            std::fs::read(d.join(INTAKE_FILE)).ok().and_then(|b| serde_json::from_slice::<crate::backend::intake::IntakeCopy>(&b).ok()).map(|c| c.chat)
        })
        .await
        .ok()
        .flatten();
        let started_by: Vec<(ChatKey, Vec<String>)> = self.read(|d| {
            d.locals
                .iter()
                .filter_map(|(k, l)| l.conversation.as_ref().and_then(|c| c.intake.as_ref()).map(|i| (k.clone(), intake_workflow_ids(i))))
                .collect()
        });
        intake_link(copy.as_ref(), &started_by, workflow_id)
    }

    /// 依頼整理チャット1件の削除の判定材料。読取りだけ（状態不明は履歴の終端を読んでから見る）。
    /// `bulk`＝左一覧のまとめて削除（ピン留め・紐づくワークフローの未終了も見る）。`except_workflow`＝いま削除するワークフロー（別の残っているワークフローの数から除く）。
    pub(in crate::host) async fn intake_delete_fact(self: &Arc<Self>, chat: &ChatKey, bulk: bool, except_workflow: Option<&str>) -> IntakeDeleteFact {
        self.confirm_unknown_terminals(Some(chat), true).await;
        let chat_fact = self.org_chat_fact(chat);
        let rec = self.read(|d| d.locals.get(chat).and_then(|l| l.conversation.as_ref()).and_then(|c| c.intake.clone()));
        let finished_intake = rec.as_ref().is_some_and(|r| intake_state_finished(&r.state));
        let (pinned, superseded) = self.read(|d| (d.chat(chat).is_some_and(|c| c.pinned) || d.locals.get(chat).is_some_and(|l| l.pinned), d.locals.get(chat).is_some_and(|l| l.superseded_by.is_some())));
        let ids: Vec<String> = rec.as_ref().map(intake_workflow_ids).unwrap_or_default();
        let root = self.org_store_root().ok();
        let mut workflow_unfinished = None;
        let mut other_alive = 0u32;
        for id in ids {
            let exists = root.as_ref().is_some_and(|r| workflow_dir(r, &id).is_dir());
            if !exists {
                continue; // 紐づけ先が消えている（見失わないよう、このチャットは通常の行に出ている）
            }
            if except_workflow == Some(id.as_str()) {
                continue;
            }
            other_alive += 1;
            if bulk && workflow_unfinished.is_none() {
                workflow_unfinished = self.org_workflow_unfinished(&id).await;
            }
        }
        // まとめて削除では、紐づくワークフローが残っていても、終わっていれば削除してよい（記録が残るだけ）。残っている件数は止める理由にしない。
        if bulk {
            other_alive = 0;
        }
        IntakeDeleteFact { chat: chat_fact, finished_intake, pinned: bulk && pinned, superseded, workflow_unfinished, other_workflows_alive: other_alive }
    }

    /// ワークフローがまだ終わっていない（または状態を確認できない）ときの理由。終わっていれば None。
    async fn org_workflow_unfinished(self: &Arc<Self>, workflow_id: &str) -> Option<String> {
        let facts = match self.org_manage_facts(workflow_id).await {
            Ok(f) => f,
            Err(e) => return Some(e.message),
        };
        let (v, _) = self.org_verdict(ManageKind::Archive, &facts, false).await;
        match v {
            OrgManageVerdict::Allowed => None,
            OrgManageVerdict::Blocked { reasons } => Some(reasons.join("／")),
        }
    }

    /// 判定。削除で役割チャットも消すなら、状態不明のチャットは履歴の終端を読んでから見る（読取りだけ。resumeしない）。
    async fn org_verdict(self: &Arc<Self>, kind: ManageKind, facts: &Facts, delete_chats: bool) -> (OrgManageVerdict, Vec<ChatFact>) {
        let mut chat_facts = Vec::new();
        if kind == ManageKind::Delete {
            for chat in &facts.targets {
                if delete_chats {
                    self.confirm_unknown_terminals(Some(chat), true).await;
                }
                chat_facts.push(self.org_chat_fact(chat));
            }
        }
        let tasks: Option<Vec<TaskFact>> = facts.snapshot.as_ref().map(|s| s.tasks.iter().map(|t| TaskFact { status: t.status, confirm: t.confirm }).collect());
        let v = verdict(
            kind,
            &VerdictInput {
                tasks: tasks.as_deref(),
                driver_on_this: self.org_driver_on(&facts.workflow_id),
                plan_pending: facts.snapshot.as_ref().is_some_and(|s| s.halt == Some(HaltReason::AwaitingPlanApproval)),
                connected: self.is_connected(),
                delete_chats,
                chats: &chat_facts,
            },
        );
        (v, chat_facts)
    }

    fn blocked_error(reasons: &[String]) -> IpcError {
        blocked(BlockedReason::ChatBusy, reasons.join("\n"))
    }

    // ───────────────────────── 削除確認の内容（読取りのみ） ─────────────────────────

    pub async fn org_preview_delete(self: &Arc<Self>, args: OrgWorkflowArgs) -> Result<OrgDeletePreview, IpcError> {
        let facts = self.org_manage_facts(&args.workflow_id).await?;
        let (verdict, chat_facts) = self.org_verdict(ManageKind::Delete, &facts, true).await;
        let snapshot_tasks = facts.snapshot.as_ref().map(|s| s.tasks.clone()).unwrap_or_default();
        let chats: Vec<OrgDeleteChatRow> = chat_facts
            .iter()
            .map(|f| {
                let binding = self.read(|d| d.locals.get(&f.chat).and_then(|l| l.org.clone()));
                let from_task = snapshot_tasks.iter().find(|t| t.chat.as_ref().and_then(chat_key_of_ref).as_ref() == Some(&f.chat));
                let (task_id, role_id) = match (&binding, from_task) {
                    (Some(b), _) => (b.task_id.clone(), b.role_id.clone()),
                    (None, Some(t)) => (t.id.0.clone(), t.role.clone()),
                    (None, None) => (String::new(), String::new()),
                };
                let name = self.read(|d| d.chat(&f.chat).and_then(|c| c.name.value().cloned()));
                OrgDeleteChatRow { chat: f.chat.clone(), task_id, role_id, name, blocked: chat_block_reason(f) }
            })
            .collect();
        let worktrees = self.org_kept_worktrees(&facts.workflow_id).await;
        let d = facts.dir.clone();
        let record_bytes = match tokio::task::spawn_blocking(move || dir_bytes(&d)).await {
            Ok(Some(n)) => Known::direct(n),
            _ => Known::NotFetched,
        };
        // 開始元の依頼整理の会話（紐づけを確認できたときだけ。できなければ対象にしない）。
        let intake = match self.org_intake_link(&facts.dir, &facts.workflow_id).await {
            Some(chat) => {
                let rec = self.read(|d| d.locals.get(&chat).and_then(|l| l.conversation.as_ref()).and_then(|c| c.intake.clone()));
                let known = self.read(|d| d.chat(&chat).is_some() || d.locals.contains_key(&chat));
                if !known {
                    Some(OrgDeleteIntakeRow { chat, name: None, state: "削除済み".to_string(), gone: true, blocked: None })
                } else {
                    let fact = self.intake_delete_fact(&chat, false, Some(&facts.workflow_id)).await;
                    let name = self.read(|d| d.chat(&chat).and_then(|c| c.name.value().cloned()));
                    Some(OrgDeleteIntakeRow { chat, name, state: rec.as_ref().map_or("状態不明", |r| intake_state_text(r)).to_string(), gone: false, blocked: intake_delete_block(&fact) })
                }
            }
            None => None,
        };
        let intake_unlinked = intake.is_none();
        Ok(OrgDeletePreview {
            intake,
            intake_unlinked,
            workflow_id: facts.workflow_id.clone(),
            title: facts.snapshot.as_ref().and_then(|s| s.title.clone()),
            verdict,
            chats,
            kept_released: facts.kept_released.clone(),
            worktrees,
            record_bytes,
            deletion: facts.record.as_ref().map(|r| OrgDeletionView { requested_at: r.requested_at, done: r.done.clone(), failed: r.failed.clone() }),
        })
    }

    /// このワークフローが作った所有worktree（残る）。状態は読取りのgitだけで調べ、読めなければ「未取得」。
    async fn org_kept_worktrees(self: &Arc<Self>, workflow_id: &str) -> Vec<OrgKeptWorktreeRow> {
        let recs: Vec<crate::backend::model::WorktreeRecord> =
            self.read(|d| d.worktrees.iter().filter(|r| r.org.as_ref().is_some_and(|o| o.workflow_id == workflow_id)).cloned().collect());
        let git = self.git_for_worktree();
        let mut rows = Vec::new();
        for rec in recs {
            let Some(owner) = rec.org.clone() else { continue };
            let path = Path::new(&rec.path);
            let exists = path.is_dir();
            let dirty = if exists {
                match (gw::dirty_count(&git, path).await, gw::ignored_count(&git, path).await) {
                    (Ok(a), Ok(b)) => Known::direct(a + b),
                    _ => Known::NotFetched,
                }
            } else {
                Known::NotFetched
            };
            // ブランチのない読取り用の detached worktree には、取り込むコミットがない。
            let unmerged_commits = if rec.branch.is_empty() {
                Known::Value { value: 0, basis: Basis::Derived }
            } else {
                match gw::unmerged_count(&git, Path::new(&rec.repo_root), &rec.branch).await {
                    Ok(n) => Known::direct(n),
                    Err(_) => Known::NotFetched,
                }
            };
            rows.push(OrgKeptWorktreeRow { path: rec.path.clone(), branch: rec.branch.clone(), access: owner.access, exists, dirty, unmerged_commits });
        }
        rows
    }

    // ───────────────────────── 削除 ─────────────────────────

    /// 削除を実行する（利用者が確認画面で選んだ後だけ）。結果は `Deleted` か `Partial`（続きは同じ操作で）。
    pub async fn org_delete_workflow(self: &Arc<Self>, args: OrgDeleteArgs, confirmed: &UserConfirmed) -> Result<OrgDeleteOutcome, IpcError> {
        let wf = args.workflow_id.clone();
        // 実行中の印は守り（Drop）で外す。panic・早期 return・future の破棄でも残らない。
        let Some(_deleting) = SetGuard::try_acquire(&self.org_rt.deleting, wf.clone()) else {
            return Err(err(IpcErrorCode::Rejected, "このワークフローの削除を実行中です"));
        };
        let res = self.org_delete_inner(args, confirmed).await;
        match &res {
            Ok(OrgDeleteOutcome::Deleted { intake }) => crate::diag::log("org_delete", &format!("deleted wf={wf} intake={:?}", intake.as_ref().map(|i| i.deleted))),
            Ok(OrgDeleteOutcome::Partial { done, failed, .. }) => crate::diag::log("org_delete", &format!("partial wf={wf} done={} failed={}", done.len(), failed.join(" / "))),
            Err(e) => crate::diag::log("org_delete", &format!("refused/failed wf={wf} code={:?}: {}", e.code, e.message.replace('\n', " / "))),
        }
        res
    }

    /// 削除の進行を画面へ知らせる（表示だけ）。
    fn org_delete_progress(&self, workflow_id: &str, rec: &DeleteRecord, step: &str) {
        let (done, total) = delete_progress_counts(rec);
        let ev = HostEvent::OrgDeleteProgress { workflow_id: workflow_id.to_string(), done, total, step: step.to_string() };
        self.mutate(|_| ((), vec![ev]));
    }

    async fn org_delete_inner(self: &Arc<Self>, args: OrgDeleteArgs, confirmed: &UserConfirmed) -> Result<OrgDeleteOutcome, IpcError> {
        let facts = self.org_manage_facts(&args.workflow_id).await?;
        // 途中の削除は、始めたときの選択（会話も消すか）で続ける。
        let delete_chats = facts.record.as_ref().map_or(args.delete_chats, |r| r.delete_chats);
        let (v, _) = self.org_verdict(ManageKind::Delete, &facts, delete_chats).await;
        if let OrgManageVerdict::Blocked { reasons } = v {
            return Err(Self::blocked_error(&reasons));
        }
        let same = |a: &[ChatKey], b: &[ChatKey]| a.len() == b.len() && a.iter().all(|c| b.contains(c)) && b.iter().all(|c| a.contains(c));
        if !same(&facts.targets, &args.expect_chats) {
            return Err(err(IpcErrorCode::Rejected, "確認した後に、削除の対象のチャットが変わりました。もう一度、削除の確認から行ってください"));
        }
        // 一緒に消す依頼整理の会話は、確認時の紐づけと同じときだけ（違えば何も消さずに確認し直し）。
        let intake_target = match &args.intake_chat {
            Some(k) => {
                if self.org_intake_link(&facts.dir, &facts.workflow_id).await.as_ref() != Some(k) {
                    return Err(err(IpcErrorCode::Rejected, "確認した後に、依頼整理の会話の紐づけが変わりました。もう一度、削除の確認から行ってください"));
                }
                Some(k.clone())
            }
            None => None,
        };
        let mut rec = match facts.record.clone() {
            Some(r) => r,
            None => {
                let rec = DeleteRecord { requested_at: now_ms(), delete_chats, chats: facts.targets.clone(), done: Vec::new(), failed: Vec::new() };
                // 書けなければ何もしない。
                let (d, r) = (facts.dir.clone(), rec.clone());
                tokio::task::spawn_blocking(move || write_delete_record(&d, &r)).await.map_err(join_err)?.map_err(|m| err(IpcErrorCode::Io, format!("削除の記録を書けないため、何も削除していません（{m}）")))?;
                rec
            }
        };
        rec.failed.clear();
        let workflow_id = facts.workflow_id.clone();
        self.org_delete_progress(&workflow_id, &rec, "開始");
        // 失敗した段はこの実行では繰り返さない（続きは同じ操作でもう一度）。チャットが全部済んだら、次の周でフォルダの段が出る。
        let mut attempted: Vec<String> = Vec::new();
        loop {
            let steps: Vec<Step> = next_steps(&rec).into_iter().filter(|s| !attempted.contains(&step_label(s))).collect();
            if steps.is_empty() {
                break;
            }
            for step in steps {
                let label = step_label(&step);
                attempted.push(label.clone());
                let result: Result<(), String> = match &step {
                    Step::Chat(chat) => self.org_delete_one_chat(chat, &workflow_id, delete_chats, confirmed).await,
                    Step::Files => {
                        let d = facts.dir.clone();
                        match tokio::task::spawn_blocking(move || remove_workflow_files(&d)).await {
                            Ok(r) => r,
                            Err(e) => Err(format!("削除処理が異常終了しました: {e}")),
                        }
                    }
                };
                let ok = result.is_ok();
                match result {
                    Ok(()) => rec.done.push(label.clone()),
                    Err(m) => rec.failed.push(format!("{label}: {m}")),
                }
                self.org_delete_progress(&workflow_id, &rec, &label);
                if ok && matches!(step, Step::Files) {
                    continue; // フォルダごと消えたので、記録は書き直さない
                }
                let (d, r) = (facts.dir.clone(), rec.clone());
                if let Ok(Err(m)) = tokio::task::spawn_blocking(move || write_delete_record(&d, &r)).await {
                    self.warn(format!("削除の進みを記録できませんでした（{m}）"));
                }
            }
        }
        // 依頼整理の会話は別の結果として扱う。消せなくても、ワークフローの削除と再開（「削除を続ける」）は妨げない。
        let intake = match &intake_target {
            Some(k) => Some(self.org_delete_intake_chat(k, &workflow_id, confirmed).await),
            None => None,
        };
        if rec.failed.is_empty() && rec.done.iter().any(|d| d == FILES_LABEL) {
            self.org_forget_workflow_runtime(&workflow_id, &facts.targets, &facts.kept_released);
            return Ok(OrgDeleteOutcome::Deleted { intake });
        }
        // 完了と偽らない。失敗が出た段で止まり、続きは同じ操作で（済んだ段は飛ばす）。
        if rec.failed.is_empty() {
            rec.failed.push("削除を完了できませんでした".to_string());
        }
        Ok(OrgDeleteOutcome::Partial { done: rec.done, failed: rec.failed, intake })
    }

    /// 依頼整理の会話1件の削除（既存の通常チャット削除。停止を確認できない・実行中・保存失敗・引き継ぎ済みは削除せず理由を返す）。
    async fn org_delete_intake_chat(self: &Arc<Self>, chat: &ChatKey, workflow_id: &str, confirmed: &UserConfirmed) -> OrgIntakeDeleteResult {
        let known = self.read(|d| d.chat(chat).is_some() || d.locals.contains_key(chat));
        if !known {
            return OrgIntakeDeleteResult { deleted: true, reason: None }; // すでに削除済み
        }
        let fact = self.intake_delete_fact(chat, false, Some(workflow_id)).await;
        if let Some(reason) = intake_delete_block(&fact) {
            return OrgIntakeDeleteResult { deleted: false, reason: Some(reason) };
        }
        match self.delete_chat_core(chat.clone(), confirmed, DeleteMode::User).await {
            Ok(DeleteOutcome::Deleted) => OrgIntakeDeleteResult { deleted: true, reason: None },
            Ok(DeleteOutcome::Partial { failed, .. }) => OrgIntakeDeleteResult { deleted: false, reason: Some(failed.join("、")) },
            Ok(DeleteOutcome::Pending { .. }) => OrgIntakeDeleteResult { deleted: false, reason: Some("停止を確認できていません".to_string()) },
            Err(e) => OrgIntakeDeleteResult { deleted: false, reason: Some(e.message) },
        }
    }

    /// 左一覧の「依頼整理の会話をまとめて削除」の1件分。開始済み・やめたものだけ。ピン留め・紐づくワークフローが未終了・実行中・停止未確認・保存失敗・引き継ぎ済みは削除しない（中断もしない）。
    /// 実行は既存の通常チャット削除（`DeleteMode::User`）。判定はここが最終。
    pub async fn intake_delete_chat(self: &Arc<Self>, args: crate::host::ChatArgs, confirmed: &UserConfirmed) -> Result<DeleteOutcome, IpcError> {
        let chat = args.chat;
        if !self.read(|d| d.locals.get(&chat).and_then(|l| l.conversation.as_ref()).is_some_and(|c| c.intake.is_some())) {
            return Err(err(IpcErrorCode::NotFound, "依頼整理の会話ではありません"));
        }
        let fact = self.intake_delete_fact(&chat, true, None).await;
        if let Some(reason) = intake_delete_block(&fact) {
            return Err(blocked(BlockedReason::ChatBusy, reason));
        }
        self.delete_chat_core(chat, confirmed, DeleteMode::User).await
    }

    /// 役割チャット1件の削除（または「残す」なら手動への切り替え）。成功＝その段は済み。
    async fn org_delete_one_chat(self: &Arc<Self>, chat: &ChatKey, workflow_id: &str, delete_chats: bool, confirmed: &UserConfirmed) -> Result<(), String> {
        let known = self.read(|d| d.chat(chat).is_some() || d.locals.contains_key(chat));
        if !known {
            // すでにアプリの記録にない（前回の削除で消えた等）。消す対象がない。
            return Ok(());
        }
        if delete_chats {
            match self.delete_chat_core(chat.clone(), confirmed, DeleteMode::OrgWorkflow { workflow_id: workflow_id.to_string() }).await {
                Ok(DeleteOutcome::Deleted) => Ok(()),
                Ok(DeleteOutcome::Partial { failed, .. }) => Err(failed.join("、")),
                Ok(DeleteOutcome::Pending { .. }) => Err("停止を確認できていません".to_string()),
                Err(e) => Err(e.message),
            }
        } else {
            let has_binding = self.read(|d| d.locals.get(chat).is_some_and(|l| l.org.is_some()));
            if !has_binding {
                return Ok(());
            }
            self.org_release_chat(chat, confirmed).await.map_err(|e| e.message)
        }
    }

    /// 削除が済んだワークフローの作業状態を片付ける（写し・監視の対応・解放表）。
    fn org_forget_workflow_runtime(&self, workflow_id: &str, targets: &[ChatKey], kept: &[ChatKey]) {
        {
            let mut view = self.org_rt.view.lock().unwrap();
            if view.as_ref().is_some_and(|v| v.workflow.0 == workflow_id) {
                *view = None;
            }
        }
        let mut obs = self.org_rt.observer.lock().unwrap();
        for c in targets.iter().chain(kept) {
            obs.unbind(c);
            self.org_rt.grants.forget_chat(c);
        }
    }

    // ───────────────────────── アーカイブ（アプリ内だけ） ─────────────────────────

    /// アーカイブする（確認なし）。終わったワークフローだけ。Codex の `thread/archive` は役割チャットへ送らない。
    pub async fn org_archive_workflow(self: &Arc<Self>, args: OrgWorkflowArgs) -> Result<(), IpcError> {
        let facts = self.org_manage_facts(&args.workflow_id).await?;
        if facts.record.is_some() {
            return Err(err(IpcErrorCode::Rejected, "削除の途中のため、アーカイブできません"));
        }
        let (v, _) = self.org_verdict(ManageKind::Archive, &facts, false).await;
        if let OrgManageVerdict::Blocked { reasons } = v {
            return Err(Self::blocked_error(&reasons));
        }
        self.precheck_space()?;
        let (d, at) = (facts.dir, now_ms());
        tokio::task::spawn_blocking(move || {
            // すでにアーカイブ済みなら時刻を変えない。
            if read_archived_at(&d).is_some() {
                return Ok(());
            }
            write_archived(&d, at)
        })
        .await
        .map_err(join_err)?
        .map_err(|m| io_err("アーカイブの印を書けませんでした", m))
    }

    /// アーカイブを解除する（確認なし。印を消すだけ）。
    pub async fn org_unarchive_workflow(self: &Arc<Self>, args: OrgWorkflowArgs) -> Result<(), IpcError> {
        let root = self.org_store_root()?;
        let dir = workflow_dir(&root, &args.workflow_id);
        if dir.file_name().is_none_or(|n| n.to_string_lossy() != args.workflow_id) || !dir.is_dir() {
            return Err(err(IpcErrorCode::NotFound, "ワークフローが見つかりません"));
        }
        tokio::task::spawn_blocking(move || remove_archived(&dir)).await.map_err(join_err)?.map_err(|m| io_err("アーカイブの印を消せませんでした", m))
    }
}
