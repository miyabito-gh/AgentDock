//! Git基準の控えによる「変更を戻す」の判定（P3B-2、`app/DESIGN_P3B.md` §1.3・§4.3・§4.4）。純粋ロジック（I/OもGit実行もしない）。
//!
//! - `classify`: 控える対象の分類（Hash／Gone／Skip）。
//! - `plan`: ファイルごとの判定。まず「止める（強制不可）」を調べ、当たらなければ「要確認（強制可）」をすべて集める。
//!   止める条件が要確認より常に優先。要確認がなければ `Revertable`。推定で `Revertable` にしない。
//! - `admit`: 実行時の再計画と、計画時の判定・`forced` との突き合わせ（強制の受付判定）。
//! - `recheck`: 書込み直前の再照合（計画時の `expect` と現在の内容）。
//! - `aggregate`: 書込み結果の集計。成功分だけを記録し、部分成功を完了と偽らない。
//!
//! 旧 `rules::revert`（差分の逆適用）とは独立している（差し替えは P3B-4）。

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::backend::baseline::*;
use crate::backend::changes::{OverrideReason, RevertBlockCode, RevertFailure, RevertVerdict};
use crate::backend::model::*;

/// バイト列のSHA-256（16進小文字）。書込み直前の再照合に使う。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// パスの比較用の形（Windows: 区切りと大文字小文字を区別しない）。
pub fn norm_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

// ───────────────────────────── classify ─────────────────────────────

/// 作業ツリーのパスの状態（`symlink_metadata` の結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileMeta {
    Missing,
    Regular { size: u64 },
    /// シンボリックリンク・ジャンクション。
    Link,
    /// ディレクトリ（入れ子のリポジトリなど）。
    Directory,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classified {
    /// 生バイトでハッシュして控える。
    Hash,
    /// 削除されている（木から外す）。
    Gone,
    Skip(SkipReason),
}

/// HEADと違うパス1つの分類。`wt_mode` は status の作業ツリー側 mode（未追跡は空）、`used` はこの控えで取り込んだ生バイト合計（取り込むなら加算する）。
pub fn classify(wt_mode: &str, meta: &FileMeta, used: &mut u64) -> Classified {
    match wt_mode {
        "160000" => return Classified::Skip(SkipReason::Submodule),
        "120000" => return Classified::Skip(SkipReason::Link),
        "" | "000000" | "100644" | "100755" => {}
        _ => return Classified::Skip(SkipReason::Unreadable),
    }
    match meta {
        FileMeta::Missing => Classified::Gone,
        FileMeta::Link => Classified::Skip(SkipReason::Link),
        FileMeta::Directory | FileMeta::Unreadable => Classified::Skip(SkipReason::Unreadable),
        FileMeta::Regular { size } => {
            if *size > BASELINE_FILE_LIMIT {
                Classified::Skip(SkipReason::TooLarge)
            } else if used.saturating_add(*size) > BASELINE_BUDGET {
                Classified::Skip(SkipReason::Budget)
            } else {
                *used += *size;
                Classified::Hash
            }
        }
    }
}

// ───────────────────────────── 入力 ─────────────────────────────

/// あるスナップショットにおけるパス1つの状態（ホストが `git ls-tree` で引いて渡す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    Present(EntryRef),
    Absent,
    Skipped(SkipReason),
}

/// どの時点の状態を引くか。数は `PlanInput::segments` の添字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateAt {
    Base(usize),
    End(usize),
    Current,
}

/// 区間の終了の控え。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegEnd {
    /// 終了未確認（切断・再起動・作業中）。
    Unknown,
    Snapshot { at: UnixMillis, head: Option<String>, branch: Option<String> },
    /// 次の区間の基準をそのまま終了とする。
    NextBase,
}

/// 同時作業の相手。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrentRec {
    /// AgentDock外。この区間で変わったパスはすべて帰属不明。
    External,
    /// AgentDockの別チャット。`other_changed` は相手の重なった区間で変わったパス、`other_end_known=false` なら相手の終了が不明（変わった可能性あり）。
    Chat { other_changed: Vec<String>, other_end_known: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub turn: Option<ExternalId>,
    pub base_at: UnixMillis,
    pub base_head: Option<String>,
    pub base_branch: Option<String>,
    pub end: SegEnd,
    pub concurrent: Vec<ConcurrentRec>,
}

/// AgentDockの「戻し」の記録（パス1件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertMark {
    pub at: UnixMillis,
    pub path: String,
    pub restored: Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoStatus {
    Ready,
    NotARepository,
    GitUnavailable,
}

/// 現在の作業ツリー上のパス。`sha` は直接読んだ内容のSHA-256。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentFile {
    Missing,
    File { sha: String },
    Directory,
}

pub type Confine<'a> = &'a dyn Fn(&str) -> Result<(), String>;

pub struct PlanInput<'a> {
    pub repo: RepoStatus,
    /// rebase・merge・cherry-pick 等の途中。
    pub git_busy: bool,
    /// 作業フォルダ（チャットのcwd）のリポジトリ相対パス（`/` 区切り。ルートそのものなら空）。
    pub work_prefix: String,
    /// 区間（時系列、基準を取れたものだけ）。
    pub segments: &'a [Segment],
    /// この区間以降を戻す（None＝最初の区間から）。
    pub from_turn: Option<ExternalId>,
    pub current_head: Option<String>,
    pub current_branch: Option<String>,
    pub reverted: &'a [RevertMark],
    /// 対象パス（リポジトリ相対、`/` 区切り）。
    pub paths: &'a [String],
    pub states: &'a dyn Fn(StateAt, &str) -> EntryState,
    pub files: &'a dyn Fn(&str) -> CurrentFile,
    /// 解決後の位置が作業フォルダの内側かを確かめる（リンク越えなど）。
    pub confine: Confine<'a>,
}

// ───────────────────────────── 出力 ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub code: RevertBlockCode,
    pub message: String,
}

fn reason(code: RevertBlockCode, message: impl Into<String>) -> Reason {
    Reason { code, message: message.into() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    Revertable,
    /// 要確認（強制可）。理由はすべて集めたもの。
    NeedsOverride(Vec<Reason>),
    /// 止める（強制不可）。複数に当たるときは優先度の高い1つ。
    Blocked(Reason),
}

impl Judgement {
    pub fn to_verdict(&self) -> RevertVerdict {
        match self {
            Judgement::Revertable => RevertVerdict::Revertible { summary: "控えの内容へ戻せます".into() },
            Judgement::NeedsOverride(rs) => RevertVerdict::NeedsOverride {
                summary: format!("要確認: 別の変更が含まれている可能性があります（{}件の理由）", rs.len()),
                reasons: rs.iter().map(|r| OverrideReason { code: r.code, message: r.message.clone() }).collect(),
            },
            Judgement::Blocked(r) => RevertVerdict::Blocked { code: r.code, message: r.message.clone() },
        }
    }

    fn codes(&self) -> Vec<RevertBlockCode> {
        match self {
            Judgement::NeedsOverride(rs) => rs.iter().map(|r| r.code).collect(),
            _ => Vec::new(),
        }
    }
}

/// 書込みの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// 現在ある内容を控えの内容で上書きする。
    Overwrite,
    /// 控えの時点でなかった（新規追加された）ファイルを削除する。
    RemoveAdded,
    /// 削除されたファイルを控えの内容で復元する。
    Restore,
}

/// 書込み操作。`target` が `None` なら削除。`expect` は計画時の現在の内容のSHA-256（無ければ `None`）で、書込み直前に再照合する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOp {
    pub path: String,
    pub kind: WriteKind,
    pub target: Option<EntryRef>,
    pub expect: Option<String>,
}

impl WriteOp {
    pub fn restored(&self) -> Restored {
        match &self.target {
            Some(e) => Restored::Entry { entry: e.clone() },
            None => Restored::Absent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPlan {
    pub path: String,
    pub judgement: Judgement,
    /// 一緒に戻る、より後のturn。
    pub includes_turns: Vec<ExternalId>,
    /// `Blocked` 以外のとき。
    pub write: Option<WriteOp>,
}

// ───────────────────────────── パスの閉じ込め ─────────────────────────────

/// 書き換える場所として安全か。字句検査（絶対パス・`..`・ドライブ・バックスラッシュ）→作業フォルダの内側→解決後の位置。
pub fn check_inside(path: &str, work_prefix: &str, confine: Confine) -> Result<(), Reason> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|c| c == ".." || c.is_empty())
        || path.split('/').next().is_some_and(|c| c.contains(':'))
        || std::path::Path::new(path).is_absolute();
    if bad {
        return Err(reason(RevertBlockCode::PathOutside, format!("{path}: 絶対パス・`..`・別ドライブなどを含むため、戻せません")));
    }
    let prefix = work_prefix.trim_matches('/');
    if !prefix.is_empty() {
        let (p, w) = (norm_path(path), norm_path(prefix));
        let inside = p.strip_prefix(&w).is_some_and(|rest| rest.starts_with('\\'));
        if !inside {
            return Err(reason(RevertBlockCode::OutsideWorkFolder, format!("{path}: 作業フォルダの外のファイルは戻しません")));
        }
    }
    confine(path).map_err(|m| reason(RevertBlockCode::PathOutside, format!("{path}: 作業フォルダの内側であることを確認できないため、戻せません（{m}）")))
}

// ───────────────────────────── plan ─────────────────────────────

fn global_block(input: &PlanInput, start: Option<usize>) -> Option<Reason> {
    if input.git_busy {
        return Some(reason(RevertBlockCode::GitBusy, "Gitの操作の途中のため戻せません。完了または中止してから再確認してください"));
    }
    match input.repo {
        RepoStatus::NotARepository => {
            return Some(reason(RevertBlockCode::NotARepository, "作業フォルダがGitリポジトリではないため、変更の控えを取っていません。戻せません"));
        }
        RepoStatus::GitUnavailable => {
            return Some(reason(RevertBlockCode::GitUnavailable, "Gitを実行できません（未導入、または設定のGitの場所が違います）"));
        }
        RepoStatus::Ready => {}
    }
    if start.is_none() {
        return Some(reason(RevertBlockCode::NoBaseline, "このturnは控えを取る前のものです（または控えを取れませんでした）"));
    }
    None
}

/// `from_turn` に対応する区間の添字（None＝最初）。見つからなければ None。
fn start_index(input: &PlanInput) -> Option<usize> {
    if input.segments.is_empty() {
        return None;
    }
    match &input.from_turn {
        None => Some(0),
        Some(t) => input.segments.iter().position(|s| s.turn.as_ref() == Some(t)),
    }
}

/// HEAD・ブランチの連続性（S4）。控えの時点の並び B_k, E_k?, B_k+1, ..., 現在 で隣り合う既知の点が変わっていれば真。
fn head_moved(input: &PlanInput, k: usize) -> bool {
    let mut points: Vec<(Option<&String>, Option<&String>)> = Vec::new();
    for s in &input.segments[k..] {
        points.push((s.base_head.as_ref(), s.base_branch.as_ref()));
        if let SegEnd::Snapshot { head, branch, .. } = &s.end {
            points.push((head.as_ref(), branch.as_ref()));
        }
    }
    points.push((input.current_head.as_ref(), input.current_branch.as_ref()));
    points.windows(2).any(|w| w[0] != w[1])
}

/// 区間jの終了時のパスの状態。終了不明（最後の区間の NextBase を含む）は None。
fn end_state(input: &PlanInput, j: usize, p: &str) -> Option<EntryState> {
    match &input.segments[j].end {
        SegEnd::Snapshot { .. } => Some((input.states)(StateAt::End(j), p)),
        SegEnd::NextBase if j + 1 < input.segments.len() => Some((input.states)(StateAt::Base(j + 1), p)),
        _ => None,
    }
}

fn mark_state(m: &RevertMark) -> EntryState {
    match &m.restored {
        Restored::Entry { entry } => EntryState::Present(entry.clone()),
        Restored::Absent => EntryState::Absent,
    }
}

fn plan_path(input: &PlanInput, k: usize, head_moved: bool, latest_at: UnixMillis, p: &str) -> PathPlan {
    let segs = input.segments;
    let n = segs.len() - 1;
    let blocked = |r: Reason| PathPlan { path: p.to_string(), judgement: Judgement::Blocked(r), includes_turns: Vec::new(), write: None };

    // 止める（強制不可）。S2 → S3 → S4 → S5 の順に最初に当たったもの。
    if let Err(r) = check_inside(p, &input.work_prefix, input.confine) {
        return blocked(r);
    }
    let cur = (input.states)(StateAt::Current, p);
    let mut skipped = matches!(cur, EntryState::Skipped(_));
    for j in k..=n {
        skipped |= matches!((input.states)(StateAt::Base(j), p), EntryState::Skipped(_));
        skipped |= matches!(end_state(input, j, p), Some(EntryState::Skipped(_)));
    }
    if skipped {
        return blocked(reason(RevertBlockCode::NotSnapshotted, format!("{p}: 控えの対象外です（シンボリックリンク／サブモジュール／64MiB超／読めないファイル）")));
    }
    let file = (input.files)(p);
    if file == CurrentFile::Directory {
        return blocked(reason(RevertBlockCode::NotSnapshotted, format!("{p}: 同じ名前のフォルダがあるため、戻せません")));
    }
    if head_moved {
        return blocked(reason(RevertBlockCode::HeadMoved, "turnの後にGitのHEADまたはブランチが変わったため、控えへは戻せません。Gitの操作（git revert 等）で戻してください"));
    }
    let target = (input.states)(StateAt::Base(k), p);
    if let Some(m) = input.reverted.iter().filter(|m| norm_path(&m.path) == norm_path(p) && m.at >= latest_at).max_by_key(|m| m.at) {
        if mark_state(m) == cur {
            return blocked(reason(RevertBlockCode::AlreadyReverted, format!("{p}: すでに控えの内容へ戻してあります")));
        }
    }
    if target == cur {
        return blocked(reason(RevertBlockCode::AlreadyReverted, format!("{p}: すでに控えの内容と同じです")));
    }

    // 要確認（強制可）。すべて集める。
    let mut reasons: Vec<Reason> = Vec::new();
    let mut push = |r: Reason| {
        if !reasons.iter().any(|x| x.code == r.code) {
            reasons.push(r);
        }
    };
    if segs[k..].iter().any(|s| !matches!(s.end, SegEnd::Snapshot { .. } | SegEnd::NextBase)) || matches!(segs[n].end, SegEnd::NextBase) {
        push(reason(RevertBlockCode::EndUnknown, format!("{p}: turnの終了時の控えがありません（切断・再起動など）。turn後の別の変更と区別できません")));
    }
    for j in k..n {
        let (SegEnd::Snapshot { at: end_at, .. }, Some(e)) = (&segs[j].end, end_state(input, j, p)) else { continue };
        let next = (input.states)(StateAt::Base(j + 1), p);
        if e != next {
            let bridged = input.reverted.iter().any(|m| norm_path(&m.path) == norm_path(p) && m.at >= *end_at && m.at <= segs[j + 1].base_at && mark_state(m) == next);
            if !bridged {
                push(reason(RevertBlockCode::ChangedBetween, format!("{p}: turnの間に、別の会話・エディタ等による変更があります")));
            }
        }
    }
    if let (SegEnd::Snapshot { .. }, Some(e)) = (&segs[n].end, end_state(input, n, p)) {
        if e != cur {
            push(reason(RevertBlockCode::ChangedAfter, format!("{p}: turnの後に、別の会話・エディタ等による変更があります")));
        }
    }
    let np = norm_path(p);
    for j in k..=n {
        if segs[j].concurrent.is_empty() {
            continue;
        }
        // この区間でpが変わった（終了不明は変わった可能性あり）。
        let changed = match end_state(input, j, p) {
            Some(e) => e != (input.states)(StateAt::Base(j), p),
            None => true,
        };
        if !changed {
            continue;
        }
        let hit = segs[j].concurrent.iter().any(|c| match c {
            ConcurrentRec::External => true,
            ConcurrentRec::Chat { other_changed, other_end_known } => !other_end_known || other_changed.iter().any(|x| norm_path(x) == np),
        });
        if hit {
            push(reason(RevertBlockCode::ConcurrentChange, format!("{p}: 同じリポジトリで同時に作業していた別の会話も、このファイルを変更した可能性があります")));
        }
    }

    let includes_turns: Vec<ExternalId> = ((k + 1)..=n)
        .filter(|&j| match end_state(input, j, p) {
            Some(e) => e != (input.states)(StateAt::Base(j), p),
            None => true,
        })
        .filter_map(|j| segs[j].turn.clone())
        .collect();
    let expect = match &file {
        CurrentFile::File { sha } => Some(sha.clone()),
        _ => None,
    };
    let (kind, target_ref) = match (&cur, &target) {
        (EntryState::Absent, EntryState::Present(e)) => (WriteKind::Restore, Some(e.clone())),
        (EntryState::Present(_), EntryState::Absent) => (WriteKind::RemoveAdded, None),
        (_, EntryState::Present(e)) => (WriteKind::Overwrite, Some(e.clone())),
        // 両方 Absent は target == cur で止めてある。Skipped は S3 で止めてある。
        _ => return blocked(reason(RevertBlockCode::NotSnapshotted, format!("{p}: 控えの状態を判定できません"))),
    };
    let write = Some(WriteOp { path: p.to_string(), kind, target: target_ref, expect });
    let judgement = if reasons.is_empty() { Judgement::Revertable } else { Judgement::NeedsOverride(reasons) };
    PathPlan { path: p.to_string(), judgement, includes_turns, write }
}

/// 対象パスごとの判定と書込み計画を作る。全体を止める条件（Git操作途中・リポジトリなし・控えなし）は全パスが `Blocked` になる。
pub fn plan(input: &PlanInput) -> Vec<PathPlan> {
    let start = start_index(input);
    if let Some(r) = global_block(input, start) {
        return input
            .paths
            .iter()
            .map(|p| PathPlan { path: p.clone(), judgement: Judgement::Blocked(r.clone()), includes_turns: Vec::new(), write: None })
            .collect();
    }
    let k = start.expect("checked by global_block");
    let moved = head_moved(input, k);
    let last = &input.segments[input.segments.len() - 1];
    let latest_at = match &last.end {
        SegEnd::Snapshot { at, .. } if *at > last.base_at => *at,
        _ => last.base_at,
    };
    input.paths.iter().map(|p| plan_path(input, k, moved, latest_at, p)).collect()
}

// ───────────────────────────── 強制の受付判定 ─────────────────────────────

/// 実行する1件。`forced` は強制として戻すもの、`overridden` はそのとき無視した理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecItem {
    pub op: WriteOp,
    pub forced: bool,
    pub overridden: Vec<RevertBlockCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub execute: Vec<ExecItem>,
    pub failed: Vec<RevertFailure>,
    /// 計画が古い（理由が増えた・計画後に状態が変わった）。あれば何も実行しない。
    pub stale: Option<String>,
}

fn fail(path: &str, msg: impl Into<String>) -> RevertFailure {
    RevertFailure { path: path.to_string(), reason: msg.into() }
}

/// `old`＝保存した計画、`fresh`＝書込みロック内の再計画。`paths` と `forced` はユーザーの指定。
/// - 再計画で `Revertable` → そのまま実行（計画時が要確認でも、強制の指定があれば）。
/// - 強制の指定があり再計画も `NeedsOverride` で理由が増えていない → 強制として実行。理由が増えた・計画時は戻せるはずだった → `stale`。
/// - 再計画で `Blocked` → 強制の指定があっても書かず `failed`。
pub fn admit(old: &[PathPlan], fresh: &[PathPlan], paths: &[String], forced: &[String]) -> Admission {
    let mut out = Admission { execute: Vec::new(), failed: Vec::new(), stale: None };
    let mut seen: HashSet<String> = HashSet::new();
    let requested = paths.iter().chain(forced.iter());
    for p in requested {
        if !seen.insert(norm_path(p)) {
            continue;
        }
        let is_forced = forced.iter().any(|f| norm_path(f) == norm_path(p));
        let find = |plans: &'_ [PathPlan]| plans.iter().find(|x| norm_path(&x.path) == norm_path(p)).cloned();
        let (Some(o), Some(f)) = (find(old), find(fresh)) else {
            out.stale.get_or_insert_with(|| p.clone());
            continue;
        };
        match (&f.judgement, &f.write) {
            (Judgement::Blocked(r), _) => out.failed.push(fail(p, r.message.clone())),
            (Judgement::Revertable, Some(op)) => {
                if matches!(o.judgement, Judgement::Blocked(_)) {
                    out.failed.push(fail(p, "計画時に戻せないと判定されたファイルです"));
                } else if matches!(o.judgement, Judgement::NeedsOverride(_)) && !is_forced {
                    out.failed.push(fail(p, "要確認のファイルです。強制の指定がないため戻しません"));
                } else {
                    out.execute.push(ExecItem { op: op.clone(), forced: false, overridden: Vec::new() });
                }
            }
            (Judgement::NeedsOverride(rs), Some(op)) => {
                let old_codes = o.judgement.codes();
                let new_codes = f.judgement.codes();
                let grew = !new_codes.iter().all(|c| old_codes.contains(c));
                if !matches!(o.judgement, Judgement::NeedsOverride(_)) || grew {
                    out.stale.get_or_insert_with(|| p.clone());
                } else if !is_forced {
                    out.failed.push(fail(p, rs.iter().map(|r| r.message.as_str()).collect::<Vec<_>>().join(" / ")));
                } else {
                    out.execute.push(ExecItem { op: op.clone(), forced: true, overridden: new_codes });
                }
            }
            _ => out.failed.push(fail(p, "書込み内容を決められませんでした")),
        }
    }
    out
}

// ───────────────────────────── 再照合・集計 ─────────────────────────────

/// 書込み直前の再照合。現在の内容が計画時の `expect` と同じなら Ok。強制のファイルも同じ。
pub fn recheck(expect: &Option<String>, now: &CurrentFile) -> Result<(), String> {
    let same = match (expect, now) {
        (None, CurrentFile::Missing) => true,
        (Some(h), CurrentFile::File { sha }) => h == sha,
        _ => false,
    };
    if same {
        Ok(())
    } else {
        Err("計画後に変更されたため書き換えませんでした".into())
    }
}

/// 書込み1件の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub item: ExecItem,
    /// 書き換え前の内容の控え（`revert-backup` 内のファイル名）。
    pub backup_file: Option<String>,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    /// 成功分だけ（`Reverted` の記録に入れる）。
    pub items: Vec<RevertedPath>,
    pub reverted: Vec<String>,
    pub forced: Vec<String>,
    pub failed: Vec<RevertFailure>,
}

impl Aggregate {
    /// 失敗が1件もない場合だけ完了。1件でもあれば「一部のみ」。
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// 書込み結果の集計。`prior_failed` は実行前に失敗とした分（`Admission::failed`）。
pub fn aggregate(outcomes: Vec<WriteOutcome>, prior_failed: Vec<RevertFailure>) -> Aggregate {
    let mut agg = Aggregate { items: Vec::new(), reverted: Vec::new(), forced: Vec::new(), failed: prior_failed };
    for o in outcomes {
        match o.result {
            Ok(()) => {
                agg.reverted.push(o.item.op.path.clone());
                if o.item.forced {
                    agg.forced.push(o.item.op.path.clone());
                }
                agg.items.push(RevertedPath {
                    path: o.item.op.path.clone(),
                    restored: o.item.op.restored(),
                    backup_file: o.backup_file,
                    forced: o.item.forced,
                    overridden: if o.item.forced { o.item.overridden } else { Vec::new() },
                });
            }
            Err(m) => agg.failed.push(fail(&o.item.op.path, m)),
        }
    }
    agg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const OK: RevertBlockCode = RevertBlockCode::ChangedAfter;

    fn ent(oid: &str) -> EntryState {
        EntryState::Present(EntryRef { oid: oid.into(), form: EntryForm::Raw })
    }
    fn head_ent(oid: &str) -> EntryState {
        EntryState::Present(EntryRef { oid: oid.into(), form: EntryForm::Head })
    }
    fn ext(s: &str) -> Option<ExternalId> {
        Some(ExternalId(s.into()))
    }
    fn seg(turn: &str, at: i64, end: SegEnd) -> Segment {
        Segment { turn: ext(turn), base_at: UnixMillis(at), base_head: Some("h1".into()), base_branch: Some("main".into()), end, concurrent: vec![] }
    }
    fn snap_end(at: i64) -> SegEnd {
        SegEnd::Snapshot { at: UnixMillis(at), head: Some("h1".into()), branch: Some("main".into()) }
    }

    /// 状態表つきの入力の元。`set` に無い (時点, パス) は Absent。
    struct Fx {
        segs: Vec<Segment>,
        states: HashMap<(StateAt, String), EntryState>,
        files: HashMap<String, CurrentFile>,
        reverted: Vec<RevertMark>,
        from_turn: Option<ExternalId>,
        head: Option<String>,
        branch: Option<String>,
        busy: bool,
        repo: RepoStatus,
        prefix: String,
    }

    impl Fx {
        fn new(segs: Vec<Segment>) -> Fx {
            Fx {
                segs,
                states: HashMap::new(),
                files: HashMap::new(),
                reverted: vec![],
                from_turn: None,
                head: Some("h1".into()),
                branch: Some("main".into()),
                busy: false,
                repo: RepoStatus::Ready,
                prefix: String::new(),
            }
        }
        fn set(&mut self, at: StateAt, p: &str, s: EntryState) -> &mut Fx {
            self.states.insert((at, p.to_string()), s);
            self
        }
        /// 現在の状態と実ファイルを同時に決める（oid が空なら Absent）。
        fn cur(&mut self, p: &str, oid: &str) -> &mut Fx {
            if oid.is_empty() {
                self.states.insert((StateAt::Current, p.into()), EntryState::Absent);
                self.files.insert(p.into(), CurrentFile::Missing);
            } else {
                self.states.insert((StateAt::Current, p.into()), ent(oid));
                self.files.insert(p.into(), CurrentFile::File { sha: format!("sha-{oid}") });
            }
            self
        }
        fn run(&self, paths: &[&str]) -> Vec<PathPlan> {
            let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
            let states = |at: StateAt, p: &str| self.states.get(&(at, p.to_string())).cloned().unwrap_or(EntryState::Absent);
            let files = |p: &str| self.files.get(p).cloned().unwrap_or(CurrentFile::Missing);
            let confine = |_: &str| Ok(());
            plan(&PlanInput {
                repo: self.repo,
                git_busy: self.busy,
                work_prefix: self.prefix.clone(),
                segments: &self.segs,
                from_turn: self.from_turn.clone(),
                current_head: self.head.clone(),
                current_branch: self.branch.clone(),
                reverted: &self.reverted,
                paths: &paths,
                states: &states,
                files: &files,
                confine: &confine,
            })
        }
        fn one(&self, p: &str) -> Judgement {
            self.run(&[p]).remove(0).judgement
        }
    }

    fn codes(j: &Judgement) -> Vec<RevertBlockCode> {
        match j {
            Judgement::NeedsOverride(rs) => rs.iter().map(|r| r.code).collect(),
            other => panic!("not NeedsOverride: {other:?}"),
        }
    }
    fn blocked(j: &Judgement) -> RevertBlockCode {
        match j {
            Judgement::Blocked(r) => r.code,
            other => panic!("not Blocked: {other:?}"),
        }
    }

    /// 1区間 B(a) → E(b)、現在 b（変更なし）。
    fn simple() -> Fx {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20))]);
        f.set(StateAt::Base(0), "a.txt", ent("a")).set(StateAt::End(0), "a.txt", ent("b")).cur("a.txt", "b");
        f
    }

    // ── classify ──
    #[test]
    fn classify_covers_regular_gone_link_submodule_and_limits() {
        let mut used = 0;
        assert_eq!(classify("100644", &FileMeta::Regular { size: 5 }, &mut used), Classified::Hash);
        assert_eq!(used, 5);
        assert_eq!(classify("", &FileMeta::Regular { size: 1 }, &mut used), Classified::Hash);
        assert_eq!(classify("000000", &FileMeta::Missing, &mut used), Classified::Gone);
        assert_eq!(classify("100644", &FileMeta::Missing, &mut used), Classified::Gone);
        assert_eq!(classify("120000", &FileMeta::Link, &mut used), Classified::Skip(SkipReason::Link));
        assert_eq!(classify("100644", &FileMeta::Link, &mut used), Classified::Skip(SkipReason::Link));
        assert_eq!(classify("160000", &FileMeta::Directory, &mut used), Classified::Skip(SkipReason::Submodule));
        assert_eq!(classify("100644", &FileMeta::Unreadable, &mut used), Classified::Skip(SkipReason::Unreadable));
        assert_eq!(classify("040000", &FileMeta::Regular { size: 1 }, &mut used), Classified::Skip(SkipReason::Unreadable));
        assert_eq!(classify("100644", &FileMeta::Regular { size: BASELINE_FILE_LIMIT + 1 }, &mut used), Classified::Skip(SkipReason::TooLarge));
        assert_eq!(classify("100644", &FileMeta::Regular { size: BASELINE_FILE_LIMIT }, &mut 0), Classified::Hash);
        let mut near = BASELINE_BUDGET - 10;
        assert_eq!(classify("100644", &FileMeta::Regular { size: 11 }, &mut near), Classified::Skip(SkipReason::Budget));
        assert_eq!(near, BASELINE_BUDGET - 10, "skipped files are not counted");
    }

    // ── plan: 基本 ──
    #[test]
    fn unchanged_since_end_is_revertable_with_overwrite() {
        let f = simple();
        let p = f.run(&["a.txt"]).remove(0);
        assert_eq!(p.judgement, Judgement::Revertable);
        let w = p.write.unwrap();
        assert_eq!(w.kind, WriteKind::Overwrite);
        assert_eq!(w.target, Some(EntryRef { oid: "a".into(), form: EntryForm::Raw }));
        assert_eq!(w.expect.as_deref(), Some("sha-b"));
    }

    #[test]
    fn added_deleted_and_modified_get_the_right_write_kind() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20))]);
        // 追加: B無し→E有り
        f.set(StateAt::End(0), "new.txt", ent("n")).cur("new.txt", "n");
        // 削除: B有り→E無し
        f.set(StateAt::Base(0), "gone.txt", ent("g")).cur("gone.txt", "");
        let r = f.run(&["new.txt", "gone.txt"]);
        assert_eq!(r[0].judgement, Judgement::Revertable);
        let w = r[0].write.clone().unwrap();
        assert_eq!((w.kind, w.target.is_none(), w.expect.as_deref()), (WriteKind::RemoveAdded, true, Some("sha-n")));
        assert_eq!(r[1].judgement, Judgement::Revertable);
        let w = r[1].write.clone().unwrap();
        assert_eq!((w.kind, w.target.is_some(), w.expect), (WriteKind::Restore, true, None));
    }

    #[test]
    fn head_form_and_raw_form_with_same_oid_differ() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20))]);
        f.set(StateAt::Base(0), "a", ent("x")).set(StateAt::End(0), "a", ent("y")).states.insert((StateAt::Current, "a".into()), head_ent("y"));
        f.files.insert("a".into(), CurrentFile::File { sha: "s".into() });
        assert_eq!(codes(&f.one("a")), vec![OK], "same oid but different form counts as changed");
    }

    #[test]
    fn no_change_in_target_is_already_reverted_not_revertable() {
        let mut f = simple();
        f.cur("a.txt", "a");
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::AlreadyReverted);
    }

    // ── 要確認 ──
    #[test]
    fn change_after_end_needs_override() {
        let mut f = simple();
        f.cur("a.txt", "zzz");
        let p = f.run(&["a.txt"]).remove(0);
        assert_eq!(codes(&p.judgement), vec![RevertBlockCode::ChangedAfter]);
        assert!(p.write.is_some(), "a forced revert still has a plan");
    }

    #[test]
    fn change_between_segments_needs_override() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20)), seg("t2", 30, snap_end(40))]);
        f.set(StateAt::Base(0), "a", ent("a0")).set(StateAt::End(0), "a", ent("a1")).set(StateAt::Base(1), "a", ent("EXT")).set(StateAt::End(1), "a", ent("a2")).cur("a", "a2");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::ChangedBetween]);
    }

    #[test]
    fn revert_in_between_bridges_the_gap() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20)), seg("t2", 30, snap_end(40))]);
        f.set(StateAt::Base(0), "a", ent("a0")).set(StateAt::End(0), "a", ent("a1")).set(StateAt::Base(1), "a", ent("a0")).set(StateAt::End(1), "a", ent("a2")).cur("a", "a2");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::ChangedBetween]);
        f.reverted.push(RevertMark { at: UnixMillis(25), path: "a".into(), restored: Restored::Entry { entry: EntryRef { oid: "a0".into(), form: EntryForm::Raw } } });
        assert_eq!(f.one("a"), Judgement::Revertable);
        f.reverted[0].at = UnixMillis(5);
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::ChangedBetween], "a revert outside the gap does not bridge");
    }

    #[test]
    fn unknown_end_needs_override_and_skips_after_check() {
        let mut f = Fx::new(vec![seg("t1", 10, SegEnd::Unknown)]);
        f.set(StateAt::Base(0), "a", ent("a")).cur("a", "zzz");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::EndUnknown]);
    }

    #[test]
    fn unknown_end_in_the_middle_still_needs_override() {
        let mut f = Fx::new(vec![seg("t1", 10, SegEnd::Unknown), seg("t2", 30, snap_end(40))]);
        f.set(StateAt::Base(0), "a", ent("a")).set(StateAt::Base(1), "a", ent("b")).set(StateAt::End(1), "a", ent("c")).cur("a", "c");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::EndUnknown]);
    }

    #[test]
    fn end_is_next_base_is_continuous() {
        let mut f = Fx::new(vec![seg("t1", 10, SegEnd::NextBase), seg("t2", 30, snap_end(40))]);
        f.set(StateAt::Base(0), "a", ent("a0")).set(StateAt::Base(1), "a", ent("a1")).set(StateAt::End(1), "a", ent("a2")).cur("a", "a2");
        let p = f.run(&["a"]).remove(0);
        assert_eq!(p.judgement, Judgement::Revertable, "no ChangedBetween across EndIsNextBase");
        assert_eq!(p.includes_turns, vec![ExternalId("t2".into())]);
    }

    #[test]
    fn last_segment_next_base_is_treated_as_unknown() {
        let mut f = Fx::new(vec![seg("t1", 10, SegEnd::NextBase)]);
        f.set(StateAt::Base(0), "a", ent("a")).cur("a", "b");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::EndUnknown]);
    }

    #[test]
    fn concurrent_work_cases() {
        let mut f = simple();
        // 相手も変えた
        f.segs[0].concurrent = vec![ConcurrentRec::Chat { other_changed: vec!["A.TXT".into()], other_end_known: true }];
        assert_eq!(codes(&f.one("a.txt")), vec![RevertBlockCode::ConcurrentChange]);
        // 相手は別のファイルだけ
        f.segs[0].concurrent = vec![ConcurrentRec::Chat { other_changed: vec!["other.txt".into()], other_end_known: true }];
        assert_eq!(f.one("a.txt"), Judgement::Revertable);
        // 相手の終了が不明
        f.segs[0].concurrent = vec![ConcurrentRec::Chat { other_changed: vec![], other_end_known: false }];
        assert_eq!(codes(&f.one("a.txt")), vec![RevertBlockCode::ConcurrentChange]);
        // 外部
        f.segs[0].concurrent = vec![ConcurrentRec::External];
        assert_eq!(codes(&f.one("a.txt")), vec![RevertBlockCode::ConcurrentChange]);
    }

    #[test]
    fn unchanged_path_in_a_concurrent_segment_is_not_flagged() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20))]);
        f.segs[0].concurrent = vec![ConcurrentRec::External];
        // B と E が同じ（この区間では変わっていない）が、後で別の変更があり戻す候補になった
        f.set(StateAt::Base(0), "a", ent("a")).set(StateAt::End(0), "a", ent("a")).cur("a", "z");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::ChangedAfter]);
    }

    #[test]
    fn multiple_reasons_are_all_collected_in_order() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20)), seg("t2", 30, SegEnd::Unknown)]);
        f.segs[1].concurrent = vec![ConcurrentRec::External];
        f.set(StateAt::Base(0), "a", ent("a0")).set(StateAt::End(0), "a", ent("a1")).set(StateAt::Base(1), "a", ent("EXT")).cur("a", "z");
        assert_eq!(codes(&f.one("a")), vec![RevertBlockCode::EndUnknown, RevertBlockCode::ChangedBetween, RevertBlockCode::ConcurrentChange]);
        let Judgement::NeedsOverride(rs) = f.one("a") else { unreachable!() };
        assert!(rs.iter().all(|r| !r.message.is_empty()));
    }

    // ── 止める ──
    #[test]
    fn blocked_wins_over_needs_override() {
        let mut f = simple();
        f.cur("a.txt", "zzz"); // ChangedAfter
        f.segs[0].concurrent = vec![ConcurrentRec::External];
        f.head = Some("h2".into());
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::HeadMoved);
        let p = f.run(&["a.txt"]).remove(0);
        assert!(p.write.is_none(), "blocked has no write");
    }

    #[test]
    fn head_or_branch_moves_block_inside_between_and_after() {
        // 区間内
        let mut f = simple();
        f.segs[0].end = SegEnd::Snapshot { at: UnixMillis(20), head: Some("h2".into()), branch: Some("main".into()) };
        f.head = Some("h2".into());
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::HeadMoved);
        // 区間の間
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20)), seg("t2", 30, snap_end(40))]);
        f.segs[1].base_head = Some("h2".into());
        f.set(StateAt::Base(0), "a", ent("a")).set(StateAt::End(1), "a", ent("b")).cur("a", "b");
        assert_eq!(blocked(&f.one("a")), RevertBlockCode::HeadMoved);
        // 区間後（HEAD）
        let mut f = simple();
        f.head = Some("h2".into());
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::HeadMoved);
        // ブランチ名だけ
        let mut f = simple();
        f.branch = Some("dev".into());
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::HeadMoved);
        // detached（None）も変化
        let mut f = simple();
        f.branch = None;
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::HeadMoved);
        // 終了不明でも基準とのHEADを比べる
        let mut f = Fx::new(vec![seg("t1", 10, SegEnd::Unknown)]);
        f.head = Some("h2".into());
        f.set(StateAt::Base(0), "a", ent("a")).cur("a", "b");
        assert_eq!(blocked(&f.one("a")), RevertBlockCode::HeadMoved);
    }

    #[test]
    fn git_busy_not_a_repo_git_missing_and_no_baseline_block_everything() {
        let mut f = simple();
        f.busy = true;
        assert!(f.run(&["a.txt", "b.txt"]).iter().all(|p| matches!(&p.judgement, Judgement::Blocked(r) if r.code == RevertBlockCode::GitBusy)));
        let mut f = simple();
        f.repo = RepoStatus::NotARepository;
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::NotARepository);
        f.repo = RepoStatus::GitUnavailable;
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::GitUnavailable);
        let mut f = simple();
        f.segs.clear();
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::NoBaseline);
        let mut f = simple();
        f.from_turn = ext("unknown-turn");
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::NoBaseline);
    }

    #[test]
    fn skipped_entries_block_wherever_they_appear() {
        for at in [StateAt::Base(0), StateAt::End(0), StateAt::Current] {
            for reason in [SkipReason::Link, SkipReason::Submodule, SkipReason::TooLarge, SkipReason::Budget, SkipReason::Unreadable] {
                let mut f = simple();
                f.set(at, "a.txt", EntryState::Skipped(reason));
                assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::NotSnapshotted, "{at:?} {reason:?}");
            }
        }
        let mut f = simple();
        f.files.insert("a.txt".into(), CurrentFile::Directory);
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::NotSnapshotted, "directory in place of a file");
    }

    #[test]
    fn already_reverted_is_blocked_and_later_change_reopens_it() {
        let mut f = simple();
        f.cur("a.txt", "a2"); // 戻した結果（B と違う形で書かれた想定）
        f.reverted.push(RevertMark { at: UnixMillis(50), path: "A.txt".into(), restored: Restored::Entry { entry: EntryRef { oid: "a2".into(), form: EntryForm::Raw } } });
        assert_eq!(blocked(&f.one("a.txt")), RevertBlockCode::AlreadyReverted);
        f.cur("a.txt", "edited");
        assert_eq!(codes(&f.one("a.txt")), vec![RevertBlockCode::ChangedAfter]);
        // 古い戻し（最新の控えより前）は無視
        let mut f = simple();
        f.cur("a.txt", "a2");
        f.reverted.push(RevertMark { at: UnixMillis(5), path: "a.txt".into(), restored: Restored::Entry { entry: EntryRef { oid: "a2".into(), form: EntryForm::Raw } } });
        assert_eq!(codes(&f.one("a.txt")), vec![RevertBlockCode::ChangedAfter]);
        // 削除で戻した（Absent どうし）
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20))]);
        f.set(StateAt::End(0), "n", ent("n")).cur("n", "");
        f.reverted.push(RevertMark { at: UnixMillis(50), path: "n".into(), restored: Restored::Absent });
        assert_eq!(blocked(&f.one("n")), RevertBlockCode::AlreadyReverted);
    }

    #[test]
    fn path_confinement_is_never_overridable() {
        let mut f = simple();
        f.prefix = "sub/dir".into();
        for p in ["a.txt", "sub/other/a.txt", "sub/dirx/a.txt"] {
            assert_eq!(blocked(&f.one(p)), RevertBlockCode::OutsideWorkFolder, "{p}");
        }
        for p in ["../x", "sub/dir/../../x", "/abs", "C:/x", "C:\\x", "sub\\dir\\x", "", "sub/dir//x"] {
            assert_eq!(blocked(&f.one(p)), RevertBlockCode::PathOutside, "{p:?}");
        }
        // 内側は通る（SUB/DIR の大文字小文字は区別しない）
        f.cur("SUB/dir/a.txt", "q");
        f.set(StateAt::Base(0), "SUB/dir/a.txt", ent("o")).set(StateAt::End(0), "SUB/dir/a.txt", ent("q"));
        assert_eq!(f.one("SUB/dir/a.txt"), Judgement::Revertable);
        // 解決後の位置が外ならPathOutside
        let states = |_: StateAt, _: &str| EntryState::Absent;
        let files = |_: &str| CurrentFile::Missing;
        let confine = |_: &str| Err("junction".to_string());
        let segs = [seg("t1", 10, snap_end(20))];
        let paths = ["a.txt".to_string()];
        let r = plan(&PlanInput {
            repo: RepoStatus::Ready, git_busy: false, work_prefix: String::new(), segments: &segs, from_turn: None,
            current_head: Some("h1".into()), current_branch: Some("main".into()), reverted: &[], paths: &paths,
            states: &states, files: &files, confine: &confine,
        });
        assert_eq!(blocked(&r[0].judgement), RevertBlockCode::PathOutside);
    }

    #[test]
    fn from_turn_picks_the_range_and_lists_included_turns() {
        let mut f = Fx::new(vec![seg("t1", 10, snap_end(20)), seg("t2", 30, snap_end(40)), seg("t3", 50, snap_end(60))]);
        f.set(StateAt::Base(0), "a", ent("1")).set(StateAt::End(0), "a", ent("2")).set(StateAt::Base(1), "a", ent("2")).set(StateAt::End(1), "a", ent("3"))
            .set(StateAt::Base(2), "a", ent("3")).set(StateAt::End(2), "a", ent("3")).cur("a", "3");
        f.from_turn = ext("t2");
        let p = f.run(&["a"]).remove(0);
        assert_eq!(p.judgement, Judgement::Revertable);
        assert_eq!(p.write.unwrap().target, Some(EntryRef { oid: "2".into(), form: EntryForm::Raw }), "restores B of the chosen segment");
        assert!(p.includes_turns.is_empty(), "t3 did not change a");
        f.from_turn = None;
        f.set(StateAt::End(2), "a", ent("4")).cur("a", "4");
        let p = f.run(&["a"]).remove(0);
        assert_eq!(p.includes_turns, vec![ExternalId("t2".into()), ExternalId("t3".into())]);
    }

    // ── 強制の受付判定 ──
    fn needs(reasons: &[RevertBlockCode]) -> Judgement {
        Judgement::NeedsOverride(reasons.iter().map(|c| reason(*c, "m")).collect())
    }
    fn pp(path: &str, j: Judgement) -> PathPlan {
        let write = if matches!(j, Judgement::Blocked(_)) {
            None
        } else {
            Some(WriteOp { path: path.into(), kind: WriteKind::Overwrite, target: Some(EntryRef { oid: "o".into(), form: EntryForm::Raw }), expect: Some("s".into()) })
        };
        PathPlan { path: path.into(), judgement: j, includes_turns: vec![], write }
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn admit_accepts_forced_with_same_or_fewer_reasons() {
        let old = [pp("a", needs(&[RevertBlockCode::ChangedAfter, RevertBlockCode::EndUnknown])), pp("b", Judgement::Revertable)];
        let fresh = [pp("a", needs(&[RevertBlockCode::ChangedAfter])), pp("b", Judgement::Revertable)];
        let r = admit(&old, &fresh, &s(&["a", "b"]), &s(&["a"]));
        assert_eq!(r.stale, None);
        assert!(r.failed.is_empty());
        assert_eq!(r.execute.len(), 2);
        assert!(r.execute[0].forced && r.execute[0].overridden == vec![RevertBlockCode::ChangedAfter]);
        assert!(!r.execute[1].forced);
    }

    #[test]
    fn admit_rejects_added_reasons_as_stale() {
        let old = [pp("a", needs(&[RevertBlockCode::ChangedAfter]))];
        let fresh = [pp("a", needs(&[RevertBlockCode::ChangedAfter, RevertBlockCode::ConcurrentChange]))];
        let r = admit(&old, &fresh, &s(&["a"]), &s(&["a"]));
        assert_eq!(r.stale.as_deref(), Some("a"));
        // 計画時は戻せるはずだったのに要確認になった
        let old = [pp("a", Judgement::Revertable)];
        let fresh = [pp("a", needs(&[RevertBlockCode::ChangedAfter]))];
        assert_eq!(admit(&old, &fresh, &s(&["a"]), &s(&["a"])).stale.as_deref(), Some("a"));
        assert_eq!(admit(&old, &fresh, &s(&["a"]), &[]).stale.as_deref(), Some("a"));
    }

    #[test]
    fn admit_never_writes_blocked_even_when_forced() {
        let old = [pp("a", needs(&[RevertBlockCode::ChangedAfter]))];
        let fresh = [pp("a", Judgement::Blocked(reason(RevertBlockCode::HeadMoved, "head moved")))];
        let r = admit(&old, &fresh, &s(&["a"]), &s(&["a"]));
        assert!(r.execute.is_empty() && r.stale.is_none());
        assert_eq!(r.failed, vec![RevertFailure { path: "a".into(), reason: "head moved".into() }]);
    }

    #[test]
    fn admit_runs_a_forced_path_that_became_revertable_and_refuses_unforced_override() {
        let old = [pp("a", needs(&[RevertBlockCode::ChangedAfter])), pp("c", needs(&[RevertBlockCode::EndUnknown]))];
        let fresh = [pp("a", Judgement::Revertable), pp("c", needs(&[RevertBlockCode::EndUnknown]))];
        let r = admit(&old, &fresh, &s(&["a", "c"]), &s(&["a"]));
        assert_eq!(r.execute.len(), 1);
        assert!(!r.execute[0].forced, "no longer needs override");
        assert_eq!(r.failed.len(), 1, "c is needs-override but not forced");
        assert_eq!(r.failed[0].path, "c");
        // 要確認だったが強制の指定なしのファイルは、再計画で戻せるようになっても実行しない
        let r = admit(&old, &fresh, &s(&["a"]), &[]);
        assert!(r.execute.is_empty() && r.failed.len() == 1);
    }

    #[test]
    fn admit_flags_paths_missing_from_either_plan() {
        let old = [pp("a", Judgement::Revertable)];
        assert_eq!(admit(&old, &[], &s(&["a"]), &[]).stale.as_deref(), Some("a"));
        assert_eq!(admit(&[], &old, &s(&["a"]), &[]).stale.as_deref(), Some("a"));
    }

    // ── 再照合・集計 ──
    #[test]
    fn recheck_compares_against_the_planned_hash() {
        let sha = |h: &str| CurrentFile::File { sha: h.into() };
        assert!(recheck(&Some("x".into()), &sha("x")).is_ok());
        assert!(recheck(&Some("x".into()), &sha("y")).is_err());
        assert!(recheck(&None, &CurrentFile::Missing).is_ok());
        assert!(recheck(&None, &sha("x")).is_err(), "appeared after planning");
        assert!(recheck(&Some("x".into()), &CurrentFile::Missing).is_err(), "vanished after planning");
        assert!(recheck(&Some("x".into()), &CurrentFile::Directory).is_err());
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn aggregate_records_only_successes_and_reports_partial() {
        let op = |p: &str, target: bool| WriteOp {
            path: p.into(),
            kind: if target { WriteKind::Overwrite } else { WriteKind::RemoveAdded },
            target: target.then(|| EntryRef { oid: "o".into(), form: EntryForm::Head }),
            expect: None,
        };
        let outcomes = vec![
            WriteOutcome { item: ExecItem { op: op("a", true), forced: true, overridden: vec![RevertBlockCode::ChangedAfter] }, backup_file: Some("0001".into()), result: Ok(()) },
            WriteOutcome { item: ExecItem { op: op("b", false), forced: false, overridden: vec![RevertBlockCode::ChangedAfter] }, backup_file: None, result: Ok(()) },
            WriteOutcome { item: ExecItem { op: op("c", true), forced: true, overridden: vec![] }, backup_file: None, result: Err("locked".into()) },
        ];
        let agg = aggregate(outcomes, vec![fail("d", "blocked")]);
        assert_eq!(agg.reverted, vec!["a", "b"]);
        assert_eq!(agg.forced, vec!["a"]);
        assert_eq!(agg.failed.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["d", "c"]);
        assert!(!agg.is_complete());
        assert_eq!(agg.items.len(), 2, "failed paths are not recorded");
        assert!(agg.items[0].forced && agg.items[0].overridden == vec![RevertBlockCode::ChangedAfter]);
        assert_eq!(agg.items[1].restored, Restored::Absent);
        assert!(!agg.items[1].forced && agg.items[1].overridden.is_empty(), "overridden is kept only for forced items");
        assert!(aggregate(vec![], vec![]).is_complete());
    }
}
