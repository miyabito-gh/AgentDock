//! worktree操作の呼出し（P3-7、DESIGN_P3 §2.3）。許可した `GitOp` だけを使い、結果を解析して返す。
//!
//! 削除は `--force` なしが基本で、`force` は呼出し側が2段目の確認の後にだけ指定する。ブランチの削除は `-d`（マージ済みのみ）で、`-D` は提供しない。
//! 出力の解析は純粋ロジック（`rules::worktree`）。ここはGitの実行と終了コードの扱いだけ。

use std::path::Path;

use super::{Git, GitError, GitOp, GitOutput};
use crate::rules::diff::{count_ignored_v2, parse_status_v2};
use crate::rules::worktree::{parse_worktree_list, ListedWorktree};

/// gitを実行できなかった（`Run`）か、実行したが非0で終わった（`Exit`。標準エラーの先頭行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WtError {
    Run(GitError),
    Exit { message: String },
}

impl WtError {
    pub fn message(&self) -> String {
        match self {
            WtError::Run(e) => e.to_string(),
            WtError::Exit { message } => message.clone(),
        }
    }
    /// Gitを起動できない（未導入・パス違い）。
    pub fn is_unavailable(&self) -> bool {
        matches!(self, WtError::Run(GitError::Unavailable(_)))
    }
    /// 時間内に終わらず、結果が分からない。
    pub fn is_timeout(&self) -> bool {
        matches!(self, WtError::Run(GitError::Timeout))
    }
}

impl From<GitError> for WtError {
    fn from(e: GitError) -> Self {
        WtError::Run(e)
    }
}

const MESSAGE_MAX_CHARS: usize = 300;

fn first_line(bytes: &[u8]) -> String {
    let t = String::from_utf8_lossy(bytes);
    let line = t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(MESSAGE_MAX_CHARS).collect()
}

fn ok(out: GitOutput) -> Result<GitOutput, WtError> {
    if out.success() {
        return Ok(out);
    }
    let detail = first_line(&out.stderr);
    let message = match (detail.is_empty(), out.exit_code) {
        (false, _) => detail,
        (true, Some(c)) => format!("gitが終了コード {c} で終わりました"),
        (true, None) => "gitが異常終了しました".to_string(),
    };
    Err(WtError::Exit { message })
}

/// `rev-parse --show-toplevel` の結果の分類。所有者違い（dubious ownership）は「リポジトリではない」と区別する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoRootKind {
    Root(String),
    NotRepo,
    /// Gitが所有者違いを理由に拒否した（リポジトリかどうかは確認できていない）。
    /// `repo` はGitが示したリポジトリの場所（示されていなければ空）。
    OwnerMismatch { repo: String },
}

/// 終了の成否・標準出力・標準エラーからの分類（純粋ロジック）。
pub fn classify_repo_root(success: bool, stdout: &str, stderr: &str) -> RepoRootKind {
    if !success {
        return match crate::rules::baseline::dubious_repo(stderr, "") {
            Some(repo) => RepoRootKind::OwnerMismatch { repo },
            None => RepoRootKind::NotRepo,
        };
    }
    let root = stdout.trim();
    if root.is_empty() { RepoRootKind::NotRepo } else { RepoRootKind::Root(root.to_string()) }
}

/// リポジトリのルートを分類して返す。
pub async fn repo_root_kind(git: &Git, folder: &Path) -> Result<RepoRootKind, GitError> {
    let out = git.run(folder, &GitOp::RepoRoot).await?;
    Ok(classify_repo_root(out.success(), &out.stdout_text(), &String::from_utf8_lossy(&out.stderr)))
}

/// リポジトリのルート。リポジトリでない・所有者違いで確認できないときは `Ok(None)`（区別が要るときは `repo_root_kind`）。
pub async fn repo_root(git: &Git, folder: &Path) -> Result<Option<String>, GitError> {
    Ok(match repo_root_kind(git, folder).await? {
        RepoRootKind::Root(r) => Some(r),
        RepoRootKind::NotRepo | RepoRootKind::OwnerMismatch { .. } => None,
    })
}

/// worktreeの一覧（先頭がメインの作業ツリー）。
pub async fn list(git: &Git, repo: &Path) -> Result<Vec<ListedWorktree>, WtError> {
    let out = ok(git.run(repo, &GitOp::WorktreeList).await?)?;
    Ok(parse_worktree_list(&out.stdout_text()))
}

/// 参照をコミットのIDにする。解決できなければ `Exit`。
pub async fn resolve_commit(git: &Git, repo: &Path, rev: &str) -> Result<String, WtError> {
    let out = git.run(repo, &GitOp::RevParse { rev: rev.to_string() }).await?;
    let sha = out.stdout_text().trim().to_string();
    if out.success() && !sha.is_empty() {
        return Ok(sha);
    }
    Err(WtError::Exit { message: format!("「{rev}」をコミットとして解決できません") })
}

/// 新しいブランチを作ってworktreeを追加する。
pub async fn add(git: &Git, repo: &Path, path: &str, branch: &str, base: &str) -> Result<(), WtError> {
    ok(git.run(repo, &GitOp::WorktreeAdd { path: path.into(), branch: branch.into(), base: base.into() }).await?).map(|_| ())
}

/// worktreeを削除する。`force` は未コミットの変更を捨てる（2段目の確認の後だけ）。
pub async fn remove(git: &Git, repo: &Path, path: &str, force: bool) -> Result<(), WtError> {
    ok(git.run(repo, &GitOp::WorktreeRemove { path: path.into(), force }).await?).map(|_| ())
}

/// マージ済みのブランチだけ削除する（`-d`）。マージされていなければ `Exit`（残る）。
pub async fn delete_branch_merged(git: &Git, repo: &Path, branch: &str) -> Result<(), WtError> {
    ok(git.run(repo, &GitOp::BranchDeleteMerged { branch: branch.into() }).await?).map(|_| ())
}

/// 作業ツリーの未コミット・未追跡の変更の件数（読取りのみ）。
pub async fn dirty_count(git: &Git, worktree: &Path) -> Result<u32, WtError> {
    let out = ok(git.run(worktree, &GitOp::Status).await?)?;
    Ok(parse_status_v2(&String::from_utf8_lossy(&out.stdout)).len() as u32)
}

/// 作業ツリーの無視ファイル（`.gitignore` 対象）の件数（読取りのみ）。`git worktree remove` は無視ファイルを確認なしに消す。
pub async fn ignored_count(git: &Git, worktree: &Path) -> Result<u32, WtError> {
    let out = ok(git.run(worktree, &GitOp::StatusIgnored).await?)?;
    Ok(count_ignored_v2(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_prefers_the_first_stderr_line_then_the_exit_code() {
        let o = |code, err: &str| GitOutput { exit_code: code, stdout: vec![], stderr: err.as_bytes().to_vec() };
        assert_eq!(ok(o(Some(128), "\n fatal: bad thing\nmore")).unwrap_err(), WtError::Exit { message: "fatal: bad thing".into() });
        assert_eq!(ok(o(Some(1), "")).unwrap_err(), WtError::Exit { message: "gitが終了コード 1 で終わりました".into() });
        assert_eq!(ok(o(None, "")).unwrap_err(), WtError::Exit { message: "gitが異常終了しました".into() });
        assert!(ok(o(Some(0), "warn")).is_ok());
    }

    #[test]
    fn repo_root_is_classified_as_root_not_repo_or_owner_mismatch() {
        assert_eq!(classify_repo_root(true, "C:/w/r\n", ""), RepoRootKind::Root("C:/w/r".into()));
        assert_eq!(classify_repo_root(true, "\n", ""), RepoRootKind::NotRepo);
        assert_eq!(classify_repo_root(false, "", "fatal: not a git repository (or any of the parent directories): .git"), RepoRootKind::NotRepo);
        let dubious = "fatal: detected dubious ownership in repository at 'C:/Users/wmasa/Documents/Rust/DocTrail'\nTo add an exception";
        assert_eq!(classify_repo_root(false, "", dubious), RepoRootKind::OwnerMismatch { repo: "C:/Users/wmasa/Documents/Rust/DocTrail".into() });
    }

    #[test]
    fn error_kinds_are_distinguishable() {
        assert!(WtError::Run(GitError::Unavailable("x".into())).is_unavailable());
        assert!(WtError::Run(GitError::Timeout).is_timeout());
        assert!(!WtError::Exit { message: "m".into() }.is_timeout());
    }
}
