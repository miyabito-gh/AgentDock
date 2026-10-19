//! 組織実行の成果物の読取り（DESIGN_ORG_R3 §1。読取りのみ）。作業フォルダ・索引・ブランチ・worktreeを変えない。
//!
//! - `list_files`: `base..commit` の変更ファイル一覧と行数（`--name-status -M`＋`--numstat -z -M`）。
//! - `diff_text`: 1ファイルの差分。
//! - `read_blob`: コミット内のファイルの中身（大きさを先に確かめ、上限を超えるものは読まない）。
//! - `is_ancestor`: 祖先判定（読めなければ None。含まれるとは見なさない）。

use std::path::Path;

use super::worktree::{ok, resolve_commit, WtError};
use super::{Git, GitOp, OUTPUT_LIMIT};
use crate::backend::org_artifacts::OrgArtifactFile;
use crate::rules::org_artifacts::{merge_files, parse_numstat_z};
use crate::rules::org_worktree::parse_name_status;

/// 変更ファイルの一覧。`from`・`to` はコミット（または `to` にツリー）。参照は解決してから使う。
pub async fn list_files(git: &Git, repo: &Path, from: &str, to: &str) -> Result<Vec<OrgArtifactFile>, WtError> {
    let from = resolve_object(git, repo, from).await?;
    let to = resolve_object(git, repo, to).await?;
    let names = ok(git.run(repo, &GitOp::DiffCommitsNamesM { from: from.clone(), to: to.clone() }).await?)?.stdout_text();
    let nums = ok(git.run(repo, &GitOp::DiffCommitsNumstat { from, to }).await?)?.stdout_text();
    Ok(merge_files(&parse_name_status(&names), &parse_numstat_z(&nums)))
}

/// コミットまたはツリーのID。コミットとして解決できなければツリーとして試す（`rev-parse --verify` は `^{commit}` を付けるので、ツリーは別に扱う）。
async fn resolve_object(git: &Git, repo: &Path, rev: &str) -> Result<String, WtError> {
    let is_oid = (rev.len() == 40 || rev.len() == 64) && rev.bytes().all(|b| b.is_ascii_hexdigit());
    match resolve_commit(git, repo, rev).await {
        Ok(sha) => Ok(sha),
        // 40桁の16進で、コミットではない（ツリーのID）なら、そのまま差分の相手にできる。
        Err(_) if is_oid => Ok(rev.to_string()),
        Err(e) => Err(e),
    }
}

/// 1ファイルの統一差分（移動は両パス）。
pub async fn diff_text(git: &Git, repo: &Path, from: &str, to: &str, path: &str, old_path: Option<&str>) -> Result<String, WtError> {
    let from = resolve_object(git, repo, from).await?;
    let to = resolve_object(git, repo, to).await?;
    let out = ok(git.run(repo, &GitOp::DiffCommitsFile { from, to, path: path.to_string(), old_path: old_path.map(str::to_string) }).await?)?;
    Ok(out.stdout_text())
}

/// `a` が `b` の祖先か。祖先でなければ `Some(false)`、読めなければ `None`。
pub async fn is_ancestor(git: &Git, repo: &Path, a: &str, b: &str) -> Option<bool> {
    let out = git.run(repo, &GitOp::IsAncestor { a: a.to_string(), b: b.to_string() }).await.ok()?;
    match out.exit_code {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// コミット内のファイルの大きさ。
pub async fn blob_size(git: &Git, repo: &Path, commit: &str, path: &str) -> Result<u64, WtError> {
    let out = ok(git.run(repo, &GitOp::BlobSize { commit: commit.to_string(), path: path.to_string() }).await?)?;
    out.stdout_text().trim().parse::<u64>().map_err(|_| WtError::Exit { message: "ファイルの大きさを読み取れませんでした".to_string() })
}

/// 読めたファイルの中身と、全体の大きさ。
pub struct Blob {
    pub bytes: Vec<u8>,
    pub size: u64,
}

/// コミット内のファイルを読む。`max` を超えるものは読まずに `Ok(None)`（大きさだけ返す側で扱う）。Gitの出力上限（16MiB）を超えるものも読まない。
pub async fn read_blob(git: &Git, repo: &Path, commit: &str, path: &str, max: u64) -> Result<Result<Blob, u64>, WtError> {
    let size = blob_size(git, repo, commit, path).await?;
    if size > max || size as usize > OUTPUT_LIMIT {
        return Ok(Err(size));
    }
    let out = ok(git.run(repo, &GitOp::ShowBlob { commit: commit.to_string(), path: path.to_string() }).await?)?;
    Ok(Ok(Blob { bytes: out.stdout, size }))
}

/// コミットを含むリモート追跡ブランチがあるか。読めなければ None（送信済みでないとは見なさない）。
pub async fn remote_contains(git: &Git, repo: &Path, commit: &str) -> Option<bool> {
    let out = git.run(repo, &GitOp::RemoteContains { commit: commit.to_string() }).await.ok()?;
    if !out.success() {
        return None;
    }
    Some(out.stdout_text().lines().any(|l| !l.trim().is_empty()))
}
