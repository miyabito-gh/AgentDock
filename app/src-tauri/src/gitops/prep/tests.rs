use super::*;

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("agentdock-prep-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git_cmd(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.com"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn git_text(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// フォルダの中身（`.git` を除く）の、パスと内容の組。作業フォルダが変わっていないことの確認用。
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn work_folder(name: &str) -> (PathBuf, PathBuf) {
    let folder = temp(name);
    std::fs::write(folder.join("a.txt"), "alpha").unwrap();
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("sub").join("b.docx"), "beta").unwrap();
    (folder, temp(&format!("{name}-tmp")))
}

async fn token_of(git: &Git, folder: &Path, tmp: &Path) -> String {
    preview(git, folder, tmp, None, false).await.unwrap().preview_token
}

fn spec<'a>(folder: &'a Path, tmp: &'a Path, token: &'a str, apply: bool) -> PrepareSpec<'a> {
    PrepareSpec { folder, tmp_base: tmp, expected_token: token, apply_excludes: apply, folder_refusal: None, prep_id: "prep-1".into(), now_ms: 1 }
}

#[tokio::test]
async fn status_distinguishes_not_repo_no_commit_ready_and_nested() {
    let git = Git::new(None);
    let (folder, _) = work_folder("status");
    assert_eq!(repo_status(&git, &folder).await, RepoStatus::NotRepo);
    git_cmd(&folder, &["init", "-q"]);
    assert!(matches!(repo_status(&git, &folder).await, RepoStatus::NoCommit { .. }));
    git_cmd(&folder, &["add", "-A"]);
    git_cmd(&folder, &["commit", "-q", "-m", "first"]);
    assert!(matches!(repo_status(&git, &folder).await, RepoStatus::Ready { .. }));
    assert!(matches!(repo_status(&git, &folder.join("sub")).await, RepoStatus::Nested { .. }));
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn preparing_a_plain_folder_creates_only_dot_git_and_changes_nothing_else() {
    let git = Git::new(None);
    let (folder, tmp) = work_folder("create");
    let before = snapshot(&folder);
    let pv = preview(&git, &folder, &tmp, None, false).await.unwrap();
    assert_eq!((pv.kind, pv.file_count), (PrepKind::NotRepo, 2));
    assert!(pv.refusal.is_none() && pv.suggested_excludes.is_empty());
    // the preview itself wrote nothing into the folder and left no temporary gitdir behind
    assert!(!folder.join(".git").exists());
    assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
    let out = prepare(&git, spec(&folder, &tmp, &pv.preview_token, false)).await;
    let PrepOutcome::Prepared { commit } = out else { panic!("{out:?}") };
    assert_eq!(commit.len(), 40);
    // only `.git` was added: no .gitignore, other files byte-identical
    assert!(!folder.join(".gitignore").exists());
    assert_eq!(snapshot(&folder), before);
    let entries: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(entries.contains(&".git".to_string()) && entries.len() == 3, "{entries:?}");
    assert_eq!(git_text(&folder, &["ls-files"]).lines().count(), 2);
    assert_eq!(git_text(&folder, &["log", "-1", "--format=%s"]).trim(), FIRST_COMMIT_MESSAGE);
    assert!(matches!(repo_status(&git, &folder).await, RepoStatus::Ready { .. }));
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn a_changed_folder_after_the_preview_runs_nothing() {
    let git = Git::new(None);
    let (folder, tmp) = work_folder("token");
    let token = token_of(&git, &folder, &tmp).await;
    std::fs::write(folder.join("c.txt"), "new").unwrap();
    let out = prepare(&git, spec(&folder, &tmp, &token, false)).await;
    assert!(matches!(&out, PrepOutcome::Failed { reason, leftover: None } if reason.contains("変わりました")), "{out:?}");
    assert!(!folder.join(".git").exists());
    // an empty token never matches
    assert!(matches!(prepare(&git, spec(&folder, &tmp, "", false)).await, PrepOutcome::Failed { .. }));
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn a_repo_that_already_has_a_commit_is_not_preparable() {
    let git = Git::new(None);
    let (folder, tmp) = work_folder("ready");
    git_cmd(&folder, &["init", "-q"]);
    git_cmd(&folder, &["add", "-A"]);
    git_cmd(&folder, &["commit", "-q", "-m", "first"]);
    assert!(preview(&git, &folder, &tmp, None, false).await.unwrap_err().contains("すでにGitのリポジトリ"));
    let out = prepare(&git, spec(&folder, &tmp, "x", false)).await;
    assert!(matches!(out, PrepOutcome::Failed { .. }));
    let _ = std::fs::remove_dir_all(&folder);
}

#[cfg(windows)]
#[tokio::test]
async fn a_clear_failure_removes_only_the_git_dir_this_prepare_created() {
    use std::os::windows::fs::OpenOptionsExt;
    let git = Git::new(None);
    let (folder, tmp) = work_folder("rollback");
    let token = token_of(&git, &folder, &tmp).await;
    let before = snapshot(&folder);
    // an exclusively opened file cannot be read by `git add`
    let locked = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(folder.join("a.txt")).unwrap();
    let out = prepare(&git, spec(&folder, &tmp, &token, false)).await;
    drop(locked);
    assert!(matches!(&out, PrepOutcome::RolledBack { reason } if reason.contains("元に戻しました")), "{out:?}");
    assert!(!folder.join(".git").exists());
    assert_eq!(snapshot(&folder), before);
    let _ = std::fs::remove_dir_all(&folder);
}

#[cfg(windows)]
#[tokio::test]
async fn an_existing_repository_is_never_removed_even_when_the_commit_fails() {
    use std::os::windows::fs::OpenOptionsExt;
    let git = Git::new(None);
    let (folder, tmp) = work_folder("keep");
    git_cmd(&folder, &["init", "-q"]);
    let pv = preview(&git, &folder, &tmp, None, false).await.unwrap();
    assert_eq!(pv.kind, PrepKind::NoCommit);
    let locked = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(folder.join("a.txt")).unwrap();
    let out = prepare(&git, spec(&folder, &tmp, &pv.preview_token, false)).await;
    drop(locked);
    assert!(matches!(&out, PrepOutcome::Failed { reason, leftover: None } if reason.contains("既存のリポジトリ")), "{out:?}");
    assert!(folder.join(".git").is_dir());
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn excludes_are_not_written_by_default_and_append_only_when_chosen() {
    let git = Git::new(None);
    let (folder, tmp) = work_folder("exclude");
    std::fs::write(folder.join("Thumbs.db"), "x").unwrap();
    let big = std::fs::File::create(folder.join("big.bin")).unwrap();
    big.set_len(BIG_FILE_BYTES + 1).unwrap();
    drop(big);
    // default: suggested only, everything is committed, and no exclude block is written
    let pv = preview(&git, &folder, &tmp, None, false).await.unwrap();
    assert_eq!(pv.suggested_excludes.len(), 2);
    assert_eq!(pv.file_count, 4);
    assert_eq!(pv.exclude_lines, vec!["Thumbs.db", "/big.bin"]);
    // choosing it changes what is committed (and the shown counts)
    let pv2 = preview(&git, &folder, &tmp, None, true).await.unwrap();
    assert_eq!(pv2.file_count, 2);
    assert_eq!(pv2.preview_token, pv.preview_token, "the fingerprint covers the files, not the choice");
    let out = prepare(&git, spec(&folder, &tmp, &pv2.preview_token, true)).await;
    assert!(matches!(out, PrepOutcome::Prepared { .. }), "{out:?}");
    let exclude = std::fs::read_to_string(folder.join(".git").join("info").join("exclude")).unwrap();
    assert!(exclude.contains("# AgentDock: 最初のコミットから外したファイル\nThumbs.db\n/big.bin\n"), "{exclude}");
    // git's own template text is kept (append only)
    assert!(exclude.starts_with("# git ls-files") || exclude.contains("exclude patterns"), "{exclude}");
    let tracked = git_text(&folder, &["ls-files"]);
    assert!(!tracked.contains("big.bin") && !tracked.contains("Thumbs.db") && tracked.contains("a.txt"));
    assert!(!folder.join(".gitignore").exists());
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn a_folder_with_an_inner_repository_is_refused() {
    let git = Git::new(None);
    let (folder, tmp) = work_folder("nested");
    let inner = folder.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("f.txt"), "x").unwrap();
    git_cmd(&inner, &["init", "-q"]);
    let pv = preview(&git, &folder, &tmp, None, false).await.unwrap();
    assert!(pv.refusal.as_deref().is_some_and(|r| r.contains("別のGitリポジトリ")), "{:?}", pv.refusal);
    let _ = std::fs::remove_dir_all(&folder);
}

#[tokio::test]
async fn an_interrupted_prep_is_detected_and_only_a_marked_git_dir_is_removed() {
    let git = Git::new(None);
    let (folder, _tmp) = work_folder("interrupted");
    git_cmd(&folder, &["init", "-q"]);
    // no mark: a user's repository, not ours
    assert!(remove_interrupted(&git, &folder).await.is_err());
    std::fs::write(folder.join(".git").join(MARKER_FILE), r#"{"prepId":"p","at":1}"#).unwrap();
    assert!(matches!(repo_status(&git, &folder).await, RepoStatus::PrepInterrupted { .. }));
    remove_interrupted(&git, &folder).await.unwrap();
    assert!(!folder.join(".git").exists());
    assert_eq!(snapshot(&folder).len(), 2);
    let _ = std::fs::remove_dir_all(&folder);
}
