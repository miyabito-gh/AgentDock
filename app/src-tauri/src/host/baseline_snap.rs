//! 控え（基準B・終了E）の取得（P3B-3、`app/DESIGN_P3B.md` §1.1・§1.2）。
//!
//! HEADの木に「HEADと違うファイルを生バイトで追加した」木を、AgentDock専用のobjects置き場に作る。
//! ユーザーの作業ツリー・インデックス・refs・stash・リポジトリのobjectsには書かない（git実行は必ず `SnapshotEnv` つき）。
//! 一時インデックスはAgentDock領域の一時ファイルで、取得の終わりに消す。純粋な解析（status v2）は単体でテストする。

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backend::baseline::*;
use crate::gitops::snapshot::{GitPathsInfo, GitVersion, IndexInfoEntry, SnapshotEnv, SnapshotOp};
use crate::gitops::{Git, GitError};
use crate::rules::baseline::{classify, Classified, FileMeta};
use crate::store::layout::is_inside;

/// 取得に必要な空き容量の余裕（違うファイルの合計サイズに足す）。
const SPACE_MARGIN: u64 = 1024 * 1024;

// ───────────────────────────── status v2 の解析 ─────────────────────────────

/// HEADと違うパス1件（`git status --porcelain=v2 -z` の変更行・未追跡行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusItem {
    pub path: String,
    /// 作業ツリー側のmode（未追跡は空）。
    pub wt_mode: String,
    /// 索引に載っている（未追跡でない）。
    pub tracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusInfo {
    /// HEADのコミット（初回コミット前は None）。
    pub head: Option<String>,
    /// ブランチ名（detachedは None）。
    pub branch: Option<String>,
    pub items: Vec<StatusItem>,
}

/// `status --porcelain=v2 -z --branch --no-renames` の出力を解析する。無視ファイル（`!`）は出力に出ないが、出ても読み飛ばす。
pub fn parse_status(stdout: &[u8]) -> StatusInfo {
    let text = String::from_utf8_lossy(stdout);
    let mut info = StatusInfo::default();
    let mut tokens = text.split('\0');
    while let Some(line) = tokens.next() {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            info.head = (rest != "(initial)").then(|| rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            info.branch = (rest != "(detached)").then(|| rest.trim().to_string());
        } else if line.starts_with('#') {
            continue;
        } else if let Some(path) = line.strip_prefix("? ") {
            info.items.push(StatusItem { path: path.to_string(), wt_mode: String::new(), tracked: false });
        } else if line.starts_with("1 ") {
            let f: Vec<&str> = line.splitn(9, ' ').collect();
            if f.len() == 9 {
                info.items.push(StatusItem { path: f[8].to_string(), wt_mode: submodule_or(f[2], f[5]), tracked: true });
            }
        } else if line.starts_with("2 ") {
            // 改名の行（`--no-renames` なら出ない）。元のパスは次のトークン。
            let f: Vec<&str> = line.splitn(10, ' ').collect();
            let _orig = tokens.next();
            if f.len() == 10 {
                info.items.push(StatusItem { path: f[9].to_string(), wt_mode: submodule_or(f[2], f[5]), tracked: true });
            }
        } else if line.starts_with("u ") {
            let f: Vec<&str> = line.splitn(11, ' ').collect();
            if f.len() == 11 {
                info.items.push(StatusItem { path: f[10].to_string(), wt_mode: submodule_or(f[2], f[6]), tracked: true });
            }
        }
    }
    info
}

/// サブモジュール（`sub` が `S...`）は作業ツリーのmodeが何であれサブモジュールとして扱う。
fn submodule_or(sub: &str, wt_mode: &str) -> String {
    if sub.starts_with('S') {
        "160000".to_string()
    } else {
        wt_mode.to_string()
    }
}

// ───────────────────────────── リポジトリの確認 ─────────────────────────────

fn first_line(bytes: &[u8]) -> String {
    let t = String::from_utf8_lossy(bytes);
    let l = t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    l.chars().take(300).collect()
}

fn failure_of(e: GitError) -> BaselineFailure {
    match e {
        GitError::Unavailable(_) => BaselineFailure::GitUnavailable,
        GitError::Timeout => BaselineFailure::Timeout,
        other => BaselineFailure::Git { message: other.to_string() },
    }
}

/// 調べたリポジトリ。
#[derive(Debug, Clone)]
pub struct RepoProbe {
    pub info: GitPathsInfo,
}

impl RepoProbe {
    pub fn repo_ref(&self) -> RepoRef {
        RepoRef { root: self.info.toplevel.to_string_lossy().replace('\\', "/"), objects_dir: self.info.objects_dir().to_string_lossy().replace('\\', "/") }
    }
}

/// 作業フォルダのGitリポジトリを調べる（読取りのみ）。Gitの有無と版・リポジトリか・rebase等の途中かを確かめる。
pub async fn probe_repo(git: &Git, cwd: &Path) -> Result<RepoProbe, BaselineFailure> {
    if !cwd.is_dir() {
        return Err(BaselineFailure::WorkFolderUnknown);
    }
    let v = git.probe_version(cwd).await.map_err(failure_of)?;
    check_version(v)?;
    let out = git.run_repo_probe(cwd, &SnapshotOp::GitPaths).await.map_err(failure_of)?;
    if !out.success() {
        let msg = first_line(&out.stderr);
        return Err(if msg.to_lowercase().contains("not a git repository") { BaselineFailure::NotARepository } else { BaselineFailure::Git { message: msg } });
    }
    let info = GitPathsInfo::parse(&out.stdout_text()).ok_or_else(|| BaselineFailure::Git { message: "unexpected output from git rev-parse".into() })?;
    if !info.busy_markers().is_empty() {
        return Err(BaselineFailure::GitBusy);
    }
    Ok(RepoProbe { info })
}

fn check_version(v: GitVersion) -> Result<(), BaselineFailure> {
    if v.supports_snapshot() {
        Ok(())
    } else {
        Err(BaselineFailure::GitTooOld { found: v.display() })
    }
}

// ───────────────────────────── 取得 ─────────────────────────────

/// 取り終えたら一時インデックスを消す（途中で打ち切られても消える）。
struct TmpIndex(PathBuf);

impl Drop for TmpIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(PathBuf::from(lock));
    }
}

fn meta_of(p: &Path) -> FileMeta {
    match std::fs::symlink_metadata(p) {
        Ok(m) => {
            let ft = m.file_type();
            if ft.is_symlink() {
                FileMeta::Link
            } else if ft.is_file() {
                FileMeta::Regular { size: m.len() }
            } else if ft.is_dir() {
                FileMeta::Directory
            } else {
                FileMeta::Unreadable
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileMeta::Missing,
        Err(_) => FileMeta::Unreadable,
    }
}

fn unique_name() -> String {
    format!("idx-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

fn git_fail(out: &crate::gitops::GitOutput, what: &str) -> BaselineFailure {
    let msg = first_line(&out.stderr);
    BaselineFailure::Git { message: if msg.is_empty() { format!("{what} failed") } else { format!("{what}: {msg}") } }
}

/// 空き容量の確認（必要量を渡し、足りなければ `InsufficientSpace`）。
pub type SpaceCheck<'a> = &'a (dyn Fn(u64) -> Result<(), BaselineFailure> + Send + Sync);

/// 控えを1つ取る。`objects` は専用置き場、`tmp_dir` は一時インデックスの置き場（どちらも作成済み）、
/// `exclude` はAgentDock領域（リポジトリの中にあっても控えの対象にしない）。時間の上限は呼出し側が掛ける。
pub async fn capture(git: &Git, probe: &RepoProbe, objects: &Path, tmp_dir: &Path, exclude: &Path, space: SpaceCheck<'_>) -> Result<Snapshot, BaselineFailure> {
    let started = Instant::now();
    let top = probe.info.toplevel.as_path();
    let index_path = tmp_dir.join(unique_name());
    let _cleanup = TmpIndex(index_path.clone());
    let env = SnapshotEnv::new(objects.to_path_buf(), probe.info.objects_dir(), index_path).map_err(|e| BaselineFailure::Git { message: e.to_string() })?;

    let st = git.run_snapshot(top, &env, &SnapshotOp::StatusSnapshot).await.map_err(failure_of)?;
    if !st.success() {
        return Err(git_fail(&st, "status"));
    }
    let status = parse_status(&st.stdout);

    // 分類: 取り込む（Hash）・削除（Gone）・取り込まない（Skip）。
    let mut used = 0u64;
    let mut hash: Vec<&StatusItem> = Vec::new();
    let mut gone: Vec<&StatusItem> = Vec::new();
    let mut skipped: Vec<SkippedPath> = Vec::new();
    for item in &status.items {
        let abs = top.join(&item.path);
        if is_inside(exclude, &abs) {
            continue;
        }
        let bad_name = item.path.is_empty() || item.path.chars().any(|c| matches!(c, '\0' | '\n' | '\r'));
        let class = if bad_name { Classified::Skip(SkipReason::Unreadable) } else { classify(&item.wt_mode, &meta_of(&abs), &mut used) };
        match class {
            Classified::Hash => hash.push(item),
            Classified::Gone => gone.push(item),
            Classified::Skip(reason) => skipped.push(SkippedPath { path: item.path.clone(), reason }),
        }
    }
    space(used.saturating_add(SPACE_MARGIN))?;

    // 生バイトのままハッシュして専用置き場へ書く。
    let mut oids: Vec<String> = Vec::new();
    if !hash.is_empty() {
        let paths: Vec<String> = hash.iter().map(|i| i.path.clone()).collect();
        let hs = git.run_snapshot(top, &env, &SnapshotOp::HashObjectsRaw { write: true, paths }).await.map_err(failure_of)?;
        if !hs.success() {
            return Err(git_fail(&hs, "hash-object"));
        }
        oids = hs.stdout_text().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        if oids.len() != hash.len() {
            return Err(BaselineFailure::Git { message: "hash-object returned an unexpected number of ids".into() });
        }
    }

    // 一時インデックス: HEADの木 → 違うファイルだけ置き換え・削除 → 木を書く。
    let rt = git.run_snapshot(top, &env, &SnapshotOp::ReadTree { head: status.head.clone() }).await.map_err(failure_of)?;
    if !rt.success() {
        return Err(git_fail(&rt, "read-tree"));
    }
    let hex_len = status.head.as_ref().map(|h| h.len()).or_else(|| oids.first().map(|o| o.len())).unwrap_or(40);
    let mut entries: Vec<IndexInfoEntry> = Vec::new();
    for (item, oid) in hash.iter().zip(&oids) {
        let mode = if item.wt_mode == "100755" { "100755" } else { "100644" };
        entries.push(IndexInfoEntry::Set { mode: mode.into(), oid: oid.clone(), path: item.path.clone() });
    }
    for item in gone.iter().filter(|i| i.tracked) {
        entries.push(IndexInfoEntry::Remove { path: item.path.clone() });
    }
    if !entries.is_empty() {
        let ui = git.run_snapshot(top, &env, &SnapshotOp::UpdateIndexInfo { entries, oid_hex_len: hex_len }).await.map_err(failure_of)?;
        if !ui.success() {
            return Err(git_fail(&ui, "update-index"));
        }
    }
    let wt = git.run_snapshot(top, &env, &SnapshotOp::WriteTree).await.map_err(failure_of)?;
    if !wt.success() {
        return Err(git_fail(&wt, "write-tree"));
    }
    let tree = wt.stdout_text().trim().to_string();
    if tree.is_empty() {
        return Err(BaselineFailure::Git { message: "write-tree returned no tree".into() });
    }
    let mut raw: Vec<String> = hash.iter().map(|i| i.path.clone()).collect();
    raw.sort();
    Ok(Snapshot { head: status.head, branch: status.branch, tree, raw, skipped, took_ms: started.elapsed().as_millis() as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn status_v2_is_parsed_for_branch_changes_untracked_and_unmerged() {
        let out = b"# branch.oid 0123456789abcdef0123456789abcdef01234567\0# branch.head main\0\
1 .M N... 100644 100644 100644 aaaa bbbb dir/a b.txt\0\
1 .D N... 100644 100644 000000 aaaa bbbb gone.txt\0\
1 .M S.M. 160000 160000 160000 aaaa bbbb sub\0\
u UU N... 100644 100644 100644 100755 h1 h2 h3 conflict.txt\0\
? new file.txt\0! ignored.txt\0";
        let s = parse_status(out);
        assert_eq!(s.head.as_deref(), Some("0123456789abcdef0123456789abcdef01234567"));
        assert_eq!(s.branch.as_deref(), Some("main"));
        let by = |p: &str| s.items.iter().find(|i| i.path == p).unwrap();
        assert_eq!(by("dir/a b.txt").wt_mode, "100644");
        assert_eq!(by("gone.txt").wt_mode, "000000");
        assert_eq!(by("sub").wt_mode, "160000");
        assert_eq!(by("conflict.txt").wt_mode, "100755");
        assert!(by("conflict.txt").tracked && !by("new file.txt").tracked);
        assert_eq!(by("new file.txt").wt_mode, "");
        assert!(s.items.iter().all(|i| i.path != "ignored.txt"));
    }

    #[test]
    fn initial_and_detached_have_no_head_or_branch_and_renames_consume_the_original_path() {
        let s = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0? a.txt\0");
        assert_eq!((s.head, s.branch), (None, None));
        assert_eq!(s.items.len(), 1);
        let r = parse_status(b"2 R. N... 100644 100644 100644 aaaa bbbb R100 new.txt\0old.txt\0? x\0");
        assert_eq!(r.items.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["new.txt", "x"]);
    }

    #[test]
    fn old_git_is_rejected_with_the_found_version() {
        assert!(check_version(GitVersion::parse("git version 2.40.0").unwrap()).is_ok());
        assert_eq!(check_version(GitVersion::parse("git version 2.30.0").unwrap()), Err(BaselineFailure::GitTooOld { found: "2.30.0".into() }));
    }

    // ── 結合: 一時リポジトリ（gitがなければスキップ）──

    fn git_ok() -> bool {
        Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn temp_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentdock-bsnap-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    fn no_space_check() -> impl Fn(u64) -> Result<(), BaselineFailure> + Send + Sync {
        |_| Ok(())
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
    async fn non_repositories_and_busy_repositories_are_reported_by_reason() {
        if !git_ok() {
            return;
        }
        let base = temp_base("probe");
        let plain = base.join("plain");
        let repo = base.join("repo");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let git = Git::new(None);
        // 親にリポジトリがない一時フォルダ（環境によっては親がリポジトリのことがあるので、その場合は検査しない）。
        match probe_repo(&git, &plain).await {
            Err(BaselineFailure::NotARepository) | Err(BaselineFailure::GitTooOld { .. }) | Ok(_) => {}
            Err(e) => panic!("{e:?}"),
        }
        assert_eq!(probe_repo(&git, &base.join("missing")).await.unwrap_err(), BaselineFailure::WorkFolderUnknown);
        sh(&repo, &["init", "-q"]);
        if matches!(probe_repo(&git, &repo).await, Err(BaselineFailure::GitTooOld { .. })) {
            std::fs::remove_dir_all(&base).ok();
            return;
        }
        std::fs::write(repo.join(".git").join("MERGE_HEAD"), b"x").unwrap();
        assert_eq!(probe_repo(&git, &repo).await.unwrap_err(), BaselineFailure::GitBusy);
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn initial_commit_less_repository_and_the_agentdock_area_inside_the_repository() {
        if !git_ok() {
            return;
        }
        let base = temp_base("init");
        let repo = base.join("repo");
        // データ領域（ルート）がリポジトリの中にある構成: 他チャットの領域も控えに入れない。
        let data = repo.join("agentdock-data");
        let area = data.join("chats").join("c1").join("baselines");
        for d in [&area.join("objects"), &area.join("tmp"), &data.join("chats").join("c2").join("attachments")] {
            std::fs::create_dir_all(d).unwrap();
        }
        sh(&repo, &["init", "-q"]);
        std::fs::write(repo.join("first.txt"), b"hello
").unwrap();
        std::fs::write(area.join("objects").join("noise.bin"), b"noise").unwrap();
        std::fs::write(data.join("chats").join("c2").join("attachments").join("other.bin"), b"other chat").unwrap();
        let git = Git::new(None);
        let Ok(probe) = probe_repo(&git, &repo).await else {
            std::fs::remove_dir_all(&base).ok();
            return;
        };
        let space = no_space_check();
        let snap = capture(&git, &probe, &area.join("objects"), &area.join("tmp"), &data, &space).await.expect("capture");
        assert_eq!(snap.head, None);
        assert_eq!(snap.raw, ["first.txt"], "the whole AgentDock data area is never captured, even inside the repository");
        assert_eq!(blob_at(&git, &probe, &area.join("objects"), &area.join("tmp"), &snap.tree, "first.txt").await.unwrap(), b"hello
");
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn insufficient_space_stops_the_capture_before_anything_is_written() {
        if !git_ok() {
            return;
        }
        let base = temp_base("space");
        let repo = base.join("repo");
        let area = base.join("area");
        for d in [&repo, &area.join("objects"), &area.join("tmp")] {
            std::fs::create_dir_all(d).unwrap();
        }
        sh(&repo, &["init", "-q"]);
        std::fs::write(repo.join("f.txt"), b"data").unwrap();
        let git = Git::new(None);
        let Ok(probe) = probe_repo(&git, &repo).await else {
            std::fs::remove_dir_all(&base).ok();
            return;
        };
        let space = |need: u64| Err(BaselineFailure::InsufficientSpace { required: need, available: 1 });
        let r = capture(&git, &probe, &area.join("objects"), &area.join("tmp"), &area, &space).await;
        assert!(matches!(r, Err(BaselineFailure::InsufficientSpace { available: 1, .. })));
        assert_eq!(area.join("objects").read_dir().unwrap().count(), 0);
        std::fs::remove_dir_all(&base).ok();
    }
}
