use super::*;
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("agentdock-push-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git_cmd(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "core.autocrlf=false"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git_cmd(dir, &["add", "-A"]);
    git_cmd(dir, &["commit", "-q", "-m", &format!("add {file}")]);
    git_cmd(dir, &["rev-parse", "HEAD"])
}

/// リモート（bare）と、そこへ向けたローカル（`main`、コミット1つ、追跡先なし）。
fn setup(name: &str) -> (PathBuf, PathBuf) {
    let bare = temp(&format!("{name}-bare"));
    git_cmd(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let local = temp(name);
    git_cmd(&local, &["init", "-q", "-b", "main"]);
    commit(&local, "a.txt", "1");
    git_cmd(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
    (local, bare)
}

#[tokio::test]
async fn preview_lists_the_commits_to_send_and_blocks_without_a_remote() {
    let git = Git::new(None);
    let (local, _bare) = setup("preview");
    let sha = commit(&local, "b.txt", "2");
    std::fs::write(local.join("dirty.txt"), "x").unwrap();
    let p = preview(&git, &local, None).await;
    assert_eq!((p.branch.as_deref(), p.remote.as_deref(), p.upstream.as_deref()), (Some("main"), Some("origin"), None));
    assert_eq!(p.head_sha.as_deref(), Some(sha.as_str()));
    assert_eq!((p.commit_total, p.commits.len(), p.dirty_count), (2, 2, 1));
    assert!(p.blocker.is_none(), "{:?}", p.blocker);
    // no remote
    let none = temp("noremote");
    git_cmd(&none, &["init", "-q", "-b", "main"]);
    commit(&none, "a.txt", "1");
    assert_eq!(preview(&git, &none, None).await.blocker.map(|b| b.kind), Some(PushBlocker::NoRemote));
    // an agentdock/ branch and a detached HEAD cannot be sent
    git_cmd(&local, &["checkout", "-q", "-b", "agentdock/org/x"]);
    assert_eq!(preview(&git, &local, None).await.blocker.map(|b| b.kind), Some(PushBlocker::InternalBranch));
    git_cmd(&local, &["checkout", "-q", "--detach"]);
    assert_eq!(preview(&git, &local, None).await.blocker.map(|b| b.kind), Some(PushBlocker::Detached));
}

#[tokio::test]
async fn push_sends_only_the_confirmed_commit_and_can_set_the_upstream() {
    let git = Git::new(None);
    let (local, bare) = setup("send");
    let confirmed = git_cmd(&local, &["rev-parse", "HEAD"]);
    // a newer commit made after the confirmation must not be sent: the push is refused until re-confirmed
    let newer = commit(&local, "b.txt", "2");
    let r = push(&git, &local, "origin", "main", &confirmed, false).await;
    assert!(r.unwrap_err().contains("変わっています"));
    let out = std::process::Command::new("git").current_dir(&bare).args(["rev-parse", "--verify", "--quiet", "refs/heads/main"]).output().unwrap();
    assert!(!out.status.success(), "nothing must have been sent");
    let r = push(&git, &local, "origin", "main", &newer, true).await.unwrap();
    assert_eq!(r, PushResult::Pushed { sha: newer.clone(), upstream_note: None });
    assert_eq!(git_cmd(&bare, &["rev-parse", "refs/heads/main"]), newer);
    assert_eq!(git_cmd(&local, &["rev-parse", "--abbrev-ref", "main@{u}"]), "origin/main");
    assert_eq!(verify(&git, &local, "origin", "main", &newer).await, PushVerify::Confirmed);
    assert_eq!(verify(&git, &local, "origin", "main", &confirmed).await, PushVerify::NotThere);
    assert_eq!(verify(&git, &local, "origin", "other", &newer).await, PushVerify::NotThere);
    // nothing new to send after pushing
    assert_eq!(preview(&git, &local, None).await.blocker.map(|b| b.kind), Some(PushBlocker::NothingToPush));
}

#[tokio::test]
async fn a_remote_that_moved_ahead_is_a_non_fast_forward_rejection_and_nothing_is_forced() {
    let git = Git::new(None);
    let (local, bare) = setup("nff");
    let first = git_cmd(&local, &["rev-parse", "HEAD"]);
    assert!(matches!(push(&git, &local, "origin", "main", &first, false).await.unwrap(), PushResult::Pushed { .. }));
    // another clone pushes a commit
    let other = temp("nff-other");
    git_cmd(&other, &["clone", "-q", &bare.to_string_lossy(), "."]);
    let theirs = commit(&other, "theirs.txt", "t");
    git_cmd(&other, &["push", "-q", "origin", "main"]);
    let mine = commit(&local, "mine.txt", "m");
    let r = push(&git, &local, "origin", "main", &mine, false).await.unwrap();
    assert_eq!(r, PushResult::Rejected { reason: PushRejectReason::NonFastForward });
    // the remote still has the other clone's commit
    assert_eq!(git_cmd(&bare, &["rev-parse", "refs/heads/main"]), theirs);
}

#[tokio::test]
async fn a_missing_remote_name_and_a_changed_branch_send_nothing() {
    let git = Git::new(None);
    let (local, _bare) = setup("guards");
    let sha = git_cmd(&local, &["rev-parse", "HEAD"]);
    assert!(push(&git, &local, "nope", "main", &sha, false).await.unwrap_err().contains("見つかりません"));
    assert!(push(&git, &local, "origin", "other", &sha, false).await.unwrap_err().contains("ブランチ"));
    assert!(push(&git, &local, "origin", "agentdock/x", &sha, false).await.is_err());
    assert!(push(&git, &local, "--mirror", "main", &sha, false).await.is_err());
}

#[test]
fn push_args_never_contain_force_or_wide_options() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let a = GitOp::Push { remote: "origin".into(), branch: "main".into(), sha: sha.into() }.args().unwrap();
    assert_eq!(a, ["push", "--porcelain", "--", "origin", &format!("{sha}:refs/heads/main")]);
    for forbidden in ["--force", "-f", "--force-with-lease", "--mirror", "--all", "--tags", "--delete", "-d", "--prune"] {
        assert!(!a.iter().any(|x| x == forbidden), "{forbidden}");
    }
    assert!(!a.iter().any(|x| x.starts_with('+')));
    // refspecs that would force, delete, or push a different source cannot be built
    assert!(GitOp::Push { remote: "origin".into(), branch: "+main".into(), sha: sha.into() }.args().is_err());
    assert!(GitOp::Push { remote: "origin".into(), branch: "".into(), sha: sha.into() }.args().is_err());
    assert!(GitOp::Push { remote: "origin".into(), branch: "main".into(), sha: "HEAD".into() }.args().is_err());
    assert!(GitOp::Push { remote: "origin".into(), branch: "main".into(), sha: format!("+{sha}") }.args().is_err());
    assert!(GitOp::Push { remote: "--mirror".into(), branch: "main".into(), sha: sha.into() }.args().is_err());
    assert!(GitOp::Push { remote: "https://x/y".into(), branch: "main".into(), sha: sha.into() }.args().is_err());
    assert!(GitOp::Push { remote: "origin".into(), branch: "agentdock/org/x".into(), sha: sha.into() }.args().is_err());
}

#[tokio::test]
async fn the_commit_list_follows_the_real_target_when_the_upstream_has_another_name() {
    let git = Git::new(None);
    let (local, bare) = setup("othername");
    let first = git_cmd(&local, &["rev-parse", "HEAD"]);
    assert!(matches!(push(&git, &local, "origin", "main", &first, false).await.unwrap(), PushResult::Pushed { .. }));
    // a local branch that tracks origin/main but would be sent to origin/feature
    git_cmd(&local, &["checkout", "-q", "-b", "feature"]);
    git_cmd(&local, &["branch", "--set-upstream-to=origin/main", "feature"]);
    commit(&local, "f.txt", "f");
    let p = preview(&git, &local, None).await;
    assert_eq!((p.branch.as_deref(), p.upstream.as_deref()), (Some("feature"), Some("origin/main")));
    // origin/feature does not exist; the base is "not on any origin branch", so only the new commit is listed
    assert_eq!(p.commit_total, 1);
    let _ = bare;
}
