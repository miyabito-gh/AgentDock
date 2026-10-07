//! Git実行の共通部品（段階③ DESIGN_P3 §2.3）。バックエンドに依存しないAgentDock管理の処理。
//!
//! 公開するのは許可した操作（[`GitOp`]）だけで、任意のgitコマンドは受け付けない。読取り系（status・diff・一覧）と
//! worktree操作（追加・削除・マージ済みブランチの削除）に限る。引数は配列（シェルなし）、コンソール窓なし、
//! タイムアウトと出力上限（16MiB）あり、実行は直列化する。`git` の場所は設定 `tools.git`（なければPATH）。
//! 強制系（`--force`）は削除操作の2段目の確認後にだけ呼出し側が指定する。`branch -D` は提供しない。

pub mod worktree;

use std::path::Path;
use std::time::Duration;

use crate::exec::{run_bounded, RunError, RunSpec};

/// 出力の上限。超えたら途中までを返さず失敗にする。
pub const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitOp {
    /// `git --version`（Gitの有無の確認）。
    Version,
    /// リポジトリのルート。リポジトリでなければ非0終了。
    RepoRoot,
    /// 現在のコミット。
    Head,
    /// 現在のブランチ名（detachedなら `HEAD`）。
    CurrentBranch,
    /// 参照（ブランチ名・コミット）をコミットのIDに解決する（読取りのみ。なければ非0終了）。
    RevParse { rev: String },
    /// 作業ツリーの状態（索引を書かない）。
    Status,
    /// 作業ツリーの状態に、無視ファイルも含める（読取りのみ。worktree削除前に、消えるものを数える）。
    StatusIgnored,
    /// 作業ツリーの差分（読取りのみ）。`path` があればそのファイルだけ。
    Diff { path: Option<String> },
    /// 作業ツリーとHEADの差分（索引・作業ツリーの変更をまとめて。読取りのみ）。`path` があればそのファイルだけ。
    DiffHead { path: Option<String> },
    /// ローカルブランチの一覧。
    Branches,
    /// 直近のコミット（`<sha>`と`<件名>`をタブ区切りで1行ずつ。読取りのみ）。コードレビューの対象選択用。
    RecentCommits,
    /// worktreeの一覧（`--porcelain`）。
    WorktreeList,
    /// worktreeの追加（新しいブランチを作る）。
    WorktreeAdd { path: String, branch: String, base: String },
    /// worktreeの削除。`force` は未コミット変更を捨てる（2段目の確認の後だけ）。
    WorktreeRemove { path: String, force: bool },
    /// マージ済みのブランチだけ削除（`-d`。`-D` は使わない）。
    BranchDeleteMerged { branch: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    /// 引数が許可の形ではない（実行していない）。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// gitを起動できない（未導入・パス違い）。
    #[error("git unavailable: {0}")]
    Unavailable(String),
    #[error("git timed out")]
    Timeout,
    #[error("git output exceeded {0} bytes")]
    OutputTooLarge(usize),
    #[error("io: {0}")]
    Io(String),
}

impl From<RunError> for GitError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::Spawn(m) => GitError::Unavailable(m),
            RunError::Timeout => GitError::Timeout,
            RunError::OutputTooLarge(n) => GitError::OutputTooLarge(n),
            RunError::Io(m) => GitError::Io(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// ブランチ・コミットなどの参照名として安全か（先頭が `-` でない、空白・制御文字・git特殊文字を含まない）。
fn valid_ref(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

fn valid_path(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c == '\0' || c == '\n' || c == '\r')
}

fn check(ok: bool, what: &str) -> Result<(), GitError> {
    if ok {
        Ok(())
    } else {
        Err(GitError::InvalidArgument(what.to_string()))
    }
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

impl GitOp {
    /// 引数の配列（`git` の後ろ）。値は検証し、パスは `--` の後ろに置く。
    pub fn args(&self) -> Result<Vec<String>, GitError> {
        Ok(match self {
            GitOp::Version => strings(&["--version"]),
            GitOp::RepoRoot => strings(&["rev-parse", "--show-toplevel"]),
            GitOp::Head => strings(&["rev-parse", "HEAD"]),
            GitOp::CurrentBranch => strings(&["rev-parse", "--abbrev-ref", "HEAD"]),
            GitOp::RevParse { rev } => {
                check(valid_ref(rev), "rev")?;
                vec!["rev-parse".into(), "--verify".into(), "--quiet".into(), format!("{rev}^{{commit}}")]
            }
            GitOp::Status => strings(&["--no-optional-locks", "status", "--porcelain=v2", "-z"]),
            GitOp::StatusIgnored => strings(&["--no-optional-locks", "status", "--porcelain=v2", "-z", "--ignored=matching"]),
            GitOp::Diff { path } => {
                let mut a = strings(&["--no-optional-locks", "-c", "core.quotepath=false", "diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
                if let Some(p) = path {
                    check(valid_path(p), "path")?;
                    a.push("--".into());
                    a.push(p.clone());
                }
                a
            }
            GitOp::DiffHead { path } => {
                let mut a = strings(&["--no-optional-locks", "-c", "core.quotepath=false", "diff", "HEAD", "--no-ext-diff", "--no-textconv", "--no-color"]);
                if let Some(p) = path {
                    check(valid_path(p), "path")?;
                    a.push("--".into());
                    a.push(p.clone());
                }
                a
            }
            GitOp::Branches => strings(&["for-each-ref", "--format=%(refname:short)", "refs/heads"]),
            GitOp::RecentCommits => strings(&["--no-optional-locks", "log", "-n", "30", "--no-color", "--format=%H%x09%s"]),
            GitOp::WorktreeList => strings(&["worktree", "list", "--porcelain"]),
            GitOp::WorktreeAdd { path, branch, base } => {
                check(valid_path(path), "path")?;
                check(valid_ref(branch), "branch")?;
                check(valid_ref(base), "base")?;
                let mut a = strings(&["worktree", "add", "-b"]);
                a.extend([branch.clone(), "--".into(), path.clone(), base.clone()]);
                a
            }
            GitOp::WorktreeRemove { path, force } => {
                check(valid_path(path), "path")?;
                let mut a = strings(&["worktree", "remove"]);
                if *force {
                    a.push("--force".into());
                }
                a.extend(["--".into(), path.clone()]);
                a
            }
            GitOp::BranchDeleteMerged { branch } => {
                check(valid_ref(branch), "branch")?;
                let mut a = strings(&["branch", "-d", "--"]);
                a.push(branch.clone());
                a
            }
        })
    }

    fn timeout(&self) -> Duration {
        match self {
            GitOp::WorktreeAdd { .. } | GitOp::WorktreeRemove { .. } | GitOp::BranchDeleteMerged { .. } => WRITE_TIMEOUT,
            _ => READ_TIMEOUT,
        }
    }
}

/// Gitの実行口。同じインスタンスの実行は直列化する（索引ロックの競合を避ける）。
pub struct Git {
    exe: String,
    lock: tokio::sync::Mutex<()>,
}

impl Git {
    /// `exe` は設定 `tools.git`。None・空白だけならPATHの `git`。
    pub fn new(exe: Option<&str>) -> Self {
        let exe = exe.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("git").to_string();
        Git { exe, lock: tokio::sync::Mutex::new(()) }
    }

    pub async fn run(&self, cwd: &Path, op: &GitOp) -> Result<GitOutput, GitError> {
        let args = op.args()?;
        let _serial = self.lock.lock().await;
        let out = run_bounded(RunSpec {
            program: &self.exe,
            args: &args,
            cwd: Some(cwd),
            env: &[("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0")],
            timeout: op.timeout(),
            stdout_limit: OUTPUT_LIMIT,
        })
        .await?;
        Ok(GitOutput { exit_code: out.exit_code, stdout: out.stdout, stderr: out.stderr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_ops_use_no_optional_locks_and_paths_follow_double_dash() {
        let a = GitOp::Diff { path: Some("src/a.rs".into()) }.args().unwrap();
        assert_eq!(&a[a.len() - 2..], ["--", "src/a.rs"]);
        assert!(GitOp::Status.args().unwrap().contains(&"--no-optional-locks".to_string()));
        assert!(GitOp::StatusIgnored.args().unwrap().contains(&"--ignored=matching".to_string()));
    }

    #[test]
    fn diff_head_is_read_only_and_paths_follow_double_dash() {
        let a = GitOp::DiffHead { path: Some("a b.rs".into()) }.args().unwrap();
        assert!(a.contains(&"HEAD".to_string()) && a.contains(&"--no-optional-locks".to_string()));
        assert_eq!(&a[a.len() - 2..], ["--", "a b.rs"]);
        assert!(GitOp::DiffHead { path: Some("a
b".into()) }.args().is_err());
    }

    #[test]
    fn option_like_or_unsafe_values_are_rejected() {
        let add = |branch: &str, base: &str| GitOp::WorktreeAdd { path: "C:/w".into(), branch: branch.into(), base: base.into() }.args();
        assert!(add("--upload-pack=x", "HEAD").is_err());
        assert!(add("ok/branch", "-c").is_err());
        assert!(add("a b", "HEAD").is_err());
        assert!(add("a..b", "HEAD").is_err());
        assert!(add("agentdock/x-1", "HEAD").is_ok());
        assert!(GitOp::BranchDeleteMerged { branch: "-D".into() }.args().is_err());
    }

    #[test]
    fn rev_parse_rejects_option_like_values_and_peels_to_a_commit() {
        assert_eq!(GitOp::RevParse { rev: "main".into() }.args().unwrap(), ["rev-parse", "--verify", "--quiet", "main^{commit}"]);
        assert!(GitOp::RevParse { rev: "--all".into() }.args().is_err());
        assert!(GitOp::RevParse { rev: "a^b".into() }.args().is_err());
    }

    #[test]
    fn branch_delete_is_lowercase_d_only_and_force_is_explicit() {
        assert_eq!(GitOp::BranchDeleteMerged { branch: "x".into() }.args().unwrap(), ["branch", "-d", "--", "x"]);
        let plain = GitOp::WorktreeRemove { path: "p".into(), force: false }.args().unwrap();
        assert!(!plain.contains(&"--force".to_string()));
        let forced = GitOp::WorktreeRemove { path: "p".into(), force: true }.args().unwrap();
        assert!(forced.contains(&"--force".to_string()));
    }
}
