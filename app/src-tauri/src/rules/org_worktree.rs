//! 組織実行（P-ORG-5）が使うworktreeの純粋ロジック。I/Oをしない。
//!
//! - 所有の確認: `WorktreeRecord.org` のある記録だけが対象。利用者が作ったworktree・利用者のブランチ・メインの作業ツリーには、
//!   コミットも削除もしない（拒否の理由は [`Refusal`]）。
//! - ブランチ名は `agentdock/org/<workflow>/<task>`。コミット間差分（`--name-status`）の解析。

use super::baseline::norm_path;
use super::worktree::{is_under, safe_branch_slug};
use crate::backend::model::{OrgWorktreeAccess, WorktreeRecord, WorktreeState};

pub const ORG_BRANCH_PREFIX: &str = "agentdock/org/";

/// 組織実行のWorkerのブランチ名 `agentdock/org/<workflow>/<task>`（各要素は安全化する）。
pub fn org_branch_name(workflow: &str, task: &str) -> String {
    format!("{ORG_BRANCH_PREFIX}{}/{}", safe_branch_slug(workflow), safe_branch_slug(task))
}

/// 組織実行がそのworktreeを操作できない理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 台帳にそのパスの記録がない。
    NoRecord,
    /// 記録はあるが組織実行の所有ではない（利用者が作ったworktree）。
    NotOrgOwned,
    /// 別のワークフロー・タスクの所有。
    OtherOwner,
    /// 読取り用（detached）のworktree。コミットしない。
    ReadOnly,
    /// 作成が完了していない記録。
    NotReady,
    /// パスがAgentDockの置き場所の外（リンク越えを含む）。
    OutsideRoot,
    /// ブランチが `agentdock/org/` ではない（利用者のブランチ）。
    BranchNotOrg,
    /// リポジトリ本体の作業ツリー。
    MainWorktree,
    /// Gitのworktree一覧にない。
    NotListed,
    /// Gitが示すブランチが記録と違う（別のブランチへ切り替えられている・detached）。
    BranchMismatch,
    /// フォルダが実在しない。
    PathMissing,
    /// worktreeの `.git` が、記録のgitdir（このworktreeの管理フォルダ）を指していない（別のgitdir・本体へ向けられている）。
    GitDirMismatch,
    /// 読取り用（detached）のworktreeの記録ではない。
    NotDetachedRecord,
}

impl Refusal {
    pub fn message(&self) -> &'static str {
        match self {
            Refusal::NoRecord => "AgentDockの記録にないフォルダのため、操作しません",
            Refusal::NotOrgOwned => "組織実行が作ったworktreeではないため、操作しません",
            Refusal::OtherOwner => "別のタスクのworktreeのため、操作しません",
            Refusal::ReadOnly => "読取り用のworktreeには、コミットしません",
            Refusal::NotReady => "worktreeの作成が完了していないため、操作しません",
            Refusal::OutsideRoot => "AgentDockの置き場所の外（またはリンク先が外）のため、操作しません",
            Refusal::BranchNotOrg => "組織実行のブランチ（agentdock/org/…）ではないため、操作しません",
            Refusal::MainWorktree => "リポジトリ本体の作業ツリーのため、操作しません",
            Refusal::NotListed => "Gitのworktree一覧にないため、操作しません",
            Refusal::BranchMismatch => "Gitが示すブランチが記録と違うため、操作しません",
            Refusal::PathMissing => "worktreeのフォルダが見つからないため、操作しません",
            Refusal::GitDirMismatch => "worktreeの.gitが、このworktreeの管理フォルダを指していないため、操作しません",
            Refusal::NotDetachedRecord => "読取り用（detached）のworktreeの記録ではないため、操作しません",
        }
    }
}

fn same_path(a: &str, b: &str) -> bool {
    norm_path(a).trim_end_matches('\\') == norm_path(b).trim_end_matches('\\')
}

/// 記録だけで判断できる拒否（Git・ファイルシステムの確認の前に通す）。通れば、その記録を返す。
/// コミットできるのは「このワークフロー・タスクの、書込み用で、作成済みの、置き場所配下で、`agentdock/org/` ブランチ」の記録だけ。
pub fn authorize_commit<'a>(records: &'a [WorktreeRecord], root: &str, path: &str, workflow: &str, task: &str) -> Result<&'a WorktreeRecord, Refusal> {
    let rec = records.iter().find(|r| same_path(&r.path, path)).ok_or(Refusal::NoRecord)?;
    let owner = rec.org.as_ref().ok_or(Refusal::NotOrgOwned)?;
    if owner.workflow_id != workflow || owner.task_id != task {
        return Err(Refusal::OtherOwner);
    }
    if owner.access != OrgWorktreeAccess::Writable {
        return Err(Refusal::ReadOnly);
    }
    if !matches!(rec.state, WorktreeState::Ready) {
        return Err(Refusal::NotReady);
    }
    if same_path(&rec.path, &rec.repo_root) {
        return Err(Refusal::MainWorktree);
    }
    if !is_under(root, &rec.path) || !is_under(root, path) {
        return Err(Refusal::OutsideRoot);
    }
    if !rec.branch.starts_with(ORG_BRANCH_PREFIX) || rec.branch.len() == ORG_BRANCH_PREFIX.len() {
        return Err(Refusal::BranchNotOrg);
    }
    Ok(rec)
}

/// 読取り用（detached）のworktreeを別のコミットへ動かせるか（再レビュー）。「このワークフロー・タスクの、読取り用で、作成済みの、置き場所配下」の記録だけ。
pub fn authorize_refresh<'a>(records: &'a [WorktreeRecord], root: &str, path: &str, workflow: &str, task: &str) -> Result<&'a WorktreeRecord, Refusal> {
    let rec = records.iter().find(|r| same_path(&r.path, path)).ok_or(Refusal::NoRecord)?;
    let owner = rec.org.as_ref().ok_or(Refusal::NotOrgOwned)?;
    if owner.workflow_id != workflow || owner.task_id != task {
        return Err(Refusal::OtherOwner);
    }
    if owner.access != OrgWorktreeAccess::DetachedReadOnly {
        return Err(Refusal::NotDetachedRecord);
    }
    if !matches!(rec.state, WorktreeState::Ready) {
        return Err(Refusal::NotReady);
    }
    if same_path(&rec.path, &rec.repo_root) {
        return Err(Refusal::MainWorktree);
    }
    if !is_under(root, &rec.path) || !is_under(root, path) {
        return Err(Refusal::OutsideRoot);
    }
    Ok(rec)
}

/// `git worktree` の管理フォルダ（`<共通gitdir>\worktrees\<名前>`）が、このworktreeのものか。
/// `gitdir`＝作業フォルダで `rev-parse --absolute-git-dir` が返した場所、`common_gitdir`＝本体リポジトリのgitdir、
/// `link`＝管理フォルダの `gitdir` ファイルの中身（このworktreeの `.git` ファイルのパス）。
pub fn admin_dir_matches(gitdir: &str, common_gitdir: &str, link: &str, worktree_path: &str) -> bool {
    let parent_ok = std::path::Path::new(gitdir)
        .parent()
        .is_some_and(|p| same_path(&p.to_string_lossy(), &format!("{}\\worktrees", common_gitdir.trim_end_matches(['/', '\\']))));
    parent_ok && same_path(link.trim(), &format!("{}\\.git", worktree_path.trim_end_matches(['/', '\\'])))
}

/// コミット間差分の1ファイル。`status` はGitの状態字（`A`・`M`・`D`・`R100` など）。名前変更・コピーは `old_path` がある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

/// `git diff --name-status` の出力を解析する。読めない行は捨てず、状態字を `?` にして行をそのまま `path` に入れる。
pub fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    out.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next().unwrap_or("");
            let first = parts.next();
            let second = parts.next();
            match (status.chars().next(), first, second) {
                (Some('R' | 'C'), Some(old), Some(new)) => ChangedFile { status: status.to_string(), path: new.to_string(), old_path: Some(old.to_string()) },
                (Some(_), Some(p), None) => ChangedFile { status: status.to_string(), path: p.to_string(), old_path: None },
                _ => ChangedFile { status: "?".to_string(), path: line.to_string(), old_path: None },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{LocalId, OrgWorktreeOwner, UnixMillis};

    const ROOT: &str = "C:\\data\\worktrees";

    fn rec(path: &str, branch: &str, org: Option<(&str, &str, OrgWorktreeAccess)>) -> WorktreeRecord {
        WorktreeRecord {
            id: LocalId("wt1".into()),
            repo_root: "C:\\repo".into(),
            path: path.into(),
            branch: branch.into(),
            base_commit: "abc".into(),
            created_at: UnixMillis(0),
            created_for_chat: None,
            state: WorktreeState::Ready,
            org: org.map(|(w, t, a)| OrgWorktreeOwner { workflow_id: w.into(), task_id: t.into(), access: a }),
        }
    }

    fn mine(path: &str) -> WorktreeRecord {
        rec(path, "agentdock/org/w1/t1", Some(("w1", "t1", OrgWorktreeAccess::Writable)))
    }

    #[test]
    fn branch_name_is_under_the_org_prefix_and_safe() {
        assert_eq!(org_branch_name("W 1", "T/1"), "agentdock/org/w-1/t-1");
        assert!(org_branch_name("..", "-x").starts_with(ORG_BRANCH_PREFIX));
        assert!(!org_branch_name("..", "-x").contains(".."));
    }

    #[test]
    fn commit_is_allowed_only_for_the_owned_writable_ready_org_branch_worktree() {
        let p = "C:\\data\\worktrees\\repo-aaaaaa";
        let ok = vec![mine(p)];
        assert!(authorize_commit(&ok, ROOT, p, "w1", "t1").is_ok());
        assert!(authorize_commit(&ok, ROOT, "c:/data/worktrees/REPO-aaaaaa/", "w1", "t1").is_ok());
    }

    #[test]
    fn commit_is_refused_without_an_owner_record_or_for_users_branches_and_trees() {
        let p = "C:\\data\\worktrees\\repo-aaaaaa";
        let refuse = |recs: Vec<WorktreeRecord>, path: &str| authorize_commit(&recs, ROOT, path, "w1", "t1").unwrap_err();
        // 記録なし
        assert_eq!(refuse(vec![], p), Refusal::NoRecord);
        assert_eq!(refuse(vec![mine(p)], "C:\\other"), Refusal::NoRecord);
        // 利用者が作ったworktree（所有記録なし）
        assert_eq!(refuse(vec![rec(p, "agentdock/org/w1/t1", None)], p), Refusal::NotOrgOwned);
        // 利用者のブランチ
        assert_eq!(refuse(vec![rec(p, "main", Some(("w1", "t1", OrgWorktreeAccess::Writable)))], p), Refusal::BranchNotOrg);
        assert_eq!(refuse(vec![rec(p, "agentdock/feature-x", Some(("w1", "t1", OrgWorktreeAccess::Writable)))], p), Refusal::BranchNotOrg);
        assert_eq!(refuse(vec![rec(p, "", Some(("w1", "t1", OrgWorktreeAccess::Writable)))], p), Refusal::BranchNotOrg);
        // 利用者の作業ツリー（リポジトリ本体）
        let mut main = mine("C:\\repo");
        main.repo_root = "C:\\repo".into();
        assert_eq!(refuse(vec![main], "C:\\repo"), Refusal::MainWorktree);
        // 置き場所の外
        assert_eq!(refuse(vec![mine("C:\\elsewhere\\x")], "C:\\elsewhere\\x"), Refusal::OutsideRoot);
        assert_eq!(refuse(vec![mine("C:\\data\\worktrees\\..\\x")], "C:\\data\\worktrees\\..\\x"), Refusal::OutsideRoot);
        // 別タスク・読取り用・未完成
        assert_eq!(refuse(vec![rec(p, "agentdock/org/w1/t2", Some(("w1", "t2", OrgWorktreeAccess::Writable)))], p), Refusal::OtherOwner);
        assert_eq!(refuse(vec![rec(p, "", Some(("w1", "t1", OrgWorktreeAccess::DetachedReadOnly)))], p), Refusal::ReadOnly);
        let mut creating = mine(p);
        creating.state = WorktreeState::Creating;
        assert_eq!(refuse(vec![creating], p), Refusal::NotReady);
    }

    #[test]
    fn refresh_is_allowed_only_for_the_owned_detached_ready_worktree() {
        let p = "C:\\data\\worktrees\\repo-bbbbbb";
        let det = |access, state| {
            let mut r = rec(p, "", Some(("w1", "t1", access)));
            r.state = state;
            r
        };
        let refuse = |r: WorktreeRecord, path: &str| authorize_refresh(&[r], ROOT, path, "w1", "t1").unwrap_err();
        assert!(authorize_refresh(&[det(OrgWorktreeAccess::DetachedReadOnly, WorktreeState::Ready)], ROOT, p, "w1", "t1").is_ok());
        assert_eq!(refuse(det(OrgWorktreeAccess::Writable, WorktreeState::Ready), p), Refusal::NotDetachedRecord);
        assert_eq!(refuse(det(OrgWorktreeAccess::DetachedReadOnly, WorktreeState::Creating), p), Refusal::NotReady);
        assert_eq!(refuse(det(OrgWorktreeAccess::DetachedReadOnly, WorktreeState::Ready), "C:\\other"), Refusal::NoRecord);
        assert_eq!(authorize_refresh(&[det(OrgWorktreeAccess::DetachedReadOnly, WorktreeState::Ready)], ROOT, p, "w1", "t2").unwrap_err(), Refusal::OtherOwner);
        assert_eq!(refuse(rec(p, "", None), p), Refusal::NotOrgOwned);
    }

    #[test]
    fn the_admin_dir_must_be_this_worktrees_own_entry_under_the_common_gitdir() {
        let wt = "C:/data/worktrees/repo-aaaaaa";
        let ok = |gitdir: &str, common: &str, link: &str| admin_dir_matches(gitdir, common, link, wt);
        assert!(ok("C:/repo/.git/worktrees/repo-aaaaaa", "C:/repo/.git", "C:/data/worktrees/repo-aaaaaa/.git\n"));
        assert!(ok("c:/REPO/.git/worktrees/x1", "C:\\repo\\.git", "C:\\data\\worktrees\\repo-aaaaaa\\.git"));
        // pointed at the repository itself, at another entry, or at an unrelated repository
        assert!(!ok("C:/repo/.git", "C:/repo/.git", "C:/data/worktrees/repo-aaaaaa/.git"));
        assert!(!ok("C:/repo/.git/worktrees/other", "C:/repo/.git", "C:/data/worktrees/other/.git"));
        assert!(!ok("C:/evil/.git/worktrees/x", "C:/repo/.git", "C:/data/worktrees/repo-aaaaaa/.git"));
        assert!(!ok("C:/repo/.git/worktrees/x", "C:/repo/.git", ""));
    }

    #[test]
    fn name_status_is_parsed_without_dropping_unreadable_lines() {
        let out = "M\ta.rs\nA\tdir/b.rs\nD\tc.rs\nR100\told.rs\tnew.rs\nweird line\n";
        let v = parse_name_status(out);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], ChangedFile { status: "M".into(), path: "a.rs".into(), old_path: None });
        assert_eq!(v[3], ChangedFile { status: "R100".into(), path: "new.rs".into(), old_path: Some("old.rs".into()) });
        assert_eq!(v[4].status, "?");
        assert!(parse_name_status("").is_empty());
    }
}
