//! 変更ファイルの一覧・差分・「変更を戻す」（P3-1、`app/DESIGN_P3.md` §1 #1・#2）。
//!
//! - 観測: バックエンドが報告した完了済みのファイル変更を、ファイルの現状（観測直後のハッシュ）と合わせて
//!   `chats\<dirId>\changes.jsonl` へ追記する。観測していない変更は記録しない（推定で補わない）。
//! - 一覧・差分: 「バックエンドの報告」と「Git上の現在の差分」を混ぜずに出所つきで返す。読取りだけで、resumeしない。
//! - 戻す: 計画（`rules::revert::plan`）を作り、実行時に再計画して一致したものだけ、控えを保存してから書き換える。
//!   控えを保存できなければ何も変えない。作業中・停止未確認・受理不明・削除保留・外部実行中は止める。部分成功を完了と偽らない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;

use super::persist::save_failure_message;
use super::state::{external_send_locked, HostData};
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::changes::*;
use crate::backend::ipc::*;
use crate::backend::model::*;
use crate::gitops::{Git, GitError, GitOp};
use crate::rules::diff::{count_for, parse_status_v2, split_files, StatusEntry};
use crate::rules::revert::{self, norm_path, Planned, ReadResult, RevertMark, Selection, Write};
use crate::store::{atomic, layout, StoreError};

/// 計画の有効時間。
const PLAN_TTL_MS: i64 = 5 * 60 * 1000;
const MAX_PLANS: usize = 8;
/// ハッシュ・差分の逆適用のために読むファイルの上限。超えるものは「確認できない」として扱う。
const FILE_READ_LIMIT: u64 = 64 * 1024 * 1024;
/// 記録する差分の上限。超えた変更は、差分を保存せず「戻せない」記録にする（切り詰めて保存しない）。
const DIFF_STORE_LIMIT: usize = 8 * 1024 * 1024;

const NOTE_REPORTED: &str = "AgentDockが観測した、Codexが報告したファイル変更だけを表示します。コマンドの実行による変更や、起動前・外部での変更は含まれない場合があります。";
const NOTE_GIT: &str = "Git上の現在の差分（HEADとの比較）です。AgentDockが観測した変更とは別の情報で、コミット済みの変更は含まれません。";

struct ObserveJob {
    chat: ChatKey,
    agent: AgentKey,
    item: ItemKey,
    cwd: Option<String>,
    changes: Vec<FileChange>,
}

struct StoredPlan {
    chat: ChatKey,
    sel: Selection,
    created_at: UnixMillis,
    planned: Vec<Planned>,
}

/// 変更の観測・戻す計画の作業状態。
#[derive(Default)]
pub struct ChangesRuntime {
    observe_tx: OnceLock<tokio::sync::mpsc::UnboundedSender<ObserveJob>>,
    plans: Mutex<HashMap<LocalId, StoredPlan>>,
    /// 戻す操作は同時に1件だけ。
    revert_lock: tokio::sync::Mutex<()>,
}

/// 作業フォルダ基準で絶対パスにする（作業フォルダが分からなければそのまま。相対のままの記録は戻す対象にならない）。
fn resolve_path(raw: &str, cwd: Option<&str>) -> String {
    let p = Path::new(raw);
    match cwd {
        Some(c) if !p.is_absolute() => Path::new(c).join(p).to_string_lossy().into_owned(),
        _ => raw.to_string(),
    }
}

/// 現在のファイルの読取り。ファイルがなければ `Ok(None)`。上限を超えるものは読まずに `Err`。
fn read_current(path: &str) -> ReadResult {
    let p = Path::new(path);
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() && m.len() <= FILE_READ_LIMIT => std::fs::read(p).map(Some).map_err(|e| e.to_string()),
        Ok(m) if m.is_file() => Err("ファイルが大きすぎます".into()),
        Ok(_) => Err("ファイルではありません".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 解決（親のcanonicalize。対象がなければ最寄りの既存の祖先）した位置が、作業フォルダ（canonicalize後）の内側か。
/// リンク・ジャンクションは解決され、外へ出れば `Err`。作業フォルダが分からない・解決できないときも `Err`（書き換えない）。
fn confine_to(cwd: Option<&str>, p: &str) -> Result<(), String> {
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

/// 観測直後の状態。読めなければ `Unknown`（推定しない）。
fn observe_post_state(path: &str) -> PostState {
    match read_current(path) {
        Ok(None) => PostState::Absent,
        Ok(Some(b)) => PostState::Hash { sha256: revert::sha256_hex(&b) },
        Err(_) => PostState::Unknown,
    }
}

/// 一覧用に集約した1ファイル。
struct Reported {
    path: String,
    kind: ChangeKind,
    move_to: Option<String>,
    additions: Known<u32>,
    deletions: Known<u32>,
    turn: Option<ExternalId>,
    reverted: bool,
    text: String,
}

impl Reported {
    fn same(&self, path: &str) -> bool {
        let n = norm_path(path);
        norm_path(&self.path) == n || self.move_to.as_deref().is_some_and(|m| norm_path(m) == n)
    }
    fn to_file(&self) -> ChangedFile {
        ChangedFile {
            path: self.path.clone(),
            kind: self.kind,
            move_to: self.move_to.clone(),
            source: ChangeSource::BackendReported,
            additions: self.additions.clone(),
            deletions: self.deletions.clone(),
            turn: self.turn.clone(),
            reverted: self.reverted,
        }
    }
}

/// 記録をチャット（と任意のturn）で絞り、変更後のパスごとにまとめる。
fn group_records(chat: &ChatKey, turn: Option<&ExternalId>, records: &[ChangeRecord], marks: &[RevertMark]) -> Vec<Reported> {
    let mut groups: Vec<(String, Vec<&ChangeRecord>)> = Vec::new();
    for r in records.iter().filter(|r| &r.chat == chat && turn.is_none_or(|t| r.turn.as_ref() == Some(t))) {
        let key = norm_path(r.post_path());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(r),
            None => groups.push((key, vec![r])),
        }
    }
    groups
        .into_iter()
        .map(|(_, recs)| {
            let first = recs[0];
            let last = recs[recs.len() - 1];
            let (mut add, mut del) = (0u32, 0u32);
            let mut text = String::new();
            for r in &recs {
                let (a, d) = count_for(r.kind, &r.diff);
                add += a;
                del += d;
                let label = match r.kind {
                    ChangeKind::Added => "追加",
                    ChangeKind::Deleted => "削除",
                    ChangeKind::Modified => "変更",
                };
                text.push_str(&format!("# turn {}（{label}）\n", r.turn.as_ref().map(|t| t.0.as_str()).unwrap_or("不明")));
                text.push_str(&r.diff);
                if !r.diff.ends_with('\n') {
                    text.push('\n');
                }
            }
            Reported {
                path: first.path.clone(),
                kind: first.kind,
                move_to: (norm_path(&first.path) != norm_path(last.post_path())).then(|| last.post_path().to_string()),
                additions: Known::direct(add),
                deletions: Known::direct(del),
                turn: last.turn.clone(),
                reverted: recs.iter().all(|r| revert::is_consumed(r, marks)),
                text,
            }
        })
        .collect()
}

enum GitCtx {
    Ready { root: PathBuf, git: Git },
    NotSupported(String),
    NotFetched(String),
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or("").chars().take(200).collect()
}

fn git_error_status(e: &GitError) -> ListStatus {
    match e {
        GitError::Unavailable(_) => ListStatus::NotSupported { message: "Gitを実行できません（未導入、または設定のGitの場所が違います）".into() },
        other => ListStatus::NotFetched { message: format!("Gitから取得できませんでした（{other}）") },
    }
}

impl Host {
    // ───────────── 観測 ─────────────

    /// 観測の書込みtaskを始める（イベントpump開始時に1回。保存先が使えないときは何もしない）。
    /// 報告は届いた順に1本のtaskで処理する（同じファイルへの続けての変更で、観測直後のハッシュが前後しないように）。
    pub(super) fn start_changes_observer(self: &Arc<Self>) {
        if !self.persist.enabled() {
            return;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ObserveJob>();
        if self.changes_rt.observe_tx.set(tx).is_err() {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let h = host.clone();
                let chat = job.chat.clone();
                match tokio::task::spawn_blocking(move || h.record_changes(job)).await {
                    Ok(Ok(true)) => {
                        host.mutate(|_| ((), vec![HostEvent::ChangesUpdated { chat, turn: None }]));
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(e)) => host.warn(format!("ファイル変更の記録を保存できませんでした（このファイルは「変更を戻す」の対象になりません）: {}", save_failure_message(&e))),
                    Err(e) => host.warn(format!("ファイル変更の記録中に失敗しました: {e}")),
                }
            }
        });
    }

    /// 完了したファイル変更の報告を受けたとき（イベント処理から呼ぶ。待たない）。
    pub(super) fn observe_changes(self: &Arc<Self>, agent: AgentKey, item: ItemKey, changes: Vec<FileChange>) {
        let Some((chat, cwd)) = self.read(|d| d.view(&agent).map(|v| (v.agent.chat.clone(), d.chat(&v.agent.chat).and_then(|c| c.cwd.value().cloned())))) else { return };
        if let Some(tx) = self.changes_rt.observe_tx.get() {
            let _ = tx.send(ObserveJob { chat, agent, item, cwd, changes });
        }
    }

    /// 記録して追記する。追記したら `Ok(true)`。削除中・削除済みのチャットには書かない。
    fn record_changes(&self, job: ObserveJob) -> Result<bool, StoreError> {
        let Some(store) = self.persist.store() else { return Ok(false) };
        let _g = self.persist.io_guard();
        if self.manage_rt.is_deleting(&job.chat) || !self.read(|d| d.chat(&job.chat).is_some() || d.locals.contains_key(&job.chat)) {
            return Ok(false);
        }
        let mut wrote = false;
        for c in job.changes {
            let path = resolve_path(&c.path, job.cwd.as_deref());
            let move_to = c.move_to.as_deref().map(|m| resolve_path(m, job.cwd.as_deref()));
            let target = move_to.clone().unwrap_or_else(|| path.clone());
            let (diff, post) = if c.diff.len() > DIFF_STORE_LIMIT { (String::new(), PostState::Unknown) } else { (c.diff, observe_post_state(&target)) };
            let record = ChangeRecord {
                schema_version: 1,
                chat: job.chat.clone(),
                agent: job.agent.clone(),
                turn: job.item.turn_id.clone(),
                item: job.item.item_id.clone(),
                path,
                kind: c.kind,
                move_to,
                diff,
                post,
                observed_at: now_ms(),
            };
            store.append_change(&job.chat, &ChangeLine::Observed { record })?;
            wrote = true;
        }
        Ok(wrote)
    }

    /// 全チャットの記録と「戻し」の記録を読む（他チャットの後続の変更を見るため全領域）。
    async fn load_changes(self: &Arc<Self>) -> Result<(Vec<ChangeRecord>, Vec<RevertMark>), String> {
        let Some(store) = self.persist.store().cloned() else { return Err("保存先が使えないため、変更の記録を読めません".into()) };
        let lines = tokio::task::spawn_blocking(move || store.read_all_changes()).await.map_err(|e| e.to_string())?.map_err(|e| save_failure_message(&e))?;
        let mut records = Vec::new();
        let mut marks = Vec::new();
        for l in lines {
            match l {
                ChangeLine::Observed { record } => records.push(record),
                ChangeLine::Reverted { at, paths, .. } => marks.push(RevertMark { at, paths }),
            }
        }
        Ok((records, marks))
    }

    // ───────────── 一覧・差分（読取りのみ） ─────────────

    fn require_chat(&self, chat: &ChatKey) -> Result<(), IpcError> {
        if self.read(|d| d.chat(chat).is_none() && !d.locals.contains_key(chat)) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        Ok(())
    }

    /// 「バックエンドの報告」の一覧。取得できなかったときは理由つきの状態を返す（空と区別する）。
    async fn reported_files(self: &Arc<Self>, chat: &ChatKey, scope: &ChangeScope) -> (Vec<Reported>, ListStatus) {
        // turnの集約diff（このセッションでliveに受け取った最新値）。あればそれを使う。
        if let ChangeScope::Turn { turn } = scope {
            let diffs: Vec<String> = self.read(|d| d.turn_diffs_of_chat(chat).into_iter().filter(|(t, _)| &t.turn_id == turn).map(|(_, diff)| diff).collect());
            if !diffs.is_empty() {
                let files = diffs
                    .iter()
                    .flat_map(|diff| split_files(diff))
                    .map(|f| {
                        let (path, move_to) = match f.old_path {
                            Some(old) => (old, Some(f.path)),
                            None => (f.path, None),
                        };
                        Reported {
                            path,
                            kind: f.kind,
                            move_to,
                            additions: Known::direct(f.additions),
                            deletions: Known::direct(f.deletions),
                            turn: Some(turn.clone()),
                            reverted: false,
                            text: f.text,
                        }
                    })
                    .collect();
                return (files, ListStatus::Ready);
            }
        }
        let (records, marks) = match self.load_changes().await {
            Ok(x) => x,
            Err(message) => return (Vec::new(), ListStatus::NotFetched { message }),
        };
        let turn = match scope {
            ChangeScope::Turn { turn } => Some(turn),
            _ => None,
        };
        let files = group_records(chat, turn, &records, &marks);
        if files.is_empty() && turn.is_some() {
            return (files, ListStatus::NotFetched { message: "このturnの変更の報告を受け取っていません（AgentDockの起動前の履歴や、報告のないturnは取得できません）".into() });
        }
        (files, ListStatus::Ready)
    }

    async fn git_context(self: &Arc<Self>, chat: &ChatKey) -> GitCtx {
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
                let counts = diff_files.as_ref().and_then(|v| v.iter().find(|f| norm_path(&f.path) == norm_path(&e.path))).map(|f| (f.additions, f.deletions));
                let (path, move_to) = match &e.old_path {
                    Some(old) => (old.clone(), Some(e.path.clone())),
                    None => (e.path.clone(), None),
                };
                ChangedFile {
                    path,
                    kind: e.kind(),
                    move_to,
                    source: ChangeSource::Git,
                    additions: counts.map_or(Known::NotFetched, |c| Known::direct(c.0)),
                    deletions: counts.map_or(Known::NotFetched, |c| Known::direct(c.1)),
                    turn: None,
                    reverted: false,
                }
            })
            .collect();
        done(ListStatus::Ready, files)
    }

    /// 変更ファイルの一覧。`WorkingTree` はGit上の現在の差分、他はバックエンドの報告。読取りだけで、resumeしない。
    pub async fn get_change_list(self: &Arc<Self>, args: GetChangeListArgs) -> Result<ChangeList, IpcError> {
        self.require_chat(&args.chat)?;
        if args.scope == ChangeScope::WorkingTree {
            return Ok(self.git_list(&args.chat, args.scope).await);
        }
        let (files, status) = self.reported_files(&args.chat, &args.scope).await;
        Ok(ChangeList { scope: args.scope, source: ChangeSource::BackendReported, status, files: files.iter().map(Reported::to_file).collect(), notes: vec![NOTE_REPORTED.to_string()] })
    }

    /// 1ファイルの差分（統一diff）。出所は一覧と同じく混ぜない。
    pub async fn get_file_diff(self: &Arc<Self>, args: GetFileDiffArgs) -> Result<UnifiedDiff, IpcError> {
        self.require_chat(&args.chat)?;
        let out = |status: ListStatus, text: String| UnifiedDiff { path: args.path.clone(), source: args.source, status, text };
        match args.source {
            ChangeSource::BackendReported => {
                let scope = match &args.turn {
                    Some(t) => ChangeScope::Turn { turn: t.clone() },
                    None => ChangeScope::Chat,
                };
                let (files, status) = self.reported_files(&args.chat, &scope).await;
                if !matches!(status, ListStatus::Ready) {
                    return Ok(out(status, String::new()));
                }
                Ok(match files.iter().find(|f| f.same(&args.path)) {
                    Some(f) => out(ListStatus::Ready, f.text.clone()),
                    None => out(ListStatus::NotFetched { message: "このファイルの報告を受け取っていません".into() }, String::new()),
                })
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

    /// 戻す操作を止める条件。作業中・停止未確認・送信の受理不明・同じ作業フォルダの別チャットの作業中・削除保留・外部実行中。
    fn revert_blocker(&self, chat: &ChatKey) -> Option<IpcError> {
        if let Err(e) = self.check_not_delete_pending(chat) {
            return Some(e);
        }
        let busy = |d: &HostData, c: &ChatKey| d.agents.iter().any(|v| &v.agent.chat == c && d.running_turn.contains_key(&v.agent.key)) || d.open_stop(c).is_some();
        let (this_busy, ext, others_busy) = self.read(|d| {
            let cwd = d.chat(chat).and_then(|c| c.cwd.value().map(|s| norm_path(s)));
            let ext = d.chat(chat).is_some_and(|c| external_send_locked(c.origin, d.root_view(chat).map(|v| v.freshness)));
            let others = cwd.is_some_and(|w| d.chats.iter().any(|c| &c.key != chat && c.cwd.value().is_some_and(|x| norm_path(x) == w) && busy(d, &c.key)));
            (busy(d, chat), ext, others)
        });
        if ext {
            return Some(blocked(BlockedReason::ExternalRunning, "外部で作成された会話は閲覧のみです。再開して状態を確認するまで、変更は戻せません"));
        }
        if this_busy || self.unknown_attempt(chat).is_some() {
            return Some(blocked(BlockedReason::ChatBusy, "作業中・停止未確認、または送信の受理が未確認のため、変更は戻せません。完了または停止の確認後に実行してください"));
        }
        if others_busy {
            return Some(blocked(BlockedReason::ChatBusy, "同じ作業フォルダで作業中の別のチャットがあるため、変更は戻せません"));
        }
        None
    }

    async fn plan_now(self: &Arc<Self>, chat: &ChatKey, sel: &Selection) -> Result<Vec<Planned>, IpcError> {
        let (records, marks) = self.load_changes().await.map_err(|m| err(IpcErrorCode::Io, m))?;
        let cwd = self.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned()));
        let (chat, sel) = (chat.clone(), sel.clone());
        tokio::task::spawn_blocking(move || revert::plan_confined(&chat, &records, &marks, &sel, &|p: &str| read_current(p), &|p: &str| confine_to(cwd.as_deref(), p)))
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("計画の作成に失敗しました: {e}")))
    }

    /// 戻す計画の作成（読取りのみ）。ファイルごとに戻せるか・戻せない理由を返す。計画は5分間有効。
    pub async fn preview_revert(self: &Arc<Self>, args: PreviewRevertArgs) -> Result<RevertPlan, IpcError> {
        self.require_chat(&args.chat)?;
        let sel = Selection { turn: args.turn, paths: args.paths };
        let planned = self.plan_now(&args.chat, &sel).await?;
        let now = now_ms();
        let id = self.local_id("rvp");
        let items = planned.iter().map(to_item).collect();
        {
            let mut plans = self.changes_rt.plans.lock().unwrap();
            plans.retain(|_, p| now.0 - p.created_at.0 < PLAN_TTL_MS);
            while plans.len() >= MAX_PLANS {
                let Some(oldest) = plans.iter().min_by_key(|(_, p)| p.created_at).map(|(k, _)| k.clone()) else { break };
                plans.remove(&oldest);
            }
            plans.insert(id.clone(), StoredPlan { chat: args.chat.clone(), sel, created_at: now, planned });
        }
        Ok(RevertPlan { id, chat: args.chat, items, created_at: now, expires_at: UnixMillis(now.0 + PLAN_TTL_MS) })
    }

    /// 変更を戻す（確認画面の後だけ）。実行前に再計画して計画と一致しなければ止める。控えを保存できなければ何も変えない。
    pub async fn revert_changes(self: &Arc<Self>, args: RevertChangesArgs, _confirmed: &UserConfirmed) -> Result<RevertResult, IpcError> {
        self.require_chat(&args.chat)?;
        let _one_at_a_time = self.changes_rt.revert_lock.lock().await;
        let stale = || blocked(BlockedReason::PlanStale, "戻す計画が古くなったか、見つかりません。計画を作り直してください");
        let (sel, stored) = {
            let mut plans = self.changes_rt.plans.lock().unwrap();
            match plans.remove(&args.plan_id) {
                Some(p) if p.chat == args.chat && now_ms().0 - p.created_at.0 < PLAN_TTL_MS => (p.sel, p.planned),
                _ => return Err(stale()),
            }
        };
        if args.paths.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "戻すファイルが選ばれていません"));
        }
        if let Some(e) = self.revert_blocker(&args.chat) {
            return Err(e);
        }
        let fresh = self.plan_now(&args.chat, &sel).await?;
        let mut run: Vec<(String, Vec<Write>)> = Vec::new();
        let mut failed: Vec<RevertFailure> = Vec::new();
        for p in &args.paths {
            let key = norm_path(p);
            let find = |v: &'_ [Planned]| v.iter().find(|x| norm_path(&x.path) == key).cloned();
            let (Some(a), Some(b)) = (find(&stored), find(&fresh)) else {
                failed.push(RevertFailure { path: p.clone(), reason: "計画にないファイルです".into() });
                continue;
            };
            if a != b {
                return Err(stale());
            }
            match a.outcome {
                Ok(writes) => run.push((a.path, writes)),
                Err(block) => failed.push(RevertFailure { path: p.clone(), reason: block.message }),
            }
        }
        if run.is_empty() {
            return Ok(RevertResult { reverted: vec![], failed, backup_dir: None });
        }
        let Some(store) = self.persist.store().cloned() else { return Err(err(IpcErrorCode::Io, "保存先が使えないため、戻す前の控えを保存できません。何も変更していません")) };
        let at = now_ms();
        let backup = store.revert_backup_dir(&args.chat, at);
        let chat_for_manifest = args.chat.clone();
        let run_for_blocking = run.clone();
        let host = self.clone();
        let cwd = self.read(|d| d.chat(&args.chat).and_then(|c| c.cwd.value().cloned()));
        let outcome = tokio::task::spawn_blocking(move || {
            // 領域の書込みの直列化ロックの中で行う（チャット領域の削除と同時に書かない）。
            let _g = host.persist.io_guard();
            execute_revert(&store, &chat_for_manifest, at, &backup, &run_for_blocking, cwd.as_deref())
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("戻す処理に失敗しました: {e}")))?;
        let done = match outcome {
            Ok(d) => d,
            Err(RevertStop::Space(required, available)) => {
                return Err(blocked(BlockedReason::InsufficientSpace { required, available }, "控えを保存する空き容量が足りないため、何も変更していません。容量を空けてから再実行してください（保存データは自動では削除しません）"))
            }
            Err(RevertStop::Outside(message)) => return Err(err(IpcErrorCode::InvalidArgs, format!("書き換え先が作業フォルダの外、または確認できないため、何も変更していません: {message}"))),
            Err(RevertStop::Backup(message)) => return Err(err(IpcErrorCode::Io, format!("戻す前の控えを保存できなかったため、何も変更していません: {message}"))),
        };
        failed.extend(done.failed);
        if !done.touched.is_empty() {
            let line = ChangeLine::Reverted { at, chat: args.chat.clone(), paths: done.touched, backup_dir: done.backup_dir.clone() };
            let store = self.persist.store().cloned();
            if let Some(store) = store {
                let chat = args.chat.clone();
                let res = tokio::task::spawn_blocking(move || store.append_change(&chat, &line)).await;
                if !matches!(res, Ok(Ok(()))) {
                    self.warn("戻した記録を保存できませんでした。ファイルは戻っています（控えは保存済み）。同じ変更をもう一度戻すことはできません");
                }
            }
        }
        self.mutate(|_| ((), vec![HostEvent::ChangesUpdated { chat: args.chat.clone(), turn: None }]));
        Ok(RevertResult { reverted: done.reverted, failed, backup_dir: Some(done.backup_dir) })
    }
}

fn to_item(p: &Planned) -> RevertItem {
    let verdict = match &p.outcome {
        Ok(writes) => {
            let summary = if writes.len() > 1 {
                format!("移動を元に戻します（{} へ）", writes[0].path)
            } else if writes.first().is_some_and(|w| w.content.is_none()) {
                "追加されたファイルを削除します".to_string()
            } else if p.kind == Some(ChangeKind::Deleted) {
                "削除されたファイルを復元します".to_string()
            } else {
                "変更前の内容に戻します".to_string()
            };
            RevertVerdict::Revertible { summary }
        }
        Err(b) => RevertVerdict::Blocked { code: b.code, message: b.message.clone() },
    };
    RevertItem { path: p.path.clone(), kind: p.kind, includes_turns: p.includes_turns.clone(), verdict }
}

enum RevertStop {
    /// 書換え直前の再検査で、作業フォルダの外・解決不能のパスが見つかった（何も書き換えていない）。
    Outside(String),
    Space(u64, u64),
    Backup(String),
}

struct Executed {
    reverted: Vec<String>,
    failed: Vec<RevertFailure>,
    /// 書き換えた（または書換えを始めた）パス。以後の判定で消化済みにする。
    touched: Vec<String>,
    backup_dir: String,
}

/// 控えを保存してから書き換える。控えの保存に失敗したら、何も書き換えずに止める。
fn execute_revert(store: &crate::store::Store, chat: &ChatKey, at: UnixMillis, backup: &Path, run: &[(String, Vec<Write>)], cwd: Option<&str>) -> Result<Executed, RevertStop> {
    // 0. 書き換える全パス（移動元を含む）を、控え・書込み・削除の前にもう一度、解決後の位置で検査する。1つでも外なら何もしない。
    for (_, writes) in run {
        for w in writes {
            revert::check_inside(&w.path, &|p: &str| confine_to(cwd, p)).map_err(|b| RevertStop::Outside(b.message))?;
        }
    }
    // 1. 控え（書き換えるすべてのファイルの現在の内容）。
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut contents: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (_, writes) in run {
        for w in writes {
            if seen.iter().any(|s| norm_path(s) == norm_path(&w.path)) {
                continue;
            }
            seen.push(w.path.clone());
            let idx = entries.len();
            match read_current(&w.path) {
                Ok(Some(bytes)) => {
                    total += bytes.len() as u64;
                    entries.push((w.path.clone(), true));
                    contents.push((idx, bytes));
                }
                Ok(None) => entries.push((w.path.clone(), false)),
                Err(m) => return Err(RevertStop::Backup(format!("{}: {m}", w.path))),
            }
        }
    }
    match store.check_space(total + 64 * 1024) {
        Ok(()) => {}
        Err(StoreError::InsufficientSpace { required, available }) => return Err(RevertStop::Space(required, available)),
        Err(e) => return Err(RevertStop::Backup(save_failure_message(&e))),
    }
    let mut manifest_entries = Vec::new();
    for (idx, (path, existed)) in entries.iter().enumerate() {
        let file = if *existed {
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let file = format!("{idx}-{}", layout::sanitize_file_name(&name));
            let bytes = &contents.iter().find(|(i, _)| *i == idx).expect("content of an existing file").1;
            atomic::write_atomic(&backup.join(&file), bytes).map_err(|e| RevertStop::Backup(e.to_string()))?;
            Some(file)
        } else {
            None
        };
        manifest_entries.push(json!({"file": file, "original": path, "existed": existed}));
    }
    let manifest = json!({"schemaVersion": 1, "createdAt": at.0, "chat": {"backend": chat.backend, "id": chat.id.0}, "entries": manifest_entries});
    let text = serde_json::to_vec_pretty(&manifest).map_err(|e| RevertStop::Backup(e.to_string()))?;
    atomic::write_atomic(&backup.join("manifest.json"), &text).map_err(|e| RevertStop::Backup(e.to_string()))?;

    // 2. 書換え。ファイルごとに成否を記録する（一部だけ成功しても完了とは言わない）。
    let mut reverted = Vec::new();
    let mut failed = Vec::new();
    let mut touched = Vec::new();
    for (path, writes) in run {
        let mut step_err: Option<String> = None;
        for w in writes {
            touched.push(w.path.clone());
            let res = match &w.content {
                Some(bytes) => atomic::write_atomic(Path::new(&w.path), bytes),
                None => match std::fs::remove_file(&w.path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                },
            };
            if let Err(e) = res {
                step_err = Some(format!("{}: {e}", w.path));
                break;
            }
        }
        match step_err {
            None => reverted.push(path.clone()),
            Some(m) => failed.push(RevertFailure { path: path.clone(), reason: format!("書き換えに失敗しました（一部だけ実行された可能性があります。控えから確認できます）: {m}") }),
        }
    }
    Ok(Executed { reverted, failed, touched, backup_dir: backup.to_string_lossy().into_owned() })
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
}
