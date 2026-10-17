//! 組織実行（P-ORG-5）のGit操作。所有記録（`WorktreeRecord.org`）のあるworktreeだけに書く。
//!
//! - `commit_all_in_owned_worktree`: 所有記録・ブランチ（`agentdock/org/`）・Gitの一覧・実在パスをすべて確認してからコミットする。
//!   利用者のブランチ・作業ツリー・記録のないパスは拒否する（コミットしない）。push・merge・reset・force・チェックアウトの操作は持たない。
//! - `add_detached_worktree`: 対象コミットを固定したdetached worktree（Reviewer等の読取り用）。ブランチを作らず、利用者のブランチを使わない。
//! - `diff_commits`: コミット間差分（統計と変更ファイル一覧。読取りのみ）。
//!
//! 判定は純粋ロジック（`rules::org_worktree`）。ここはGitの実行と実在確認だけ。統合（マージ）は手動で、この層には作らない。

use std::path::Path;

use super::worktree::{self as gw, ok, WtError};
use super::{Git, GitOp};
use crate::backend::model::WorktreeRecord;
use crate::rules::baseline::norm_path;
use crate::rules::org_worktree::{authorize_commit, parse_name_status, ChangedFile, Refusal};
use crate::rules::worktree::is_under;

/// 組織実行のGit操作の失敗。拒否（操作していない）とGitの失敗を区別する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgGitError {
    /// 安全条件に当たり、何もしていない。
    Refused(Refusal),
    Git(WtError),
}

impl OrgGitError {
    pub fn message(&self) -> String {
        match self {
            OrgGitError::Refused(r) => r.message().to_string(),
            OrgGitError::Git(e) => e.message(),
        }
    }
    /// 時間内に終わらず、結果が分からない。
    pub fn is_timeout(&self) -> bool {
        matches!(self, OrgGitError::Git(e) if e.is_timeout())
    }
}

impl From<WtError> for OrgGitError {
    fn from(e: WtError) -> Self {
        OrgGitError::Git(e)
    }
}

impl From<Refusal> for OrgGitError {
    fn from(r: Refusal) -> Self {
        OrgGitError::Refused(r)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitResult {
    Committed { commit: String },
    /// 変更がなかった（成功にも失敗にもしない）。
    NothingToCommit,
}

/// 実在するパスの解決後の形（リンクを解決し、Windowsの `\\?\` を外す）。実在しなければ None。
pub fn resolved(p: &Path) -> Option<String> {
    let c = std::fs::canonicalize(p).ok()?.to_string_lossy().into_owned();
    Some(c.strip_prefix("\\\\?\\").map(str::to_string).unwrap_or(c))
}

/// 所有worktreeの中で、変更をすべてコミットする。確認はすべて通った場合だけ実行する。
pub async fn commit_all_in_owned_worktree(git: &Git, records: &[WorktreeRecord], root: &str, path: &str, workflow: &str, task: &str, message: &str) -> Result<CommitResult, OrgGitError> {
    // 1. 記録（所有・書込み用・作成済み・置き場所配下・組織のブランチ）。
    let rec = authorize_commit(records, root, path, workflow, task)?;
    // 2. 実在と、リンク越えでないこと。
    let (Some(real), Some(real_root)) = (resolved(Path::new(&rec.path)), resolved(Path::new(root))) else {
        return Err(Refusal::PathMissing.into());
    };
    if !is_under(&real_root, &real) {
        return Err(Refusal::OutsideRoot.into());
    }
    // 3. Gitの一覧: メインの作業ツリーではなく、記録のブランチを今も指している。
    let listed = gw::list(git, Path::new(&rec.repo_root)).await?;
    let idx = listed.iter().position(|w| norm_path(&w.path) == norm_path(&rec.path)).ok_or(Refusal::NotListed)?;
    if idx == 0 {
        return Err(Refusal::MainWorktree.into());
    }
    if listed[idx].branch.as_deref() != Some(rec.branch.as_str()) {
        return Err(Refusal::BranchMismatch.into());
    }
    let dir = Path::new(&rec.path);
    if gw::dirty_count(git, dir).await? == 0 {
        return Ok(CommitResult::NothingToCommit);
    }
    ok(git.run(dir, &GitOp::StageAll).await.map_err(WtError::from)?)?;
    ok(git.run(dir, &GitOp::CommitStaged { message: message.to_string() }).await.map_err(WtError::from)?)?;
    let head = ok(git.run(dir, &GitOp::Head).await.map_err(WtError::from)?)?;
    let commit = head.stdout_text().trim().to_string();
    if commit.is_empty() {
        return Err(WtError::Exit { message: "コミットのIDを読めませんでした".into() }.into());
    }
    Ok(CommitResult::Committed { commit })
}

/// 対象コミットを固定したdetached worktreeを `path`（置き場所 `root` の配下）に追加する。解決したコミットのIDを返す。
/// 既存のパスには作らない。ブランチは作らず、利用者のブランチもチェックアウトしない。
pub async fn add_detached_worktree(git: &Git, repo: &Path, root: &str, path: &str, commit: &str) -> Result<String, OrgGitError> {
    if !is_under(root, path) {
        return Err(Refusal::OutsideRoot.into());
    }
    let target = Path::new(path);
    if target.exists() {
        return Err(WtError::Exit { message: format!("作成先がすでに存在します: {path}") }.into());
    }
    let parent_real = target.parent().and_then(resolved);
    let root_real = resolved(Path::new(root));
    match (parent_real, root_real) {
        (Some(p), Some(r)) if norm_path(&p) == norm_path(&r) || is_under(&r, &p) => {}
        (None, _) | (_, None) => return Err(Refusal::PathMissing.into()),
        _ => return Err(Refusal::OutsideRoot.into()),
    }
    let sha = gw::resolve_commit(git, repo, commit).await?;
    ok(git.run(repo, &GitOp::WorktreeAddDetached { path: path.to_string(), commit: sha.clone() }).await.map_err(WtError::from)?)?;
    Ok(sha)
}

/// 2つのコミット間の差分（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiff {
    pub stat: String,
    pub files: Vec<ChangedFile>,
}

pub async fn diff_commits(git: &Git, repo: &Path, from: &str, to: &str) -> Result<CommitDiff, WtError> {
    let from = gw::resolve_commit(git, repo, from).await?;
    let to = gw::resolve_commit(git, repo, to).await?;
    let stat = ok(git.run(repo, &GitOp::DiffCommitsStat { from: from.clone(), to: to.clone() }).await?)?.stdout_text();
    let names = ok(git.run(repo, &GitOp::DiffCommitsNames { from, to }).await?)?.stdout_text();
    Ok(CommitDiff { stat, files: parse_name_status(&names) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{LocalId, OrgWorktreeAccess, OrgWorktreeOwner, UnixMillis, WorktreeState};
    use std::process::Command;

    fn git_ok() -> bool {
        Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    struct Rig {
        base: std::path::PathBuf,
        repo: std::path::PathBuf,
        root: String,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            // worktreeの管理ファイルがあるので強制で消す。
            std::fs::remove_dir_all(&self.base).ok();
        }
    }

    fn rig() -> Rig {
        let t = std::env::temp_dir().join(format!("agentdock-orgwt-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&t).unwrap();
        let base = std::path::PathBuf::from(resolved(&t).unwrap());
        let repo = base.join("repo");
        let root = base.join("worktrees");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        sh(&repo, &["config", "user.email", "t@example.com"]);
        sh(&repo, &["config", "user.name", "t"]);
        sh(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        Rig { base, repo, root: root.to_string_lossy().into_owned() }
    }

    fn record(r: &Rig, name: &str, branch: &str, org: Option<OrgWorktreeAccess>) -> WorktreeRecord {
        WorktreeRecord {
            id: LocalId(format!("wt-{name}")),
            repo_root: r.repo.to_string_lossy().into_owned(),
            path: Path::new(&r.root).join(name).to_string_lossy().into_owned(),
            branch: branch.into(),
            base_commit: "x".into(),
            created_at: UnixMillis(0),
            created_for_chat: None,
            state: WorktreeState::Ready,
            org: org.map(|access| OrgWorktreeOwner { workflow_id: "w1".into(), task_id: "t1".into(), access }),
        }
    }

    fn refused(r: Result<CommitResult, OrgGitError>) -> Refusal {
        match r {
            Err(OrgGitError::Refused(x)) => x,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn commit_works_in_the_owned_worktree_and_leaves_the_users_branch_alone() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        let rec = record(&r, "w-ok", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &rec.path, &rec.branch, "HEAD").await.unwrap();
        let main_head = sh(&r.repo, &["rev-parse", "main"]);
        let records = vec![rec.clone()];

        // 変更なし: 成功にも失敗にもしない。
        assert_eq!(commit_all_in_owned_worktree(&git, &records, &r.root, &rec.path, "w1", "t1", "m").await, Ok(CommitResult::NothingToCommit));

        std::fs::write(Path::new(&rec.path).join("a.txt"), "two\n").unwrap();
        std::fs::write(Path::new(&rec.path).join("new.txt"), "n\n").unwrap();
        let out = commit_all_in_owned_worktree(&git, &records, &r.root, &rec.path, "w1", "t1", "agentdock: t1 work").await.unwrap();
        let CommitResult::Committed { commit } = out else { panic!("expected a commit") };
        assert_eq!(sh(Path::new(&rec.path), &["rev-parse", "HEAD"]), commit);
        assert_eq!(sh(Path::new(&rec.path), &["log", "-1", "--format=%an|%s"]), "AgentDock|agentdock: t1 work");
        assert_eq!(sh(Path::new(&rec.path), &["status", "--porcelain"]), "");
        // 利用者のブランチと作業ツリーは変わらない。
        assert_eq!(sh(&r.repo, &["rev-parse", "main"]), main_head);
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\n");
        assert_eq!(sh(&r.repo, &["status", "--porcelain"]), "");

        // コミット間差分。
        let d = diff_commits(&git, &r.repo, &main_head, &commit).await.unwrap();
        assert!(d.stat.contains("a.txt") && d.stat.contains("new.txt"));
        assert_eq!(d.files.iter().map(|f| (f.status.as_str(), f.path.as_str())).collect::<Vec<_>>(), [("M", "a.txt"), ("A", "new.txt")]);
        assert!(diff_commits(&git, &r.repo, "--all", &commit).await.is_err());
    }

    #[tokio::test]
    async fn commit_is_refused_for_unowned_paths_users_branches_and_the_users_working_tree() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        // 利用者が作ったworktree（所有記録なし。ブランチ名は組織風でも拒否）。
        let user = record(&r, "w-user", "agentdock/org/w1/other", None);
        gw::add(&git, &r.repo, &user.path, &user.branch, "HEAD").await.unwrap();
        std::fs::write(Path::new(&user.path).join("a.txt"), "user edit\n").unwrap();
        // 所有記録はあるが、ブランチが利用者のもの。
        let wrong = record(&r, "w-wrong", "feature/x", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &wrong.path, &wrong.branch, "HEAD").await.unwrap();
        std::fs::write(Path::new(&wrong.path).join("a.txt"), "edit\n").unwrap();
        // 所有記録とブランチ名は正しいが、Git上は別のブランチへ切り替えられている。
        let switched = record(&r, "w-switched", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &switched.path, "agentdock/org/w1/t1", "HEAD").await.unwrap();
        sh(Path::new(&switched.path), &["switch", "-q", "-c", "elsewhere"]);
        std::fs::write(Path::new(&switched.path).join("a.txt"), "edit\n").unwrap();
        // 利用者の作業ツリー（リポジトリ本体）に所有記録を付けても拒否。
        std::fs::write(r.repo.join("a.txt"), "user local change\n").unwrap();
        let mut main = record(&r, "w-main", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        main.path = r.repo.to_string_lossy().into_owned();

        let records = vec![user.clone(), wrong.clone(), switched.clone(), main.clone()];
        let tip = |p: &Path| sh(p, &["rev-parse", "HEAD"]);
        let before = (tip(&r.repo), tip(Path::new(&user.path)), tip(Path::new(&wrong.path)), tip(Path::new(&switched.path)));

        // 記録のないパス。
        let stray = Path::new(&r.root).join("stray");
        std::fs::create_dir_all(&stray).unwrap();
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &stray.to_string_lossy(), "w1", "t1", "m").await), Refusal::NoRecord);
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &user.path, "w1", "t1", "m").await), Refusal::NotOrgOwned);
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &wrong.path, "w1", "t1", "m").await), Refusal::BranchNotOrg);
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &switched.path, "w1", "t1", "m").await), Refusal::BranchMismatch);
        // 本体の作業ツリーは、所有記録を偽っても書かない。
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &r.repo.to_string_lossy(), "w1", "t1", "m").await), Refusal::MainWorktree);
        // 別タスク。
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &[switched.clone()], &r.root, &switched.path, "w1", "t2", "m").await), Refusal::OtherOwner);

        // どのブランチにもコミットは増えず、変更もステージされていない。
        assert_eq!(before, (tip(&r.repo), tip(Path::new(&user.path)), tip(Path::new(&wrong.path)), tip(Path::new(&switched.path))));
        assert!(sh(&r.repo, &["status", "--porcelain"]).contains("a.txt"));
        assert!(sh(Path::new(&user.path), &["status", "--porcelain"]).contains("a.txt"));
        assert!(sh(Path::new(&wrong.path), &["status", "--porcelain"]).contains("a.txt"));
    }

    #[tokio::test]
    async fn detached_worktree_is_pinned_to_the_commit_and_cannot_be_committed_to() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        let first = sh(&r.repo, &["rev-parse", "HEAD"]);
        std::fs::write(r.repo.join("a.txt"), "two\n").unwrap();
        sh(&r.repo, &["commit", "-q", "-am", "second"]);
        let branches_before = sh(&r.repo, &["for-each-ref", "--format=%(refname)"]);

        let path = Path::new(&r.root).join("w-detached").to_string_lossy().into_owned();
        let sha = add_detached_worktree(&git, &r.repo, &r.root, &path, &first).await.unwrap();
        assert_eq!(sha, first);
        assert_eq!(sh(Path::new(&path), &["rev-parse", "HEAD"]), first);
        assert_eq!(std::fs::read_to_string(Path::new(&path).join("a.txt")).unwrap(), "one\n");
        assert_eq!(sh(Path::new(&path), &["branch", "--show-current"]), "");
        assert_eq!(sh(&r.repo, &["for-each-ref", "--format=%(refname)"]), branches_before);

        // 既存のパス・置き場所の外・解決できない参照・オプション風の値は拒否。
        assert!(add_detached_worktree(&git, &r.repo, &r.root, &path, &first).await.is_err());
        let outside = r.base.join("outside").to_string_lossy().into_owned();
        assert_eq!(add_detached_worktree(&git, &r.repo, &r.root, &outside, &first).await, Err(OrgGitError::Refused(Refusal::OutsideRoot)));
        let p2 = Path::new(&r.root).join("w-d2").to_string_lossy().into_owned();
        assert!(add_detached_worktree(&git, &r.repo, &r.root, &p2, "--all").await.is_err());
        assert!(add_detached_worktree(&git, &r.repo, &r.root, &p2, "no-such-ref").await.is_err());
        assert!(!Path::new(&p2).exists());

        // 読取り用の記録にはコミットしない。
        let mut rec = record(&r, "w-detached", "", Some(OrgWorktreeAccess::DetachedReadOnly));
        rec.path = path.clone();
        std::fs::write(Path::new(&path).join("a.txt"), "edit\n").unwrap();
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &[rec], &r.root, &path, "w1", "t1", "m").await), Refusal::ReadOnly);
        assert_eq!(sh(Path::new(&path), &["rev-parse", "HEAD"]), first);
    }
}
