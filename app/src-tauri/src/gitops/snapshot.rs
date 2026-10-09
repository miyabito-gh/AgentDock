//! 「変更の控え」用のGit操作（P3B-0、`app/DESIGN_P3B.md` §1.1）。
//!
//! 控え用の操作はAgentDock領域（チャット領域内の専用objects置き場 `baselines\objects` と一時インデックス）にだけ書く。
//! ユーザーの作業ツリー・インデックス・stash・ブランチ・refs・リポジトリの `objects` には書かない。そのため:
//! - 操作は列挙型 [`SnapshotOp`] だけで、任意のgitコマンドは受け付けない。実行 [`Git::run_snapshot`] は必ず [`SnapshotEnv`] を要求する。
//! - 書込みを伴う操作には `GIT_OBJECT_DIRECTORY`（専用置き場）・`GIT_ALTERNATE_OBJECT_DIRECTORIES`（リポジトリのobjectsは参照のみ）・
//!   `GIT_INDEX_FILE`（AgentDockの一時インデックス）を渡す。環境は操作の種類ごとに必要な分だけ（[`SnapshotOp::env_kind`]）。
//! - 読取りは `--no-optional-locks`。パスは `--` の後ろか標準入力で渡す。`checkout` 系・`add`・`stash` は提供しない。
//! - 生バイトで取り込むため `hash-object --no-filters`、改行・フィルター変換が要る取り出しだけ `cat-file --filters`。

use std::path::{Path, PathBuf};

use super::{check, strings, valid_path, Git, GitError, GitOp, GitOutput, INHERITED_GIT_VARS, OUTPUT_LIMIT, READ_TIMEOUT, WRITE_TIMEOUT};
use crate::exec::{run_bounded, RunSpec};
use crate::store::layout::is_inside;

/// `--path-format=absolute` を使えるGitの最低版（2.31）。
pub const MIN_GIT_VERSION: (u32, u32) = (2, 31);
/// 1ファイル 64MiB までを控える（DESIGN_P3B §1.2）。取り出し（cat-file）の出力上限はそれより少し大きくする。
pub const CAT_FILE_LIMIT: usize = 80 * 1024 * 1024;

// ───────────────────────────── 環境 ─────────────────────────────

/// 控え用のgit実行に渡す環境。専用objects置き場・リポジトリobjects（参照のみ）・一時インデックスの3つの場所。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEnv {
    objects: PathBuf,
    alternates: PathBuf,
    index: PathBuf,
}

/// 操作が必要とする環境の範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvKind {
    /// ユーザーのリポジトリを読むだけ（上書きなし）。
    Repo,
    /// 専用objects置き場に書き、リポジトリのobjectsは参照のみ。
    Objects,
    /// 上に加え、一時インデックスを使う。
    ObjectsAndIndex,
}

impl SnapshotEnv {
    /// すべて絶対パス。`objects`（専用置き場）はリポジトリのobjectsと別の場所、`index`（一時インデックス）はリポジトリのgitディレクトリの外であること
    /// （ユーザーのインデックスやobjectsを指す設定を型の入口で拒む）。
    pub fn new(objects: PathBuf, alternates: PathBuf, index: PathBuf) -> Result<Self, GitError> {
        check(objects.is_absolute() && alternates.is_absolute() && index.is_absolute(), "snapshot env paths must be absolute")?;
        check(!is_inside(&alternates, &objects) && !is_inside(&objects, &alternates), "snapshot objects must differ from the repository objects")?;
        if let Some(common) = alternates.parent() {
            check(!is_inside(common, &index) && !is_inside(common, &objects), "snapshot files must be outside the repository git directory")?;
        }
        Ok(SnapshotEnv { objects, alternates, index })
    }

    pub fn objects(&self) -> &Path {
        &self.objects
    }
    pub fn index(&self) -> &Path {
        &self.index
    }

    /// 環境変数の組（`kind` に必要な分だけ）。パスはUTF-8でなければ拒む。
    pub fn vars(&self, kind: EnvKind) -> Result<Vec<(String, String)>, GitError> {
        let s = |p: &Path| p.to_str().map(str::to_string).ok_or_else(|| GitError::InvalidArgument("non-UTF-8 path".into()));
        let mut v = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())];
        if matches!(kind, EnvKind::Objects | EnvKind::ObjectsAndIndex) {
            v.push(("GIT_OBJECT_DIRECTORY".into(), s(&self.objects)?));
            v.push(("GIT_ALTERNATE_OBJECT_DIRECTORIES".into(), s(&self.alternates)?));
        }
        if kind == EnvKind::ObjectsAndIndex {
            v.push(("GIT_INDEX_FILE".into(), s(&self.index)?));
        }
        Ok(v)
    }
}

// ───────────────────────────── 操作 ─────────────────────────────

/// `update-index --index-info` に渡す1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexInfoEntry {
    /// `mode` は `100644`／`100755`（リンクなどは控えない）。
    Set { mode: String, oid: String, path: String },
    /// 削除されたパス（モード0）。
    Remove { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOp {
    /// HEAD・ブランチ・HEADと違うパス（索引・作業ツリー・未追跡）。索引を書かない。
    StatusSnapshot,
    /// 作業ツリーのファイルを生バイトのままハッシュする（パスは標準入力。`write` なら専用置き場へ書く）。出力はパスと同じ順のoid。
    HashObjectsRaw { write: bool, paths: Vec<String> },
    /// 一時インデックスへHEADの木を読み込む（`head` がなければ空）。
    ReadTree { head: Option<String> },
    /// 一時インデックスのエントリを置き換える・消す。`oid_hex_len` はoidの16進桁数（40／64）。削除エントリの0埋めに使う。
    UpdateIndexInfo { entries: Vec<IndexInfoEntry>, oid_hex_len: usize },
    /// 一時インデックスから木を作る（専用置き場へ書く）。
    WriteTree,
    /// 木の中のエントリ（`mode type oid\tpath` をNUL区切り）。`paths` が空なら全体。
    LsTree { tree: String, paths: Vec<String> },
    /// 2つの木で違うパス（NUL区切り）。
    DiffTreeNames { a: String, b: String },
    /// 2つの木の統一diff（表示専用）。
    DiffTrees { a: String, b: String, path: String },
    /// 2つの木の全体の統一diff（一覧の行数集計用の読取り。表示専用）。
    DiffTreesAll { a: String, b: String },
    /// blobの生バイト（生形の取り出し）。
    CatFileBlob { oid: String },
    /// checkoutと同じ変換（改行・smudge・LFS）をかけたバイト列（HEAD形の取り出し）。
    CatFileFiltered { path: String, oid: String },
    /// リポジトリのルート・共通gitディレクトリ・進行中の操作の目印（`--path-format=absolute`、Git 2.31以上）。
    GitPaths,
}

/// Git操作の進行中を示す目印（`--git-path` で引く。ファイルまたはフォルダがあれば途中）。
pub const IN_PROGRESS_MARKERS: [&str; 5] = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"];

/// oidとして安全か（16進のみ。桁数は検査しない＝SHA-1／SHA-256のどちらでも通す）。
fn valid_oid(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_mode(s: &str) -> bool {
    matches!(s, "100644" | "100755")
}

impl SnapshotOp {
    pub fn env_kind(&self) -> EnvKind {
        match self {
            SnapshotOp::StatusSnapshot | SnapshotOp::GitPaths => EnvKind::Repo,
            SnapshotOp::ReadTree { .. } | SnapshotOp::UpdateIndexInfo { .. } | SnapshotOp::WriteTree => EnvKind::ObjectsAndIndex,
            SnapshotOp::HashObjectsRaw { .. }
            | SnapshotOp::LsTree { .. }
            | SnapshotOp::DiffTreeNames { .. }
            | SnapshotOp::DiffTrees { .. }
            | SnapshotOp::DiffTreesAll { .. }
            | SnapshotOp::CatFileBlob { .. }
            | SnapshotOp::CatFileFiltered { .. } => EnvKind::Objects,
        }
    }

    /// 引数の配列（`git` の後ろ）。値は検証し、パスは `--` の後ろ・標準入力・`--path=` で渡す。
    pub fn args(&self) -> Result<Vec<String>, GitError> {
        Ok(match self {
            SnapshotOp::StatusSnapshot => strings(&[
                "--no-optional-locks", "status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all", "--no-renames", "--ignore-submodules=none",
            ]),
            SnapshotOp::HashObjectsRaw { write, .. } => {
                let mut a = strings(&["hash-object"]);
                if *write {
                    a.push("-w".into());
                }
                a.extend(strings(&["--no-filters", "--stdin-paths"]));
                a
            }
            SnapshotOp::ReadTree { head } => match head {
                Some(h) => {
                    check(valid_oid(h), "head")?;
                    vec!["read-tree".into(), h.clone()]
                }
                None => strings(&["read-tree", "--empty"]),
            },
            SnapshotOp::UpdateIndexInfo { .. } => strings(&["update-index", "-z", "--index-info"]),
            SnapshotOp::WriteTree => strings(&["write-tree"]),
            SnapshotOp::LsTree { tree, paths } => {
                check(valid_oid(tree), "tree")?;
                let mut a = strings(&["--literal-pathspecs", "ls-tree", "-r", "-z", "--full-tree"]);
                a.push(tree.clone());
                if !paths.is_empty() {
                    a.push("--".into());
                    for p in paths {
                        check(valid_path(p), "path")?;
                        a.push(p.clone());
                    }
                }
                a
            }
            SnapshotOp::DiffTreeNames { a, b } => {
                check(valid_oid(a) && valid_oid(b), "tree")?;
                let mut v = strings(&["diff-tree", "-r", "-z", "--no-renames", "--name-only"]);
                v.extend([a.clone(), b.clone()]);
                v
            }
            SnapshotOp::DiffTrees { a, b, path } => {
                check(valid_oid(a) && valid_oid(b), "tree")?;
                check(valid_path(path), "path")?;
                let mut v = strings(&["--no-optional-locks", "--literal-pathspecs", "-c", "core.quotepath=false", "diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
                v.extend([a.clone(), b.clone(), "--".into(), path.clone()]);
                v
            }
            SnapshotOp::DiffTreesAll { a, b } => {
                check(valid_oid(a) && valid_oid(b), "tree")?;
                let mut v = strings(&["--no-optional-locks", "-c", "core.quotepath=false", "diff", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames"]);
                v.extend([a.clone(), b.clone()]);
                v
            }
            SnapshotOp::CatFileBlob { oid } => {
                check(valid_oid(oid), "oid")?;
                vec!["cat-file".into(), "blob".into(), oid.clone()]
            }
            SnapshotOp::CatFileFiltered { path, oid } => {
                check(valid_oid(oid), "oid")?;
                check(valid_path(path), "path")?;
                vec!["cat-file".into(), "--filters".into(), format!("--path={path}"), oid.clone()]
            }
            SnapshotOp::GitPaths => {
                // `--path-format` は後ろに置いた指定にだけ効く。
                let mut a = strings(&["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"]);
                for m in IN_PROGRESS_MARKERS {
                    a.push("--git-path".into());
                    a.push(m.into());
                }
                a
            }
        })
    }

    /// 標準入力へ渡す内容。
    pub fn stdin(&self) -> Result<Option<Vec<u8>>, GitError> {
        match self {
            SnapshotOp::HashObjectsRaw { paths, .. } => {
                let mut buf = Vec::new();
                for p in paths {
                    check(valid_path(p), "path")?;
                    // `--stdin-paths` は `"` で始まる行をC形式の引用として解釈する。相対表記にして避ける。
                    if p.starts_with('"') {
                        buf.extend_from_slice(b"./");
                    }
                    buf.extend_from_slice(p.as_bytes());
                    buf.push(b'\n');
                }
                Ok(Some(buf))
            }
            SnapshotOp::UpdateIndexInfo { entries, oid_hex_len } => {
                check(matches!(oid_hex_len, 40 | 64), "oid length")?;
                let null = "0".repeat(*oid_hex_len);
                let mut buf = Vec::new();
                for e in entries {
                    let line = match e {
                        IndexInfoEntry::Set { mode, oid, path } => {
                            check(valid_mode(mode) && valid_oid(oid) && oid.len() == *oid_hex_len && valid_path(path), "index entry")?;
                            format!("{mode} {oid}\t{path}")
                        }
                        IndexInfoEntry::Remove { path } => {
                            check(valid_path(path), "path")?;
                            format!("0 {null}\t{path}")
                        }
                    };
                    buf.extend_from_slice(line.as_bytes());
                    buf.push(0);
                }
                Ok(Some(buf))
            }
            _ => Ok(None),
        }
    }

    fn timeout(&self) -> std::time::Duration {
        match self {
            SnapshotOp::CatFileBlob { .. } | SnapshotOp::CatFileFiltered { .. } => WRITE_TIMEOUT,
            _ => READ_TIMEOUT,
        }
    }

    fn stdout_limit(&self) -> usize {
        match self {
            SnapshotOp::CatFileBlob { .. } | SnapshotOp::CatFileFiltered { .. } => CAT_FILE_LIMIT,
            _ => OUTPUT_LIMIT,
        }
    }
}

impl Git {
    /// 控え用の操作を実行する。`env` は必須（無い呼出しは型で書けない）。実行は他のGit操作と同じく直列化する。
    pub async fn run_snapshot(&self, cwd: &Path, env: &SnapshotEnv, op: &SnapshotOp) -> Result<GitOutput, GitError> {
        let vars = env.vars(op.env_kind())?;
        self.exec_snapshot(cwd, vars, op).await
    }

    /// リポジトリを読むだけの操作（`EnvKind::Repo`: `StatusSnapshot`・`GitPaths`）を、環境なしで実行する。
    /// 専用置き場や一時インデックスが決まる前（リポジトリの場所を調べる段階）に使う。書込みを伴う操作は受け付けない。
    pub async fn run_repo_probe(&self, cwd: &Path, op: &SnapshotOp) -> Result<GitOutput, GitError> {
        check(op.env_kind() == EnvKind::Repo, "repo probe takes read-only operations only")?;
        let vars = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())];
        self.exec_snapshot(cwd, vars, op).await
    }

    async fn exec_snapshot(&self, cwd: &Path, vars: Vec<(String, String)>, op: &SnapshotOp) -> Result<GitOutput, GitError> {
        let args = op.args()?;
        let stdin = op.stdin()?;
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let _serial = self.lock.lock().await;
        let out = run_bounded(RunSpec {
            program: &self.exe,
            args: &args,
            cwd: Some(cwd),
            env: &pairs,
            env_remove: INHERITED_GIT_VARS,
            timeout: op.timeout(),
            stdout_limit: op.stdout_limit(),
            stdin: stdin.as_deref(),
        })
        .await?;
        Ok(GitOutput { exit_code: out.exit_code, stdout: out.stdout, stderr: out.stderr })
    }

    /// Gitの版を調べる（`git --version`）。控えには `--path-format=absolute` が使える版（2.31以上）が要る。
    pub async fn probe_version(&self, cwd: &Path) -> Result<GitVersion, GitError> {
        let out = self.run(cwd, &GitOp::Version).await?;
        if !out.success() {
            return Err(GitError::Io(format!("git --version failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        GitVersion::parse(&out.stdout_text()).ok_or_else(|| GitError::Io(format!("unrecognized git version: {}", out.stdout_text().trim())))
    }
}

// ───────────────────────────── 純粋な解析 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// `git version 2.43.0.windows.1` などから先頭の数値3つ（なければ0）を取る。数値が読めなければ None。
    pub fn parse(text: &str) -> Option<GitVersion> {
        let rest = text.trim().strip_prefix("git version")?.trim();
        let mut it = rest.split(|c: char| c == '.' || c.is_whitespace());
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        Some(GitVersion { major, minor, patch })
    }

    /// 控えに使える版か（`--path-format=absolute` が要る＝2.31以上）。
    pub fn supports_snapshot(&self) -> bool {
        (self.major, self.minor) >= MIN_GIT_VERSION
    }

    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `SnapshotOp::GitPaths` の出力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPathsInfo {
    /// 作業ツリーのルート。
    pub toplevel: PathBuf,
    /// 共通gitディレクトリ（worktreeでも本体側）。
    pub common_dir: PathBuf,
    /// 進行中の操作の目印の場所（`IN_PROGRESS_MARKERS` と同じ順）。存在するかは [`GitPathsInfo::busy_markers`] で調べる。
    pub marker_paths: Vec<PathBuf>,
}

impl GitPathsInfo {
    /// 出力は `toplevel`、`common-dir`、目印ごとの場所の順に1行ずつ。行数が合わなければ None（推定しない）。
    pub fn parse(stdout: &str) -> Option<GitPathsInfo> {
        let lines: Vec<&str> = stdout.lines().map(|l| l.trim_end_matches('\r')).collect();
        if lines.len() != 2 + IN_PROGRESS_MARKERS.len() || lines.iter().any(|l| l.is_empty()) {
            return None;
        }
        let (toplevel, common_dir) = (PathBuf::from(lines[0]), PathBuf::from(lines[1]));
        if !toplevel.is_absolute() || !common_dir.is_absolute() {
            return None;
        }
        Some(GitPathsInfo { toplevel, common_dir, marker_paths: lines[2..].iter().map(PathBuf::from).collect() })
    }

    /// リポジトリ共通のobjects置き場（控えの alternates に使う）。
    pub fn objects_dir(&self) -> PathBuf {
        self.common_dir.join("objects")
    }

    /// 進行中の操作の目印のうち、今あるもの（名前）。空でなければ rebase・merge 等の途中。
    pub fn busy_markers(&self) -> Vec<&'static str> {
        IN_PROGRESS_MARKERS.iter().zip(&self.marker_paths).filter(|(_, p)| p.exists()).map(|(n, _)| *n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn env() -> SnapshotEnv {
        SnapshotEnv::new(PathBuf::from(r"C:\data\chats\d1\baselines\objects"), PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\data\chats\d1\baselines\tmp\i1")).unwrap()
    }

    #[test]
    fn status_is_read_only_and_untracked_aware() {
        let a = SnapshotOp::StatusSnapshot.args().unwrap();
        assert_eq!(a[0], "--no-optional-locks");
        for f in ["--porcelain=v2", "-z", "--branch", "--untracked-files=all", "--no-renames", "--ignore-submodules=none"] {
            assert!(a.contains(&f.to_string()), "{f}");
        }
        assert_eq!(SnapshotOp::StatusSnapshot.env_kind(), EnvKind::Repo);
    }

    #[test]
    fn hash_objects_is_raw_and_reads_paths_from_stdin() {
        let op = SnapshotOp::HashObjectsRaw { write: true, paths: vec!["a b.txt".into(), "\"q\".txt".into()] };
        assert_eq!(op.args().unwrap(), ["hash-object", "-w", "--no-filters", "--stdin-paths"]);
        assert_eq!(op.stdin().unwrap().unwrap(), b"a b.txt\n./\"q\".txt\n");
        let ro = SnapshotOp::HashObjectsRaw { write: false, paths: vec![] };
        assert!(!ro.args().unwrap().contains(&"-w".to_string()));
        assert!(SnapshotOp::HashObjectsRaw { write: true, paths: vec!["a\nb".into()] }.stdin().is_err());
        // どの引数にも作業ツリーのパスは載らない。
        assert!(!op.args().unwrap().iter().any(|x| x.contains("a b.txt")));
    }

    #[test]
    fn index_ops_use_the_temp_index_and_nul_separated_stdin() {
        assert_eq!(SnapshotOp::ReadTree { head: None }.args().unwrap(), ["read-tree", "--empty"]);
        assert_eq!(SnapshotOp::ReadTree { head: Some(OID.into()) }.args().unwrap(), ["read-tree", OID]);
        assert!(SnapshotOp::ReadTree { head: Some("--all".into()) }.args().is_err());
        for op in [SnapshotOp::ReadTree { head: None }, SnapshotOp::WriteTree, SnapshotOp::UpdateIndexInfo { entries: vec![], oid_hex_len: 40 }] {
            assert_eq!(op.env_kind(), EnvKind::ObjectsAndIndex);
        }
        let op = SnapshotOp::UpdateIndexInfo {
            entries: vec![
                IndexInfoEntry::Set { mode: "100644".into(), oid: OID.into(), path: "src/a.rs".into() },
                IndexInfoEntry::Remove { path: "gone.txt".into() },
            ],
            oid_hex_len: 40,
        };
        assert_eq!(op.args().unwrap(), ["update-index", "-z", "--index-info"]);
        let expected = format!("100644 {OID}\tsrc/a.rs\0" ) + &format!("0 {}\tgone.txt\0", "0".repeat(40));
        assert_eq!(op.stdin().unwrap().unwrap(), expected.into_bytes());
        // リンク・不正なmode・桁数違いは入れられない。
        let bad_mode = SnapshotOp::UpdateIndexInfo { entries: vec![IndexInfoEntry::Set { mode: "120000".into(), oid: OID.into(), path: "l".into() }], oid_hex_len: 40 };
        assert!(bad_mode.stdin().is_err());
        let bad_len = SnapshotOp::UpdateIndexInfo { entries: vec![IndexInfoEntry::Set { mode: "100644".into(), oid: OID.into(), path: "l".into() }], oid_hex_len: 64 };
        assert!(bad_len.stdin().is_err());
    }

    #[test]
    fn tree_readers_put_paths_after_double_dash() {
        let a = SnapshotOp::LsTree { tree: OID.into(), paths: vec!["x y".into(), "-r".into()] }.args().unwrap();
        let i = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(&a[i..], ["--", "x y", "-r"]);
        assert!(a[..i].contains(&"--literal-pathspecs".to_string()));
        assert!(!SnapshotOp::LsTree { tree: OID.into(), paths: vec![] }.args().unwrap().contains(&"--".to_string()));
        assert!(SnapshotOp::LsTree { tree: "HEAD".into(), paths: vec![] }.args().is_err());

        let d = SnapshotOp::DiffTrees { a: OID.into(), b: OID.into(), path: "f.txt".into() }.args().unwrap();
        assert_eq!(&d[d.len() - 2..], ["--", "f.txt"]);
        for f in ["--no-optional-locks", "--no-ext-diff", "--no-textconv", "--no-color"] {
            assert!(d.contains(&f.to_string()), "{f}");
        }
        let all = SnapshotOp::DiffTreesAll { a: OID.into(), b: OID.into() }.args().unwrap();
        assert!(all.contains(&"--no-renames".to_string()) && all.contains(&"--no-ext-diff".to_string()) && !all.contains(&"--".to_string()));
        assert_eq!(SnapshotOp::DiffTreesAll { a: OID.into(), b: OID.into() }.env_kind(), EnvKind::Objects);
        let n = SnapshotOp::DiffTreeNames { a: OID.into(), b: OID.into() }.args().unwrap();
        assert!(n.contains(&"--no-renames".to_string()) && n.contains(&"-z".to_string()) && n.contains(&"--name-only".to_string()));
    }

    #[test]
    fn cat_file_variants_are_blob_only_and_filters_use_path_option() {
        assert_eq!(SnapshotOp::CatFileBlob { oid: OID.into() }.args().unwrap(), ["cat-file", "blob", OID]);
        assert_eq!(SnapshotOp::CatFileFiltered { path: "dir/f.txt".into(), oid: OID.into() }.args().unwrap(), ["cat-file", "--filters", "--path=dir/f.txt", OID]);
        assert!(SnapshotOp::CatFileBlob { oid: "-p".into() }.args().is_err());
        assert!(SnapshotOp::CatFileFiltered { path: "a\nb".into(), oid: OID.into() }.args().is_err());
        assert!(SnapshotOp::CatFileBlob { oid: OID.into() }.stdout_limit() > 64 * 1024 * 1024);
    }

    #[test]
    fn git_paths_asks_for_absolute_paths_and_all_markers() {
        let a = SnapshotOp::GitPaths.args().unwrap();
        assert_eq!(&a[..2], ["rev-parse", "--path-format=absolute"]);
        assert_eq!(a.iter().filter(|s| *s == "--git-path").count(), IN_PROGRESS_MARKERS.len());
    }

    #[test]
    fn env_requires_separate_locations_and_scopes_variables() {
        // 専用置き場がリポジトリのobjectsと同じ・一時インデックスがgitディレクトリの中、は作れない。
        assert!(SnapshotEnv::new(PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\t\i")).is_err());
        assert!(SnapshotEnv::new(PathBuf::from(r"C:\t\objects"), PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\repo\.git\index")).is_err());
        assert!(SnapshotEnv::new(PathBuf::from(r"C:\repo\.git\objects\x"), PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\t\i")).is_err());
        assert!(SnapshotEnv::new(PathBuf::from("rel/objects"), PathBuf::from(r"C:\repo\.git\objects"), PathBuf::from(r"C:\t\i")).is_err());
        let e = env();
        let has = |v: &[(String, String)], k: &str| v.iter().any(|(n, _)| n == k);
        let repo = e.vars(EnvKind::Repo).unwrap();
        assert!(!has(&repo, "GIT_OBJECT_DIRECTORY") && !has(&repo, "GIT_INDEX_FILE"));
        let obj = e.vars(EnvKind::Objects).unwrap();
        assert!(has(&obj, "GIT_OBJECT_DIRECTORY") && has(&obj, "GIT_ALTERNATE_OBJECT_DIRECTORIES") && !has(&obj, "GIT_INDEX_FILE"));
        let all = e.vars(EnvKind::ObjectsAndIndex).unwrap();
        assert!(has(&all, "GIT_INDEX_FILE"));
        assert_eq!(all.iter().find(|(k, _)| k == "GIT_OBJECT_DIRECTORY").unwrap().1, r"C:\data\chats\d1\baselines\objects");
    }

    #[test]
    fn git_version_parses_and_gates_at_2_31() {
        let v = GitVersion::parse("git version 2.43.0.windows.1\n").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (2, 43, 0));
        assert!(v.supports_snapshot());
        assert!(GitVersion::parse("git version 2.31.0").unwrap().supports_snapshot());
        assert!(!GitVersion::parse("git version 2.30.9").unwrap().supports_snapshot());
        assert!(!GitVersion::parse("git version 1.9").unwrap().supports_snapshot());
        assert!(GitVersion::parse("git version 3.0.1").unwrap().supports_snapshot());
        assert_eq!(GitVersion::parse("git version 2.40").unwrap().patch, 0);
        assert!(GitVersion::parse("not git").is_none());
    }

    #[test]
    fn git_paths_output_needs_the_exact_shape() {
        let ok = "C:/repo\nC:/repo/.git\nC:/repo/.git/rebase-merge\nC:/repo/.git/rebase-apply\nC:/repo/.git/MERGE_HEAD\nC:/repo/.git/CHERRY_PICK_HEAD\nC:/repo/.git/REVERT_HEAD\n";
        let info = GitPathsInfo::parse(ok).unwrap();
        assert_eq!(info.objects_dir(), PathBuf::from("C:/repo/.git").join("objects"));
        assert_eq!(info.marker_paths.len(), 5);
        assert!(info.busy_markers().is_empty());
        assert!(GitPathsInfo::parse("C:/repo\nC:/repo/.git\n").is_none());
        assert!(GitPathsInfo::parse(&ok.replace("C:/repo\n", "repo\n")).is_none(), "relative paths are not accepted");
    }

    // ── 結合: 一時リポジトリ（gitがなければスキップ）──

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
        let refs = Command::new("git").current_dir(repo).args(["for-each-ref"]).output().unwrap();
        let stash = Command::new("git").current_dir(repo).args(["stash", "list"]).output().unwrap();
        let reflog = Command::new("git").current_dir(repo).args(["reflog"]).output().unwrap();
        let mut files = Vec::new();
        fn walk(d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push(e.path().to_string_lossy().into_owned());
                }
            }
        }
        walk(&repo.join(".git").join("objects"), &mut files);
        files.sort();
        let text = format!("{}|{}|{}", String::from_utf8_lossy(&refs.stdout), String::from_utf8_lossy(&stash.stdout), String::from_utf8_lossy(&reflog.stdout));
        (index, text, files)
    }

    #[tokio::test]
    async fn snapshot_ops_never_write_to_the_user_repository() {
        if !git_ok() {
            return;
        }
        let base = std::env::temp_dir().join(format!("agentdock-snap-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let repo = base.join("repo");
        let area = base.join("area");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q"]);
        sh(&repo, &["config", "user.email", "t@example.com"]);
        sh(&repo, &["config", "user.name", "t"]);
        sh(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("a.txt"), b"one\r\ntwo\r\n").unwrap();
        std::fs::write(repo.join("keep.txt"), b"same\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        // 作業ツリーの変更: 変更・未追跡・削除。
        std::fs::write(repo.join("a.txt"), b"one\r\nTWO\r\nthree\r\n").unwrap();
        std::fs::write(repo.join("new.txt"), b"fresh\r\n").unwrap();
        std::fs::remove_file(repo.join("keep.txt")).unwrap();

        let git = Git::new(None);
        let v = git.probe_version(&repo).await.unwrap();
        if !v.supports_snapshot() {
            std::fs::remove_dir_all(&base).ok();
            return;
        }
        let gp = git.run_snapshot(&repo, &SnapshotEnv::new(area.join("objects"), repo.join(".git").join("objects-unused"), area.join("tmp").join("i")).unwrap(), &SnapshotOp::GitPaths).await.unwrap();
        assert!(gp.success(), "{}", String::from_utf8_lossy(&gp.stderr));
        let info = GitPathsInfo::parse(&gp.stdout_text()).expect("git paths");
        assert!(info.busy_markers().is_empty());
        std::fs::create_dir_all(area.join("objects")).unwrap();
        std::fs::create_dir_all(area.join("tmp")).unwrap();
        let env = SnapshotEnv::new(area.join("objects"), info.objects_dir(), area.join("tmp").join("i1")).unwrap();

        let before = repo_state(&repo);

        let st = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::StatusSnapshot).await.unwrap();
        assert!(st.success());
        let text = st.stdout_text();
        assert!(text.contains("# branch.oid ") && text.contains("a.txt") && text.contains("new.txt") && text.contains("keep.txt"), "{text:?}");
        let head = text.split('\0').find_map(|l| l.strip_prefix("# branch.oid ")).unwrap().trim().to_string();

        // 生バイトでハッシュして専用置き場へ書く。
        let hs = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::HashObjectsRaw { write: true, paths: vec!["a.txt".into(), "new.txt".into()] }).await.unwrap();
        assert!(hs.success(), "{}", String::from_utf8_lossy(&hs.stderr));
        let oids: Vec<String> = hs.stdout_text().lines().map(str::to_string).collect();
        assert_eq!(oids.len(), 2);

        let rt = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::ReadTree { head: Some(head.clone()) }).await.unwrap();
        assert!(rt.success(), "{}", String::from_utf8_lossy(&rt.stderr));
        let ui = git
            .run_snapshot(
                &info.toplevel,
                &env,
                &SnapshotOp::UpdateIndexInfo {
                    entries: vec![
                        IndexInfoEntry::Set { mode: "100644".into(), oid: oids[0].clone(), path: "a.txt".into() },
                        IndexInfoEntry::Set { mode: "100644".into(), oid: oids[1].clone(), path: "new.txt".into() },
                        IndexInfoEntry::Remove { path: "keep.txt".into() },
                    ],
                    oid_hex_len: head.len(),
                },
            )
            .await
            .unwrap();
        assert!(ui.success(), "{}", String::from_utf8_lossy(&ui.stderr));
        let wt = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::WriteTree).await.unwrap();
        assert!(wt.success(), "{}", String::from_utf8_lossy(&wt.stderr));
        let tree = wt.stdout_text().trim().to_string();

        let ls = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::LsTree { tree: tree.clone(), paths: vec![] }).await.unwrap();
        let ls_text = ls.stdout_text();
        assert!(ls_text.contains("a.txt\0") && ls_text.contains("new.txt\0") && !ls_text.contains("keep.txt"), "{ls_text:?}");
        // 生バイトのまま（CRLFを保つ）取り出せる。
        let blob = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::CatFileBlob { oid: oids[0].clone() }).await.unwrap();
        assert_eq!(blob.stdout, b"one\r\nTWO\r\nthree\r\n");
        // HEADの木との差。
        let names = git.run_snapshot(&info.toplevel, &env, &SnapshotOp::DiffTreeNames { a: head_tree(&git, &repo).await, b: tree.clone() }).await.unwrap();
        let names = names.stdout_text();
        assert!(names.contains("a.txt") && names.contains("new.txt") && names.contains("keep.txt"), "{names:?}");

        // 専用置き場には書かれ、ユーザーのリポジトリは何も変わらない。
        assert!(area.join("objects").read_dir().unwrap().count() > 0);
        assert!(area.join("tmp").join("i1").exists(), "the temp index lives in the AgentDock area");
        assert_eq!(repo_state(&repo), before, "the user's repository must not change");
        assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"one\r\nTWO\r\nthree\r\n");
        std::fs::remove_dir_all(&base).ok();
    }

    async fn head_tree(git: &Git, repo: &Path) -> String {
        let out = git.run(repo, &GitOp::RevParse { rev: "HEAD".into() }).await.unwrap();
        let commit = out.stdout_text().trim().to_string();
        let out = Command::new("git").current_dir(repo).args(["rev-parse", &format!("{commit}^{{tree}}")]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}
