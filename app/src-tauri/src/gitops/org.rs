//! 組織実行（P-ORG-5）のGit操作。所有記録（`WorktreeRecord.org`）のあるworktreeだけに書く。
//!
//! - `commit_all_in_owned_worktree`: 所有記録・ブランチ（`agentdock/org/`）・Gitの一覧・実在パスをすべて確認してからコミットする。
//!   利用者のブランチ・作業ツリー・記録のないパスは拒否する（コミットしない）。push・merge・reset・force・チェックアウトの操作は持たない。
//!   書込みは、作業フォルダの `.git` が記録のworktreeの管理フォルダを指していることを確かめたうえで、`--git-dir`・`--work-tree` を明示して行う
//!   （Workerのsandboxから `.git` ファイルを書き換えられても、別のgitdir・ブランチへは書かない）。フックは走らせない。
//! - `refresh_detached_worktree`: 再レビューのため、所有する読取り用worktreeを別のコミットへ動かす（確認は同じ。HEADを読み戻して確かめる）。
//! - `add_detached_worktree`: 対象コミットを固定したdetached worktree（Reviewer等の読取り用）。ブランチを作らず、利用者のブランチを使わない。
//! - `diff_commits`: コミット間差分（統計と変更ファイル一覧。読取りのみ）。
//!
//! 判定は純粋ロジック（`rules::org_worktree`）。ここはGitの実行と実在確認だけ。統合（マージ）は手動で、この層には作らない。

use std::path::Path;

use super::worktree::{self as gw, ok, WtError};
use super::{Git, GitOp};
use crate::backend::model::WorktreeRecord;
use crate::rules::baseline::norm_path;
use crate::rules::org_worktree::{admin_dir_matches, authorize_commit, authorize_refresh, parse_name_status, ChangedFile, Refusal};
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
    check_location(rec, root)?;
    // 3. Gitの一覧: メインの作業ツリーではなく、記録のブランチを今も指している。
    let listed = gw::list(git, Path::new(&rec.repo_root)).await?;
    let idx = listed.iter().position(|w| norm_path(&w.path) == norm_path(&rec.path)).ok_or(Refusal::NotListed)?;
    if idx == 0 {
        return Err(Refusal::MainWorktree.into());
    }
    if listed[idx].branch.as_deref() != Some(rec.branch.as_str()) {
        return Err(Refusal::BranchMismatch.into());
    }
    // 4. 作業フォルダの `.git` が、このworktreeの管理フォルダを指している（一覧は管理フォルダを読むので、`.git` の書換えはここで見つける）。
    let gitdir = verify_admin_dir(git, rec).await?;
    let dir = Path::new(&rec.path);
    if gw::dirty_count(git, dir).await? == 0 {
        return Ok(CommitResult::NothingToCommit);
    }
    // 5. 確認した管理フォルダを明示して書く（確認の後に `.git` を書き換えられても、そこへは書かない）。
    ok(git.run_pinned(dir, &gitdir, &GitOp::StageAll).await.map_err(WtError::from)?)?;
    ok(git.run_pinned(dir, &gitdir, &GitOp::CommitStaged { message: message.to_string() }).await.map_err(WtError::from)?)?;
    let head = ok(git.run_pinned(dir, &gitdir, &GitOp::Head).await.map_err(WtError::from)?)?;
    let commit = head.stdout_text().trim().to_string();
    if commit.is_empty() {
        return Err(WtError::Exit { message: "コミットのIDを読めませんでした".into() }.into());
    }
    Ok(CommitResult::Committed { commit })
}

/// 記録のパスが実在し、置き場所の配下にある（リンクを解決した後も）。
fn check_location(rec: &WorktreeRecord, root: &str) -> Result<(), OrgGitError> {
    let (Some(real), Some(real_root)) = (resolved(Path::new(&rec.path)), resolved(Path::new(root))) else {
        return Err(Refusal::PathMissing.into());
    };
    if !is_under(&real_root, &real) {
        return Err(Refusal::OutsideRoot.into());
    }
    Ok(())
}

/// 作業フォルダが実際に従うgitdirが、記録のworktreeの管理フォルダ（本体のgitdir直下の `worktrees/<名前>` で、その `gitdir` ファイルがこのworktreeを指す）か。
/// 本体のgitdirは、利用者のリポジトリ側から読む。違えば拒否する。確認したgitdirを返す。
async fn verify_admin_dir(git: &Git, rec: &WorktreeRecord) -> Result<String, OrgGitError> {
    let read = |out: crate::gitops::GitOutput| -> Result<String, WtError> { Ok(ok(out)?.stdout_text().trim().to_string()) };
    let gitdir = read(git.run(Path::new(&rec.path), &GitOp::AbsoluteGitDir).await.map_err(WtError::from)?)?;
    let common = read(git.run(Path::new(&rec.repo_root), &GitOp::AbsoluteGitDir).await.map_err(WtError::from)?)?;
    let link = std::fs::read_to_string(Path::new(&gitdir).join("gitdir")).unwrap_or_default();
    if gitdir.is_empty() || common.is_empty() || !admin_dir_matches(&gitdir, &common, &link, &rec.path) {
        return Err(Refusal::GitDirMismatch.into());
    }
    Ok(gitdir)
}

/// 再レビュー: 所有する読取り用（detached）worktreeを、対象コミットへ動かす。すでにそのコミットなら何もしない。
/// 記録・実在・一覧（本体でない・detached）・管理フォルダの確認は書込みと同じ。動かした後にHEADを読み戻し、対象と違えば失敗にする。解決したコミットのIDを返す。
pub async fn refresh_detached_worktree(git: &Git, records: &[WorktreeRecord], root: &str, path: &str, workflow: &str, task: &str, commit: &str) -> Result<String, OrgGitError> {
    let rec = authorize_refresh(records, root, path, workflow, task)?;
    check_location(rec, root)?;
    let listed = gw::list(git, Path::new(&rec.repo_root)).await?;
    let idx = listed.iter().position(|w| norm_path(&w.path) == norm_path(&rec.path)).ok_or(Refusal::NotListed)?;
    if idx == 0 {
        return Err(Refusal::MainWorktree.into());
    }
    if listed[idx].branch.is_some() {
        return Err(Refusal::BranchMismatch.into());
    }
    let gitdir = verify_admin_dir(git, rec).await?;
    let sha = gw::resolve_commit(git, Path::new(&rec.repo_root), commit).await?;
    let dir = Path::new(&rec.path);
    let head_of = |out: crate::gitops::GitOutput| -> Result<String, WtError> { Ok(ok(out)?.stdout_text().trim().to_string()) };
    if head_of(git.run_pinned(dir, &gitdir, &GitOp::Head).await.map_err(WtError::from)?)? != sha {
        ok(git.run_pinned(dir, &gitdir, &GitOp::CheckoutDetached { commit: sha.clone() }).await.map_err(WtError::from)?)?;
        if head_of(git.run_pinned(dir, &gitdir, &GitOp::Head).await.map_err(WtError::from)?)? != sha {
            return Err(WtError::Exit { message: "動かした後のHEADが対象コミットと違います".into() }.into());
        }
    }
    Ok(sha)
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

    /// 作業フォルダの `.git` ファイルを、別のgitdirへ向ける（Workerのsandboxから書ける想定の改ざん）。
    fn point_dot_git_at(worktree: &str, gitdir: &Path) {
        std::fs::write(Path::new(worktree).join(".git"), format!("gitdir: {}\n", gitdir.to_string_lossy().replace('\\', "/"))).unwrap();
    }

    #[tokio::test]
    async fn a_rewritten_dot_git_cannot_redirect_the_commit_to_the_users_branch_or_another_worktree() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        let rec = record(&r, "w-ok", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &rec.path, &rec.branch, "HEAD").await.unwrap();
        let other = record(&r, "w-other", "agentdock/org/w1/t2", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &other.path, &other.branch, "HEAD").await.unwrap();
        let records = vec![rec.clone(), other.clone()];
        let main_head = sh(&r.repo, &["rev-parse", "main"]);
        let other_head = sh(Path::new(&other.path), &["rev-parse", "HEAD"]);
        let count = |dir: &Path| sh(dir, &["rev-list", "--all", "--count"]);
        let commits_before = count(&r.repo);
        std::fs::write(Path::new(&rec.path).join("a.txt"), "worker edit\n").unwrap();

        // 1. `.git` を利用者のリポジトリ本体へ向ける。
        point_dot_git_at(&rec.path, &r.repo.join(".git"));
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &rec.path, "w1", "t1", "m").await), Refusal::GitDirMismatch);
        // 2. 別のworktree（所有は別タスク）の管理フォルダへ向ける。
        let admin_of_other = r.repo.join(".git").join("worktrees").join("w-other");
        assert!(admin_of_other.is_dir());
        point_dot_git_at(&rec.path, &admin_of_other);
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &rec.path, "w1", "t1", "m").await), Refusal::GitDirMismatch);
        // 3. 無関係なリポジトリへ向ける。
        let evil = r.base.join("evil");
        std::fs::create_dir_all(&evil).unwrap();
        sh(&evil, &["init", "-q", "-b", "main"]);
        point_dot_git_at(&rec.path, &evil.join(".git"));
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &records, &r.root, &rec.path, "w1", "t1", "m").await), Refusal::GitDirMismatch);

        assert_eq!(sh(&r.repo, &["rev-parse", "main"]), main_head, "the users branch did not move");
        assert_eq!(sh(Path::new(&other.path), &["rev-parse", "HEAD"]), other_head);
        assert_eq!(count(&r.repo), commits_before, "nothing was committed anywhere");
    }

    #[tokio::test]
    async fn commits_do_not_run_repository_hooks() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        let marker = r.base.join("hook-ran");
        let hook = r.repo.join(".git").join("hooks").join("post-commit");
        std::fs::write(&hook, format!("#!/bin/sh\necho ran > \"{}\"\n", marker.to_string_lossy().replace('\\', "/"))).unwrap();
        // 前提の確認: 普通のコミットではこの環境でもフックが走る（走らない環境では、下の確認は何も保証しない）。
        std::fs::write(r.repo.join("b.txt"), "b\n").unwrap();
        sh(&r.repo, &["add", "."]);
        sh(&r.repo, &["commit", "-q", "-m", "plain"]);
        let hooks_run_here = marker.exists();
        let _ = std::fs::remove_file(&marker);

        let rec = record(&r, "w-hook", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &rec.path, &rec.branch, "HEAD").await.unwrap();
        std::fs::write(Path::new(&rec.path).join("a.txt"), "two\n").unwrap();
        let path = rec.path.clone();
        let out = commit_all_in_owned_worktree(&git, &[rec], &r.root, &path, "w1", "t1", "agentdock: t1").await.unwrap();
        assert!(matches!(out, CommitResult::Committed { .. }));
        assert!(!marker.exists(), "no hook ran for the organisation commit (hooks run in this environment: {hooks_run_here})");
    }

    #[tokio::test]
    async fn a_reviewers_worktree_is_moved_to_the_new_commit_and_nothing_else_is_touched() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        let first = sh(&r.repo, &["rev-parse", "HEAD"]);
        std::fs::write(r.repo.join("a.txt"), "two\n").unwrap();
        sh(&r.repo, &["commit", "-q", "-am", "second"]);
        let second = sh(&r.repo, &["rev-parse", "HEAD"]);
        let path = Path::new(&r.root).join("w-rev").to_string_lossy().into_owned();
        add_detached_worktree(&git, &r.repo, &r.root, &path, &first).await.unwrap();
        let mut rec = record(&r, "w-rev", "", Some(OrgWorktreeAccess::DetachedReadOnly));
        rec.path = path.clone();
        let records = vec![rec.clone()];
        assert_eq!(std::fs::read_to_string(Path::new(&path).join("a.txt")).unwrap(), "one\n");

        // 古いコミットのまま: 新しいコミットへ動かす。同じコミットなら何もしない。
        assert_eq!(refresh_detached_worktree(&git, &records, &r.root, &path, "w1", "t1", &second).await.unwrap(), second);
        assert_eq!(sh(Path::new(&path), &["rev-parse", "HEAD"]), second);
        assert_eq!(std::fs::read_to_string(Path::new(&path).join("a.txt")).unwrap(), "two\n");
        assert_eq!(refresh_detached_worktree(&git, &records, &r.root, &path, "w1", "t1", &second).await.unwrap(), second);
        assert_eq!(sh(&r.repo, &["branch", "--show-current"]), "main");

        // 拒否: 別タスク・書込み用の記録・解決できない参照・`.git` の書換え。
        let e = |r: Result<String, OrgGitError>| match r {
            Err(OrgGitError::Refused(x)) => x,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(e(refresh_detached_worktree(&git, &records, &r.root, &path, "w1", "t2", &second).await), Refusal::OtherOwner);
        let mut writable = rec.clone();
        writable.org.as_mut().unwrap().access = OrgWorktreeAccess::Writable;
        assert_eq!(e(refresh_detached_worktree(&git, &[writable], &r.root, &path, "w1", "t1", &second).await), Refusal::NotDetachedRecord);
        assert!(refresh_detached_worktree(&git, &records, &r.root, &path, "w1", "t1", "no-such-ref").await.is_err());
        point_dot_git_at(&path, &r.repo.join(".git"));
        assert_eq!(e(refresh_detached_worktree(&git, &records, &r.root, &path, "w1", "t1", &first).await), Refusal::GitDirMismatch);
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), second, "the users checkout did not move");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_worktree_path_that_is_a_junction_to_outside_the_root_is_refused() {
        if !git_ok() {
            return;
        }
        let r = rig();
        let git = Git::new(None);
        // 置き場所の外に本物のworktreeを作り、置き場所の中にはそこへのジャンクションを置く。
        let outside = r.base.join("outside-wt");
        let real = record(&r, "unused", "agentdock/org/w1/t1", Some(OrgWorktreeAccess::Writable));
        gw::add(&git, &r.repo, &outside.to_string_lossy(), &real.branch, "HEAD").await.unwrap();
        std::fs::write(outside.join("a.txt"), "edit\n").unwrap();
        let link = Path::new(&r.root).join("w-junction");
        let made = Command::new("cmd").args(["/C", "mklink", "/J", &link.to_string_lossy(), &outside.to_string_lossy()]).output().unwrap();
        if !made.status.success() {
            return;
        }
        let mut rec = record(&r, "w-junction", &real.branch, Some(OrgWorktreeAccess::Writable));
        rec.path = link.to_string_lossy().into_owned();
        let before = sh(&outside, &["rev-parse", "HEAD"]);
        assert_eq!(refused(commit_all_in_owned_worktree(&git, &[rec], &r.root, &link.to_string_lossy(), "w1", "t1", "m").await), Refusal::OutsideRoot);
        assert_eq!(sh(&outside, &["rev-parse", "HEAD"]), before);
    }
}
