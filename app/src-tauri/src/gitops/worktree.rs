//! worktree操作の呼出し（P3-7、DESIGN_P3 §2.3）。許可した `GitOp` だけを使い、結果を解析して返す。
//!
//! 削除は `--force` なしが基本で、`force` は呼出し側が2段目の確認の後にだけ指定する。ブランチの削除は `-d`（マージ済みのみ）で、`-D` は提供しない。
//! 出力の解析は純粋ロジック（`rules::worktree`）。ここはGitの実行と終了コードの扱いだけ。

use std::path::Path;

use super::{Git, GitError, GitOp, GitOutput};
use crate::rules::diff::parse_status_v2;
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

/// リポジトリのルート。リポジトリでなければ `Ok(None)`。
pub async fn repo_root(git: &Git, folder: &Path) -> Result<Option<String>, GitError> {
    let out = git.run(folder, &GitOp::RepoRoot).await?;
    if !out.success() {
        return Ok(None);
    }
    let root = out.stdout_text().trim().to_string();
    Ok((!root.is_empty()).then_some(root))
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
    fn error_kinds_are_distinguishable() {
        assert!(WtError::Run(GitError::Unavailable("x".into())).is_unavailable());
        assert!(WtError::Run(GitError::Timeout).is_timeout());
        assert!(!WtError::Exit { message: "m".into() }.is_timeout());
    }
}
