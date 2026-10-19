//! Gitでないフォルダの準備（DESIGN_ORG_INTAKE §2）。許可した `GitOp` だけを使う。
//!
//! - 検査（`repo_status`）とプレビュー（`preview`）は読取りだけ。一時のgitdirはAgentDock自身の一時領域に作って必ず消す。
//! - 実行（`prepare`）で作るのは `.git` フォルダだけ。作業フォルダの他のファイルは作らず、変えない。
//! - 失敗時の巻き戻しは `rules::git_prep::rollback_decision` に従う。結果が分からない・既存の `.git` は消さない。
//! - 除外ファイル（`.git\info\exclude`）への追記は既定で行わない。利用者が選んだときだけ、追記のみ・前後を照合する。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::worktree::{repo_root_kind, resolve_commit, RepoRootKind, WtError};
use super::{Git, GitError, GitOp, GitOutput};
use crate::backend::git_ops::*;
use crate::rules::diff::parse_status_v2;
use crate::rules::git_prep::*;
use crate::rules::work_folders::{normalize_path, path_key};

/// `.git` の中の「AgentDockが作った」印のファイル名。
pub const MARKER_FILE: &str = "agentdock-init.json";
/// 初回コミットのメッセージ。
pub const FIRST_COMMIT_MESSAGE: &str = "AgentDock: 最初のコミット";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkerFile {
    prep_id: String,
    at: i64,
}

fn git_dir_of(folder: &Path) -> PathBuf {
    folder.join(".git")
}

fn same_path(a: &str, b: &str) -> bool {
    match (normalize_path(a), normalize_path(b)) {
        (Some(a), Some(b)) => path_key(&a).trim_end_matches('\\') == path_key(&b).trim_end_matches('\\'),
        _ => a == b,
    }
}

fn run_err(e: &GitError) -> String {
    match e {
        GitError::Unavailable(_) => "Gitを起動できません（設定のgitのパスを確認してください）".to_string(),
        GitError::Timeout => "Gitが時間内に終わりませんでした".to_string(),
        other => other.to_string(),
    }
}

fn unavailable(e: &WtError) -> RepoStatus {
    RepoStatus::Unavailable { message: e.message() }
}

// ───────────────────────────── 検査 ─────────────────────────────

/// 作業フォルダのGitの状態（読取りのみ）。
pub async fn repo_status(git: &Git, folder: &Path) -> RepoStatus {
    let root = match repo_root_kind(git, folder).await {
        Ok(RepoRootKind::Root(r)) => r,
        Ok(RepoRootKind::NotRepo) => return RepoStatus::NotRepo,
        Ok(RepoRootKind::OwnerMismatch { .. }) => return RepoStatus::OwnerMismatch,
        Err(e) => return RepoStatus::Unavailable { message: run_err(&e) },
    };
    match resolve_commit(git, Path::new(&root), "HEAD").await {
        Ok(_) => {
            if same_path(&root, &folder.to_string_lossy()) {
                let branch = match git.run(folder, &GitOp::CurrentBranch).await {
                    Ok(o) if o.success() => o.stdout_text().trim().to_string(),
                    _ => "HEAD".to_string(),
                };
                RepoStatus::Ready { root, branch }
            } else {
                RepoStatus::Nested { root }
            }
        }
        Err(e) if e.is_timeout() || e.is_unavailable() => unavailable(&e),
        Err(_) => {
            // コミットがない。AgentDockの印があり、作業フォルダ自身の `.git` なら、前回の準備が途中で止まっている。
            if same_path(&root, &folder.to_string_lossy()) && read_marker(&git_dir_of(Path::new(&root))).is_some() {
                RepoStatus::PrepInterrupted { root }
            } else {
                RepoStatus::NoCommit { root }
            }
        }
    }
}

fn read_marker(git_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(git_dir.join(MARKER_FILE)).ok()?;
    serde_json::from_str::<MarkerFile>(&text).ok().map(|m| m.prep_id)
}

fn marker_state(git_dir: &Path, prep_id: &str) -> crate::rules::git_prep::Marker {
    use crate::rules::git_prep::Marker as M;
    match read_marker(git_dir) {
        Some(id) if id == prep_id => M::Mine,
        Some(_) => M::Other,
        None => M::Missing,
    }
}

// ───────────────────────────── ファイルの一覧 ─────────────────────────────

/// 最初のコミットに入りうるファイルの一覧と、見つかった内側のリポジトリ。
pub struct Scan {
    pub files: Vec<PrepFile>,
    /// 内側に別のリポジトリがあるフォルダ（`add -A` が扱えない）。
    pub nested_repos: Vec<String>,
}

fn stat_files(root: &Path, entries: Vec<String>) -> Result<Scan, String> {
    let mut files = Vec::new();
    let mut nested = Vec::new();
    for path in entries {
        if path.ends_with('/') {
            nested.push(path.trim_end_matches('/').to_string());
            continue;
        }
        let abs = root.join(&path);
        let meta = std::fs::symlink_metadata(&abs).map_err(|e| format!("ファイルを調べられません: {path}（{e}）"))?;
        if meta.is_dir() {
            continue;
        }
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        files.push(PrepFile { path, size: meta.len(), mtime_ms });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Scan { files, nested_repos: nested })
}

/// `git status` の結果から、入るファイルのパスを集める（コミットしないもの・無視されたものは出ない）。
fn paths_of(out: &GitOutput) -> Vec<String> {
    parse_status_v2(&String::from_utf8_lossy(&out.stdout)).into_iter().filter(|e| e.x != 'D' && e.y != 'D').map(|e| e.path).collect()
}

/// 一時のgitdirの置き場所を作って返す（使い終わったら `remove_temp_gitdir`）。
fn temp_gitdir(tmp_base: &Path) -> Result<PathBuf, String> {
    let id = format!("gitprep-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let dir = tmp_base.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("一時領域を作れません: {e}"))?;
    Ok(dir)
}

fn remove_temp_gitdir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// 最初のコミットに入るファイルを調べる。`NotRepo` は一時のgitdirでフォルダの既存の `.gitignore` を反映して調べ、フォルダには何も書かない。
pub async fn scan(git: &Git, folder: &Path, kind: PrepKind, tmp_base: &Path) -> Result<Scan, String> {
    match kind {
        PrepKind::NotRepo => {
            let dir = temp_gitdir(tmp_base)?;
            let result = async {
                let g = dir.to_string_lossy().into_owned();
                let init = git.run_pinned(folder, &g, &GitOp::Init).await.map_err(|e| run_err(&e))?;
                if !init.success() {
                    return Err("一時領域でGitを初期化できませんでした".to_string());
                }
                let st = git.run_pinned(folder, &g, &GitOp::StatusAllUntracked).await.map_err(|e| run_err(&e))?;
                if !st.success() {
                    return Err(format!("ファイルの一覧を取得できません: {}", String::from_utf8_lossy(&st.stderr).lines().next().unwrap_or("")));
                }
                Ok(paths_of(&st))
            }
            .await;
            remove_temp_gitdir(&dir);
            stat_files(folder, result?)
        }
        PrepKind::NoCommit | PrepKind::Interrupted => {
            let root = match repo_root_kind(git, folder).await {
                Ok(RepoRootKind::Root(r)) => PathBuf::from(r),
                _ => return Err("リポジトリを確認できません".to_string()),
            };
            let st = git.run(&root, &GitOp::StatusAllUntracked).await.map_err(|e| run_err(&e))?;
            if !st.success() {
                return Err(format!("ファイルの一覧を取得できません: {}", String::from_utf8_lossy(&st.stderr).lines().next().unwrap_or("")));
            }
            stat_files(&root, paths_of(&st))
        }
    }
}

async fn config_value(git: &Git, folder: &Path, key: &str) -> Option<String> {
    let out = git.run(folder, &GitOp::ConfigGet { key: key.to_string() }).await.ok()?;
    out.success().then(|| out.stdout_text().trim().to_string())
}

fn kind_of(status: &RepoStatus) -> Option<PrepKind> {
    match status {
        RepoStatus::NotRepo => Some(PrepKind::NotRepo),
        RepoStatus::NoCommit { .. } => Some(PrepKind::NoCommit),
        RepoStatus::PrepInterrupted { .. } => Some(PrepKind::Interrupted),
        _ => None,
    }
}

fn not_preparable(status: &RepoStatus) -> String {
    match status {
        RepoStatus::Ready { .. } | RepoStatus::Nested { .. } => "このフォルダはすでにGitのリポジトリで、コミットがあります。準備は要りません".to_string(),
        RepoStatus::OwnerMismatch => "Gitがこのフォルダの所有者の違いを理由に拒否しました。Gitの safe.directory を確認してください".to_string(),
        RepoStatus::Unavailable { message } => format!("Gitで確認できません: {message}"),
        _ => "このフォルダは準備できる状態ではありません".to_string(),
    }
}

// ───────────────────────────── プレビュー ─────────────────────────────

/// 準備のプレビュー（読取りのみ）。`folder_refusal` はフォルダの場所を理由にした断り（ホストが `check_folder` から作る）。
pub async fn preview(git: &Git, folder: &Path, tmp_base: &Path, folder_refusal: Option<String>, apply_excludes: bool) -> Result<RepoPrepPreview, String> {
    let status = repo_status(git, folder).await;
    let Some(kind) = kind_of(&status) else { return Err(not_preparable(&status)) };
    let scanned = scan(git, folder, kind, tmp_base).await?;
    let refusal = folder_refusal.or_else(|| {
        (!scanned.nested_repos.is_empty()).then(|| format!("内側に別のGitリポジトリがあるフォルダがあります（{}）。Gitを準備できません", scanned.nested_repos.iter().take(3).cloned().collect::<Vec<_>>().join("、")))
    });
    let synced = in_sync_folder(&folder.to_string_lossy());
    let plan = plan_prep(kind, &scanned.files, apply_excludes, refusal, synced);
    let author = author_from_config(config_value(git, folder, "user.name").await.as_deref(), config_value(git, folder, "user.email").await.as_deref());
    Ok(RepoPrepPreview {
        folder: folder.to_string_lossy().into_owned(),
        kind,
        file_count: plan.included.len() as u32,
        total_bytes: plan.total_bytes,
        largest: plan.largest,
        suggested_excludes: plan.suggested,
        exclude_lines: plan.exclude_lines,
        apply_excludes,
        refusal: plan.refusal,
        notes: plan.notes,
        author,
        preview_token: preview_token(&scanned.files),
    })
}

// ───────────────────────────── 実行 ─────────────────────────────

/// 実行の指定。
pub struct PrepareSpec<'a> {
    pub folder: &'a Path,
    pub tmp_base: &'a Path,
    pub expected_token: &'a str,
    pub apply_excludes: bool,
    pub folder_refusal: Option<String>,
    pub prep_id: String,
    pub now_ms: i64,
}

enum StepFail {
    Clear(String),
    Unknown(String),
}

fn step(r: Result<GitOutput, GitError>, what: &str) -> Result<GitOutput, StepFail> {
    match r {
        Ok(o) if o.success() => Ok(o),
        Ok(o) => {
            let first = String::from_utf8_lossy(&o.stderr).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
            Err(StepFail::Clear(if first.is_empty() { format!("{what}に失敗しました") } else { format!("{what}に失敗しました: {first}") }))
        }
        Err(e @ GitError::Unavailable(_)) | Err(e @ GitError::InvalidArgument(_)) => Err(StepFail::Clear(format!("{what}: {}", run_err(&e)))),
        Err(e) => Err(StepFail::Unknown(format!("{what}: {}", run_err(&e)))),
    }
}

/// 準備を実行する。再検査（状態・指紋）が違えば何もしない。
pub async fn prepare(git: &Git, spec: PrepareSpec<'_>) -> PrepOutcome {
    let folder = spec.folder;
    let status = repo_status(git, folder).await;
    let Some(kind) = kind_of(&status) else { return PrepOutcome::Failed { reason: not_preparable(&status), leftover: None } };
    if kind == PrepKind::NotRepo && git_dir_of(folder).exists() {
        return PrepOutcome::Failed { reason: "このフォルダに `.git` がすでにあります。もう一度確認してください".to_string(), leftover: None };
    }
    let scanned = match scan(git, folder, kind, spec.tmp_base).await {
        Ok(s) => s,
        Err(m) => return PrepOutcome::Failed { reason: m, leftover: None },
    };
    if !token_matches(spec.expected_token, &preview_token(&scanned.files)) {
        return PrepOutcome::Failed { reason: "確認した後でフォルダの中身が変わりました。もう一度確認してください（何も作っていません）".to_string(), leftover: None };
    }
    let refusal = spec.folder_refusal.clone().or_else(|| (!scanned.nested_repos.is_empty()).then(|| "内側に別のGitリポジトリがあるフォルダがあります".to_string()));
    let plan = plan_prep(kind, &scanned.files, spec.apply_excludes, refusal, false);
    if let Some(r) = plan.refusal {
        return PrepOutcome::Failed { reason: r, leftover: None };
    }
    let author = author_from_config(config_value(git, folder, "user.name").await.as_deref(), config_value(git, folder, "user.email").await.as_deref());
    let root = match &status {
        RepoStatus::NoCommit { root } | RepoStatus::PrepInterrupted { root } => PathBuf::from(root),
        _ => folder.to_path_buf(),
    };
    let git_dir = git_dir_of(&root);

    let outcome: Result<String, StepFail> = async {
        if kind == PrepKind::NotRepo {
            step(git.run(folder, &GitOp::Init).await, "Gitの初期化")?;
            let marker = MarkerFile { prep_id: spec.prep_id.clone(), at: spec.now_ms };
            let text = serde_json::to_string(&marker).map_err(|e| StepFail::Clear(e.to_string()))?;
            std::fs::write(git_dir.join(MARKER_FILE), text).map_err(|e| StepFail::Clear(format!("準備の印を書けませんでした: {e}")))?;
        }
        if spec.apply_excludes && !plan.exclude_lines.is_empty() {
            append_exclude(&git_dir, &plan.exclude_lines).map_err(StepFail::Clear)?;
        }
        step(git.run(&root, &GitOp::StageAll).await, "ファイルの追加")?;
        let identity = author.is_agentdock.then(|| (author.name.clone(), author.email.clone()));
        step(git.run(&root, &GitOp::CommitStagedAs { message: FIRST_COMMIT_MESSAGE.to_string(), identity }).await, "最初のコミット")?;
        let head = step(git.run(&root, &GitOp::Head).await, "コミットの確認")?;
        let sha = head.stdout_text().trim().to_string();
        if sha.is_empty() {
            return Err(StepFail::Unknown("コミットを確認できませんでした".to_string()));
        }
        Ok(sha)
    }
    .await;

    match outcome {
        Ok(commit) => PrepOutcome::Prepared { commit },
        Err(fail) => {
            let (step_result, message) = match &fail {
                StepFail::Clear(m) => (StepResult::ClearFailure, m.clone()),
                StepFail::Unknown(m) => (StepResult::Unknown, m.clone()),
            };
            // 実行前に `.git` が無いことを確認済み（上の検査）なので、新しく作る場合は今あるものは自分が作ったもの。
            // 印の書込み前に失敗した（印がない）場合も、自分の `.git` として扱う。別の準備の印があるときだけ扱わない。
            let marker = effective_marker(kind, marker_state(&git_dir, &spec.prep_id));
            match rollback_decision(kind, &step_result, marker) {
                RollbackDecision::RemoveOwnGit if !git_dir.exists() => PrepOutcome::RolledBack { reason: format!("{message}。作られたものは残っていません") },
                RollbackDecision::RemoveOwnGit => match std::fs::remove_dir_all(&git_dir) {
                    Ok(()) => PrepOutcome::RolledBack { reason: format!("{message}。作った `.git` を取り除いて、元に戻しました") },
                    Err(e) => PrepOutcome::Failed { reason: format!("{message}。作った `.git` を取り除けませんでした（{e}）"), leftover: Some(git_dir.to_string_lossy().into_owned()) },
                },
                RollbackDecision::Keep(why) => {
                    if step_result == StepResult::Unknown {
                        // 実際の状態を読み直して示す（成功とも失敗とも断定しない）。
                        PrepOutcome::Unknown { status: repo_status(git, folder).await }
                    } else {
                        let leftover = (kind != PrepKind::NoCommit && git_dir.exists()).then(|| git_dir.to_string_lossy().into_owned());
                        let index_note = if kind == PrepKind::NoCommit { "（ファイルの追加まで進んでいた場合、既存のリポジトリの索引に追加した結果が残ることがあります）" } else { "" };
                        PrepOutcome::Failed { reason: format!("{message}。{why}{index_note}"), leftover }
                    }
                }
            }
        }
    }
}

/// 除外ファイルへ追記する（追記のみ。すでにある行は足さない。書いた後で前後を照合する）。
fn append_exclude(git_dir: &Path, lines: &[String]) -> Result<(), String> {
    use std::io::Write;
    let path = git_dir.join("info").join("exclude");
    let before = match std::fs::read(&path) {
        Ok(b) => String::from_utf8(b).map_err(|_| "除外ファイルが文字として読めないため、追記しません".to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("除外ファイルを読めません: {e}")),
    };
    let block = exclude_block(&before, lines);
    if block.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("除外ファイルの場所を作れません: {e}"))?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("除外ファイルを開けません: {e}"))?;
    f.write_all(block.as_bytes()).map_err(|e| format!("除外ファイルへ書けません: {e}"))?;
    f.flush().map_err(|e| format!("除外ファイルへ書けません: {e}"))?;
    drop(f);
    let after = std::fs::read_to_string(&path).map_err(|e| format!("書いた除外ファイルを読み直せません: {e}"))?;
    if !exclude_append_verified(&before, &block, &after) {
        return Err("除外ファイルの前後の照合が合いませんでした".to_string());
    }
    Ok(())
}

/// 前回の途中の準備（印があり、コミットがない `.git`）を取り除く。印の準備IDを確認し、コミットがないことを再確認してから消す。
pub async fn remove_interrupted(git: &Git, folder: &Path) -> Result<(), String> {
    let status = repo_status(git, folder).await;
    let RepoStatus::PrepInterrupted { root } = status else {
        return Err("取り除ける途中の準備がありません".to_string());
    };
    let git_dir = git_dir_of(Path::new(&root));
    if read_marker(&git_dir).is_none() || !git_dir.is_dir() {
        return Err("AgentDockが作った印を確認できないため、取り除きません".to_string());
    }
    std::fs::remove_dir_all(&git_dir).map_err(|e| format!("`.git` を取り除けませんでした: {e}"))
}

#[cfg(test)]
mod tests;
