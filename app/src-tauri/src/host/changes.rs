//! 変更ファイルの一覧・差分・「変更を戻す」（Git基準の控え、P3B-4、`app/DESIGN_P3B.md` §4・§7）。
//!
//! - 一覧・差分: 「控え基準」（区間の開始時 B と終了時 E、または現在 C の差）と「Git上の現在の差分（HEAD比較）」を混ぜずに出所つきで返す。
//!   読取りだけで、resumeしない。バックエンドの報告（fileChange・turn差分）は使わない（観測は廃止）。
//! - 戻す: 判定は `rules::baseline::plan`。ファイルごとに「選んだ区間の B の内容」へ書き戻す。実行は
//!   チャットの作業キュー → リポジトリ書込みロック → `FolderOpGuard`（リポジトリルート基準）の中で、再計画して `admit`（強制は要確認のまま
//!   理由が増えていないときだけ）→ **書換え前の内容を必ず `revert-backup` へ控える**（失敗なら何も変えない）→ 書込み直前に `recheck`
//!   → 成功したファイルだけ `Reverted` に記録する。作業中・停止未確認・受理不明・削除保留・外部実行中は、強制でも戻さない。
//! - 書込み先は作業ツリーだけ。ユーザーの索引・refs・stash・リポジトリのobjectsには書かない（git実行は必ず `SnapshotEnv` つき）。
//! - 旧方式の `changes.jsonl` は読まず、消さない。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use super::baseline::failure_text;
use super::baseline_snap::{probe_repo_info, RepoProbe};
use super::cloud::paths_overlap;
use super::persist::save_failure_message;
use super::state::{external_send_locked, HostData};
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::baseline::*;
use crate::backend::changes::*;
use crate::backend::ipc::*;
use crate::backend::model::*;
use crate::gitops::snapshot::{SnapshotEnv, SnapshotOp};
use crate::gitops::{Git, GitError, GitOp, GitOutput};
use crate::rules::baseline::{self as rb, CurrentFile, EntryState, ExecItem, PathPlan, PlanInput, RepoStatus, RevertMark, SegEnd, Segment, StateAt, WriteKind, WriteOp, WriteOutcome};
use crate::rules::diff::{parse_status_v2, split_files, StatusEntry};
use crate::store::{atomic, layout, Store, StoreError};

/// 計画の有効時間。
const PLAN_TTL_MS: i64 = 5 * 60 * 1000;
const MAX_PLANS: usize = 8;
/// 作業フォルダの外を指す作業フォルダ接頭辞（すべてのパスを「作業フォルダの外」にする）。
const OUTSIDE_PREFIX: &str = "<outside>";
/// `ls-tree` に一度に渡すパスの数・文字数の上限（Windowsのコマンドライン長の上限に余裕を持たせる）。
const LS_CHUNK_PATHS: usize = 200;
const LS_CHUNK_CHARS: usize = 12_000;

const NOTE_BASELINE: &str = "turnの開始時と終了時にAgentDockがGitで控えた内容の差です。.gitignore 対象・サブモジュール・作業フォルダ外は含みません。turnの最中に別のツールで変えた内容も含まれます。";
const NOTE_BASELINE_MID: &str = "このturnの終了時の控えがない（作業中・切断・再起動など）ため、次の控えまたは現在の状態との差です。途中の状態を含みます。";
const NOTE_GIT: &str = "Git上の現在の差分（HEADとの比較）です。AgentDockの控えとは別の情報で、コミット済みの変更は含まれません。";

struct StoredPlan {
    chat: ChatKey,
    from_turn: Option<ExternalId>,
    created_at: UnixMillis,
    repo_root: String,
    start_seg: Option<LocalId>,
    planned: Vec<PathPlan>,
}

/// 戻す計画の作業状態。
#[derive(Default)]
pub struct ChangesRuntime {
    plans: Mutex<HashMap<LocalId, StoredPlan>>,
    /// 戻す操作は同時に1件だけ。
    revert_lock: tokio::sync::Mutex<()>,
}

// ───────────────────────────── 純粋な部分 ─────────────────────────────

/// 区間の終了の記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EndRec {
    /// 終了未確認。
    Unknown,
    Snapshot { at: UnixMillis, snap: Snapshot },
    NextBase { at: UnixMillis },
}

/// 記録から組み立てた区間（基準を取れて、受理なしで外れていないもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SegRec {
    pub seg: LocalId,
    pub turn: Option<ExternalId>,
    pub started_at: UnixMillis,
    pub repo_root: String,
    pub cwd: String,
    pub base: Snapshot,
    pub end: EndRec,
    pub concurrent: Vec<ConcurrentWith>,
}

/// `segments.jsonl` の行から、区間と「戻し」の記録を組み立てる。Eのない区間は終了未確認のまま（推定で補わない）。
pub(super) fn build_segments(lines: &[BaselineLine]) -> (Vec<SegRec>, Vec<RevertMark>) {
    let mut segs: Vec<SegRec> = Vec::new();
    let mut abandoned: HashSet<String> = HashSet::new();
    let mut marks: Vec<RevertMark> = Vec::new();
    for l in lines {
        match l {
            BaselineLine::Started { seg, at, repo, cwd, base, .. } => segs.push(SegRec {
                seg: seg.clone(),
                turn: None,
                started_at: *at,
                repo_root: repo.root.clone(),
                cwd: cwd.clone(),
                base: base.clone(),
                end: EndRec::Unknown,
                concurrent: Vec::new(),
            }),
            BaselineLine::Bound { seg, turn } => {
                if let Some(s) = segs.iter_mut().find(|s| &s.seg == seg) {
                    s.turn = Some(turn.clone());
                }
            }
            BaselineLine::Ended { seg, at, end } => {
                if let Some(s) = segs.iter_mut().find(|s| &s.seg == seg) {
                    if s.end == EndRec::Unknown {
                        s.end = match end {
                            SegmentEnd::Snapshot { snapshot } => EndRec::Snapshot { at: *at, snap: snapshot.clone() },
                            SegmentEnd::EndIsNextBase => EndRec::NextBase { at: *at },
                        };
                    }
                }
            }
            BaselineLine::Abandoned { seg, .. } => {
                abandoned.insert(seg.0.clone());
            }
            BaselineLine::Concurrent { seg, with } => {
                if let Some(s) = segs.iter_mut().find(|s| &s.seg == seg) {
                    s.concurrent.push(with.clone());
                }
            }
            BaselineLine::Reverted { at, items, .. } => {
                for it in items {
                    marks.push(RevertMark { at: *at, path: it.path.clone(), restored: it.restored.clone() });
                }
            }
            BaselineLine::Failed { .. } => {}
        }
    }
    // 「次の基準で終了とする」は記録上の直後の区間だけで解決する。その送信が受理なしで外れた・別のリポジトリだった場合は、
    // さらに後の区間を使わず終了未確認にする（無関係な期間の変更を、このturnのものにしない）。
    for i in 0..segs.len() {
        if matches!(segs[i].end, EndRec::NextBase { .. }) {
            let ok = segs.get(i + 1).is_some_and(|n| !abandoned.contains(&n.seg.0) && rb::norm_path(&n.repo_root) == rb::norm_path(&segs[i].repo_root));
            if !ok && i + 1 < segs.len() {
                segs[i].end = EndRec::Unknown;
            }
        }
    }
    segs.retain(|s| !abandoned.contains(&s.seg.0));
    (segs, marks)
}

fn rule_segment(s: &SegRec, concurrent: Vec<rb::ConcurrentRec>) -> Segment {
    Segment {
        turn: s.turn.clone(),
        base_at: s.started_at,
        base_head: s.base.head.clone(),
        base_branch: s.base.branch.clone(),
        end: match &s.end {
            EndRec::Unknown => SegEnd::Unknown,
            EndRec::Snapshot { at, snap } => SegEnd::Snapshot { at: *at, head: snap.head.clone(), branch: snap.branch.clone() },
            EndRec::NextBase { .. } => SegEnd::NextBase,
        },
        concurrent,
    }
}

/// 区間の時間帯（終了未確認は上限なし＝重なりを広めに見る）。
fn seg_interval(s: &SegRec) -> (i64, i64) {
    let end = match &s.end {
        EndRec::Snapshot { at, .. } | EndRec::NextBase { at } => at.0,
        EndRec::Unknown => i64::MAX,
    };
    (s.started_at.0, end)
}

/// `from_turn` に対応する区間の添字（None＝最初の区間）。
fn start_index(segs: &[SegRec], from_turn: Option<&ExternalId>) -> Option<usize> {
    if segs.is_empty() {
        return None;
    }
    match from_turn {
        None => Some(0),
        Some(t) => segs.iter().position(|s| s.turn.as_ref() == Some(t)),
    }
}

/// 戻した最新の記録を判定する基準の時刻（最後の区間の終了、なければ開始）。`rules::baseline::plan` と同じ。
fn latest_at(segs: &[SegRec]) -> UnixMillis {
    let Some(last) = segs.last() else { return UnixMillis(0) };
    match &last.end {
        EndRec::Snapshot { at, .. } if *at > last.started_at => *at,
        _ => last.started_at,
    }
}

/// 控えの1時点（スナップショット）の、パスを引くための情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Point {
    pub tree: String,
    pub raw: HashSet<String>,
    pub skipped: HashMap<String, SkipReason>,
    pub head: Option<String>,
    pub branch: Option<String>,
}

impl Point {
    pub(super) fn of(s: &Snapshot) -> Point {
        Point { tree: s.tree.clone(), raw: s.raw.iter().cloned().collect(), skipped: s.skipped.iter().map(|k| (k.path.clone(), k.reason)).collect(), head: s.head.clone(), branch: s.branch.clone() }
    }
}

/// `ls-tree -z` の出力（`mode type oid\tpath` をNUL区切り）。
fn parse_ls_tree(out: &[u8]) -> HashMap<String, (String, String)> {
    String::from_utf8_lossy(out)
        .split('\0')
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut it = meta.split_whitespace();
            let mode = it.next()?;
            let _kind = it.next()?;
            let oid = it.next()?;
            Some((path.to_string(), (mode.to_string(), oid.to_string())))
        })
        .collect()
}

/// NUL区切りのパス名の一覧。
fn parse_nul_names(out: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(out).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// ある時点のパスの状態。リンク・サブモジュールは控えの対象外（書き戻さない）。
fn entry_state(p: &Point, ls: &HashMap<String, (String, String)>, path: &str) -> EntryState {
    if let Some(r) = p.skipped.get(path) {
        return EntryState::Skipped(*r);
    }
    match ls.get(path) {
        None => EntryState::Absent,
        Some((mode, _)) if mode == "120000" => EntryState::Skipped(SkipReason::Link),
        Some((mode, _)) if mode == "160000" => EntryState::Skipped(SkipReason::Submodule),
        Some((_, oid)) => EntryState::Present(EntryRef { oid: oid.clone(), form: if p.raw.contains(path) { EntryForm::Raw } else { EntryForm::Head } }),
    }
}

fn kind_of(a: &EntryState, b: &EntryState) -> ChangeKind {
    match (a, b) {
        (EntryState::Absent, EntryState::Present(_)) => ChangeKind::Added,
        (EntryState::Present(_), EntryState::Absent) => ChangeKind::Deleted,
        _ => ChangeKind::Modified,
    }
}

fn chunk_paths(paths: &[String]) -> Vec<&[String]> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0usize, 0usize);
    for (i, p) in paths.iter().enumerate() {
        if i > start && (i - start >= LS_CHUNK_PATHS || chars + p.len() > LS_CHUNK_CHARS) {
            out.push(&paths[start..i]);
            start = i;
            chars = 0;
        }
        chars += p.len() + 1;
    }
    if start < paths.len() {
        out.push(&paths[start..]);
    }
    out
}

/// リポジトリ相対パス（`/` 区切り）をIPCへ出す絶対パスにする。
fn abs_of(root: &str, rel: &str) -> String {
    format!("{}/{}", root.trim_end_matches('/'), rel)
}

/// IPCで受け取ったパス（絶対でも相対でも）をリポジトリ相対（`/` 区切り）にする。リポジトリの外の絶対パスはそのまま残す（後段が止める）。
fn to_rel(root: &str, p: &str) -> String {
    let pn = p.replace('\\', "/");
    let rn = root.replace('\\', "/");
    let rn = rn.trim_end_matches('/');
    if pn.to_lowercase().starts_with(&format!("{}/", rn.to_lowercase())) {
        if let Some(rest) = pn.get(rn.len() + 1..) {
            return rest.to_string();
        }
    }
    pn
}

/// 作業フォルダ（チャットのcwd）のリポジトリ相対パス。リポジトリの外なら、すべてを「作業フォルダの外」にする値。
fn work_prefix(root: &str, cwd: Option<&str>) -> String {
    let Some(cwd) = cwd else { return OUTSIDE_PREFIX.to_string() };
    let canon = |s: &str| -> String {
        let p = std::fs::canonicalize(s).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| s.to_string());
        p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', "/").trim_end_matches('/').to_string()
    };
    let (r, c) = (canon(root), canon(cwd));
    let (rl, cl) = (r.to_lowercase(), c.to_lowercase());
    if cl == rl {
        String::new()
    } else if cl.starts_with(&format!("{rl}/")) {
        c.get(r.len() + 1..).map(str::to_string).unwrap_or_else(|| OUTSIDE_PREFIX.to_string())
    } else {
        OUTSIDE_PREFIX.to_string()
    }
}

/// 控えの最新時点より後に戻した記録のうち、現在の状態と一致するもの＝「戻し済み」（`rules::baseline::plan` の S5 と同じ）。
fn is_reverted(marks: &[RevertMark], latest: UnixMillis, path: &str, cur: &EntryState) -> bool {
    marks.iter().filter(|m| rb::norm_path(&m.path) == rb::norm_path(path) && m.at >= latest).max_by_key(|m| m.at).is_some_and(|m| {
        let st = match &m.restored {
            Restored::Entry { entry } => EntryState::Present(entry.clone()),
            Restored::Absent => EntryState::Absent,
        };
        &st == cur
    })
}

/// 作業ツリーのファイル1つの現在の状態。
struct FileInfo {
    file: CurrentFile,
    skip: Option<SkipReason>,
}

fn file_info(p: &Path) -> FileInfo {
    let plain = |file: CurrentFile| FileInfo { file, skip: None };
    let skipped = |r: SkipReason| FileInfo { file: CurrentFile::Directory, skip: Some(r) };
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => plain(CurrentFile::Missing),
        Err(_) => skipped(SkipReason::Unreadable),
        Ok(m) => {
            let ft = m.file_type();
            if ft.is_symlink() {
                skipped(SkipReason::Link)
            } else if ft.is_dir() {
                plain(CurrentFile::Directory)
            } else if ft.is_file() {
                if m.len() > BASELINE_FILE_LIMIT {
                    skipped(SkipReason::TooLarge)
                } else {
                    match std::fs::read(p) {
                        Ok(b) => plain(CurrentFile::File { sha: rb::sha256_hex(&b) }),
                        Err(_) => skipped(SkipReason::Unreadable),
                    }
                }
            } else {
                skipped(SkipReason::Unreadable)
            }
        }
    }
}

/// 解決（親のcanonicalize。対象がなければ最寄りの既存の祖先）した位置が、作業フォルダ（canonicalize後）の内側か。
/// リンク・ジャンクションは解決され、外へ出れば `Err`。作業フォルダが分からない・解決できないときも `Err`（書き換えない）。
pub(super) fn confine_to(cwd: Option<&str>, p: &str) -> Result<(), String> {
    fn plain(s: &Path) -> String {
        let t = s.to_string_lossy().replace('/', "\\").to_lowercase();
        t.strip_prefix(r"\\?\").unwrap_or(&t).trim_end_matches('\\').to_string()
    }
    let Some(cwd) = cwd else { return Err("作業フォルダが分かりません".into()) };
    let root = std::fs::canonicalize(cwd).map_err(|e| format!("作業フォルダを解決できません: {e}"))?;
    let mut probe = Path::new(p);
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    // 存在する最も近い祖先まで遡る（対象自身が存在すればそれ。リンクなら解決される）。
    while std::fs::symlink_metadata(probe).is_err() {
        let (Some(name), Some(parent)) = (probe.file_name(), probe.parent()) else { return Err("既存の親フォルダが見つかりません".into()) };
        rest.push(name);
        probe = parent;
    }
    let mut resolved = std::fs::canonicalize(probe).map_err(|e| format!("パスを解決できません: {e}"))?;
    for n in rest.iter().rev() {
        resolved.push(n);
    }
    let (r, t) = (plain(&root), plain(&resolved));
    if t == r || t.starts_with(&format!("{r}\\")) {
        Ok(())
    } else {
        Err("解決した位置が作業フォルダの外です".into())
    }
}

/// 判定の入力（所有データ）。ファイルの読込みと判定を、ブロッキング処理でまとめて行う。
struct PlanData {
    repo: RepoStatus,
    git_busy: bool,
    work_prefix: String,
    segments: Vec<Segment>,
    from_turn: Option<ExternalId>,
    head: Option<String>,
    branch: Option<String>,
    marks: Vec<RevertMark>,
    paths: Vec<String>,
    states: HashMap<(StateAt, String), EntryState>,
    top: PathBuf,
    cwd: Option<String>,
}

/// 対象パスの現在のファイルを読み（上限つき）、`rules::baseline::plan` で判定する。作業フォルダの外のパスは読まない。
fn run_plan(d: PlanData) -> Vec<PathPlan> {
    let confine = |p: &str| confine_to(d.cwd.as_deref(), &d.top.join(p).to_string_lossy());
    let mut files: HashMap<String, FileInfo> = HashMap::new();
    for p in &d.paths {
        if rb::check_inside(p, &d.work_prefix, &confine).is_ok() {
            files.insert(p.clone(), file_info(&d.top.join(p)));
        }
    }
    let mut states = d.states;
    for (p, fi) in &files {
        if let Some(r) = fi.skip {
            states.insert((StateAt::Current, p.clone()), EntryState::Skipped(r));
        }
    }
    let st = |at: StateAt, p: &str| states.get(&(at, p.to_string())).cloned().unwrap_or(EntryState::Absent);
    let fl = |p: &str| files.get(p).map(|f| f.file.clone()).unwrap_or(CurrentFile::Missing);
    rb::plan(&PlanInput {
        repo: d.repo,
        git_busy: d.git_busy,
        work_prefix: d.work_prefix.clone(),
        segments: &d.segments,
        from_turn: d.from_turn.clone(),
        current_head: d.head.clone(),
        current_branch: d.branch.clone(),
        reverted: &d.marks,
        paths: &d.paths,
        states: &st,
        files: &fl,
        confine: &confine,
    })
}

// ───────────────────────────── Git の読取り（控え用の環境つき） ─────────────────────────────

pub(super) enum GitCtx {
    Ready { root: PathBuf, git: Git },
    NotSupported(String),
    NotFetched(String),
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or("").chars().take(200).collect()
}

pub(super) fn git_error_status(e: &GitError) -> ListStatus {
    match e {
        GitError::Unavailable(_) => ListStatus::NotSupported { message: "Gitを実行できません（未導入、または設定のGitの場所が違います）".into() },
        other => ListStatus::NotFetched { message: format!("Gitから取得できませんでした（{other}）") },
    }
}

enum OpenFail {
    NoStore,
    NotARepository,
    GitUnavailable,
    WorkFolderUnknown,
    Other(String),
}

impl OpenFail {
    fn text(&self) -> String {
        match self {
            OpenFail::NoStore => "保存先が使えないため、変更の控えを読めません".into(),
            OpenFail::NotARepository => failure_text(&BaselineFailure::NotARepository),
            OpenFail::GitUnavailable => failure_text(&BaselineFailure::GitUnavailable),
            OpenFail::WorkFolderUnknown => failure_text(&BaselineFailure::WorkFolderUnknown),
            OpenFail::Other(m) => m.clone(),
        }
    }
    fn status(&self) -> ListStatus {
        match self {
            OpenFail::NotARepository | OpenFail::GitUnavailable | OpenFail::WorkFolderUnknown => ListStatus::NotSupported { message: self.text() },
            _ => ListStatus::NotFetched { message: self.text() },
        }
    }
    fn ipc(&self) -> IpcError {
        match self {
            OpenFail::NotARepository | OpenFail::GitUnavailable | OpenFail::WorkFolderUnknown => err(IpcErrorCode::Unsupported, self.text()),
            _ => err(IpcErrorCode::Io, self.text()),
        }
    }
}

/// 控えの読取り・現在状態の取得の作業単位。呼出し側がチャットの作業キューとリポジトリロックを持った状態で使う。
struct Session {
    host: Arc<Host>,
    chat: ChatKey,
    store: Arc<Store>,
    git: Git,
    probe: RepoProbe,
    env: SnapshotEnv,
    /// リポジトリのルート（`/` 区切り）。
    root: String,
    top: PathBuf,
    current: Option<Point>,
}

impl Session {
    async fn open(host: &Arc<Host>, chat: &ChatKey, cwd: &str) -> Result<Session, OpenFail> {
        let store = host.persist.store().cloned().ok_or(OpenFail::NoStore)?;
        let tool = host.read(|d| d.settings.tools.git.clone());
        let git = Git::new(tool.as_deref());
        let limit = Duration::from_millis(BASELINE_TIME_LIMIT_MS);
        let probe = tokio::time::timeout(limit, probe_repo_info(&git, Path::new(cwd)))
            .await
            .map_err(|_| OpenFail::Other(failure_text(&BaselineFailure::Timeout)))?
            .map_err(|f| match f {
                BaselineFailure::NotARepository => OpenFail::NotARepository,
                BaselineFailure::GitUnavailable => OpenFail::GitUnavailable,
                BaselineFailure::WorkFolderUnknown => OpenFail::WorkFolderUnknown,
                other => OpenFail::Other(failure_text(&other)),
            })?;
        let (objects, tmp) = store.ensure_baseline_dirs(chat).map_err(|e| OpenFail::Other(save_failure_message(&e)))?;
        let env = SnapshotEnv::new(objects, probe.info.objects_dir(), tmp.join("read-idx")).map_err(|e| OpenFail::Other(e.to_string()))?;
        let root = probe.repo_ref().root;
        let top = probe.info.toplevel.clone();
        Ok(Session { host: host.clone(), chat: chat.clone(), store, git, probe, env, root, top, current: None })
    }

    fn busy(&self) -> bool {
        !self.probe.info.busy_markers().is_empty()
    }

    async fn run_in(&self, env: &SnapshotEnv, op: &SnapshotOp) -> Result<GitOutput, String> {
        let out = self.git.run_snapshot(&self.top, env, op).await.map_err(|e| e.to_string())?;
        if out.success() {
            Ok(out)
        } else {
            Err(first_line(&out.stderr))
        }
    }

    async fn run(&self, op: &SnapshotOp) -> Result<GitOutput, String> {
        self.run_in(&self.env, op).await
    }

    /// 他のチャットの専用置き場を使うための環境（読取り専用）。
    fn env_for(&self, chat: &ChatKey) -> Result<SnapshotEnv, String> {
        SnapshotEnv::new(self.store.baseline_objects_dir(chat), self.probe.info.objects_dir(), self.store.baseline_tmp_dir(chat).join("read-idx")).map_err(|e| e.to_string())
    }

    /// 木の中のパスのエントリを引く。パスが空なら何も引かない（全体は引かない）。
    async fn ls(&self, tree: &str, paths: &[String]) -> Result<HashMap<String, (String, String)>, String> {
        let mut all = HashMap::new();
        for chunk in chunk_paths(paths) {
            let out = self.run(&SnapshotOp::LsTree { tree: tree.to_string(), paths: chunk.to_vec() }).await?;
            all.extend(parse_ls_tree(&out.stdout));
        }
        Ok(all)
    }

    /// 2つの時点の間で変わったパス。木の差に加え、形（生／HEAD）の違いと、控えの対象外になった／戻った違いも含める。
    async fn names_with(&self, env: &SnapshotEnv, a: &Point, b: &Point) -> Result<BTreeSet<String>, String> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        if a.tree != b.tree {
            let out = self.run_in(env, &SnapshotOp::DiffTreeNames { a: a.tree.clone(), b: b.tree.clone() }).await?;
            set.extend(parse_nul_names(&out.stdout));
        }
        set.extend(a.raw.symmetric_difference(&b.raw).cloned());
        for k in a.skipped.keys().chain(b.skipped.keys()) {
            if a.skipped.get(k) != b.skipped.get(k) {
                set.insert(k.clone());
            }
        }
        Ok(set)
    }

    async fn names(&self, a: &Point, b: &Point) -> Result<BTreeSet<String>, String> {
        self.names_with(&self.env, a, b).await
    }

    /// 現在の状態 C（専用置き場に書くだけで、作業ツリーとリポジトリは変えない）。一度取ったものは使い回す。
    async fn current(&mut self) -> Result<Point, String> {
        if let Some(p) = &self.current {
            return Ok(p.clone());
        }
        let snap = self.host.baseline_capture_locked(&self.chat, &self.store, &self.git, &self.probe, Instant::now()).await.map_err(|f| failure_text(&f))?;
        let p = Point::of(&snap);
        self.current = Some(p.clone());
        Ok(p)
    }

    /// 区間jの終了の時点。Eがなければ次の基準、最後の区間なら現在。`allow_current` が偽で現在が要るときは None。
    async fn end_point(&mut self, segs: &[SegRec], j: usize, allow_current: bool) -> Result<Option<Point>, String> {
        match &segs[j].end {
            EndRec::Snapshot { snap, .. } => Ok(Some(Point::of(snap))),
            _ if j + 1 < segs.len() => Ok(Some(Point::of(&segs[j + 1].base))),
            _ if allow_current => Ok(Some(self.current().await?)),
            _ => Ok(None),
        }
    }

    /// 同時作業の相手の記録を、判定の入力にする（相手の区間で変わったパスと、相手の終了が分かるか）。
    async fn concurrent_recs(&self, segs: &[SegRec], j: usize) -> Vec<rb::ConcurrentRec> {
        let mut recs = Vec::new();
        for c in &segs[j].concurrent {
            match c {
                ConcurrentWith::External { .. } => recs.push(rb::ConcurrentRec::External),
                ConcurrentWith::Chat { chat } => recs.push(self.chat_concurrent(&segs[j], chat).await),
            }
        }
        recs
    }

    async fn chat_concurrent(&self, mine: &SegRec, other: &ChatKey) -> rb::ConcurrentRec {
        let unknown = rb::ConcurrentRec::Chat { other_changed: Vec::new(), other_end_known: false };
        let Ok(lines) = self.store.read_baselines(other) else { return unknown };
        let (osegs, _) = build_segments(&lines);
        let osegs: Vec<SegRec> = osegs.into_iter().filter(|s| rb::norm_path(&s.repo_root) == rb::norm_path(&self.root)).collect();
        let Ok(env) = self.env_for(other) else { return unknown };
        let (s0, e0) = seg_interval(mine);
        let (mut changed, mut known, mut any) = (BTreeSet::new(), true, false);
        for (oi, o) in osegs.iter().enumerate() {
            if !o.concurrent.iter().any(|c| matches!(c, ConcurrentWith::Chat { chat } if chat == &self.chat)) {
                continue;
            }
            let (s1, e1) = seg_interval(o);
            if !(s1 <= e0 && s0 <= e1) {
                continue;
            }
            any = true;
            let x = match &o.end {
                EndRec::Snapshot { snap, .. } => Point::of(snap),
                EndRec::NextBase { .. } if oi + 1 < osegs.len() => Point::of(&osegs[oi + 1].base),
                _ => {
                    // 相手の終了が不明: 変わったかもしれない（推定しない）。
                    known = false;
                    continue;
                }
            };
            match self.names_with(&env, &Point::of(&o.base), &x).await {
                Ok(n) => changed.extend(n),
                Err(_) => known = false,
            }
        }
        if !any {
            return unknown;
        }
        rb::ConcurrentRec::Chat { other_changed: changed.into_iter().collect(), other_end_known: known }
    }

    /// 現在のファイルがstatusに「変更あり（生形）」と出ていても、その内容が、HEAD形の控え（BまたはE）をcheckoutと同じ変換
    /// （改行・smudge・LFS。`cat-file --filters`）に通した内容と同じなら、現在の状態をその控えのエントリとみなす。
    /// autocrlfなどでLFとCRLFが違うだけの内容を「別の変更」にしない。生形の控えは生バイトのまま比べる（ここでは扱わない）。
    async fn align_current_with_filtered(&self, states: &mut HashMap<(StateAt, String), EntryState>, paths: &[String], k: usize, n: usize, prefix: &str, cwd: Option<&str>) {
        for p in paths {
            if !matches!(states.get(&(StateAt::Current, p.clone())), Some(EntryState::Present(EntryRef { form: EntryForm::Raw, .. }))) {
                continue;
            }
            // 作業フォルダの外のパスは読まない。
            let confine = |q: &str| confine_to(cwd, &self.top.join(q).to_string_lossy());
            if rb::check_inside(p, prefix, &confine).is_err() {
                continue;
            }
            let abs = self.top.join(p);
            let Ok(info) = tokio::task::spawn_blocking(move || file_info(&abs)).await else { continue };
            let CurrentFile::File { sha } = info.file else { continue };
            let mut order = vec![StateAt::Base(k)];
            if matches!(states.get(&(StateAt::End(n), p.clone())), Some(EntryState::Present(_))) {
                order.push(StateAt::End(n));
            }
            for at in order {
                let Some(EntryState::Present(e)) = states.get(&(at, p.clone())).cloned() else { continue };
                if e.form != EntryForm::Head {
                    continue;
                }
                let Ok(out) = self.run(&SnapshotOp::CatFileFiltered { path: p.clone(), oid: e.oid.clone() }).await else { continue };
                if rb::sha256_hex(&out.stdout) == sha {
                    states.insert((StateAt::Current, p.clone()), EntryState::Present(e));
                    break;
                }
            }
        }
    }

    /// 戻す計画を作る（読取りのみ）。`req` が None なら、選んだ区間以降で変わったすべてのパスが対象。
    /// 戻り値は判定と、選んだ区間（見つからなければ None）。
    async fn plan(&mut self, segs: &[SegRec], marks: &[RevertMark], from_turn: Option<&ExternalId>, req: Option<Vec<String>>, cwd: Option<String>) -> Result<(Vec<PathPlan>, Option<LocalId>), IpcError> {
        let gerr = |m: String| err(IpcErrorCode::Io, format!("Gitの情報を取得できませんでした（{m}）"));
        let k = start_index(segs, from_turn);
        let busy = self.busy();
        let prefix = work_prefix(&self.root, cwd.as_deref());
        let mut rule_segs: Vec<Segment> = Vec::new();
        let mut states: HashMap<(StateAt, String), EntryState> = HashMap::new();
        let (mut head, mut branch) = (None, None);
        let mut paths: Vec<String> = req.clone().unwrap_or_default();
        if let Some(k) = k {
            let n = segs.len() - 1;
            for (j, s) in segs.iter().enumerate() {
                let conc = if j >= k && !busy { self.concurrent_recs(segs, j).await } else { Vec::new() };
                rule_segs.push(rule_segment(s, conc));
            }
            // 時点（基準・終了・現在）。
            let mut pts: Vec<(StateAt, Point)> = Vec::new();
            for j in k..=n {
                pts.push((StateAt::Base(j), Point::of(&segs[j].base)));
                if let EndRec::Snapshot { snap, .. } = &segs[j].end {
                    pts.push((StateAt::End(j), Point::of(snap)));
                }
            }
            if !busy {
                let cur = self.current().await.map_err(gerr)?;
                head = cur.head.clone();
                branch = cur.branch.clone();
                pts.push((StateAt::Current, cur));
            }
            if req.is_none() {
                let mut set: BTreeSet<String> = BTreeSet::new();
                for j in k..=n {
                    if let Some(x) = self.end_point(segs, j, !busy).await.map_err(gerr)? {
                        set.extend(self.names(&Point::of(&segs[j].base), &x).await.map_err(gerr)?);
                    }
                }
                paths = set.into_iter().collect();
            }
            if !busy {
                let mut by_tree: HashMap<String, HashMap<String, (String, String)>> = HashMap::new();
                for (_, pt) in &pts {
                    if !by_tree.contains_key(&pt.tree) {
                        let ls = self.ls(&pt.tree, &paths).await.map_err(gerr)?;
                        by_tree.insert(pt.tree.clone(), ls);
                    }
                }
                for (at, pt) in &pts {
                    let ls = &by_tree[&pt.tree];
                    for p in &paths {
                        states.insert((*at, p.clone()), entry_state(pt, ls, p));
                    }
                }
                self.align_current_with_filtered(&mut states, &paths, k, n, &prefix, cwd.as_deref()).await;
            }
        }
        let data = PlanData {
            repo: RepoStatus::Ready,
            git_busy: busy,
            work_prefix: prefix,
            segments: rule_segs,
            from_turn: from_turn.cloned(),
            head,
            branch,
            marks: marks.to_vec(),
            paths,
            states,
            top: self.top.clone(),
            cwd,
        };
        let plans = tokio::task::spawn_blocking(move || run_plan(data)).await.map_err(|e| err(IpcErrorCode::Io, format!("計画の作成に失敗しました: {e}")))?;
        Ok((plans, k.map(|k| segs[k].seg.clone())))
    }
}

/// 読み込んだ控えの記録。
struct Loaded {
    segs: Vec<SegRec>,
    marks: Vec<RevertMark>,
    last_failure: Option<BaselineFailure>,
}

fn load_from(store: &Store, chat: &ChatKey) -> Result<Loaded, String> {
    let lines = store.read_baselines(chat).map_err(|e| save_failure_message(&e))?;
    let last_failure = lines
        .iter()
        .rev()
        .find_map(|l| match l {
            BaselineLine::Failed { reason, .. } => Some(reason.clone()),
            _ => None,
        });
    let (segs, marks) = build_segments(&lines);
    Ok(Loaded { segs, marks, last_failure })
}

fn no_baseline_text(l: &Loaded) -> String {
    match &l.last_failure {
        Some(f) => format!("このチャットの変更の控えがありません（{}）", failure_text(f)),
        None => "このチャットには変更の控えがありません（控えを取る前のturn、または設定で無効です）".into(),
    }
}

impl Host {
    pub(super) fn require_chat(&self, chat: &ChatKey) -> Result<(), IpcError> {
        if self.read(|d| d.chat(chat).is_none() && !d.locals.contains_key(chat)) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        Ok(())
    }

    async fn load_baseline(self: &Arc<Self>, chat: &ChatKey) -> Result<Loaded, String> {
        let Some(store) = self.persist.store().cloned() else { return Err("保存先が使えないため、変更の控えを読めません".into()) };
        let chat = chat.clone();
        tokio::task::spawn_blocking(move || load_from(&store, &chat)).await.map_err(|e| e.to_string())?
    }

    /// 控えの読取りに使う作業フォルダ（チャットの作業フォルダ、なければ最後の区間のもの）。
    fn baseline_cwd(&self, chat: &ChatKey, segs: &[SegRec]) -> Option<String> {
        self.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned())).or_else(|| segs.last().map(|s| s.cwd.clone()))
    }

    pub(super) async fn git_context(self: &Arc<Self>, chat: &ChatKey) -> GitCtx {
        let Some(cwd) = self.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned())) else {
            return GitCtx::NotSupported("作業フォルダが分からないため、Gitの差分を取得できません".into());
        };
        let tool = self.read(|d| d.settings.tools.git.clone());
        let git = Git::new(tool.as_deref());
        match git.run(Path::new(&cwd), &GitOp::RepoRoot).await {
            Err(e) => match git_error_status(&e) {
                ListStatus::NotSupported { message } => GitCtx::NotSupported(message),
                ListStatus::NotFetched { message } => GitCtx::NotFetched(message),
                ListStatus::Ready => GitCtx::NotFetched("Gitの状態を取得できませんでした".into()),
            },
            Ok(out) if !out.success() => GitCtx::NotSupported(format!("作業フォルダはGitのリポジトリではありません（{}）", first_line(&out.stderr))),
            Ok(out) => {
                let root = out.stdout_text().trim().to_string();
                if root.is_empty() {
                    return GitCtx::NotFetched("Gitのリポジトリの場所を取得できませんでした".into());
                }
                GitCtx::Ready { root: PathBuf::from(root), git }
            }
        }
    }

    async fn git_list(self: &Arc<Self>, chat: &ChatKey, scope: ChangeScope) -> ChangeList {
        let done = |status: ListStatus, files: Vec<ChangedFile>| ChangeList { scope: scope.clone(), source: ChangeSource::Git, status, files, notes: vec![NOTE_GIT.to_string()] };
        let (root, git) = match self.git_context(chat).await {
            GitCtx::Ready { root, git } => (root, git),
            GitCtx::NotSupported(message) => return done(ListStatus::NotSupported { message }, vec![]),
            GitCtx::NotFetched(message) => return done(ListStatus::NotFetched { message }, vec![]),
        };
        let entries: Vec<StatusEntry> = match git.run(&root, &GitOp::Status).await {
            Ok(out) if out.success() => parse_status_v2(&String::from_utf8_lossy(&out.stdout)),
            Ok(out) => return done(ListStatus::NotFetched { message: format!("Gitの状態を取得できませんでした（{}）", first_line(&out.stderr)) }, vec![]),
            Err(e) => return done(git_error_status(&e), vec![]),
        };
        // 行数はHEADとの差分から。HEADがない（初回コミット前）・取得に失敗したときは数えられないので「未取得」にする。
        let diff_files = match git.run(&root, &GitOp::DiffHead { path: None }).await {
            Ok(out) if out.success() => Some(split_files(&String::from_utf8_lossy(&out.stdout))),
            _ => None,
        };
        let files = entries
            .iter()
            .map(|e| {
                let found = diff_files.as_ref().and_then(|v| v.iter().find(|f| rb::norm_path(&f.path) == rb::norm_path(&e.path)));
                let binary = found.is_some_and(|f| f.binary);
                let counts = found.filter(|f| !f.binary).map(|f| (f.additions, f.deletions));
                let none = if binary { Known::Unsupported } else { Known::NotFetched };
                let (path, move_to) = match &e.old_path {
                    Some(old) => (old.clone(), Some(e.path.clone())),
                    None => (e.path.clone(), None),
                };
                ChangedFile {
                    path,
                    kind: e.kind(),
                    move_to,
                    source: ChangeSource::Git,
                    additions: counts.map_or(none.clone(), |c| Known::direct(c.0)),
                    deletions: counts.map_or(none, |c| Known::direct(c.1)),
                    turn: None,
                    reverted: false,
                    binary,
                }
            })
            .collect();
        done(ListStatus::Ready, files)
    }

    /// 範囲 A→X（控え基準）。`Turn` はそのturnの区間、`Chat` は最初の区間の基準から最新の控えまで。
    /// 戻り値は (A, X, 範囲の区間 j0..=j1, 終了が不明の区間を含むか)。リポジトリロックとチャットの作業キューを持った状態で呼ぶ。
    async fn baseline_range(s: &mut Session, segs: &[SegRec], scope: &ChangeScope) -> Result<(Point, Point, usize, usize, bool), ListStatus> {
        let (j0, j1) = match scope {
            ChangeScope::Turn { turn } => match segs.iter().position(|x| x.turn.as_ref() == Some(turn)) {
                Some(j) => (j, j),
                None => return Err(ListStatus::NotSupported { message: "このturnは控えを取る前のものです（または控えを取れませんでした）".into() }),
            },
            ChangeScope::Chat => (0, segs.len() - 1),
        };
        let fetch = |m: String| ListStatus::NotFetched { message: format!("Gitの情報を取得できませんでした（{m}）") };
        let a = Point::of(&segs[j0].base);
        let x = s.end_point(segs, j1, true).await.map_err(fetch)?.expect("current is allowed");
        let mid = segs[j0..=j1].iter().any(|g| g.end == EndRec::Unknown);
        Ok((a, x, j0, j1, mid))
    }

    /// 控え基準の一覧。
    async fn baseline_list(self: &Arc<Self>, chat: &ChatKey, scope: ChangeScope) -> ChangeList {
        let done = |status: ListStatus, files: Vec<ChangedFile>, notes: Vec<String>| ChangeList { scope: scope.clone(), source: ChangeSource::Baseline, status, files, notes };
        let loaded = match self.load_baseline(chat).await {
            Ok(l) => l,
            Err(m) => return done(ListStatus::NotFetched { message: m }, vec![], vec![]),
        };
        if loaded.segs.is_empty() {
            return done(ListStatus::NotSupported { message: no_baseline_text(&loaded) }, vec![], vec![]);
        }
        let Some(cwd) = self.baseline_cwd(chat, &loaded.segs) else { return done(OpenFail::WorkFolderUnknown.status(), vec![], vec![]) };
        let mut s = match Session::open(self, chat, &cwd).await {
            Ok(s) => s,
            Err(f) => return done(f.status(), vec![], vec![]),
        };
        let segs: Vec<SegRec> = loaded.segs.iter().filter(|g| rb::norm_path(&g.repo_root) == rb::norm_path(&s.root)).cloned().collect();
        if segs.is_empty() {
            return done(ListStatus::NotSupported { message: "この作業フォルダのリポジトリでは、変更の控えがありません".into() }, vec![], vec![]);
        }
        let lock = self.baseline_rt.repo_lock(&s.root);
        let _r = lock.read().await;
        let (a, x, j0, j1, mid) = match Self::baseline_range(&mut s, &segs, &scope).await {
            Ok(r) => r,
            Err(status) => return done(status, vec![], vec![]),
        };
        let fetch = |m: String| ListStatus::NotFetched { message: format!("Gitの情報を取得できませんでした（{m}）") };
        let names = match s.names(&a, &x).await {
            Ok(n) => n,
            Err(m) => return done(fetch(m), vec![], vec![]),
        };
        let paths: Vec<String> = names.into_iter().collect();
        // 各パスを最後に変えた区間のturn。
        let mut turn_of: HashMap<String, Option<ExternalId>> = HashMap::new();
        if matches!(scope, ChangeScope::Turn { .. }) {
            for p in &paths {
                turn_of.insert(p.clone(), segs[j0].turn.clone());
            }
        } else {
            for j in j0..=j1 {
                let (b, e) = (Point::of(&segs[j].base), match s.end_point(&segs, j, true).await {
                    Ok(Some(e)) => e,
                    _ => continue,
                });
                if let Ok(set) = s.names(&b, &e).await {
                    for p in set {
                        turn_of.insert(p, segs[j].turn.clone());
                    }
                }
            }
        }
        // 種類（追加・削除・変更）は、両端の木での有無から。
        let (la, lx) = match (s.ls(&a.tree, &paths).await, s.ls(&x.tree, &paths).await) {
            (Ok(la), Ok(lx)) => (la, lx),
            (Err(m), _) | (_, Err(m)) => return done(fetch(m), vec![], vec![]),
        };
        // 戻し済みの判定には現在の状態が要る（戻した記録があるときだけ取る）。
        let latest = latest_at(&segs);
        let cur_ls = if loaded.marks.iter().any(|m| m.at >= latest) {
            match s.current().await {
                Ok(c) => s.ls(&c.tree, &paths).await.ok().map(|ls| (c, ls)),
                Err(_) => None,
            }
        } else {
            None
        };
        // 行数は木の全体diffから（取れなければ「未取得」。0で代用しない）。
        let diff_files = match s.run(&SnapshotOp::DiffTreesAll { a: a.tree.clone(), b: x.tree.clone() }).await {
            Ok(out) => Some(split_files(&String::from_utf8_lossy(&out.stdout))),
            Err(_) => None,
        };
        let files: Vec<ChangedFile> = paths
            .iter()
            .map(|p| {
                let (sa, sx) = (entry_state(&a, &la, p), entry_state(&x, &lx, p));
                let found = diff_files.as_ref().and_then(|v| v.iter().find(|f| rb::norm_path(&f.path) == rb::norm_path(p)));
                let binary = found.is_some_and(|f| f.binary);
                let counts = found.filter(|f| !f.binary).map(|f| (f.additions, f.deletions));
                let none = if binary { Known::Unsupported } else { Known::NotFetched };
                let reverted = cur_ls.as_ref().is_some_and(|(c, ls)| is_reverted(&loaded.marks, latest, p, &entry_state(c, ls, p)));
                ChangedFile {
                    path: abs_of(&s.root, p),
                    kind: kind_of(&sa, &sx),
                    move_to: None,
                    source: ChangeSource::Baseline,
                    additions: counts.map_or(none.clone(), |c| Known::direct(c.0)),
                    deletions: counts.map_or(none, |c| Known::direct(c.1)),
                    turn: turn_of.get(p).cloned().flatten(),
                    reverted,
                    binary,
                }
            })
            .collect();
        let mut notes = vec![NOTE_BASELINE.to_string()];
        if mid {
            notes.push(NOTE_BASELINE_MID.to_string());
        }
        done(ListStatus::Ready, files, notes)
    }

    /// 変更ファイルの一覧。`Baseline` は控え基準（`scope` で範囲）、`Git` は作業ツリーのHEAD比較（`scope` は無視）。読取りだけで、resumeしない。
    pub async fn get_change_list(self: &Arc<Self>, args: GetChangeListArgs) -> Result<ChangeList, IpcError> {
        self.require_chat(&args.chat)?;
        Ok(match args.source {
            ChangeSource::Git => self.git_list(&args.chat, args.scope).await,
            ChangeSource::Baseline => self.baseline_list(&args.chat, args.scope).await,
        })
    }

    /// 1ファイルの差分（統一diff）。出所は一覧と同じく混ぜない。
    pub async fn get_file_diff(self: &Arc<Self>, args: GetFileDiffArgs) -> Result<UnifiedDiff, IpcError> {
        self.require_chat(&args.chat)?;
        let out = |status: ListStatus, text: String| UnifiedDiff { path: args.path.clone(), source: args.source, status, text };
        match args.source {
            ChangeSource::Baseline => {
                let loaded = match self.load_baseline(&args.chat).await {
                    Ok(l) => l,
                    Err(m) => return Ok(out(ListStatus::NotFetched { message: m }, String::new())),
                };
                if loaded.segs.is_empty() {
                    return Ok(out(ListStatus::NotSupported { message: no_baseline_text(&loaded) }, String::new()));
                }
                let Some(cwd) = self.baseline_cwd(&args.chat, &loaded.segs) else { return Ok(out(OpenFail::WorkFolderUnknown.status(), String::new())) };
                let mut s = match Session::open(self, &args.chat, &cwd).await {
                    Ok(s) => s,
                    Err(f) => return Ok(out(f.status(), String::new())),
                };
                let segs: Vec<SegRec> = loaded.segs.iter().filter(|g| rb::norm_path(&g.repo_root) == rb::norm_path(&s.root)).cloned().collect();
                if segs.is_empty() {
                    return Ok(out(ListStatus::NotSupported { message: "この作業フォルダのリポジトリでは、変更の控えがありません".into() }, String::new()));
                }
                let lock = self.baseline_rt.repo_lock(&s.root);
                let _r = lock.read().await;
                let scope = match &args.turn {
                    Some(t) => ChangeScope::Turn { turn: t.clone() },
                    None => ChangeScope::Chat,
                };
                let (a, x, ..) = match Self::baseline_range(&mut s, &segs, &scope).await {
                    Ok(r) => r,
                    Err(status) => return Ok(out(status, String::new())),
                };
                let rel = to_rel(&s.root, &args.path);
                match s.run(&SnapshotOp::DiffTrees { a: a.tree, b: x.tree, path: rel }).await {
                    Ok(o) if o.stdout.is_empty() => Ok(out(ListStatus::NotSupported { message: "この範囲の差分がありません（内容が同じ、または控えの対象外です）".into() }, String::new())),
                    Ok(o) => Ok(out(ListStatus::Ready, o.stdout_text())),
                    Err(m) => Ok(out(ListStatus::NotFetched { message: format!("控えの差分を取得できませんでした（{m}）") }, String::new())),
                }
            }
            ChangeSource::Git => {
                let (root, git) = match self.git_context(&args.chat).await {
                    GitCtx::Ready { root, git } => (root, git),
                    GitCtx::NotSupported(message) => return Ok(out(ListStatus::NotSupported { message }, String::new())),
                    GitCtx::NotFetched(message) => return Ok(out(ListStatus::NotFetched { message }, String::new())),
                };
                match git.run(&root, &GitOp::DiffHead { path: Some(args.path.clone()) }).await {
                    Ok(o) if o.success() && o.stdout.is_empty() => {
                        Ok(out(ListStatus::NotSupported { message: "Gitの差分がありません（未追跡のファイル、または変更のない状態です）".into() }, String::new()))
                    }
                    Ok(o) if o.success() => Ok(out(ListStatus::Ready, o.stdout_text())),
                    Ok(o) => Ok(out(ListStatus::NotFetched { message: format!("Gitの差分を取得できませんでした（{}）", first_line(&o.stderr)) }, String::new())),
                    Err(e) => Ok(out(git_error_status(&e), String::new())),
                }
            }
        }
    }

    // ───────────── 変更を戻す ─────────────

    /// 戻す操作を止める条件。作業中・停止未確認・送信の受理不明・同じリポジトリの別チャットの作業中・削除保留・外部実行中。
    /// 強制の有無にかかわらず適用する。
    fn revert_blocker(&self, chat: &ChatKey, repo_root: &str) -> Option<IpcError> {
        if let Err(e) = self.check_not_delete_pending(chat) {
            return Some(e);
        }
        // 作業中・停止未確認、または受理不明の操作（レビュー・圧縮。turnが動いている可能性）がある。
        let busy = |d: &HostData, c: &ChatKey| {
            d.agents.iter().any(|v| &v.agent.chat == c && (d.running_turn.contains_key(&v.agent.key) || matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)))
                || d.open_stop(c).is_some() || d.locals.get(c).is_some_and(|l| !l.pending_ops.is_empty())
        };
        let (this_busy, ext, others_busy) = self.read(|d| {
            let ext = d.chat(chat).is_some_and(|c| external_send_locked(c.origin, d.root_view(chat).map(|v| v.freshness)));
            // 親・子フォルダの関係にある作業フォルダも、リポジトリと同じ場所として扱う。
            let others = d.chats.iter().any(|c| &c.key != chat && c.cwd.value().is_some_and(|x| paths_overlap(x, repo_root)) && busy(d, &c.key));
            (busy(d, chat), ext, others)
        });
        if ext {
            return Some(blocked(BlockedReason::ExternalRunning, "外部で作成された会話は閲覧のみです。再開して状態を確認するまで、変更は戻せません"));
        }
        if this_busy || self.unknown_attempt(chat).is_some() {
            return Some(blocked(BlockedReason::ChatBusy, "作業中・停止未確認、または送信の受理が未確認のため、変更は戻せません。完了または停止の確認後に実行してください"));
        }
        if others_busy {
            return Some(blocked(BlockedReason::ChatBusy, "同じリポジトリで作業中の別のチャットがあるため、変更は戻せません"));
        }
        // 終端は観測したが、終了の控え（E）をまだ取っていない区間（このチャット・同じリポジトリの別チャット）。今戻すと、戻した書込みがEに入る。
        if self.baseline_rt.pending_end_roots().iter().any(|(c, r)| c == chat || paths_overlap(r, repo_root)) {
            return Some(blocked(BlockedReason::ChatBusy, "turnの終了時の控えを取っている最中のため、変更は戻せません。少し待ってから実行してください"));
        }
        None
    }

    /// 戻す計画の作成（読取りのみ）。ファイルごとに戻せるか・要確認か・戻せない理由を返す。計画は5分間有効。
    pub async fn preview_revert(self: &Arc<Self>, args: PreviewRevertArgs) -> Result<RevertPlan, IpcError> {
        self.require_chat(&args.chat)?;
        let loaded = self.load_baseline(&args.chat).await.map_err(|m| err(IpcErrorCode::Io, m))?;
        if loaded.segs.is_empty() {
            return Err(err(IpcErrorCode::NotFound, no_baseline_text(&loaded)));
        }
        let cwd = self.baseline_cwd(&args.chat, &loaded.segs).ok_or_else(|| OpenFail::WorkFolderUnknown.ipc())?;
        // 読取りだけ。作業キューは取らず、リポジトリの読取りロックだけで行う（送信前の控えを長く待たせない）。
        let mut s = Session::open(self, &args.chat, &cwd).await.map_err(|f| f.ipc())?;
        let segs: Vec<SegRec> = loaded.segs.iter().filter(|g| rb::norm_path(&g.repo_root) == rb::norm_path(&s.root)).cloned().collect();
        let lock = self.baseline_rt.repo_lock(&s.root);
        let _r = lock.read().await;
        let req = args.paths.as_ref().map(|v| v.iter().map(|p| to_rel(&s.root, p)).collect::<Vec<_>>());
        let (planned, start_seg) = s.plan(&segs, &loaded.marks, args.turn.as_ref(), req, Some(cwd)).await?;
        if start_seg.is_none() && planned.is_empty() {
            return Err(err(IpcErrorCode::NotFound, "このturnは控えを取る前のものです（または控えを取れませんでした）"));
        }
        let now = now_ms();
        let id = self.local_id("rvp");
        let items = planned.iter().map(|p| to_item(&s.root, p)).collect();
        {
            let mut plans = self.changes_rt.plans.lock().unwrap();
            plans.retain(|_, p| now.0 - p.created_at.0 < PLAN_TTL_MS);
            while plans.len() >= MAX_PLANS {
                let Some(oldest) = plans.iter().min_by_key(|(_, p)| p.created_at).map(|(k, _)| k.clone()) else { break };
                plans.remove(&oldest);
            }
            plans.insert(id.clone(), StoredPlan { chat: args.chat.clone(), from_turn: args.turn.clone(), created_at: now, repo_root: s.root.clone(), start_seg, planned });
        }
        Ok(RevertPlan { id, chat: args.chat, items, created_at: now, expires_at: UnixMillis(now.0 + PLAN_TTL_MS) })
    }

    /// 変更を戻す（確認画面の後だけ）。実行直前に再計画し、強制は「要確認のまま理由が増えていない」ファイルだけ受け付ける。
    /// 書換え前の内容を控えられなければ何も変えない。書込み直前に再照合し、成功したファイルだけ記録する。
    pub async fn revert_changes(self: &Arc<Self>, args: RevertChangesArgs, _confirmed: &UserConfirmed) -> Result<RevertResult, IpcError> {
        self.require_chat(&args.chat)?;
        let _one_at_a_time = self.changes_rt.revert_lock.lock().await;
        let stale = || blocked(BlockedReason::PlanStale, "戻す計画が古くなったか、見つかりません。計画を作り直してください");
        let stored = {
            let mut plans = self.changes_rt.plans.lock().unwrap();
            match plans.remove(&args.plan_id) {
                Some(p) if p.chat == args.chat && now_ms().0 - p.created_at.0 < PLAN_TTL_MS => p,
                _ => return Err(stale()),
            }
        };
        if args.paths.is_empty() && args.forced.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "戻すファイルが選ばれていません"));
        }
        let root = stored.repo_root.clone();
        // 実行中は、重なる作業フォルダのチャットのキューを保留する（先に印を付けてから判定する）。
        let _folder_op = self.begin_folder_op(vec![root.clone()]);
        if let Some(e) = self.revert_blocker(&args.chat, &root) {
            return Err(e);
        }
        // このチャットの控え作業（B・E・現在状態の取得）と直列にし、リポジトリの書込みロックを取る（待っている間の状態変化は取り直す）。
        let queue = self.baseline_rt.queue_lock(&args.chat);
        let _q = queue.lock().await;
        let loaded = self.load_baseline(&args.chat).await.map_err(|m| err(IpcErrorCode::Io, m))?;
        let cwd = self.baseline_cwd(&args.chat, &loaded.segs).ok_or_else(|| OpenFail::WorkFolderUnknown.ipc())?;
        let mut s = Session::open(self, &args.chat, &cwd).await.map_err(|f| f.ipc())?;
        if rb::norm_path(&s.root) != rb::norm_path(&root) {
            return Err(stale());
        }
        let lock = self.baseline_rt.repo_lock(&s.root);
        let _w = lock.write().await;
        if let Some(e) = self.revert_blocker(&args.chat, &root) {
            return Err(e);
        }
        let rel = |v: &[String]| v.iter().map(|p| to_rel(&s.root, p)).collect::<Vec<_>>();
        let (paths, forced) = (rel(&args.paths), rel(&args.forced));
        let segs: Vec<SegRec> = loaded.segs.iter().filter(|g| rb::norm_path(&g.repo_root) == rb::norm_path(&s.root)).cloned().collect();
        let requested: Vec<String> = paths.iter().chain(forced.iter()).cloned().collect();
        let (fresh, start_seg) = s.plan(&segs, &loaded.marks, stored.from_turn.as_ref(), Some(requested), Some(cwd.clone())).await?;
        let adm = rb::admit(&stored.planned, &fresh, &paths, &forced);
        if adm.stale.is_some() || start_seg != stored.start_seg {
            return Err(stale());
        }
        let abs = |p: &str| abs_of(&s.root, p);
        let map_failed = |v: Vec<RevertFailure>| v.into_iter().map(|f| RevertFailure { path: abs(&f.path), reason: f.reason }).collect::<Vec<_>>();
        if adm.execute.is_empty() {
            return Ok(RevertResult { reverted: vec![], forced: vec![], failed: map_failed(adm.failed), backup_dir: None });
        }
        let Some(store) = self.persist.store().cloned() else { return Err(err(IpcErrorCode::Io, "保存先が使えないため、戻す前の控えを保存できません。何も変更していません")) };
        let at = now_ms();
        let backup = store.revert_backup_dir(&args.chat, at);
        let top = s.top.clone();

        // 1. 書き換える先をもう一度、解決後の位置で検査する。1つでも外なら何もしない。
        // 2. 書換え前の内容を控える（失敗・空き不足なら何も変えない）。
        let items = adm.execute.clone();
        let prep = {
            let (host, store, chat, backup, top, cwd, items) = (self.clone(), store.clone(), args.chat.clone(), backup.clone(), top.clone(), cwd.clone(), items.clone());
            tokio::task::spawn_blocking(move || prepare_backup(&host, &store, &chat, at, &backup, &top, &cwd, &items)).await.map_err(|e| err(IpcErrorCode::Io, format!("戻す処理に失敗しました: {e}")))?
        };
        let backup_files = match prep {
            Ok(v) => v,
            Err(RevertStop::Space(required, available)) => {
                return Err(blocked(BlockedReason::InsufficientSpace { required, available }, "控えを保存する空き容量が足りないため、何も変更していません。容量を空けてから再実行してください（保存データは自動では削除しません）"))
            }
            Err(RevertStop::Outside(message)) => return Err(err(IpcErrorCode::InvalidArgs, format!("書き換え先が作業フォルダの外、または確認できないため、何も変更していません: {message}"))),
            Err(RevertStop::Backup(message)) => return Err(err(IpcErrorCode::Io, format!("戻す前の控えを保存できなかったため、何も変更していません: {message}"))),
        };
        if self.check_not_delete_pending(&args.chat).is_err() {
            return Err(err(IpcErrorCode::Io, "チャットが削除の保留中になったため、何も変更していません"));
        }

        // 3. ファイルごと: 書き戻す内容を取り出す（失敗はそのファイルだけ）→ 書込み直前の再照合 → 書く。
        let mut outcomes: Vec<WriteOutcome> = Vec::new();
        for (item, backup_file) in items.into_iter().zip(backup_files) {
            let content = match (&item.op.kind, &item.op.target) {
                (WriteKind::RemoveAdded, _) => Ok(None),
                (_, Some(entry)) => {
                    let op = match entry.form {
                        EntryForm::Raw => SnapshotOp::CatFileBlob { oid: entry.oid.clone() },
                        EntryForm::Head => SnapshotOp::CatFileFiltered { path: item.op.path.clone(), oid: entry.oid.clone() },
                    };
                    s.run(&op).await.map(|o| Some(o.stdout)).map_err(|m| format!("書き戻す内容を取り出せませんでした（{m}）"))
                }
                (_, None) => Err("書き戻す内容が決まっていません".to_string()),
            };
            let result = match content {
                Err(m) => Err(m),
                Ok(bytes) => {
                    let (top, cwd, item) = (top.clone(), cwd.clone(), item.clone());
                    tokio::task::spawn_blocking(move || write_one(&top, Some(&cwd), &item.op, bytes)).await.unwrap_or_else(|e| Err(format!("書換え中に失敗しました: {e}")))
                }
            };
            outcomes.push(WriteOutcome { item, backup_file, result });
        }
        let agg = rb::aggregate(outcomes, adm.failed);

        // 4. 成功したファイルだけ記録する（失敗は記録しない）。
        let backup_dir = backup.to_string_lossy().into_owned();
        if !agg.items.is_empty() {
            let line = BaselineLine::Reverted { at, from_seg: start_seg.unwrap_or_else(|| LocalId(String::new())), items: agg.items.clone(), failed: agg.failed.clone(), backup_dir: Some(backup_dir.clone()) };
            let (host, chat) = (self.clone(), args.chat.clone());
            let res = tokio::task::spawn_blocking(move || {
                let _g = host.persist.io_guard();
                store.append_baseline(&chat, &line)
            })
            .await;
            if !matches!(res, Ok(Ok(()))) {
                self.warn("戻した記録を保存できませんでした。ファイルは戻っています（控えは保存済み）。同じ変更をもう一度戻すことはできません");
            }
        }
        self.mutate(|_| ((), vec![HostEvent::ChangesUpdated { chat: args.chat.clone(), turn: None }]));
        Ok(RevertResult {
            reverted: agg.reverted.iter().map(|p| abs(p)).collect(),
            forced: agg.forced.iter().map(|p| abs(p)).collect(),
            failed: map_failed(agg.failed),
            backup_dir: Some(backup_dir),
        })
    }
}

fn to_item(root: &str, p: &PathPlan) -> RevertItem {
    let kind = p.write.as_ref().map(|w| match w.kind {
        WriteKind::RemoveAdded => ChangeKind::Added,
        WriteKind::Restore => ChangeKind::Deleted,
        WriteKind::Overwrite => ChangeKind::Modified,
    });
    let verdict = match (&p.judgement, &p.write) {
        (rb::Judgement::Revertable, Some(w)) => RevertVerdict::Revertible {
            summary: match w.kind {
                WriteKind::RemoveAdded => "追加されたファイルを削除します".to_string(),
                WriteKind::Restore => "削除されたファイルを復元します".to_string(),
                WriteKind::Overwrite => "控えの内容に戻します".to_string(),
            },
        },
        (j, _) => j.to_verdict(),
    };
    RevertItem { path: abs_of(root, &p.path), kind, includes_turns: p.includes_turns.clone(), verdict }
}

enum RevertStop {
    /// 書換え直前の再検査で、作業フォルダの外・解決不能のパスが見つかった（何も書き換えていない）。
    Outside(String),
    Space(u64, u64),
    Backup(String),
}

/// 書き換える先の再検査と、書換え前の内容の控え（`revert-backup\<ms>\`）。戻り値は各項目の控えのファイル名（元がなかったものは None）。
/// 控えの保存の間だけ、書込みの直列化ロックを持つ（チャット領域の削除と同時に書かない）。
#[allow(clippy::too_many_arguments)]
fn prepare_backup(host: &Host, store: &Store, chat: &ChatKey, at: UnixMillis, backup: &Path, top: &Path, cwd: &str, items: &[ExecItem]) -> Result<Vec<Option<String>>, RevertStop> {
    let guard = host.persist.io_guard();
    for it in items {
        confine_to(Some(cwd), &top.join(&it.op.path).to_string_lossy()).map_err(|m| RevertStop::Outside(format!("{}: {m}", it.op.path)))?;
    }
    // 控えるサイズの合計を先に出し（空き確認のため）、内容は1ファイルずつ読んで書く（全ファイルを同時にメモリへ置かない）。
    let mut total: u64 = 0;
    for it in items {
        let p = top.join(&it.op.path);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_file() && m.len() <= BASELINE_FILE_LIMIT => total += m.len(),
            Ok(m) if m.file_type().is_file() => return Err(RevertStop::Backup(format!("{}: ファイルが大きすぎます", it.op.path))),
            Ok(_) => return Err(RevertStop::Backup(format!("{}: ファイルではありません", it.op.path))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(RevertStop::Backup(format!("{}: {e}", it.op.path))),
        }
    }
    match store.check_space(total + 64 * 1024) {
        Ok(()) => {}
        Err(StoreError::InsufficientSpace { required, available }) => return Err(RevertStop::Space(required, available)),
        Err(e) => return Err(RevertStop::Backup(save_failure_message(&e))),
    }
    let mut files: Vec<Option<String>> = Vec::new();
    let mut manifest_entries = Vec::new();
    for (idx, it) in items.iter().enumerate() {
        let original = top.join(&it.op.path).to_string_lossy().into_owned();
        let content = match std::fs::read(top.join(&it.op.path)) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RevertStop::Backup(format!("{}: {e}", it.op.path))),
        };
        let file = match &content {
            Some(bytes) => {
                let name = Path::new(&it.op.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                let file = format!("{idx}-{}", layout::sanitize_file_name(&name));
                atomic::write_atomic(&backup.join(&file), bytes).map_err(|e| RevertStop::Backup(e.to_string()))?;
                Some(file)
            }
            None => None,
        };
        manifest_entries.push(json!({"file": file, "original": original, "existed": file.is_some()}));
        files.push(file);
    }
    let manifest = json!({"schemaVersion": 1, "createdAt": at.0, "chat": {"backend": chat.backend, "id": chat.id.0}, "entries": manifest_entries});
    let text = serde_json::to_vec_pretty(&manifest).map_err(|e| RevertStop::Backup(e.to_string()))?;
    atomic::write_atomic(&backup.join("manifest.json"), &text).map_err(|e| RevertStop::Backup(e.to_string()))?;
    drop(guard);
    Ok(files)
}

/// 1ファイルの書込み。書込み直前に、作業フォルダ内であることと、現在の内容が計画時の `expect` のままであることを確かめる
/// （強制のファイルも同じ）。違えば書かない。書き戻しは原子的（削除で空になったフォルダは消さない）。
fn write_one(top: &Path, cwd: Option<&str>, op: &WriteOp, content: Option<Vec<u8>>) -> Result<(), String> {
    let abs = top.join(&op.path);
    confine_to(cwd, &abs.to_string_lossy()).map_err(|m| format!("作業フォルダの内側であることを確認できませんでした（{m}）"))?;
    let now = file_info(&abs);
    if now.skip.is_some() {
        return Err("計画後にファイルの種類が変わったため、書き換えませんでした".into());
    }
    rb::recheck(&op.expect, &now.file)?;
    match (&op.kind, content) {
        (WriteKind::RemoveAdded, _) => match std::fs::remove_file(&abs) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("削除に失敗しました: {e}")),
            _ => Ok(()),
        },
        (_, Some(bytes)) => atomic::write_atomic(&abs, &bytes).map_err(|e| format!("書き換えに失敗しました: {e}")),
        (_, None) => Err("書き戻す内容がありません".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_resolves_dotdot_missing_targets_and_junctions() {
        let base = std::env::temp_dir().join(format!("agentdock-confine-{}", std::process::id()));
        let (work, outside) = (base.join("work"), base.join("outside"));
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        let w = work.to_string_lossy().into_owned();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert!(confine_to(Some(&w), &s(&work.join("sub").join("new.txt"))).is_ok(), "missing file under an existing parent");
        assert!(confine_to(Some(&w), &s(&work.join("a").join("b").join("c.txt"))).is_ok(), "missing parents resolve from the nearest ancestor");
        assert!(confine_to(Some(&w), &s(&outside.join("secret.txt"))).is_err(), "outside the work folder");
        assert!(confine_to(Some(&w), &s(&work.join("..").join("outside").join("secret.txt"))).is_err(), "dotdot escape");
        assert!(confine_to(None, &s(&work.join("a.txt"))).is_err(), "unknown work folder");
        // ジャンクション越え（mklink /J は管理者権限が要らない）。作れない環境では越えの検査だけ省く。
        let link = work.join("link");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J", &s(&link), &s(&outside)]).output().map(|o| o.status.success()).unwrap_or(false);
        if made {
            assert!(confine_to(Some(&w), &s(&link.join("secret.txt"))).is_err(), "through a junction");
            assert!(confine_to(Some(&w), &s(&link.join("new.txt"))).is_err(), "new file under a junction");
            let _ = std::process::Command::new("cmd").args(["/C", "rmdir", &s(&link)]).output();
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    fn id(s: &str) -> LocalId {
        LocalId(s.into())
    }
    fn eid(s: &str) -> ExternalId {
        ExternalId(s.into())
    }
    fn snap(tree: &str) -> Snapshot {
        Snapshot { head: Some("h".into()), branch: Some("main".into()), tree: tree.into(), raw: vec![], skipped: vec![], took_ms: 1 }
    }
    fn started(seg: &str, at: i64, tree: &str) -> BaselineLine {
        BaselineLine::Started {
            seg: id(seg),
            at: UnixMillis(at),
            attempt: id(&format!("a-{seg}")),
            repo: RepoRef { root: "C:/repo".into(), objects_dir: "C:/repo/.git/objects".into() },
            cwd: "C:/repo".into(),
            base: snap(tree),
        }
    }

    #[test]
    fn segments_are_built_from_records_and_end_unknown_is_not_invented() {
        let lines = vec![
            started("s1", 1, "t1"),
            BaselineLine::Bound { seg: id("s1"), turn: eid("turn1") },
            BaselineLine::Ended { seg: id("s1"), at: UnixMillis(2), end: SegmentEnd::Snapshot { snapshot: snap("e1") } },
            started("s2", 3, "t2"),
            BaselineLine::Bound { seg: id("s2"), turn: eid("turn2") },
            BaselineLine::Concurrent { seg: id("s2"), with: ConcurrentWith::External { label: "x".into() } },
            started("s3", 5, "t3"),
            BaselineLine::Abandoned { seg: id("s3"), reason: "rejected".into() },
            started("s4", 7, "t4"),
            BaselineLine::Ended { seg: id("s4"), at: UnixMillis(8), end: SegmentEnd::EndIsNextBase },
            BaselineLine::Failed { seg: Some(id("s5")), at: UnixMillis(9), attempt: id("a5"), phase: BaselinePhase::Base, reason: BaselineFailure::Timeout },
            BaselineLine::Reverted {
                at: UnixMillis(20),
                from_seg: id("s1"),
                items: vec![RevertedPath { path: "a.txt".into(), restored: Restored::Absent, backup_file: None, forced: false, overridden: vec![] }],
                failed: vec![],
                backup_dir: None,
            },
        ];
        let (segs, marks) = build_segments(&lines);
        assert_eq!(segs.iter().map(|s| s.seg.0.as_str()).collect::<Vec<_>>(), ["s1", "s2", "s4"], "abandoned and base-failed segments are not in the range");
        assert!(matches!(&segs[0].end, EndRec::Snapshot { at, .. } if at.0 == 2));
        assert_eq!(segs[1].end, EndRec::Unknown, "no end record means end unknown");
        assert_eq!(segs[1].concurrent.len(), 1);
        assert_eq!(segs[2].end, EndRec::NextBase { at: UnixMillis(8) });
        assert_eq!(segs[0].turn, Some(eid("turn1")));
        assert_eq!((marks.len(), marks[0].path.as_str(), marks[0].at.0), (1, "a.txt", 20));
        assert_eq!(start_index(&segs, Some(&eid("turn2"))), Some(1));
        assert_eq!(start_index(&segs, None), Some(0));
        assert_eq!(start_index(&segs, Some(&eid("nope"))), None);
        // 最後の区間の終了が「次の基準で代える」なら、戻しの新旧は開始時刻で比べる（rules::baseline::plan と同じ）。
        assert_eq!(latest_at(&segs).0, 7);
    }

    #[test]
    fn paths_are_mapped_between_repository_relative_and_absolute() {
        assert_eq!(abs_of("C:/repo/", "src/a.rs"), "C:/repo/src/a.rs");
        assert_eq!(to_rel("C:/repo", "C:/repo/src/a.rs"), "src/a.rs");
        assert_eq!(to_rel("c:/REPO", r"C:\repo\src\a.rs"), "src/a.rs");
        assert_eq!(to_rel("C:/repo", "src/a.rs"), "src/a.rs");
        assert_eq!(to_rel("C:/repo", "D:/other/a.rs"), "D:/other/a.rs", "outside paths stay absolute so that the plan stops them");
        assert_eq!(to_rel("C:/repo", "C:/repo2/a.rs"), "C:/repo2/a.rs", "a sibling that shares a prefix is not inside");
    }

    #[test]
    fn ls_tree_and_names_are_parsed_and_entries_keep_their_form() {
        let ls = parse_ls_tree(b"100644 blob aaaa\ta b.txt\0120000 blob bbbb\tlink\0160000 commit cccc\tsub\0100755 blob dddd\tdir/run.sh\0");
        let p = Point { tree: "t".into(), raw: ["a b.txt".to_string()].into_iter().collect(), skipped: [("big".to_string(), SkipReason::TooLarge)].into_iter().collect(), head: None, branch: None };
        assert_eq!(entry_state(&p, &ls, "a b.txt"), EntryState::Present(EntryRef { oid: "aaaa".into(), form: EntryForm::Raw }));
        assert_eq!(entry_state(&p, &ls, "dir/run.sh"), EntryState::Present(EntryRef { oid: "dddd".into(), form: EntryForm::Head }));
        assert_eq!(entry_state(&p, &ls, "link"), EntryState::Skipped(SkipReason::Link));
        assert_eq!(entry_state(&p, &ls, "sub"), EntryState::Skipped(SkipReason::Submodule));
        assert_eq!(entry_state(&p, &ls, "big"), EntryState::Skipped(SkipReason::TooLarge));
        assert_eq!(entry_state(&p, &ls, "missing"), EntryState::Absent);
        assert_eq!(parse_nul_names(b"a\0b c\0"), ["a", "b c"]);
        assert_eq!(kind_of(&EntryState::Absent, &entry_state(&p, &ls, "a b.txt")), ChangeKind::Added);
        assert_eq!(kind_of(&entry_state(&p, &ls, "a b.txt"), &EntryState::Absent), ChangeKind::Deleted);
    }

    #[test]
    fn path_chunks_are_bounded_and_lose_nothing() {
        let paths: Vec<String> = (0..450).map(|i| format!("dir/file-{i}.txt")).collect();
        let chunks = chunk_paths(&paths);
        assert!(chunks.len() >= 3 && chunks.iter().all(|c| c.len() <= LS_CHUNK_PATHS));
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), paths.len());
        assert!(chunk_paths(&[]).is_empty());
        let long = vec!["x".repeat(LS_CHUNK_CHARS); 3];
        assert_eq!(chunk_paths(&long).len(), 3);
    }

    #[test]
    fn work_folder_prefix_is_relative_to_the_repository_and_unrelated_folders_are_outside() {
        let base = std::env::temp_dir().join(format!("agentdock-prefix-{}", std::process::id()));
        let (repo, sub) = (base.join("repo"), base.join("repo").join("app"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(base.join("repo2")).unwrap();
        let s = |p: &Path| p.to_string_lossy().replace('\\', "/");
        assert_eq!(work_prefix(&s(&repo), Some(&s(&repo))), "");
        assert_eq!(work_prefix(&s(&repo), Some(&s(&sub))), "app");
        assert_eq!(work_prefix(&s(&repo), Some(&s(&base.join("repo2")))), OUTSIDE_PREFIX);
        assert_eq!(work_prefix(&s(&repo), None), OUTSIDE_PREFIX);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn reverted_means_the_latest_record_after_the_last_snapshot_matches_the_current_state() {
        let e = |oid: &str| EntryState::Present(EntryRef { oid: oid.into(), form: EntryForm::Raw });
        let mark = |at: i64, restored: Restored| RevertMark { at: UnixMillis(at), path: "a.txt".into(), restored };
        let marks = vec![mark(10, Restored::Entry { entry: EntryRef { oid: "x".into(), form: EntryForm::Raw } })];
        assert!(is_reverted(&marks, UnixMillis(5), "a.txt", &e("x")));
        assert!(!is_reverted(&marks, UnixMillis(5), "a.txt", &e("y")), "changed again after the revert");
        assert!(!is_reverted(&marks, UnixMillis(11), "a.txt", &e("x")), "a newer snapshot supersedes the record");
        assert!(!is_reverted(&marks, UnixMillis(5), "b.txt", &e("x")), "other paths are not marked");
        assert!(is_reverted(&[mark(10, Restored::Absent)], UnixMillis(5), "A.TXT", &EntryState::Absent));
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-chg-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn overwrite_op(path: &str, expect: Option<String>) -> WriteOp {
        WriteOp { path: path.into(), kind: WriteKind::Overwrite, target: Some(EntryRef { oid: "x".into(), form: EntryForm::Raw }), expect }
    }

    #[test]
    fn the_write_is_skipped_when_the_file_changed_after_the_plan_forced_or_not() {
        let dir = temp("recheck");
        let top = dir.as_path();
        let cwd = top.to_string_lossy().into_owned();
        std::fs::write(top.join("a.txt"), b"planned").unwrap();
        let expect = Some(rb::sha256_hex(b"planned"));
        // 計画のとおりなら書く。
        write_one(top, Some(&cwd), &overwrite_op("a.txt", expect.clone()), Some(b"restored".to_vec())).unwrap();
        assert_eq!(std::fs::read(top.join("a.txt")).unwrap(), b"restored");
        // 計画後に変わっていたら（強制のファイルも同じ）書かない。
        std::fs::write(top.join("a.txt"), b"edited after the plan").unwrap();
        let e = write_one(top, Some(&cwd), &overwrite_op("a.txt", expect), Some(b"restored".to_vec())).unwrap_err();
        assert!(e.contains("計画後に変更"), "{e}");
        assert_eq!(std::fs::read(top.join("a.txt")).unwrap(), b"edited after the plan");
        // 削除も同じ。計画時になかったファイルが現れていたら消さない。
        std::fs::write(top.join("new.txt"), b"appeared").unwrap();
        let rm = WriteOp { path: "new.txt".into(), kind: WriteKind::RemoveAdded, target: None, expect: None };
        assert!(write_one(top, Some(&cwd), &rm, None).is_err());
        assert!(top.join("new.txt").exists());
        let rm = WriteOp { expect: Some(rb::sha256_hex(b"appeared")), ..rm };
        write_one(top, Some(&cwd), &rm, None).unwrap();
        assert!(!top.join("new.txt").exists());
        // 作業フォルダの外へは書かない。
        let outside = write_one(top, Some(&cwd), &overwrite_op("../escape.txt", None), Some(b"x".to_vec()));
        assert!(outside.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── ホスト結合: 一時リポジトリ＋一時の保存先（gitがなければスキップ）──

    use super::super::state::agent_key_of;
    use std::process::Command;

    fn git_ok() -> bool {
        Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    struct Fx {
        base: PathBuf,
        repo: PathBuf,
        host: Arc<Host>,
        chat: ChatKey,
        n: u32,
    }

    /// a.txt（LF）・h.txt（.gitattributes で eol=crlf、作業ツリーはCRLF）・b.txt をコミット済みのリポジトリと、そのチャット。
    fn fx(tag: &str) -> Option<Fx> {
        fx_cfg(tag, "false")
    }

    fn fx_cfg(tag: &str, autocrlf: &str) -> Option<Fx> {
        if !git_ok() {
            return None;
        }
        let base = temp(tag);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q"]);
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "t"), ("core.autocrlf", autocrlf)] {
            sh(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join(".gitattributes"), b"h.txt text eol=crlf\n").unwrap();
        std::fs::write(repo.join("a.txt"), b"one\n").unwrap();
        std::fs::write(repo.join("b.txt"), b"keep\n").unwrap();
        std::fs::write(repo.join("h.txt"), b"hello\nworld\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        // 作業ツリーのh.txtをCRLFにする（チェックアウトの変換）。
        std::fs::remove_file(repo.join("h.txt")).unwrap();
        sh(&repo, &["checkout", "--", "h.txt"]);
        assert_eq!(std::fs::read(repo.join("h.txt")).unwrap(), b"hello\r\nworld\r\n", "the attribute makes the worktree CRLF");
        if autocrlf == "true" {
            // Windows既定: チェックアウトでCRLFになる。
            std::fs::remove_file(repo.join("a.txt")).unwrap();
            sh(&repo, &["checkout", "--", "a.txt"]);
            assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"one\r\n");
        }
        let store = Arc::new(Store::open(base.join("data")).unwrap());
        let host = Arc::new(Host::with_store(base.join("data"), store));
        host.set_emitter(Arc::new(|_| {}));
        let chat = ChatKey { backend: BackendKind::Codex, id: eid("c1") };
        host.data.lock().unwrap().chats.push(Chat {
            key: chat.clone(),
            kind: ChatKind::Development,
            cwd: Known::direct(repo.to_string_lossy().into_owned()),
            name: Known::NotFetched,
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
            no_history: false,
        });
        Some(Fx { base, repo, host, chat, n: 0 })
    }

    impl Fx {
        fn lines(&self) -> Vec<BaselineLine> {
            self.host.persist.store().unwrap().read_baselines(&self.chat).unwrap()
        }

        /// 1回のturn: 送信前の控え B → 受理 → 変更 → 終端の観測 → 終了の控え E。Gitが古い環境では None（スキップ）。
        async fn turn(&mut self, turn: &str, edit: impl FnOnce(&Path)) -> Option<()> {
            self.n += 1;
            let attempt = id(&format!("att-{}", self.n));
            self.host.baseline_take_base(&self.chat, &attempt, Some(self.repo.to_string_lossy().into_owned())).await;
            if matches!(self.lines().last(), Some(BaselineLine::Failed { .. })) {
                return None;
            }
            self.host.baseline_bound(&self.chat, &attempt, &eid(turn));
            edit(&self.repo);
            self.host.baseline_observe(&BackendEvent::TurnEnded {
                turn: TurnKey { agent: agent_key_of(&self.chat), turn_id: eid(turn) },
                end: TurnEnd::Completed,
                error: None,
                evidence: Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(1) },
            });
            self.host.baseline_poll();
            let want = self.n as usize;
            for _ in 0..200 {
                if self.lines().iter().filter(|l| matches!(l, BaselineLine::Ended { .. })).count() >= want {
                    return Some(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("the end of the turn was not recorded: {:?}", self.lines());
        }

        async fn preview(&self, turn: &str) -> RevertPlan {
            self.host.preview_revert(PreviewRevertArgs { chat: self.chat.clone(), turn: Some(eid(turn)), paths: None }).await.expect("preview")
        }

        async fn revert(&self, plan: &RevertPlan, paths: &[&str], forced: &[&str]) -> Result<RevertResult, IpcError> {
            let abs = |n: &str| plan.items.iter().find(|i| i.path.ends_with(n)).unwrap_or_else(|| panic!("no item {n}: {:?}", plan.items)).path.clone();
            let args = RevertChangesArgs { chat: self.chat.clone(), plan_id: plan.id.clone(), paths: paths.iter().map(|p| abs(p)).collect(), forced: forced.iter().map(|p| abs(p)).collect() };
            self.host.revert_changes(args, &UserConfirmed::from_user_command()).await
        }

        fn read(&self, name: &str) -> Vec<u8> {
            std::fs::read(self.repo.join(name)).unwrap()
        }

        fn done(self) {
            std::fs::remove_dir_all(&self.base).ok();
        }
    }

    fn verdict<'a>(plan: &'a RevertPlan, name: &str) -> &'a RevertVerdict {
        &plan.items.iter().find(|i| i.path.ends_with(name)).unwrap_or_else(|| panic!("no item {name}: {:?}", plan.items)).verdict
    }

    fn is_revertible(v: &RevertVerdict) -> bool {
        matches!(v, RevertVerdict::Revertible { .. })
    }

    fn backup_contents(dir: &str) -> Vec<Vec<u8>> {
        std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name() != "manifest.json").map(|e| std::fs::read(e.path()).unwrap()).collect()
    }

    fn reverted_items(f: &Fx) -> Vec<RevertedPath> {
        f.lines()
            .into_iter()
            .find_map(|l| match l {
                BaselineLine::Reverted { items, .. } => Some(items),
                _ => None,
            })
            .expect("a reverted record")
    }

    #[tokio::test]
    async fn the_baseline_content_is_restored_byte_for_byte_and_what_was_overwritten_stays_in_the_backup() {
        let Some(mut f) = fx("restore") else { return };
        let before_h = f.read("h.txt");
        let Some(()) = f
            .turn("t1", |r| {
                std::fs::write(r.join("a.txt"), b"two\n").unwrap();
                std::fs::write(r.join("new.txt"), b"fresh\n").unwrap();
                std::fs::write(r.join("h.txt"), b"hello\r\nCHANGED\r\n").unwrap();
            })
            .await
        else {
            f.done();
            return;
        };
        // 一覧: 控え基準では追加・変更が出る。
        let list = f.host.get_change_list(GetChangeListArgs { chat: f.chat.clone(), scope: ChangeScope::Turn { turn: eid("t1") }, source: ChangeSource::Baseline }).await.unwrap();
        assert_eq!(list.status, ListStatus::Ready, "{list:?}");
        let mut names: Vec<(String, ChangeKind)> = list.files.iter().map(|x| (x.path.rsplit('/').next().unwrap().to_string(), x.kind)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(names, [("a.txt".to_string(), ChangeKind::Modified), ("h.txt".to_string(), ChangeKind::Modified), ("new.txt".to_string(), ChangeKind::Added)]);
        assert!(list.files.iter().all(|x| x.turn == Some(eid("t1")) && !x.reverted));
        let a_path = list.files.iter().find(|x| x.path.ends_with("a.txt")).unwrap().path.clone();
        let diff = f.host.get_file_diff(GetFileDiffArgs { chat: f.chat.clone(), path: a_path, source: ChangeSource::Baseline, turn: Some(eid("t1")) }).await.unwrap();
        assert!(diff.text.contains("-one") && diff.text.contains("+two"), "{diff:?}");

        let plan = f.preview("t1").await;
        for n in ["a.txt", "h.txt", "new.txt"] {
            assert!(is_revertible(verdict(&plan, n)), "{n}: {:?}", verdict(&plan, n));
        }
        let res = f.revert(&plan, &["a.txt", "h.txt", "new.txt"], &[]).await.unwrap();
        assert!(res.failed.is_empty() && res.forced.is_empty(), "{res:?}");
        assert_eq!(res.reverted.len(), 3);
        // 開始時の内容に戻る。h.txt は checkout と同じ改行変換（CRLF）で、バイト単位で一致する。
        assert_eq!(f.read("a.txt"), b"one\n");
        assert_eq!(f.read("h.txt"), before_h);
        assert!(!f.repo.join("new.txt").exists());
        assert_eq!(sh(&f.repo, &["status", "--porcelain"]).trim(), "", "the worktree is clean again");
        // 書換え前の内容は revert-backup に残る。
        let backup = res.backup_dir.unwrap();
        let kept = backup_contents(&backup);
        assert!(kept.contains(&b"two\n".to_vec()) && kept.contains(&b"fresh\n".to_vec()) && kept.contains(&b"hello\r\nCHANGED\r\n".to_vec()), "{kept:?}");
        assert!(Path::new(&backup).join("manifest.json").exists());
        // 成功した分だけ記録され（強制の印はない）、戻した後は「戻し済み」になる。
        let items = reverted_items(&f);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| !i.forced && i.overridden.is_empty()));
        assert!(items.iter().any(|i| i.restored == Restored::Absent && i.path == "new.txt"));
        let again = f.preview("t1").await;
        assert!(again.items.iter().all(|i| matches!(&i.verdict, RevertVerdict::Blocked { code: RevertBlockCode::AlreadyReverted, .. })), "{again:?}");
        f.done();
    }

    #[tokio::test]
    async fn with_autocrlf_a_reverted_file_is_already_reverted_and_a_later_edit_needs_confirmation() {
        let Some(mut f) = fx_cfg("autocrlf", "true") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"HELLO\r\n").unwrap()).await else {
            f.done();
            return;
        };
        let plan = f.preview("t1").await;
        assert!(is_revertible(verdict(&plan, "a.txt")), "{:?}", verdict(&plan, "a.txt"));
        let res = f.revert(&plan, &["a.txt"], &[]).await.unwrap();
        assert!(res.failed.is_empty(), "{res:?}");
        assert_eq!(f.read("a.txt"), b"one\r\n", "restored through the checkout conversion");
        assert_eq!(sh(&f.repo, &["status", "--porcelain"]).trim(), "");
        // 戻した直後に開き直すと「戻し済み」（強制できない）。
        let again = f.preview("t1").await;
        assert!(matches!(verdict(&again, "a.txt"), RevertVerdict::Blocked { code: RevertBlockCode::AlreadyReverted, .. }), "{:?}", verdict(&again, "a.txt"));
        // 戻した後に次のturn（変更なし）を送っても同じ。
        let Some(()) = f.turn("t2", |_| {}).await else {
            f.done();
            return;
        };
        let after_next = f.preview("t1").await;
        assert!(matches!(verdict(&after_next, "a.txt"), RevertVerdict::Blocked { code: RevertBlockCode::AlreadyReverted, .. }), "{:?}", verdict(&after_next, "a.txt"));
        // その後に利用者が編集したら、従来どおり要確認。
        std::fs::write(f.repo.join("a.txt"), b"one\r\nmore\r\n").unwrap();
        let later = f.preview("t1").await;
        let RevertVerdict::NeedsOverride { reasons, .. } = verdict(&later, "a.txt") else { panic!("{:?}", verdict(&later, "a.txt")) };
        assert!(reasons.iter().any(|r| r.code == RevertBlockCode::ChangedAfter), "{reasons:?}");
        f.done();
    }

    #[tokio::test]
    async fn a_current_file_equal_to_the_filtered_head_entry_is_treated_as_that_entry() {
        let Some(f) = fx_cfg("align", "true") else { return };
        let cwd = f.repo.to_string_lossy().into_owned();
        let oid = sh(&f.repo, &["rev-parse", "HEAD:a.txt"]).trim().to_string();
        let session = Session::open(&f.host, &f.chat, &cwd).await;
        let Ok(s) = session else {
            f.done();
            return;
        };
        let head = EntryState::Present(EntryRef { oid: oid.clone(), form: EntryForm::Head });
        let raw = EntryState::Present(EntryRef { oid: "deadbeef".into(), form: EntryForm::Raw });
        let mk = || {
            let mut m: HashMap<(StateAt, String), EntryState> = HashMap::new();
            m.insert((StateAt::Base(0), "a.txt".into()), head.clone());
            m.insert((StateAt::Current, "a.txt".into()), raw.clone());
            m
        };
        // 作業ツリーはCRLF（checkoutの変換後）。HEAD形のBを同じ変換に通した内容と同じなので、Bのエントリとみなす。
        assert_eq!(f.read("a.txt"), b"one\r\n");
        let mut st = mk();
        s.align_current_with_filtered(&mut st, &["a.txt".to_string()], 0, 0, "", Some(&cwd)).await;
        assert_eq!(st[&(StateAt::Current, "a.txt".to_string())], head);
        // 内容が違えば（利用者の編集）そのまま。生形の控えとは生バイトで比べるので、ここでは何もしない。
        std::fs::write(f.repo.join("a.txt"), b"one\r\nmore\r\n").unwrap();
        let mut st = mk();
        s.align_current_with_filtered(&mut st, &["a.txt".to_string()], 0, 0, "", Some(&cwd)).await;
        assert_eq!(st[&(StateAt::Current, "a.txt".to_string())], raw);
        f.done();
    }

    #[tokio::test]
    async fn a_file_changed_after_the_turn_needs_a_forced_second_step_and_the_overwritten_content_is_kept() {
        let Some(mut f) = fx("force") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"two\n").unwrap()).await else {
            f.done();
            return;
        };
        // turnの後に別の変更。
        std::fs::write(f.repo.join("a.txt"), b"three, edited by someone else\n").unwrap();
        let plan = f.preview("t1").await;
        let RevertVerdict::NeedsOverride { reasons, .. } = verdict(&plan, "a.txt") else { panic!("{:?}", verdict(&plan, "a.txt")) };
        assert!(reasons.iter().any(|r| r.code == RevertBlockCode::ChangedAfter), "{reasons:?}");
        // 強制なしでは書かれない。
        let res = f.revert(&plan, &["a.txt"], &[]).await.unwrap();
        assert!(res.reverted.is_empty() && res.failed.len() == 1 && res.backup_dir.is_none(), "{res:?}");
        assert_eq!(f.read("a.txt"), b"three, edited by someone else\n");
        // 強制ありでは控えの内容と一致し、書換え前の内容がrevert-backupに残る。記録には強制の印と無視した理由が付く。
        let plan = f.preview("t1").await;
        let res = f.revert(&plan, &[], &["a.txt"]).await.unwrap();
        assert!(res.failed.is_empty() && res.reverted.len() == 1 && res.forced.len() == 1, "{res:?}");
        assert_eq!(f.read("a.txt"), b"one\n");
        assert_eq!(backup_contents(&res.backup_dir.unwrap()), vec![b"three, edited by someone else\n".to_vec()]);
        let items = reverted_items(&f);
        assert!(items[0].forced && items[0].overridden.contains(&RevertBlockCode::ChangedAfter) && items[0].backup_file.is_some(), "{items:?}");
        f.done();
    }

    #[tokio::test]
    async fn after_a_commit_the_file_is_not_written_even_when_forced() {
        let Some(mut f) = fx("commit") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"two\n").unwrap()).await else {
            f.done();
            return;
        };
        sh(&f.repo, &["commit", "-q", "-am", "the user commits the change of the turn"]);
        let plan = f.preview("t1").await;
        assert!(matches!(verdict(&plan, "a.txt"), RevertVerdict::Blocked { code: RevertBlockCode::HeadMoved, .. }), "{:?}", verdict(&plan, "a.txt"));
        let res = f.revert(&plan, &[], &["a.txt"]).await.unwrap();
        assert!(res.reverted.is_empty() && res.failed.len() == 1, "{res:?}");
        assert_eq!(f.read("a.txt"), b"two\n");
        f.done();
    }

    #[tokio::test]
    async fn nothing_is_changed_when_the_backup_cannot_be_saved() {
        let Some(mut f) = fx("nobackup") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"two\n").unwrap()).await else {
            f.done();
            return;
        };
        let plan = f.preview("t1").await;
        // 控えの置き場所に普通のファイルを置いて、控えを保存できないようにする。
        let backup_root = f.host.persist.store().unwrap().revert_backup_dir(&f.chat, UnixMillis(0)).parent().unwrap().to_path_buf();
        std::fs::create_dir_all(backup_root.parent().unwrap()).unwrap();
        std::fs::write(&backup_root, b"not a folder").unwrap();
        let res = f.revert(&plan, &["a.txt"], &[]).await;
        assert!(matches!(&res, Err(e) if e.message.contains("控えを保存できなかった")), "{res:?}");
        assert_eq!(f.read("a.txt"), b"two\n", "the file is untouched when the backup fails");
        f.done();
    }

    #[tokio::test]
    async fn a_chat_with_an_unconfirmed_send_cannot_be_reverted_even_with_a_forced_file() {
        let Some(mut f) = fx("busy") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"two\n").unwrap()).await else {
            f.done();
            return;
        };
        let plan = f.preview("t1").await;
        // 送信の受理が未確認（再送もしない状態）。
        f.host.unresolved.lock().unwrap().add(id("att-unknown"), f.chat.clone());
        let res = f.revert(&plan, &[], &["a.txt"]).await;
        assert!(matches!(&res, Err(e) if e.blocked == Some(BlockedReason::ChatBusy)), "{res:?}");
        assert_eq!(f.read("a.txt"), b"two\n");
        f.done();
    }

    #[test]
    fn next_base_is_resolved_only_by_the_record_that_follows_it() {
        let ended = |seg: &str, at: i64| BaselineLine::Ended { seg: id(seg), at: UnixMillis(at), end: SegmentEnd::EndIsNextBase };
        // 直後の区間が受理なしで外れた: さらに後の区間のBを終了として使わない（終了未確認）。
        let lines = vec![started("s1", 1, "t1"), started("s2", 2, "t2"), ended("s1", 2), BaselineLine::Abandoned { seg: id("s2"), reason: "r".into() }, started("s3", 5, "t3")];
        let (segs, _) = build_segments(&lines);
        assert_eq!(segs.iter().map(|s| s.seg.0.as_str()).collect::<Vec<_>>(), ["s1", "s3"]);
        assert_eq!(segs[0].end, EndRec::Unknown);
        // 直後の区間が別のリポジトリ。
        let mut other = started("s2", 2, "t2");
        if let BaselineLine::Started { repo, .. } = &mut other {
            repo.root = "D:/other".into();
        }
        let (segs, _) = build_segments(&[started("s1", 1, "t1"), other, ended("s1", 2)]);
        assert_eq!(segs[0].end, EndRec::Unknown);
        // 直後の区間が同じリポジトリで有効なら、そのまま。
        let (segs, _) = build_segments(&[started("s1", 1, "t1"), started("s2", 2, "t2"), ended("s1", 2)]);
        assert_eq!(segs[0].end, EndRec::NextBase { at: UnixMillis(2) });
    }

    #[tokio::test]
    async fn revert_waits_while_a_finished_turn_still_has_its_end_baseline_pending() {
        let Some(mut f) = fx("pendingend") else { return };
        let Some(()) = f.turn("t1", |r| std::fs::write(r.join("a.txt"), b"two\n").unwrap()).await else {
            f.done();
            return;
        };
        let plan = f.preview("t1").await;
        // 次のturnの終端を観測したが、終了の控え（E）はまだ取っていない。
        f.host.baseline_take_base(&f.chat, &id("att-9"), Some(f.repo.to_string_lossy().into_owned())).await;
        f.host.baseline_bound(&f.chat, &id("att-9"), &eid("t2"));
        f.host.baseline_observe(&BackendEvent::TurnEnded {
            turn: TurnKey { agent: agent_key_of(&f.chat), turn_id: eid("t2") },
            end: TurnEnd::Completed,
            error: None,
            evidence: Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(1) },
        });
        let res = f.revert(&plan, &["a.txt"], &[]).await;
        assert!(matches!(&res, Err(e) if e.blocked == Some(BlockedReason::ChatBusy)), "{res:?}");
        assert_eq!(f.read("a.txt"), b"two\n");
        f.done();
    }
}
