//! 組織実行（P-ORG-5）の `Workspace` ポートのホスト実装。worktree台帳（`worktrees.json`）と作成・コミットの安全条件を共用する。
//!
//! - 作成は組織実行のタスク用だけ（Worker＝`agentdock/org/<workflow>/<task>` ブランチの専用worktree、Reviewer＝対象コミット固定のdetached）。
//!   通常チャットのworktree（指定しないチャットは通常の作業場所）の仕様は変えない。記録（`org` つき）を保存してからgitを実行する。
//! - コミットは所有記録のあるworktreeの中だけ（`gitops::org`）。push・merge・reset・force・利用者のブランチへの操作は持たない。
//! - 失敗・拒否・結果不明を成功にしない。結果は `WorkspaceOutcome` で返す。統合（マージ）は利用者が手動で行う。

use super::*;
use crate::gitops::org::{self as go, CommitResult};
use crate::rules::org_worktree::org_branch_name;
use org_engine::{WorkflowId, WorkspaceAction, WorkspaceOutcome, WorktreeBase};

fn failed(detail: impl Into<String>) -> WorkspaceOutcome {
    WorkspaceOutcome::Failed { detail: detail.into() }
}

impl Host {
    /// `Workspace` ポートの入口。
    pub(in crate::host) async fn org_workspace(self: &Arc<Self>, workflow: &WorkflowId, action: WorkspaceAction) -> WorkspaceOutcome {
        match action {
            WorkspaceAction::CreateWorktree { task, repo, base } => self.org_create_worktree(workflow, &task, &repo, base).await,
            WorkspaceAction::Commit { task, cwd, message } => self.org_commit(workflow, &task, &cwd, &message).await,
        }
    }

    async fn org_create_worktree(self: &Arc<Self>, workflow: &WorkflowId, task: &org_engine::TaskId, repo: &str, base: WorktreeBase) -> WorkspaceOutcome {
        let access = match base {
            WorktreeBase::Head => OrgWorktreeAccess::Writable,
            WorktreeBase::DetachedAt(_) => OrgWorktreeAccess::DetachedReadOnly,
        };
        let owner = OrgWorktreeOwner { workflow_id: workflow.0.clone(), task_id: task.0.clone(), access };
        if let Err(e) = self.precheck_space() {
            return failed(e.message);
        }
        let folder = repo.trim().to_string();
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return failed(format!("リポジトリのフォルダが見つかりません: {folder}"));
        }
        let _op = self.worktree_rt.lock.lock().await;

        // 同じタスクの作成済みの記録があれば再利用する（再試行・再起動後に別のworktreeやブランチを増やさない）。
        let existing = self.read(|d| d.worktrees.iter().find(|r| r.org.as_ref() == Some(&owner) && matches!(r.state, WorktreeState::Ready) && Path::new(&r.path).is_dir()).cloned());
        if let Some(rec) = existing {
            return WorkspaceOutcome::WorktreeCreated { path: rec.path };
        }

        let git = self.git_for_worktree();
        let top = match gw::repo_root_kind(&git, Path::new(&folder)).await {
            Ok(gw::RepoRootKind::Root(r)) => r,
            Ok(gw::RepoRootKind::NotRepo) => return failed("このフォルダはGitのリポジトリではないため、worktreeを作れません"),
            Ok(gw::RepoRootKind::OwnerMismatch { repo }) => return failed(self.owner_mismatch_write_message(&repo, &folder, "worktreeの作成")),
            Err(e) => return failed(git_run_err(e).message),
        };
        let listed = match gw::list(&git, Path::new(&top)).await {
            Ok(l) => l,
            Err(e) => return failed(e.message()),
        };
        let main_path = listed.first().map(|w| w.path.clone()).unwrap_or_else(|| top.clone());
        let rev = match &base {
            WorktreeBase::Head => "HEAD".to_string(),
            WorktreeBase::DetachedAt(c) => c.clone(),
        };
        let base_commit = match gw::resolve_commit(&git, Path::new(&top), &rev).await {
            Ok(c) => c,
            Err(e) => return failed(e.message()),
        };

        let id = self.local_id("wt");
        let short = rw::short_id(seed_of(&id.0));
        let branch = match access {
            OrgWorktreeAccess::Writable => org_branch_name(&workflow.0, &task.0),
            OrgWorktreeAccess::DetachedReadOnly => String::new(),
        };
        let root = self.worktree_root();
        let root_str = root.to_string_lossy().into_owned();
        let path = root.join(rw::worktree_folder_name(&rw::repo_name_of(&main_path), &short)).to_string_lossy().into_owned();
        if !rw::is_under(&root_str, &path) {
            return failed("作成先が置き場所の外になるため、worktreeを作りません");
        }
        if Path::new(&path).exists() {
            return failed(format!("作成先がすでに存在します: {path}"));
        }
        if let Err(e) = std::fs::create_dir_all(&root) {
            return failed(format!("worktreeの置き場所を作成できません: {e}"));
        }
        if access == OrgWorktreeAccess::Writable {
            if let Err(e) = (GitOp::WorktreeAdd { path: path.clone(), branch: branch.clone(), base: base_commit.clone() }).args() {
                return failed(git_run_err(e).message);
            }
        }

        // 作る前に所有記録つきで保存する（保存できなければ実行しない）。
        let mut rec = WorktreeRecord {
            id: id.clone(),
            repo_root: main_path,
            path: path.clone(),
            branch: branch.clone(),
            base_commit: base_commit.clone(),
            created_at: now_ms(),
            created_for_chat: None,
            state: WorktreeState::Creating,
            org: Some(owner),
        };
        self.put_worktree(rec.clone());
        if let Err(message) = self.save_worktrees().await {
            self.drop_worktree(&id);
            return failed(format!("実行前の記録を保存できなかったため、worktreeは作っていません（{message}）"));
        }
        let made: Result<(), go::OrgGitError> = match access {
            OrgWorktreeAccess::Writable => gw::add(&git, Path::new(&top), &path, &branch, &base_commit).await.map_err(Into::into),
            OrgWorktreeAccess::DetachedReadOnly => go::add_detached_worktree(&git, Path::new(&top), &root_str, &path, &base_commit).await.map(|_| ()),
        };
        let outcome = match made {
            Ok(()) => {
                rec.state = WorktreeState::Ready;
                WorkspaceOutcome::WorktreeCreated { path }
            }
            Err(e) => {
                let message = if e.is_timeout() {
                    "Gitが時間内に終わらず、作成できたか分かりません。一覧で確認し、不要なら手動で削除してください".to_string()
                } else {
                    format!("{}（作られた分は自動では削除しません。必要なら手動で削除してください）", e.message())
                };
                rec.state = WorktreeState::Failed { message: message.clone() };
                failed(message)
            }
        };
        self.put_worktree(rec);
        if let Err(message) = self.save_worktrees().await {
            self.warn(format!("worktreeの記録を保存できませんでした（{message}）。再起動後に状態が戻る場合があります"));
        }
        outcome
    }

    async fn org_commit(self: &Arc<Self>, workflow: &WorkflowId, task: &org_engine::TaskId, cwd: &str, message: &str) -> WorkspaceOutcome {
        let _op = self.worktree_rt.lock.lock().await;
        let git = self.git_for_worktree();
        let records = self.read(|d| d.worktrees.clone());
        let root = self.worktree_root().to_string_lossy().into_owned();
        match go::commit_all_in_owned_worktree(&git, &records, &root, cwd, &workflow.0, &task.0, message).await {
            Ok(CommitResult::Committed { commit }) => WorkspaceOutcome::Committed { commit },
            Ok(CommitResult::NothingToCommit) => WorkspaceOutcome::NothingToCommit,
            Err(e) if e.is_timeout() => failed("Gitが時間内に終わらず、コミットできたか分かりません。worktreeの状態を確認してください"),
            Err(e) => failed(e.message()),
        }
    }
}
