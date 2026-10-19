//! 組織実行の成果の取り込みと、取り込み前に戻す（DESIGN_ORG_R3 §2。利用者の作業フォルダへ書く唯一の経路）。
//!
//! 方式（案D）: `merge-tree --write-tree` で結果を計算し（どの作業ツリー・索引・ブランチにも書かない）、`commit-tree` で
//! 取り込みコミットを作る（オブジェクトだけ）。確認後に利用者のフォルダで `merge --ff-only <そのコミット>` を1回だけ実行する。
//!
//! 守ること:
//! - `prepare`／`check_worktree`／`undo_probe` は読取りのみ（`commit-tree`・`merge-tree` が増やすのはオブジェクトだけ）。
//! - 書くのは `execute`（`merge --ff-only`）と `undo_execute`（`reset --keep`／`checkout <前のHEAD> -- <ファイル>`）だけ。
//!   直前に「ブランチが `agentdock/` でない・detachedでない・途中の操作がない・HEADが確認時のまま・重なる未コミットの変更なし・
//!   ロック中のファイルなし」を確かめ、1つでも外れれば何も書かない。`begin` の台帳が書けなければ実行しない。
//! - 実行後はHEADを読み戻して分類する（`Applied` は読み戻しで確かめたときだけ）。再実行・自動再試行はしない。
//! - フックは走らせない（`core.hooksPath` を空に）。`--hard`・`--force` は表せない。

use std::path::Path;

use super::worktree::{ok, resolve_commit, WtError};
use super::{Git, GitError, GitOp};
use crate::backend::org_artifacts::*;
use crate::rules::diff::parse_status_v2;
use crate::rules::git_push::INTERNAL_BRANCH_PREFIX;
use crate::rules::org_artifacts::*;

/// 取り込みコミットの作者が、利用者のGit設定にないときの名義（設定は書かない）。
pub const FALLBACK_IDENTITY: (&str, &str) = ("AgentDock", "agentdock@localhost");

/// 作業フォルダのGitの事実（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFacts {
    /// 今のブランチ。detached なら "HEAD"。読めなければ None。
    pub branch: Option<String>,
    pub head: Option<String>,
    pub in_progress: Option<&'static str>,
    pub git_supported: Option<bool>,
}

pub async fn repo_facts(git: &Git, repo: &Path) -> RepoFacts {
    let text = |out: Result<crate::gitops::GitOutput, GitError>| out.ok().filter(|o| o.success()).map(|o| o.stdout_text().trim().to_string()).filter(|s| !s.is_empty());
    let branch = text(git.run(repo, &GitOp::CurrentBranch).await);
    let head = text(git.run(repo, &GitOp::Head).await);
    let gitdir = text(git.run(repo, &GitOp::AbsoluteGitDir).await);
    let in_progress = gitdir.as_deref().and_then(|d| {
        let d = Path::new(d).to_path_buf();
        operation_in_progress(|name| d.join(name).exists())
    });
    let git_supported = text(git.run(repo, &GitOp::Version).await).and_then(|v| merge_tree_supported(&v));
    RepoFacts { branch, head, in_progress, git_supported }
}

/// 取り込み候補（選んだ順）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub task: String,
    pub title: String,
    pub commit: String,
}

/// 確認に使う、合成済みの結果（作業フォルダには何も書いていない）。
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub folder: String,
    pub branch: String,
    /// 確認したときのHEAD（`H`）。
    pub head: String,
    /// 取り込みコミット（`R`）。`H` の子孫で、親は [直前, 候補]。
    pub result: String,
    pub tasks: Vec<LedgerTask>,
    /// `H..R` の変更ファイル。
    pub files: Vec<OrgArtifactFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrepareOutcome {
    Ready(Prepared),
    /// 衝突（何も書いていない）。衝突したファイル。
    Conflict { files: Vec<String> },
    /// 取り込んでも変わるファイルがない。
    NoChange,
    Failed(String),
}

fn author_identity_args(has_user: bool) -> Option<(String, String)> {
    (!has_user).then(|| (FALLBACK_IDENTITY.0.to_string(), FALLBACK_IDENTITY.1.to_string()))
}

/// 利用者のGit設定に `user.name`・`user.email` があるか。
async fn has_user_identity(git: &Git, repo: &Path) -> bool {
    let has = |key: &str| {
        let key = key.to_string();
        async move { git.run(repo, &GitOp::ConfigGet { key }).await.ok().filter(|o| o.success()).is_some_and(|o| !o.stdout_text().trim().is_empty()) }
    };
    has("user.name").await && has("user.email").await
}

/// 候補を選んだ順に合成して、取り込みコミットを作る。衝突が1つでもあれば `Conflict`（作ったオブジェクトは残るが、どのrefにも結び付かない）。
/// `facts` は直前に読んだ事実（`head`・`branch` がなければ失敗）。
pub async fn prepare(git: &Git, repo: &Path, facts: &RepoFacts, workflow_name: &str, cands: &[Candidate]) -> Result<PrepareOutcome, WtError> {
    let (Some(head), Some(branch)) = (facts.head.clone(), facts.branch.clone()) else {
        return Ok(PrepareOutcome::Failed("作業フォルダの今のコミットを読めません".to_string()));
    };
    if cands.is_empty() {
        return Ok(PrepareOutcome::Failed("取り込む成果が選ばれていません".to_string()));
    }
    let identity = author_identity_args(has_user_identity(git, repo).await);
    let message = import_commit_message(workflow_name, &cands.iter().map(|c| (c.task.clone(), c.title.clone(), c.commit.clone())).collect::<Vec<_>>());
    let mut current = head.clone();
    for c in cands {
        let theirs = resolve_commit(git, repo, &c.commit).await?;
        let out = git.run(repo, &GitOp::MergeTree { ours: current.clone(), theirs: theirs.clone() }).await?;
        match parse_merge_tree(out.exit_code, &out.stdout_text()) {
            MergeTreeResult::Clean { tree } => {
                let made = ok(git.run(repo, &GitOp::CommitTree { tree, parents: vec![current.clone(), theirs], message: message.clone(), identity: identity.clone() }).await?)?;
                let sha = made.stdout_text().trim().to_string();
                if sha.is_empty() {
                    return Ok(PrepareOutcome::Failed("取り込みコミットのIDを読めませんでした".to_string()));
                }
                current = sha;
            }
            MergeTreeResult::Conflict { files } => return Ok(PrepareOutcome::Conflict { files }),
            MergeTreeResult::Failed => {
                let why = out.stderr.iter().take(300).map(|b| *b as char).collect::<String>();
                return Ok(PrepareOutcome::Failed(format!("成果の合成に失敗しました{}", if why.trim().is_empty() { String::new() } else { format!("（{}）", why.trim()) })));
            }
        }
    }
    let files = super::artifacts::list_files(git, repo, &head, &current).await?;
    if files.is_empty() {
        return Ok(PrepareOutcome::NoChange);
    }
    Ok(PrepareOutcome::Ready(Prepared {
        folder: repo.to_string_lossy().into_owned(),
        branch,
        head,
        result: current,
        tasks: cands.iter().map(|c| LedgerTask { task: c.task.clone(), commit: c.commit.clone() }).collect(),
        files,
    }))
}

/// 作業フォルダの未コミットの変更（追跡中の変更・未追跡）のパス。名前変更は元・先の両方。
pub async fn dirty_paths(git: &Git, repo: &Path) -> Result<Vec<String>, WtError> {
    let out = ok(git.run(repo, &GitOp::StatusAllUntracked).await?)?;
    let mut paths = Vec::new();
    for e in parse_status_v2(&String::from_utf8_lossy(&out.stdout)) {
        paths.push(e.path);
        if let Some(o) = e.old_path {
            paths.push(o);
        }
    }
    Ok(paths)
}

/// 取り込みの前の作業フォルダの検査（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeCheck {
    /// 未コミットの変更の件数。
    pub dirty_total: u32,
    /// 取り込みで触れるパスと重なる未コミットの変更（無視ファイルの上書きを含む）。
    pub overlap: Vec<String>,
    /// 他のアプリに開かれている書込み対象。
    pub locked: Vec<String>,
}

/// リポジトリのルート（ファイルのパスはここからの相対）。読めなければ指定のフォルダ。
async fn toplevel(git: &Git, repo: &Path) -> std::path::PathBuf {
    match git.run(repo, &GitOp::RepoRoot).await {
        Ok(o) if o.success() && !o.stdout_text().trim().is_empty() => std::path::PathBuf::from(o.stdout_text().trim()),
        _ => repo.to_path_buf(),
    }
}

pub async fn check_worktree(git: &Git, repo: &Path, files: &[OrgArtifactFile]) -> Result<WorktreeCheck, WtError> {
    let top = toplevel(git, repo).await;
    let dirty = dirty_paths(git, repo).await?;
    let touched = touched_paths(files);
    let mut overlap = dirty_overlap(&touched, &dirty);
    // 無視ファイル（`.gitignore` 対象）は、Gitが確認なしに上書きする。追跡されていない既存のファイルに新規追加が当たるなら重なりとして止める。
    for f in files.iter().filter(|f| f.kind == crate::backend::changes::ChangeKind::Added) {
        let p = new_path(f).to_string();
        if overlap.contains(&p) || !top.join(&p).exists() {
            continue;
        }
        let tracked = ok(git.run(&top, &GitOp::LsFilesCached { path: p.clone() }).await?)?.stdout.iter().any(|b| *b != 0);
        if !tracked {
            overlap.push(p);
        }
    }
    let repo_owned = top.clone();
    let to_check = touched.clone();
    let locked = tokio::task::spawn_blocking(move || locked_files(&repo_owned, &to_check)).await.unwrap_or_default();
    // 件数は未追跡のフォルダを1件と数えず、ファイル単位で数える（`StatusAllUntracked`）。名前変更の元は数えない。
    let dirty_total = parse_status_v2(&String::from_utf8_lossy(&ok(git.run(repo, &GitOp::StatusAllUntracked).await?)?.stdout)).len() as u32;
    Ok(WorktreeCheck { dirty_total, overlap, locked })
}

/// 他のアプリに開かれていて書き換えられないファイル（共有なしで開けるかを試すだけ。内容は変えない）。Windows以外では常に空。
pub fn locked_files(repo: &Path, paths: &[String]) -> Vec<String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let mut out = Vec::new();
        for p in paths {
            let full = repo.join(p);
            if !full.is_file() {
                continue;
            }
            // 共有なし（0）で開く。誰かが開いていれば共有違反（32）かロック違反（33）で失敗する。
            match std::fs::OpenOptions::new().read(true).share_mode(0).open(&full) {
                Ok(_) => {}
                Err(e) if matches!(e.raw_os_error(), Some(32) | Some(33)) => out.push(p.clone()),
                Err(_) => {}
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        let _ = (repo, paths);
        Vec::new()
    }
}

/// 実行の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum ExecOutcome {
    /// 何も書かずに止めた（理由）。台帳の `begin` も書いていない。
    Refused(String),
    /// 台帳の `begin` を書けなかったため、実行していない。
    BeginFailed(String),
    /// 実行した（結果は読み戻しで分類済み）。
    Done(OrgImportOutcome),
}

fn refuse_text(blockers: &[String]) -> String {
    blockers.join("\n")
}

/// 直前の再検査（ブランチ・HEAD・途中の操作・未コミットの変更の重なり・ロック）。外れた理由を返す。
async fn recheck(git: &Git, repo: &Path, p: &Prepared) -> Result<Option<String>, WtError> {
    let facts = repo_facts(git, repo).await;
    if facts.head.as_deref() != Some(p.head.as_str()) || facts.branch.as_deref() != Some(p.branch.as_str()) {
        return Ok(Some("状態が変わりました。もう一度確認してください（今のコミットまたはブランチが、確認したときと違います）".to_string()));
    }
    let blockers = import_blockers(&ImportFacts {
        branch: facts.branch.as_deref(),
        in_progress: facts.in_progress,
        workflow_finished: true,
        delete_pending: false,
        selected: &[("取り込み".to_string(), OrgArtifactStatus::ReadyForReview)],
        git_supported: facts.git_supported,
    });
    if !blockers.is_empty() {
        return Ok(Some(refuse_text(&blockers)));
    }
    // 取り込みコミットが確認のとおりに今も残り、HEADの子孫である。
    let still = resolve_commit(git, repo, &p.result).await.ok();
    if still.as_deref() != Some(p.result.as_str()) {
        return Ok(Some("状態が変わりました。もう一度確認してください（取り込みコミットを確認できません）".to_string()));
    }
    if super::artifacts::is_ancestor(git, repo, &p.head, &p.result).await != Some(true) {
        return Ok(Some("状態が変わりました。もう一度確認してください".to_string()));
    }
    let check = check_worktree(git, repo, &p.files).await?;
    if !check.overlap.is_empty() {
        return Ok(Some(format!("未コミットの変更と重なるファイルがあるため、何も変更していません（{}）", check.overlap.join("、"))));
    }
    if !check.locked.is_empty() {
        return Ok(Some(format!("他のアプリで開かれているファイルがあるため、何も変更していません（{}）。閉じてからもう一度お試しください", check.locked.join("、"))));
    }
    Ok(None)
}

/// 取り込みを実行する（確認ダイアログの「取り込む」だけが呼ぶ）。`begin` は台帳の `begin` の追記で、書けなければ実行しない。
pub async fn execute(git: &Git, repo: &Path, p: &Prepared, begin: impl FnOnce() -> Result<(), String>) -> ExecOutcome {
    match recheck(git, repo, p).await {
        Ok(None) => {}
        Ok(Some(why)) => return ExecOutcome::Refused(why),
        Err(e) => return ExecOutcome::Refused(format!("作業フォルダの状態を確認できないため、何も変更していません（{}）", e.message())),
    }
    if let Err(m) = begin() {
        return ExecOutcome::BeginFailed(m);
    }
    let ran = git.run(repo, &GitOp::MergeFfOnly { commit: p.result.clone() }).await;
    let failure = match &ran {
        Ok(out) if out.success() => None,
        Ok(out) => Some(String::from_utf8_lossy(&out.stderr).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect::<String>()),
        Err(e) => Some(e.to_string()),
    };
    // 結果は必ずHEADを読み戻して決める（終了コード・タイムアウトを成功にも失敗にも直結させない）。
    let head_after = git.run(repo, &GitOp::Head).await.ok().filter(|o| o.success()).map(|o| o.stdout_text().trim().to_string());
    let changed_targets = if head_after.as_deref() == Some(p.head.as_str()) { target_changes(git, repo, &p.files).await } else { None };
    ExecOutcome::Done(classify_import(&p.head, &p.result, head_after.as_deref(), changed_targets.as_deref(), failure.as_deref()))
}

/// 対象ファイルのうち、いま作業ツリーで変わっているもの（取り込み前は重ならないことを確かめているので、あれば取り込みの途中の書換え）。読めなければ None。
async fn target_changes(git: &Git, repo: &Path, files: &[OrgArtifactFile]) -> Option<Vec<String>> {
    let out = git.run(repo, &GitOp::StatusAllUntracked).await.ok().filter(|o| o.success())?;
    let mut dirty = Vec::new();
    for e in parse_status_v2(&String::from_utf8_lossy(&out.stdout)) {
        dirty.push(e.path);
    }
    Some(dirty_overlap(&touched_paths(files), &dirty))
}

/// 現在のHEADの読取り。
pub async fn head_now(git: &Git, repo: &Path) -> Option<String> {
    git.run(repo, &GitOp::Head).await.ok().filter(|o| o.success()).map(|o| o.stdout_text().trim().to_string()).filter(|s| !s.is_empty())
}

// ───────────────────────────── 戻す ─────────────────────────────

/// 戻す判定の材料を集めて判定する（読取りのみ）。`rec` の取り込み先フォルダは `repo`。
pub async fn undo_probe(git: &Git, repo: &Path, rec: &LedgerRecord, state: OrgImportState) -> Result<(UndoDecision, Option<String>), WtError> {
    let facts = repo_facts(git, repo).await;
    // 取り込みで触れたファイル（`before..result`）。
    let files = if rec.before.is_empty() || rec.result.is_empty() { Vec::new() } else { super::artifacts::list_files(git, repo, &rec.before, &rec.result).await.unwrap_or_default() };
    let touched = touched_paths(&files);
    let dirty = dirty_paths(git, repo).await.unwrap_or_default();
    let overlap = dirty_overlap(&touched, &dirty);
    let remote = if state == OrgImportState::Applied { super::artifacts::remote_contains(git, repo, &rec.result).await } else { Some(false) };
    let partial: Vec<String> = if state == OrgImportState::Partial { overlap.clone() } else { Vec::new() };
    let decision = undo_decision(&UndoFacts {
        state,
        recorded_branch: &rec.branch,
        current_branch: facts.branch.as_deref(),
        head_now: facts.head.as_deref(),
        before: &rec.before,
        result: &rec.result,
        remote_contains: remote,
        dirty_overlap: if state == OrgImportState::Partial { &[] } else { &overlap },
        partial_files: &partial,
    });
    Ok((decision, facts.head))
}

#[derive(Debug, Clone, PartialEq)]
pub enum UndoExec {
    Refused(String),
    BeginFailed(String),
    Done(OrgUndoOutcome),
}

/// 取り込み前に戻す。`expected_head` は確認したときのHEAD。条件が外れていれば何もしない。
pub async fn undo_execute(git: &Git, repo: &Path, rec: &LedgerRecord, state: OrgImportState, expected_head: &str, begin: impl FnOnce() -> Result<(), String>) -> UndoExec {
    let facts = repo_facts(git, repo).await;
    if facts.branch.as_deref().is_some_and(|b| b == "HEAD" || b.starts_with(INTERNAL_BRANCH_PREFIX)) || facts.branch.is_none() {
        return UndoExec::Refused("作業フォルダの今のブランチを確認できない、または内部用のブランチのため、戻せません".to_string());
    }
    if let Some(op) = facts.in_progress {
        return UndoExec::Refused(format!("作業フォルダで{op}の途中のため、戻せません"));
    }
    if facts.head.as_deref() != Some(expected_head) {
        return UndoExec::Refused("状態が変わりました。もう一度確認してください".to_string());
    }
    let (decision, _) = match undo_probe(git, repo, rec, state).await {
        Ok(d) => d,
        Err(e) => return UndoExec::Refused(format!("状態を確認できないため、戻せません（{}）", e.message())),
    };
    if !decision.allowed {
        return UndoExec::Refused(decision.refusal.unwrap_or_else(|| "戻せません".to_string()));
    }
    if let Err(m) = begin() {
        return UndoExec::BeginFailed(m);
    }
    let op = match decision.mode {
        OrgUndoMode::Reset => GitOp::ResetKeep { commit: rec.before.clone() },
        OrgUndoMode::RestoreFiles => GitOp::RestorePaths { commit: rec.before.clone(), paths: decision.files.clone() },
    };
    let ran = git.run(repo, &op).await;
    let failure = match &ran {
        Ok(out) if out.success() => None,
        Ok(out) => Some(String::from_utf8_lossy(&out.stderr).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect::<String>()),
        Err(e) => Some(e.to_string()),
    };
    let head_after = head_now(git, repo).await;
    UndoExec::Done(match (&head_after, decision.mode) {
        (Some(h), OrgUndoMode::Reset) if *h == rec.before => OrgUndoOutcome::Undone { head: h.clone() },
        (Some(h), OrgUndoMode::RestoreFiles) if *h == rec.before && failure.is_none() => OrgUndoOutcome::Undone { head: h.clone() },
        (Some(h), _) if *h == expected_head => OrgUndoOutcome::NotUndone { reason: failure.filter(|s| !s.is_empty()).unwrap_or_else(|| "戻せませんでした".to_string()) },
        (Some(h), _) => OrgUndoOutcome::Unknown { head_now: crate::backend::model::Known::direct(h.clone()) },
        (None, _) => OrgUndoOutcome::Unknown { head_now: crate::backend::model::Known::NotFetched },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.base).ok();
        }
    }

    impl Rig {
        fn new() -> Rig {
            let t = std::env::temp_dir().join(format!("agentdock-import-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            std::fs::create_dir_all(&t).unwrap();
            let base = std::path::PathBuf::from(crate::gitops::org::resolved(&t).unwrap());
            let repo = base.join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            sh(&repo, &["init", "-q", "-b", "main"]);
            sh(&repo, &["config", "user.email", "t@example.com"]);
            sh(&repo, &["config", "user.name", "t"]);
            sh(&repo, &["config", "core.autocrlf", "false"]);
            std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
            std::fs::write(repo.join("keep.txt"), "keep\n").unwrap();
            sh(&repo, &["add", "."]);
            sh(&repo, &["commit", "-q", "-m", "init"]);
            Rig { base, repo }
        }

        /// `branch` を `from` から作り、worktree（置き場所は rig 内）で `edit` を行ってコミットする。コミットのIDを返す。
        fn writer(&self, name: &str, from: &str, edit: impl FnOnce(&Path)) -> String {
            let wt = self.base.join(format!("wt-{name}"));
            let branch = format!("agentdock/org/w1/{name}");
            sh(&self.repo, &["worktree", "add", "-q", "-b", &branch, &wt.to_string_lossy(), from]);
            edit(&wt);
            sh(&wt, &["add", "-A"]);
            sh(&wt, &["-c", "user.name=AgentDock", "-c", "user.email=agentdock@localhost", "commit", "-q", "-m", &format!("agentdock: {name}")]);
            sh(&wt, &["rev-parse", "HEAD"])
        }

        fn refs(&self) -> String {
            sh(&self.repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
        }

        fn status(&self) -> String {
            sh(&self.repo, &["status", "--porcelain=v1", "-uall"])
        }

        fn candidate(task: &str, commit: &str) -> Candidate {
            Candidate { task: task.into(), title: format!("{task} title"), commit: commit.into() }
        }
    }

    fn ready(o: PrepareOutcome) -> Prepared {
        match o {
            PrepareOutcome::Ready(p) => p,
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn import_composes_without_touching_the_folder_then_fast_forwards_once() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| {
            std::fs::write(wt.join("a.txt"), "one\ntwo changed\nthree\n").unwrap();
            std::fs::write(wt.join("new.md"), "# new\n").unwrap();
        });
        // 利用者のブランチは別の箇所を先に進めている（早送りではなく合成になる）。
        std::fs::write(r.repo.join("keep.txt"), "keep and more\n").unwrap();
        sh(&r.repo, &["commit", "-q", "-am", "user work"]);
        // 取り込み先に、重ならない未コミットの変更と未追跡ファイルがある。
        std::fs::write(r.repo.join("scratch.txt"), "mine\n").unwrap();
        let (refs, status, head) = (r.refs(), r.status(), sh(&r.repo, &["rev-parse", "HEAD"]));

        let facts = repo_facts(&git, &r.repo).await;
        assert_eq!((facts.branch.as_deref(), facts.head.as_deref(), facts.in_progress, facts.git_supported), (Some("main"), Some(head.as_str()), None, Some(true)));
        let p = ready(prepare(&git, &r.repo, &facts, "レポート", &[Rig::candidate("t1", &c1)]).await.unwrap());
        // 確認の段階では、作業フォルダ・索引・ブランチ・worktreeは何も変わらない。
        assert_eq!(r.refs(), refs, "no ref was written");
        assert_eq!(r.status(), status);
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
        assert!(!r.repo.join("new.md").exists());
        // 結果のコミット: 親は [HEAD, 候補]。変わるのは a.txt と new.md だけ。
        assert_eq!(sh(&r.repo, &["rev-list", "--parents", "-n", "1", &p.result]), format!("{} {head} {c1}", p.result));
        let mut names: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "new.md"]);
        assert!(sh(&r.repo, &["log", "-1", "--format=%B", &p.result]).contains("- t1 t1 title"));

        let check = check_worktree(&git, &r.repo, &p.files).await.unwrap();
        assert!(check.overlap.is_empty() && check.locked.is_empty());
        assert_eq!(check.dirty_total, 1, "scratch.txt stays");

        let mut began = false;
        let out = execute(&git, &r.repo, &p, || {
            began = true;
            Ok(())
        })
        .await;
        assert!(began);
        assert_eq!(out, ExecOutcome::Done(OrgImportOutcome::Applied { head: p.result.clone() }));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), p.result);
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\ntwo changed\nthree\n");
        assert_eq!(std::fs::read_to_string(r.repo.join("new.md")).unwrap(), "# new\n");
        assert_eq!(std::fs::read_to_string(r.repo.join("scratch.txt")).unwrap(), "mine\n", "an unrelated uncommitted file is left alone");
        // worktree とブランチは残る。
        assert!(r.base.join("wt-t1").is_dir());
        assert!(r.refs().contains("refs/heads/agentdock/org/w1/t1"));
        assert_eq!(sh(&r.repo, &["branch", "--show-current"]), "main");
    }

    #[tokio::test]
    async fn two_candidates_are_chained_and_land_in_one_fast_forward() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("one.txt"), "1\n").unwrap());
        let c2 = r.writer("t2", "main", |wt| std::fs::write(wt.join("two.txt"), "2\n").unwrap());
        let head = sh(&r.repo, &["rev-parse", "HEAD"]);
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1), Rig::candidate("t2", &c2)]).await.unwrap());
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), head);
        let out = execute(&git, &r.repo, &p, || Ok(())).await;
        assert!(matches!(out, ExecOutcome::Done(OrgImportOutcome::Applied { .. })));
        assert!(r.repo.join("one.txt").exists() && r.repo.join("two.txt").exists());
        // 取り込みコミットが候補ごとに1つ（合流コミット）。
        assert_eq!(sh(&r.repo, &["rev-list", "--merges", "--count", &format!("{head}..HEAD")]), "2");
    }

    #[tokio::test]
    async fn a_conflict_writes_nothing() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("a.txt"), "one\nTWO FROM WORKER\nthree\n").unwrap());
        std::fs::write(r.repo.join("a.txt"), "one\ntwo from user\nthree\n").unwrap();
        sh(&r.repo, &["commit", "-q", "-am", "user edit"]);
        let (refs, status, head) = (r.refs(), r.status(), sh(&r.repo, &["rev-parse", "HEAD"]));
        let facts = repo_facts(&git, &r.repo).await;
        let out = prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap();
        assert_eq!(out, PrepareOutcome::Conflict { files: vec!["a.txt".into()] });
        assert_eq!((r.refs(), r.status(), sh(&r.repo, &["rev-parse", "HEAD"])), (refs, status, head));
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\ntwo from user\nthree\n");
        assert!(sh(&r.repo, &["ls-files", "-u"]).is_empty(), "the index has no conflict entries");
    }

    #[tokio::test]
    async fn an_overlapping_uncommitted_edit_stops_the_import_and_changes_nothing() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| {
            std::fs::write(wt.join("a.txt"), "one\ntwo changed\nthree\n").unwrap();
            std::fs::write(wt.join("added.txt"), "x\n").unwrap();
        });
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        // 確認の後に、利用者が同じファイルを編集し、同名の未追跡ファイルも置く。
        std::fs::write(r.repo.join("a.txt"), "one\ntwo mine\nthree\n").unwrap();
        std::fs::write(r.repo.join("added.txt"), "mine too\n").unwrap();
        let (refs, status, head) = (r.refs(), r.status(), sh(&r.repo, &["rev-parse", "HEAD"]));
        let check = check_worktree(&git, &r.repo, &p.files).await.unwrap();
        assert_eq!(check.overlap.len(), 2);
        let mut began = false;
        let out = execute(&git, &r.repo, &p, || {
            began = true;
            Ok(())
        })
        .await;
        assert!(matches!(out, ExecOutcome::Refused(ref m) if m.contains("a.txt") && m.contains("added.txt")), "{out:?}");
        assert!(!began, "no ledger row for an import that never started");
        assert_eq!((r.refs(), r.status(), sh(&r.repo, &["rev-parse", "HEAD"])), (refs, status, head));
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\ntwo mine\nthree\n");
        assert_eq!(std::fs::read_to_string(r.repo.join("added.txt")).unwrap(), "mine too\n");
    }

    #[tokio::test]
    async fn an_ignored_file_that_the_import_would_overwrite_counts_as_an_overlap() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("out.log"), "from worker\n").unwrap());
        std::fs::write(r.repo.join(".git").join("info").join("exclude"), "*.log\n").unwrap();
        std::fs::write(r.repo.join("out.log"), "my ignored data\n").unwrap();
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        let check = check_worktree(&git, &r.repo, &p.files).await.unwrap();
        assert_eq!(check.overlap, ["out.log"]);
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Refused(_)));
        assert_eq!(std::fs::read_to_string(r.repo.join("out.log")).unwrap(), "my ignored data\n");
    }

    #[tokio::test]
    async fn a_moved_head_a_user_branch_switch_or_a_failed_ledger_write_stops_the_import() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("n.txt"), "n\n").unwrap());
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        let head = sh(&r.repo, &["rev-parse", "HEAD"]);

        // 台帳の begin を書けなければ実行しない。
        let out = execute(&git, &r.repo, &p, || Err("disk full".to_string())).await;
        assert_eq!(out, ExecOutcome::BeginFailed("disk full".into()));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), head);
        assert!(!r.repo.join("n.txt").exists());

        // HEAD が確認のあとに進んだ。
        std::fs::write(r.repo.join("other.txt"), "o\n").unwrap();
        sh(&r.repo, &["add", "other.txt"]);
        sh(&r.repo, &["commit", "-q", "-m", "later"]);
        let moved = sh(&r.repo, &["rev-parse", "HEAD"]);
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Refused(m) if m.contains("状態が変わりました")));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), moved);

        // AgentDock の内部ブランチが今のブランチなら取り込まない。
        let facts = repo_facts(&git, &r.repo).await;
        let p2 = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        sh(&r.repo, &["switch", "-q", "-c", "agentdock/mine"]);
        let before = sh(&r.repo, &["rev-parse", "HEAD"]);
        assert!(matches!(execute(&git, &r.repo, &p2, || Ok(())).await, ExecOutcome::Refused(_)));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), before);
        assert!(!r.repo.join("n.txt").exists());

        // detached も、途中のマージも取り込まない。
        sh(&r.repo, &["switch", "-q", "main"]);
        sh(&r.repo, &["switch", "-q", "--detach"]);
        assert!(matches!(execute(&git, &r.repo, &p2, || Ok(())).await, ExecOutcome::Refused(_)));
        sh(&r.repo, &["switch", "-q", "main"]);
        let gitdir = r.repo.join(".git");
        std::fs::write(gitdir.join("MERGE_HEAD"), format!("{moved}\n")).unwrap();
        assert!(matches!(execute(&git, &r.repo, &p2, || Ok(())).await, ExecOutcome::Refused(m) if m.contains("マージ")));
        std::fs::remove_file(gitdir.join("MERGE_HEAD")).unwrap();
        assert!(!r.repo.join("n.txt").exists());
    }

    #[tokio::test]
    async fn undo_resets_to_the_head_before_and_keeps_unrelated_edits_and_the_branch() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| {
            std::fs::write(wt.join("a.txt"), "one\ntwo changed\nthree\n").unwrap();
            std::fs::write(wt.join("new.md"), "# n\n").unwrap();
        });
        let facts = repo_facts(&git, &r.repo).await;
        let before = facts.head.clone().unwrap();
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Done(OrgImportOutcome::Applied { .. })));
        std::fs::write(r.repo.join("scratch.txt"), "mine\n").unwrap();

        let rec = LedgerRecord {
            id: "i1".into(),
            at: crate::backend::model::UnixMillis(1),
            folder: p.folder.clone(),
            branch: "main".into(),
            before: before.clone(),
            result: p.result.clone(),
            tasks: p.tasks.clone(),
            outcome: Some(OrgImportOutcome::Applied { head: p.result.clone() }),
            undo_begun: false,
            undo_outcome: None,
        };
        let (d, now) = undo_probe(&git, &r.repo, &rec, OrgImportState::Applied).await.unwrap();
        assert!(d.allowed && d.mode == OrgUndoMode::Reset, "{d:?}");
        assert_eq!(now.as_deref(), Some(p.result.as_str()));
        // 違うHEADを確認として渡したら何もしない。
        assert!(matches!(undo_execute(&git, &r.repo, &rec, OrgImportState::Applied, &before, || Ok(())).await, UndoExec::Refused(_)));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), p.result);
        // 台帳が書けなければ戻さない。
        assert!(matches!(undo_execute(&git, &r.repo, &rec, OrgImportState::Applied, &p.result, || Err("x".into())).await, UndoExec::BeginFailed(_)));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), p.result);

        let out = undo_execute(&git, &r.repo, &rec, OrgImportState::Applied, &p.result, || Ok(())).await;
        assert_eq!(out, UndoExec::Done(OrgUndoOutcome::Undone { head: before.clone() }));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), before);
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
        assert!(!r.repo.join("new.md").exists());
        assert_eq!(std::fs::read_to_string(r.repo.join("scratch.txt")).unwrap(), "mine\n");
        assert!(r.refs().contains("refs/heads/agentdock/org/w1/t1") && r.base.join("wt-t1").is_dir(), "the writer's branch and worktree stay");
    }

    #[tokio::test]
    async fn undo_is_refused_after_the_import_was_pushed_or_work_moved_on() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let remote = r.base.join("remote.git");
        sh(&r.base, &["init", "-q", "--bare", &remote.to_string_lossy()]);
        sh(&r.repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("n.txt"), "n\n").unwrap());
        let facts = repo_facts(&git, &r.repo).await;
        let before = facts.head.clone().unwrap();
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Done(OrgImportOutcome::Applied { .. })));
        let rec = LedgerRecord {
            id: "i1".into(),
            at: crate::backend::model::UnixMillis(1),
            folder: p.folder.clone(),
            branch: "main".into(),
            before,
            result: p.result.clone(),
            tasks: p.tasks.clone(),
            outcome: Some(OrgImportOutcome::Applied { head: p.result.clone() }),
            undo_begun: false,
            undo_outcome: None,
        };
        assert!(undo_probe(&git, &r.repo, &rec, OrgImportState::Applied).await.unwrap().0.allowed);
        // 送信（リモート追跡ブランチに含まれる）後は戻せない。
        sh(&r.repo, &["push", "-q", "origin", "main"]);
        let (d, _) = undo_probe(&git, &r.repo, &rec, OrgImportState::Applied).await.unwrap();
        assert!(!d.allowed && d.refusal.unwrap().contains("送信済み"));
        assert!(matches!(undo_execute(&git, &r.repo, &rec, OrgImportState::Applied, &p.result, || Ok(())).await, UndoExec::Refused(_)));
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), p.result);
    }

    #[tokio::test]
    async fn undo_is_refused_when_the_imported_files_have_uncommitted_edits() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("a.txt"), "one\ntwo changed\nthree\n").unwrap());
        let facts = repo_facts(&git, &r.repo).await;
        let before = facts.head.clone().unwrap();
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Done(_)));
        std::fs::write(r.repo.join("a.txt"), "edited after import\n").unwrap();
        let rec = LedgerRecord {
            id: "i1".into(),
            at: crate::backend::model::UnixMillis(1),
            folder: p.folder.clone(),
            branch: "main".into(),
            before,
            result: p.result.clone(),
            tasks: p.tasks.clone(),
            outcome: Some(OrgImportOutcome::Applied { head: p.result.clone() }),
            undo_begun: false,
            undo_outcome: None,
        };
        let (d, _) = undo_probe(&git, &r.repo, &rec, OrgImportState::Applied).await.unwrap();
        assert!(!d.allowed && d.refusal.unwrap().contains("a.txt"));
        assert_eq!(std::fs::read_to_string(r.repo.join("a.txt")).unwrap(), "edited after import\n");
    }

    #[tokio::test]
    async fn an_import_with_no_effect_on_the_files_is_reported_not_committed_as_a_change() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        // 候補は main と同じ内容（変更なしのコミットを許す）。
        let wt = r.base.join("wt-same");
        sh(&r.repo, &["worktree", "add", "-q", "-b", "agentdock/org/w1/same", &wt.to_string_lossy(), "main"]);
        sh(&wt, &["-c", "user.name=a", "-c", "user.email=a@b", "commit", "-q", "--allow-empty", "-m", "empty"]);
        let c = sh(&wt, &["rev-parse", "HEAD"]);
        let facts = repo_facts(&git, &r.repo).await;
        assert_eq!(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c)]).await.unwrap(), PrepareOutcome::NoChange);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_file_held_open_by_another_program_is_reported_as_locked() {
        if !git_ok() {
            return;
        }
        use std::os::windows::fs::OpenOptionsExt;
        let r = Rig::new();
        let git = Git::new(None);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("a.txt"), "one\ntwo changed\nthree\n").unwrap());
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        assert!(check_worktree(&git, &r.repo, &p.files).await.unwrap().locked.is_empty());
        // 他のアプリが開いている状態（書込み共有なし）を作る。
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(r.repo.join("a.txt")).unwrap();
        let check = check_worktree(&git, &r.repo, &p.files).await.unwrap();
        assert_eq!(check.locked, ["a.txt"]);
        let head = sh(&r.repo, &["rev-parse", "HEAD"]);
        let out = execute(&git, &r.repo, &p, || Ok(())).await;
        assert!(matches!(out, ExecOutcome::Refused(ref m) if m.contains("開かれている")), "{out:?}");
        assert_eq!(sh(&r.repo, &["rev-parse", "HEAD"]), head);
        drop(held);
        assert!(check_worktree(&git, &r.repo, &p.files).await.unwrap().locked.is_empty());
    }

    #[tokio::test]
    async fn artifact_reads_list_files_with_counts_moves_and_binaries_and_change_nothing() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let base = sh(&r.repo, &["rev-parse", "HEAD"]);
        let c1 = r.writer("t1", "main", |wt| {
            std::fs::write(wt.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
            std::fs::rename(wt.join("keep.txt"), wt.join("kept.txt")).unwrap();
            std::fs::write(wt.join("pic.png"), [0u8, 1, 2, 3, 0, 255]).unwrap();
            std::fs::write(wt.join("日本語.md"), "# 見出し\n").unwrap();
        });
        let (refs, status) = (r.refs(), r.status());
        let files = super::super::artifacts::list_files(&git, &r.repo, &base, &c1).await.unwrap();
        let by = |p: &str| files.iter().find(|f| f.path == p).unwrap_or_else(|| panic!("{p} in {files:?}"));
        assert_eq!((by("a.txt").additions.clone(), by("a.txt").deletions.clone()), (crate::backend::model::Known::direct(1), crate::backend::model::Known::direct(0)));
        assert_eq!(by("keep.txt").move_to.as_deref(), Some("kept.txt"));
        assert!(by("pic.png").binary && by("pic.png").additions == crate::backend::model::Known::Unsupported);
        assert_eq!(by("日本語.md").preview, OrgPreviewKind::Markdown);
        let t = totals(&files);
        assert_eq!((t.files, t.binary, t.unknown), (4, 1, 0));
        let d = super::super::artifacts::diff_text(&git, &r.repo, &base, &c1, "a.txt", None).await.unwrap();
        assert!(d.contains("+four"));
        let blob = super::super::artifacts::read_blob(&git, &r.repo, &c1, "日本語.md", 1024).await.unwrap().ok().unwrap();
        assert_eq!(decode_text(&blob.bytes, false).unwrap(), "# 見出し\n");
        assert!(super::super::artifacts::read_blob(&git, &r.repo, &c1, "日本語.md", 3).await.unwrap().is_err(), "over the cap is not read");
        assert!(super::super::artifacts::read_blob(&git, &r.repo, &c1, "missing.md", 1024).await.is_err());
        assert_eq!(super::super::artifacts::is_ancestor(&git, &r.repo, &base, &c1).await, Some(true));
        assert_eq!(super::super::artifacts::is_ancestor(&git, &r.repo, &c1, &base).await, Some(false));
        assert_eq!(super::super::artifacts::is_ancestor(&git, &r.repo, &c1, "no-such-ref").await, None);
        assert_eq!(super::super::artifacts::remote_contains(&git, &r.repo, &c1).await, Some(false));
        assert_eq!((r.refs(), r.status()), (refs, status), "reading changes nothing");
    }

    #[tokio::test]
    async fn the_import_runs_no_repository_hooks() {
        if !git_ok() {
            return;
        }
        let r = Rig::new();
        let git = Git::new(None);
        let marker = r.base.join("hook-ran");
        let hook = r.repo.join(".git").join("hooks").join("post-merge");
        std::fs::write(&hook, format!("#!/bin/sh\necho ran > \"{}\"\n", marker.to_string_lossy().replace('\\', "/"))).unwrap();
        // 前提: 普通のffマージではこの環境でもフックが走る。
        let probe = r.writer("probe", "main", |wt| std::fs::write(wt.join("p.txt"), "p
").unwrap());
        sh(&r.repo, &["merge", "-q", "--ff-only", &probe]);
        let hooks_run_here = marker.exists();
        let _ = std::fs::remove_file(&marker);
        let c1 = r.writer("t1", "main", |wt| std::fs::write(wt.join("n.txt"), "n
").unwrap());
        let facts = repo_facts(&git, &r.repo).await;
        let p = ready(prepare(&git, &r.repo, &facts, "wf", &[Rig::candidate("t1", &c1)]).await.unwrap());
        assert!(matches!(execute(&git, &r.repo, &p, || Ok(())).await, ExecOutcome::Done(OrgImportOutcome::Applied { .. })));
        assert!(!marker.exists(), "no hook ran for the import (hooks run in this environment: {hooks_run_here})");
    }
}
