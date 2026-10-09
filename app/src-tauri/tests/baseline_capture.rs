//! 控えの取得の結合テスト（P3B-3）。別プロセスで動くので、環境変数（GIT_DIR など）をここで設定しても他のテストに影響しない。
//! gitがなければ何もしない。

use std::path::{Path, PathBuf};
use std::process::Command;

use agentdock_lib::backend::baseline::BaselineFailure;
use agentdock_lib::gitops::snapshot::{SnapshotEnv, SnapshotOp};
use agentdock_lib::gitops::Git;
use agentdock_lib::host::baseline_snap::{capture, probe_repo, RepoProbe};

fn git_ok() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// リポジトリ側の状態の写し（索引のバイト列・refs・stash・reflog・objectsのファイル一覧）。
fn repo_state(repo: &Path) -> (Vec<u8>, String, Vec<String>) {
    let index = std::fs::read(repo.join(".git").join("index")).unwrap();
    let text: String = [vec!["for-each-ref"], vec!["stash", "list"], vec!["reflog"]]
        .iter()
        .map(|a| String::from_utf8_lossy(&Command::new("git").current_dir(repo).args(a).output().unwrap().stdout).into_owned())
        .collect::<Vec<_>>()
        .join("|");
    fn walk(d: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                walk(&e.path(), out);
            } else {
                out.push(e.path().to_string_lossy().into_owned());
            }
        }
    }
    let mut files = Vec::new();
    walk(&repo.join(".git").join("objects"), &mut files);
    files.sort();
    (index, text, files)
}

fn temp_base(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agentdock-bcap-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
}

/// 控えの木から、パスの内容（生バイト）を取り出す。
async fn blob_at(git: &Git, probe: &RepoProbe, objects: &Path, tmp: &Path, tree: &str, path: &str) -> Option<Vec<u8>> {
    let env = SnapshotEnv::new(objects.to_path_buf(), probe.info.objects_dir(), tmp.join("read-idx")).unwrap();
    let ls = git.run_snapshot(&probe.info.toplevel, &env, &SnapshotOp::LsTree { tree: tree.into(), paths: vec![path.into()] }).await.unwrap();
    let text = ls.stdout_text();
    let meta = text.split('\t').next().filter(|m| !m.is_empty())?;
    let oid = meta.split_whitespace().nth(2)?.to_string();
    Some(git.run_snapshot(&probe.info.toplevel, &env, &SnapshotOp::CatFileBlob { oid }).await.unwrap().stdout)
}

#[tokio::test]
async fn capture_records_the_working_state_without_touching_the_repository_even_with_inherited_git_vars() {
    if !git_ok() {
        return;
    }
    let base = temp_base("cap");
    let repo = base.join("repo");
    let other = base.join("other");
    let area = base.join("area");
    for d in [&repo, &other, &area.join("objects"), &area.join("tmp")] {
        std::fs::create_dir_all(d).unwrap();
    }
    sh(&repo, &["init", "-q"]);
    sh(&other, &["init", "-q"]);
    for (k, v) in [("user.email", "t@example.com"), ("user.name", "t"), ("core.autocrlf", "true")] {
        sh(&repo, &["config", k, v]);
    }
    std::fs::write(repo.join(".gitignore"), b"ignored.txt\n").unwrap();
    std::fs::write(repo.join("a.txt"), b"one\ntwo\n").unwrap();
    std::fs::write(repo.join("keep.txt"), b"same\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-q", "-m", "init"]);
    // 作業ツリー: CRLFの変更・未追跡・削除・無視ファイル。
    std::fs::write(repo.join("a.txt"), b"one\r\nTWO\r\n").unwrap();
    std::fs::write(repo.join("new.txt"), b"fresh\r\n").unwrap();
    std::fs::write(repo.join("ignored.txt"), b"nope").unwrap();
    std::fs::remove_file(repo.join("keep.txt")).unwrap();
    let before = repo_state(&repo);

    // ユーザー環境の変数が別のリポジトリ・場所を指していても、影響されない（この後のgit呼出しはすべてAgentDock側）。
    std::env::set_var("GIT_DIR", other.join(".git"));
    std::env::set_var("GIT_WORK_TREE", &other);
    std::env::set_var("GIT_INDEX_FILE", other.join("bogus-index"));
    std::env::set_var("GIT_OBJECT_DIRECTORY", other.join("bogus-objects"));
    std::env::set_var("GIT_ALTERNATE_OBJECT_DIRECTORIES", other.join("bogus-alt"));
    std::env::set_var("GIT_COMMON_DIR", other.join(".git"));
    std::env::set_var("GIT_NAMESPACE", "bogus");
    std::env::set_var("GIT_PREFIX", "bogus/");

    let git = Git::new(None);
    let probe = match probe_repo(&git, &repo).await {
        Err(BaselineFailure::GitTooOld { .. }) => return,
        other => other.expect("probe"),
    };
    assert!(probe.repo_ref().root.to_lowercase().ends_with("/repo"), "the repository found is the one asked for, not GIT_DIR's: {}", probe.repo_ref().root);
    let (objects, tmp) = (area.join("objects"), area.join("tmp"));
    let space = |_: u64| Ok(());
    let snap = capture(&git, &probe, &objects, &tmp, &area, &space).await.expect("capture");

    assert!(snap.head.is_some() && snap.branch.is_some());
    assert_eq!(snap.raw, ["a.txt", "new.txt"], "only the files that differ from HEAD are taken raw; ignored files are not");
    assert!(snap.skipped.is_empty());
    // 生バイトのまま（CRLFを保つ）取り出せ、削除・無視ファイルは木に入らない。
    assert_eq!(blob_at(&git, &probe, &objects, &tmp, &snap.tree, "a.txt").await.unwrap(), b"one\r\nTWO\r\n");
    assert_eq!(blob_at(&git, &probe, &objects, &tmp, &snap.tree, "new.txt").await.unwrap(), b"fresh\r\n");
    assert!(blob_at(&git, &probe, &objects, &tmp, &snap.tree, "keep.txt").await.is_none());
    assert!(blob_at(&git, &probe, &objects, &tmp, &snap.tree, "ignored.txt").await.is_none());
    // 一時インデックスは残らない。専用置き場には書かれる。
    assert_eq!(std::fs::read_dir(&tmp).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("idx-")).count(), 0);
    assert!(objects.read_dir().unwrap().count() > 0);
    assert!(!other.join("bogus-index").exists() && !other.join("bogus-objects").exists());
    // 変数を戻してから、ユーザーのリポジトリが何も変わっていないことを確かめる。
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR", "GIT_NAMESPACE", "GIT_PREFIX"] {
        std::env::remove_var(k);
    }
    assert_eq!(repo_state(&repo), before, "the user's repository must not change");
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"one\r\nTWO\r\n");
    std::fs::remove_dir_all(&base).ok();
}
