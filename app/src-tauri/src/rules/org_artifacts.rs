//! 組織実行の成果物パネルと「取り込む」の判定（DESIGN_ORG_R3 §1・§2）。純粋ロジック（I/Oなし）。
//!
//! - 成果物の状態（`artifact_status`・`supersede`）、プレビューの種類と文字コード（`preview_kind`・`decode_text`）、
//!   申告と差分の照合（`report_vs_diff`）、行数の合計（`totals`）。
//! - Git出力の解析（`parse_numstat_z`・`parse_merge_tree`・`merge_tree_supported`）。
//! - 取り込みの前提（`import_blockers`・`dirty_overlap`）、結果の分類（`classify_import`・`reconcile_import`）、
//!   戻す判定（`undo_decision`）、台帳（imports.jsonl）の行と畳み込み。
//!
//! 守ること: 取れない値は欠けのまま（0や空で代用しない）。`Applied`／`Undone` は読み戻しで確かめたときだけ。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::backend::changes::ChangeKind;
use crate::backend::model::{Known, UnixMillis};
use crate::backend::org::OrgTaskPhase;
use crate::backend::org_artifacts::*;
use crate::rules::org_worktree::ChangedFile;
use org_engine::graph::SlotId;
use org_engine::{OrgEvent, OrgEventKind, TaskId};

/// 台帳（`org\<wf>\imports.jsonl`）の行の版。
pub const LEDGER_SCHEMA_VERSION: u32 = 1;
/// プレビューするテキストの上限（超えた分は先頭だけ）。
pub const TEXT_PREVIEW_MAX: usize = 1024 * 1024;
/// 画像プレビューの上限。
pub const IMAGE_PREVIEW_MAX: usize = 10 * 1024 * 1024;
/// 表のプレビューの行数。
pub const TABLE_PREVIEW_ROWS: usize = 500;

// ───────────────────────────── 成果物の状態 ─────────────────────────────

/// 状態を決める事実。
#[derive(Debug, Clone, Copy)]
pub struct StatusFacts {
    pub phase: OrgTaskPhase,
    pub has_commit: bool,
    /// コミットが今のHEADの祖先か。判定できなければ None（`ReadyForReview` で代用しない）。
    pub in_head: Option<bool>,
    pub superseded: bool,
    pub ledger_applied: bool,
    /// コミットをGitから読めた。
    pub git_ok: bool,
}

/// 成果物の状態。成果物にならないタスク（コミットのない失敗・取消し）は None。
pub fn artifact_status(f: &StatusFacts) -> Option<OrgArtifactStatus> {
    use OrgArtifactStatus as S;
    use OrgTaskPhase as P;
    Some(match f.phase {
        P::Reviewing => S::AiReviewing,
        P::Pending | P::Ready | P::Running | P::Blocked | P::Rework => S::Working,
        P::Completed | P::Failed | P::Cancelled => {
            if !f.has_commit {
                return (f.phase == P::Completed).then_some(S::NoChanges);
            }
            if !f.git_ok {
                return Some(S::Unknown);
            }
            match f.in_head {
                None => S::Unknown,
                Some(true) if f.ledger_applied => S::Imported,
                Some(true) => S::IncludedElsewhere,
                Some(false) if f.superseded => S::Superseded,
                Some(false) if f.phase == P::Completed => S::ReadyForReview,
                Some(false) => S::NotPassed,
            }
        }
    })
}

/// 台帳に「取り込んだ」とあるのに、今のブランチには含まれない（戻した等）。
pub fn applied_but_missing(f: &StatusFacts) -> bool {
    f.ledger_applied && f.in_head == Some(false)
}

pub fn status_label(s: OrgArtifactStatus) -> &'static str {
    match s {
        OrgArtifactStatus::Working => "作業中",
        OrgArtifactStatus::AiReviewing => "レビュー中（AI）",
        OrgArtifactStatus::ReadyForReview => "レビュー待ち",
        OrgArtifactStatus::NotPassed => "合格していない成果",
        OrgArtifactStatus::Superseded => "後の成果に含まれています",
        OrgArtifactStatus::Imported => "取り込み済み",
        OrgArtifactStatus::IncludedElsewhere => "すでに今のブランチに含まれています",
        OrgArtifactStatus::NoChanges => "変更なし",
        OrgArtifactStatus::Unknown => "未確認",
    }
}

pub const APPLIED_BUT_MISSING_NOTE: &str = "取り込みの記録はありますが、今のブランチには含まれていません";

/// 取り込みの対象にできる状態。
pub fn is_candidate(s: OrgArtifactStatus) -> bool {
    matches!(s, OrgArtifactStatus::ReadyForReview | OrgArtifactStatus::NotPassed)
}

/// 別の候補のコミットの祖先になっているコミットのタスク（グラフで後の書込みが前を含む）。
/// `is_ancestor(a, b)` は「a が b の祖先か」。判定できなければ None（上位に含まれるとは見なさない）。同じコミットは上位としない。
pub fn supersede(commits: &[(String, String)], is_ancestor: impl Fn(&str, &str) -> Option<bool>) -> HashSet<String> {
    let mut out = HashSet::new();
    for (task, commit) in commits {
        let included = commits.iter().any(|(other, oc)| other != task && oc != commit && is_ancestor(commit, oc) == Some(true));
        if included {
            out.insert(task.clone());
        }
    }
    out
}

// ───────────────────────────── プレビュー ─────────────────────────────

fn extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// 拡張子から見たプレビューの種類。
pub fn preview_kind(path: &str) -> OrgPreviewKind {
    match extension(path).as_str() {
        "md" | "markdown" => OrgPreviewKind::Markdown,
        "txt" | "log" | "json" | "yaml" | "yml" | "xml" | "ini" | "toml" => OrgPreviewKind::Text,
        "csv" | "tsv" => OrgPreviewKind::Table,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => OrgPreviewKind::Image,
        _ => OrgPreviewKind::None,
    }
}

/// 画像のMIME（`data:` URL用）。svg も画像として（`<img>` 経由のみ）。
pub fn image_mime(path: &str) -> Option<&'static str> {
    Some(match extension(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// 文書（プレビューと差分のタブを出す）。
pub fn is_document(path: &str) -> bool {
    matches!(extension(path).as_str(), "md" | "markdown" | "txt" | "csv" | "tsv")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeRefusal {
    /// NULを含む（バイナリ）。
    Binary,
    /// UTF-8・UTF-16（BOMあり）として読めない。
    UnknownEncoding,
}

impl DecodeRefusal {
    pub fn message(self) -> &'static str {
        match self {
            DecodeRefusal::Binary => "テキストとして表示できません（バイナリです）。「開く…」で確認してください",
            DecodeRefusal::UnknownEncoding => "文字コードを判定できません（UTF-8・UTF-16のみ表示します）。「開く…」で確認してください",
        }
    }
}

/// テキストの復号。UTF-8（BOMあり・なし）とUTF-16（BOMあり）だけ。`cut` は読んだ範囲が全体の途中で切れている印（末尾の不完全な文字は捨てる）。
pub fn decode_text(bytes: &[u8], cut: bool) -> Result<String, DecodeRefusal> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_utf8(rest, cut);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let mut body = &bytes[2..];
        if body.len() % 2 == 1 {
            if !cut {
                return Err(DecodeRefusal::UnknownEncoding);
            }
            body = &body[..body.len() - 1];
        }
        let mut units: Vec<u16> = body.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        if cut && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
            units.pop();
        }
        return String::from_utf16(&units).map_err(|_| DecodeRefusal::UnknownEncoding);
    }
    if bytes.contains(&0) {
        return Err(DecodeRefusal::Binary);
    }
    decode_utf8(bytes, cut)
}

fn decode_utf8(bytes: &[u8], cut: bool) -> Result<String, DecodeRefusal> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        // 途中で切った末尾の不完全な文字だけは捨てる。
        Err(e) if cut && e.error_len().is_none() => Ok(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()),
        Err(_) => Err(DecodeRefusal::UnknownEncoding),
    }
}

/// `data:` URL 用の base64（標準のアルファベット・パディングあり）。
pub fn base64_encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

/// 表のプレビュー用に、先頭 `rows` 行までを残す。切り詰めたら true。
pub fn limit_rows(text: &str, rows: usize) -> (String, bool) {
    let mut count = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            count += 1;
            if count >= rows {
                return (text[..i].to_string(), i + 1 < text.len());
            }
        }
    }
    (text.to_string(), false)
}

// ───────────────────────────── 申告との照合・合計 ─────────────────────────────

fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_start_matches("./").trim_end_matches('/');
    s.to_lowercase()
}

/// Worker の申告（`changedFiles`）と差分のパスの違い。(申告にあって差分にない, 差分にあって申告にない)。判定には使わない。
pub fn report_vs_diff(reported: &[String], diff_paths: &[String]) -> (Vec<String>, Vec<String>) {
    let diff: HashSet<String> = diff_paths.iter().map(|p| norm_path(p)).collect();
    let rep: HashSet<String> = reported.iter().map(|p| norm_path(p)).collect();
    let only_reported = reported.iter().filter(|p| !diff.contains(&norm_path(p))).cloned().collect();
    let only_diff = diff_paths.iter().filter(|p| !rep.contains(&norm_path(p))).cloned().collect();
    (only_reported, only_diff)
}

/// 申告と差分の違いを表示用の文にする（なければ空）。
pub fn mismatch_lines(only_reported: &[String], only_diff: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in only_reported {
        out.push(format!("申告にあるが差分にない: {p}"));
    }
    for p in only_diff {
        out.push(format!("差分にあるが申告にない: {p}"));
    }
    out
}

/// ファイル数と行数の合計。数えられないファイルは `unknown` に数え、行数には足さない（0として足さない）。
pub fn totals(files: &[OrgArtifactFile]) -> OrgArtifactTotals {
    let mut t = OrgArtifactTotals { files: files.len() as u32, ..Default::default() };
    for f in files {
        if f.binary {
            t.binary += 1;
            continue;
        }
        match (&f.additions, &f.deletions) {
            (Known::Value { value: a, .. }, Known::Value { value: d, .. }) => {
                t.additions += a;
                t.deletions += d;
            }
            _ => t.unknown += 1,
        }
    }
    t
}

// ───────────────────────────── Git出力の解析 ─────────────────────────────

/// `git diff --numstat -z -M` の1件。数えられなければ（バイナリ）行数は None。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatEntry {
    /// 変更後のパス。
    pub path: String,
    pub old_path: Option<String>,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
}

pub fn parse_numstat_z(out: &str) -> Vec<NumstatEntry> {
    let mut entries = Vec::new();
    let mut it = out.split('\0');
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let binary = a == "-" && d == "-";
        let (additions, deletions) = (a.parse::<u32>().ok(), d.parse::<u32>().ok());
        if p.is_empty() {
            // 名前変更・コピー: 続く2つが 元・先。
            let (Some(old), Some(new)) = (it.next().filter(|x| !x.is_empty()), it.next().filter(|x| !x.is_empty())) else { break };
            entries.push(NumstatEntry { path: new.to_string(), old_path: Some(old.to_string()), additions, deletions, binary });
        } else {
            entries.push(NumstatEntry { path: p.to_string(), old_path: None, additions, deletions, binary });
        }
    }
    entries
}

/// 変更の一覧（`--name-status`）と行数（`--numstat`）を合わせて成果物のファイル一覧にする。
/// 行数が取れないバイナリは `Unsupported`、突き合わせられなかったものは `NotFetched`。
pub fn merge_files(names: &[ChangedFile], nums: &[NumstatEntry]) -> Vec<OrgArtifactFile> {
    names
        .iter()
        .map(|n| {
            let status = n.status.chars().next().unwrap_or('?');
            let num = nums.iter().find(|x| x.path == n.path);
            let (kind, path, move_to) = match (status, &n.old_path) {
                ('R', Some(old)) => (ChangeKind::Modified, old.clone(), Some(n.path.clone())),
                ('A' | 'C', _) => (ChangeKind::Added, n.path.clone(), None),
                ('D', _) => (ChangeKind::Deleted, n.path.clone(), None),
                _ => (ChangeKind::Modified, n.path.clone(), None),
            };
            let binary = num.is_some_and(|x| x.binary);
            let (additions, deletions) = match num {
                Some(x) if x.binary => (Known::Unsupported, Known::Unsupported),
                Some(NumstatEntry { additions: Some(a), deletions: Some(d), .. }) => (Known::direct(*a), Known::direct(*d)),
                _ => (Known::NotFetched, Known::NotFetched),
            };
            OrgArtifactFile { preview: preview_kind(&n.path), path, kind, move_to, additions, deletions, binary }
        })
        .collect()
}

/// ファイル一覧の「変更後のパス」（移動は新しい場所）。
pub fn new_path(f: &OrgArtifactFile) -> &str {
    f.move_to.as_deref().unwrap_or(&f.path)
}

/// 取り込みで触れるパス（移動は元・先の両方）。
pub fn touched_paths(files: &[OrgArtifactFile]) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        out.push(f.path.clone());
        if let Some(m) = &f.move_to {
            out.push(m.clone());
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeTreeResult {
    /// 衝突なし。合成した結果のツリー。
    Clean { tree: String },
    /// 衝突あり（衝突したファイル）。何も書いていない。
    Conflict { files: Vec<String> },
    /// 読めない出力・想定外の終了コード。
    Failed,
}

fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `git merge-tree --write-tree -z --name-only --no-messages` の結果。終了コード 0＝成功、1＝衝突。
pub fn parse_merge_tree(exit_code: Option<i32>, out: &str) -> MergeTreeResult {
    let mut parts = out.split('\0');
    let Some(tree) = parts.next().filter(|t| is_oid(t)) else { return MergeTreeResult::Failed };
    match exit_code {
        Some(0) => MergeTreeResult::Clean { tree: tree.to_string() },
        Some(1) => {
            let mut files: Vec<String> = Vec::new();
            for f in parts.filter(|f| !f.is_empty()) {
                if !files.iter().any(|x| x == f) {
                    files.push(f.to_string());
                }
            }
            MergeTreeResult::Conflict { files }
        }
        _ => MergeTreeResult::Failed,
    }
}

/// `git --version` の出力が `merge-tree --write-tree` の使える 2.38 以降か。読めなければ None。
pub fn merge_tree_supported(version_text: &str) -> Option<bool> {
    let v = version_text.trim().strip_prefix("git version ")?;
    let mut nums = v.split('.').map(|p| p.parse::<u32>());
    let major = nums.next()?.ok()?;
    let minor = nums.next()?.ok()?;
    Some((major, minor) >= (2, 38))
}

/// gitdir の中の印から、途中の操作を見つける。`exists` は gitdir 直下の名前の有無。
pub fn operation_in_progress(exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    if exists("MERGE_HEAD") {
        Some("マージ")
    } else if exists("rebase-merge") || exists("rebase-apply") {
        Some("リベース")
    } else if exists("CHERRY_PICK_HEAD") {
        Some("チェリーピック")
    } else if exists("REVERT_HEAD") {
        Some("リバート")
    } else {
        None
    }
}

// ───────────────────────────── 取り込みの前提 ─────────────────────────────

/// 取り込みの前提の事実。
pub struct ImportFacts<'a> {
    /// 今のブランチ。読めなければ None、detached なら "HEAD"。
    pub branch: Option<&'a str>,
    pub in_progress: Option<&'a str>,
    pub workflow_finished: bool,
    pub delete_pending: bool,
    /// 選んだ候補の (タスク, 状態)。
    pub selected: &'a [(String, OrgArtifactStatus)],
    /// Git 2.38 以降か（読めなければ None）。
    pub git_supported: Option<bool>,
}

pub fn import_blockers(f: &ImportFacts) -> Vec<String> {
    let mut out = Vec::new();
    match f.git_supported {
        Some(true) => {}
        Some(false) => out.push("Git 2.38 以降が必要です。Git を更新するか、ブランチを手動で取り込んでください".to_string()),
        None => out.push("Git のバージョンを確認できないため、取り込めません".to_string()),
    }
    match f.branch {
        None => out.push("今のブランチを確認できないため、取り込めません".to_string()),
        Some("HEAD") => out.push("作業フォルダが特定のブランチを指していない状態（detached）のため、取り込めません。ブランチに切り替えてください".to_string()),
        Some(b) if b.starts_with(crate::rules::git_push::INTERNAL_BRANCH_PREFIX) => {
            out.push(format!("作業フォルダの今のブランチ「{b}」はAgentDockの内部用です。通常のブランチに切り替えてください"))
        }
        Some(_) => {}
    }
    if let Some(op) = f.in_progress {
        out.push(format!("作業フォルダで{op}の途中です。終えてから取り込んでください"));
    }
    if !f.workflow_finished {
        out.push("ワークフローが完了してから取り込めます".to_string());
    }
    if f.delete_pending {
        out.push("このワークフローは削除の途中のため、取り込めません".to_string());
    }
    if f.selected.is_empty() {
        out.push("取り込む成果が選ばれていません".to_string());
    }
    for (task, s) in f.selected {
        if !is_candidate(*s) {
            out.push(format!("{task}: 「{}」のため、取り込めません", status_label(*s)));
        }
    }
    out
}

/// 未コミットの変更（追跡中の変更・未追跡）と、取り込みで触れるパスの重なり（触れるパス側を返す。大文字小文字・区切りの違いは同じとみなす）。
/// 片方がもう片方のフォルダ配下になる場合（未追跡の `a` と取り込みの `a/b.txt`）も重なりとする。
pub fn dirty_overlap(touched: &[String], dirty: &[String]) -> Vec<String> {
    let d: Vec<String> = dirty.iter().map(|p| norm_path(p)).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in touched {
        let n = norm_path(t);
        let hit = d.iter().any(|x| *x == n || n.starts_with(&format!("{x}/")) || x.starts_with(&format!("{n}/")));
        if hit && seen.insert(n) {
            out.push(t.clone());
        }
    }
    out
}

// ───────────────────────────── 結果の分類 ─────────────────────────────

/// 実行後の読み戻しで結果を決める。`changed_targets` は、実行後に対象ファイルのうち前と違うもの（読めなければ None）。
pub fn classify_import(before: &str, result: &str, head_after: Option<&str>, changed_targets: Option<&[String]>, failure: Option<&str>) -> OrgImportOutcome {
    match head_after {
        Some(h) if h == result => OrgImportOutcome::Applied { head: h.to_string() },
        Some(h) if h == before => match changed_targets {
            Some([]) => OrgImportOutcome::NotApplied { reason: failure.filter(|s| !s.is_empty()).unwrap_or("取り込めませんでした").to_string() },
            Some(files) => OrgImportOutcome::Partial { files: files.to_vec() },
            None => OrgImportOutcome::Unknown { head_now: Known::direct(h.to_string()) },
        },
        Some(h) => OrgImportOutcome::Unknown { head_now: Known::direct(h.to_string()) },
        None => OrgImportOutcome::Unknown { head_now: Known::NotFetched },
    }
}

/// `begin` だけで `end` のない記録（途中で終わった・結果を受け取れなかった）を、今のHEADで見分ける。自動で再実行しない。
pub fn reconcile_import(before: &str, result: &str, head_now: Option<&str>) -> OrgImportOutcome {
    match head_now {
        Some(h) if h == result => OrgImportOutcome::Applied { head: h.to_string() },
        Some(h) if h == before => OrgImportOutcome::NotApplied { reason: "取り込まれていません（記録の時点のままです）".to_string() },
        Some(h) => OrgImportOutcome::Unknown { head_now: Known::direct(h.to_string()) },
        None => OrgImportOutcome::Unknown { head_now: Known::NotFetched },
    }
}

/// `undoBegin` だけの記録を今のHEADで見分ける。
pub fn reconcile_undo(before: &str, result: &str, head_now: Option<&str>) -> Option<OrgImportState> {
    match head_now {
        Some(h) if h == before => Some(OrgImportState::Undone),
        Some(h) if h == result => Some(OrgImportState::Applied),
        _ => None,
    }
}

// ───────────────────────────── 戻す ─────────────────────────────

pub struct UndoFacts<'a> {
    pub state: OrgImportState,
    pub recorded_branch: &'a str,
    pub current_branch: Option<&'a str>,
    pub head_now: Option<&'a str>,
    pub before: &'a str,
    pub result: &'a str,
    /// `R` を含むリモート追跡ブランチがあるか（読めなければ None）。
    pub remote_contains: Option<bool>,
    /// 未コミットの変更と、取り込んだ対象ファイルの重なり。
    pub dirty_overlap: &'a [String],
    pub partial_files: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoDecision {
    pub allowed: bool,
    pub refusal: Option<String>,
    pub mode: OrgUndoMode,
    pub files: Vec<String>,
}

pub fn undo_decision(f: &UndoFacts) -> UndoDecision {
    let refuse = |mode: OrgUndoMode, why: String| UndoDecision { allowed: false, refusal: Some(why), mode, files: Vec::new() };
    match f.state {
        OrgImportState::Undone => return refuse(OrgUndoMode::Reset, "すでに取り込み前に戻しています".to_string()),
        OrgImportState::NotApplied => return refuse(OrgUndoMode::Reset, "取り込まれていないため、戻すものがありません".to_string()),
        OrgImportState::Unknown => return refuse(OrgUndoMode::Reset, "取り込めたか確認できていません。先に「状態を確認」してください".to_string()),
        OrgImportState::Applied | OrgImportState::Partial => {}
    }
    let mode = if f.state == OrgImportState::Partial { OrgUndoMode::RestoreFiles } else { OrgUndoMode::Reset };
    if f.current_branch != Some(f.recorded_branch) {
        return refuse(mode, format!("作業フォルダの今のブランチが、取り込んだときのブランチ「{}」と違うため、戻せません", f.recorded_branch));
    }
    if f.state == OrgImportState::Partial {
        if f.head_now != Some(f.before) {
            return refuse(mode, "取り込み前のコミットから進んでいるため、戻せません".to_string());
        }
        return UndoDecision { allowed: true, refusal: None, mode, files: f.partial_files.to_vec() };
    }
    if f.head_now != Some(f.result) {
        return refuse(mode, "取り込みの後にコミットが進んだ（または変わった）ため、戻せません".to_string());
    }
    match f.remote_contains {
        Some(false) => {}
        Some(true) => return refuse(mode, "送信済みのため戻せません。必要なら新しい変更で打ち消してください".to_string()),
        None => return refuse(mode, "送信済みかどうかを確認できないため、戻せません".to_string()),
    }
    if !f.dirty_overlap.is_empty() {
        return refuse(mode, format!("取り込んだファイルに未コミットの変更があるため、戻せません（{}）", f.dirty_overlap.join("、")));
    }
    UndoDecision { allowed: true, refusal: None, mode, files: Vec::new() }
}

// ───────────────────────────── 取り込みコミットのメッセージ ─────────────────────────────

/// 取り込みコミットのメッセージ。`tasks` は (タスクID, 題名, コミット)。
pub fn import_commit_message(workflow_name: &str, tasks: &[(String, String, String)]) -> String {
    let name: String = workflow_name.lines().next().unwrap_or("").chars().take(60).collect();
    let mut m = format!("AgentDock: 組織実行の成果を取り込み（{}）\n", name.trim());
    m.push('\n');
    for (task, title, commit) in tasks {
        let t: String = title.lines().next().unwrap_or("").chars().take(80).collect();
        let sha: String = commit.chars().take(7).collect();
        m.push_str(&format!("- {task} {t} {sha}\n"));
    }
    m
}

// ───────────────────────────── 台帳（imports.jsonl） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerKind {
    Begin,
    End,
    UndoBegin,
    UndoEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTask {
    pub task: String,
    pub commit: String,
}

/// 台帳の1行。`begin` に実行前の事実を、`end` に結果を書く（`begin` が書けなければ実行しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerRow {
    pub schema_version: u32,
    pub kind: LedgerKind,
    pub id: String,
    pub at: UnixMillis,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub tasks: Vec<LedgerTask>,
    #[serde(default)]
    pub outcome: Option<OrgImportOutcome>,
    #[serde(default)]
    pub undo_outcome: Option<OrgUndoOutcome>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRecord {
    pub id: String,
    pub at: UnixMillis,
    pub folder: String,
    pub branch: String,
    pub before: String,
    pub result: String,
    pub tasks: Vec<LedgerTask>,
    /// `end` の結果（なければ結果を受け取れていない）。
    pub outcome: Option<OrgImportOutcome>,
    pub undo_begun: bool,
    pub undo_outcome: Option<OrgUndoOutcome>,
}

/// 台帳を読む。読めない行は飛ばす（台帳は追記のみ）。同じ `id` の行を1件に畳む（`begin` がない `end` は捨てる）。
pub fn fold_ledger(text: &str) -> Vec<LedgerRecord> {
    let mut recs: Vec<LedgerRecord> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(row) = serde_json::from_str::<LedgerRow>(line) else { continue };
        match row.kind {
            LedgerKind::Begin => {
                if recs.iter().any(|r| r.id == row.id) {
                    continue;
                }
                recs.push(LedgerRecord {
                    id: row.id,
                    at: row.at,
                    folder: row.folder.unwrap_or_default(),
                    branch: row.branch.unwrap_or_default(),
                    before: row.before.unwrap_or_default(),
                    result: row.result.unwrap_or_default(),
                    tasks: row.tasks,
                    outcome: None,
                    undo_begun: false,
                    undo_outcome: None,
                });
            }
            LedgerKind::End => {
                if let Some(r) = recs.iter_mut().find(|r| r.id == row.id) {
                    r.outcome = row.outcome;
                }
            }
            LedgerKind::UndoBegin => {
                if let Some(r) = recs.iter_mut().find(|r| r.id == row.id) {
                    r.undo_begun = true;
                    r.undo_outcome = None;
                }
            }
            LedgerKind::UndoEnd => {
                if let Some(r) = recs.iter_mut().find(|r| r.id == row.id) {
                    r.undo_outcome = row.undo_outcome;
                }
            }
        }
    }
    recs
}

/// 記録の今の状態。読み戻せていない分は今のHEADで見分ける（見分けられなければ `Unknown`）。
pub fn record_state(r: &LedgerRecord, head_now: Option<&str>) -> OrgImportState {
    let import_state = || {
        let outcome = match &r.outcome {
            Some(OrgImportOutcome::Unknown { .. }) | None => reconcile_import(&r.before, &r.result, head_now),
            Some(o) => o.clone(),
        };
        match outcome {
            OrgImportOutcome::Applied { .. } => OrgImportState::Applied,
            OrgImportOutcome::NotApplied { .. } => OrgImportState::NotApplied,
            OrgImportOutcome::Partial { .. } => OrgImportState::Partial,
            OrgImportOutcome::Unknown { .. } => OrgImportState::Unknown,
        }
    };
    if r.undo_begun {
        return match &r.undo_outcome {
            Some(OrgUndoOutcome::Undone { .. }) => OrgImportState::Undone,
            Some(OrgUndoOutcome::NotUndone { .. }) => import_state(),
            Some(OrgUndoOutcome::Unknown { .. }) | None => reconcile_undo(&r.before, &r.result, head_now).unwrap_or(OrgImportState::Unknown),
        };
    }
    import_state()
}

/// 一覧の印用: 取り込み済みのタスクの数と、そのタスクID。台帳の `Applied` だけ（戻したものは含めない）。
pub fn applied_tasks(records: &[LedgerRecord], head_now: Option<&str>) -> HashSet<String> {
    records
        .iter()
        .filter(|r| record_state(r, head_now) == OrgImportState::Applied)
        .flat_map(|r| r.tasks.iter().map(|t| t.task.clone()))
        .collect()
}

// ───────────────────────────── 所見の対応付け（イベントの記録から） ─────────────────────────────

/// グラフの検証工程が書いた判定1回分。`commit` は、その判定の対象になったコミット。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRound {
    pub round: u32,
    pub version: u32,
    pub commit: Option<String>,
}

/// 検証工程（`reviewer`）の出力スロットへの書込み（`SlotWritten`）を古い順に拾い、それぞれの対象コミットを決める。
/// 対象コミット＝その書込みより前に、書込み工程（`writer`）が最後にコミットしたもの。検証役は確定済みの最新のコミットへ戻してから読み
/// （`graphrun.rs` の `graph_launch` の `Refresh`）、その検証が終わるまで書込み工程は次のコミットをしない（差し戻しの辺が通ってから再びコミット）ので、
/// この対応で取り違えない。書込み工程がまだコミットしていなければ None（推定しない）。
pub fn review_rounds(events: &[OrgEvent], writer: &TaskId, reviewer: &TaskId, slot: &SlotId) -> Vec<ReviewRound> {
    let mut out = Vec::new();
    let mut last_commit: Option<String> = None;
    for e in events {
        match &e.kind {
            OrgEventKind::Committed { task, commit } if task == writer => last_commit = Some(commit.clone()),
            OrgEventKind::SlotWritten { task, slot: s, version, .. } if task == reviewer && s == slot => {
                out.push(ReviewRound { round: out.len() as u32 + 1, version: *version, commit: last_commit.clone() });
            }
            _ => {}
        }
    }
    out
}

/// 計画経路で差し戻した回ごとの所見（`FindingsRecorded`）。n番目＝n回目の差し戻し。
pub fn recorded_findings(events: &[OrgEvent], writer: &TaskId) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            OrgEventKind::FindingsRecorded { task, findings } if task == writer => Some(findings.clone()),
            _ => None,
        })
        .collect()
}

/// 出力スロットの最新の版（書込み工程の報告を読む版）。
pub fn latest_slot_version(events: &[OrgEvent], task: &TaskId, slot: &SlotId) -> Option<u32> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            OrgEventKind::SlotWritten { task: t, slot: s, version, .. } if t == task && s == slot => Some(*version),
            _ => None,
        })
        .max()
}

// ───────────────────────────── 保存された出力の読取り・一覧の印 ─────────────────────────────

fn string_list(v: &serde_json::Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(|x| x.as_str().map(str::to_string)).collect()
}

/// 検証役の出力（`{verdict, findings}`）。形が違えば None（推定しない）。
pub fn parse_review_value(v: &serde_json::Value) -> Option<(OrgReviewVerdict, Vec<String>)> {
    let verdict = match v.get("verdict")?.as_str()? {
        "pass" => OrgReviewVerdict::Pass,
        "rework" => OrgReviewVerdict::Rework,
        "replan" => OrgReviewVerdict::Replan,
        _ => return None,
    };
    Some((verdict, string_list(v.get("findings")?)?))
}

/// Worker の報告（`{summary, changedFiles, tests{ran, passed}}`）。形が違えば None。
pub fn parse_report_value(v: &serde_json::Value) -> Option<OrgWorkerReportView> {
    let tests = v.get("tests")?;
    Some(OrgWorkerReportView {
        summary: v.get("summary")?.as_str()?.to_string(),
        changed_files: string_list(v.get("changedFiles")?)?,
        tests_ran: tests.get("ran")?.as_bool()?,
        tests_passed: tests.get("passed")?.as_bool()?,
    })
}

/// 一覧の行の印。`tasks` は (タスクID, 依存, コミットあり, 局面)。後の書込みの依存になっている前の書込みは、後の成果に含まれるので数えない。
/// 数えるものがなければ None（印を出さない）。
pub fn artifact_hint(tasks: &[(String, Vec<String>, bool, OrgTaskPhase)], applied: &HashSet<String>) -> Option<OrgArtifactHint> {
    let committed: Vec<&(String, Vec<String>, bool, OrgTaskPhase)> = tasks.iter().filter(|t| t.2).collect();
    let mut ready = 0;
    let mut imported = 0;
    for (id, _, _, phase) in &committed {
        if !matches!(phase, OrgTaskPhase::Completed | OrgTaskPhase::Failed | OrgTaskPhase::Cancelled) {
            continue;
        }
        if applied.contains(id) {
            imported += 1;
        } else if !committed.iter().any(|o| &o.0 != id && o.1.contains(id)) {
            ready += 1;
        }
    }
    (ready + imported > 0).then_some(OrgArtifactHint { ready, imported })
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrgArtifactStatus as S;
    use OrgTaskPhase as P;

    fn facts(phase: P) -> StatusFacts {
        StatusFacts { phase, has_commit: true, in_head: Some(false), superseded: false, ledger_applied: false, git_ok: true }
    }

    #[test]
    fn status_covers_every_row_of_the_table() {
        assert_eq!(artifact_status(&StatusFacts { has_commit: false, ..facts(P::Running) }), Some(S::Working));
        assert_eq!(artifact_status(&facts(P::Rework)), Some(S::Working));
        assert_eq!(artifact_status(&facts(P::Reviewing)), Some(S::AiReviewing));
        assert_eq!(artifact_status(&facts(P::Completed)), Some(S::ReadyForReview));
        assert_eq!(artifact_status(&facts(P::Failed)), Some(S::NotPassed));
        assert_eq!(artifact_status(&facts(P::Cancelled)), Some(S::NotPassed));
        assert_eq!(artifact_status(&StatusFacts { superseded: true, ..facts(P::Completed) }), Some(S::Superseded));
        assert_eq!(artifact_status(&StatusFacts { in_head: Some(true), ledger_applied: true, ..facts(P::Completed) }), Some(S::Imported));
        assert_eq!(artifact_status(&StatusFacts { in_head: Some(true), ..facts(P::Completed) }), Some(S::IncludedElsewhere));
        assert_eq!(artifact_status(&StatusFacts { has_commit: false, ..facts(P::Completed) }), Some(S::NoChanges));
        assert_eq!(artifact_status(&StatusFacts { has_commit: false, ..facts(P::Failed) }), None, "nothing to show for a failure without a commit");
    }

    #[test]
    fn unknown_ancestry_or_unreadable_git_is_unknown_not_ready() {
        assert_eq!(artifact_status(&StatusFacts { in_head: None, ..facts(P::Completed) }), Some(S::Unknown));
        assert_eq!(artifact_status(&StatusFacts { git_ok: false, ..facts(P::Completed) }), Some(S::Unknown));
    }

    #[test]
    fn applied_in_the_ledger_but_not_in_head_is_ready_with_a_note() {
        let f = StatusFacts { ledger_applied: true, ..facts(P::Completed) };
        assert_eq!(artifact_status(&f), Some(S::ReadyForReview));
        assert!(applied_but_missing(&f));
        assert!(!applied_but_missing(&StatusFacts { in_head: Some(true), ..f }));
    }

    #[test]
    fn a_later_writer_that_contains_an_earlier_one_supersedes_it() {
        let commits = vec![("t1".to_string(), "aaa".to_string()), ("t2".to_string(), "bbb".to_string()), ("t3".to_string(), "ccc".to_string())];
        // aaa -> bbb -> ccc の鎖
        let anc = |a: &str, b: &str| Some(matches!((a, b), ("aaa", "bbb") | ("aaa", "ccc") | ("bbb", "ccc")));
        let s = supersede(&commits, anc);
        assert!(s.contains("t1") && s.contains("t2") && !s.contains("t3"));
        // 判定できない（None）ときは上位とみなさない。同じコミットも上位としない。
        assert!(supersede(&commits, |_, _| None).is_empty());
        let same = vec![("a".to_string(), "x".to_string()), ("b".to_string(), "x".to_string())];
        assert!(supersede(&same, |_, _| Some(true)).is_empty());
    }

    #[test]
    fn preview_kind_follows_the_table_and_ignores_case_and_dot_files() {
        assert_eq!(preview_kind("docs/README.MD"), OrgPreviewKind::Markdown);
        assert_eq!(preview_kind("a/b.yaml"), OrgPreviewKind::Text);
        assert_eq!(preview_kind("data.CSV"), OrgPreviewKind::Table);
        assert_eq!(preview_kind("logo.svg"), OrgPreviewKind::Image);
        assert_eq!(preview_kind("report.docx"), OrgPreviewKind::None);
        assert_eq!(preview_kind("book.xlsx"), OrgPreviewKind::None);
        assert_eq!(preview_kind(".gitignore"), OrgPreviewKind::None);
        assert_eq!(preview_kind("noext"), OrgPreviewKind::None);
        assert!(is_document("a.md") && is_document("a.csv") && !is_document("a.json"));
    }

    #[test]
    fn decode_text_handles_utf8_bom_utf16_and_refuses_the_rest() {
        assert_eq!(decode_text("日本語".as_bytes(), false).unwrap(), "日本語");
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("ok".as_bytes());
        assert_eq!(decode_text(&bom, false).unwrap(), "ok");
        let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain("あa".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(decode_text(&le, false).unwrap(), "あa");
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("あa".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
        assert_eq!(decode_text(&be, false).unwrap(), "あa");
        // Shift_JIS は判定できない。NUL入りはバイナリ。
        assert_eq!(decode_text(&[0x93, 0xFA, 0x96, 0x7B], false), Err(DecodeRefusal::UnknownEncoding));
        assert_eq!(decode_text(b"a\0b", false), Err(DecodeRefusal::Binary));
        // 途中で切れた末尾の不完全な文字は、切った印があるときだけ捨てる。
        let cut = &"あい".as_bytes()[..4];
        assert_eq!(decode_text(cut, true).unwrap(), "あ");
        assert_eq!(decode_text(cut, false), Err(DecodeRefusal::UnknownEncoding));
        // 切り詰めの途中のUTF-16サロゲート。
        let pair: Vec<u8> = [0xFF, 0xFE].into_iter().chain("a😀".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(decode_text(&pair[..pair.len() - 2], true).unwrap(), "a");
    }

    fn ev(seq: u64, kind: OrgEventKind) -> OrgEvent {
        OrgEvent { schema_version: 1, seq, at_ms: 0, kind }
    }

    #[test]
    fn graph_verdicts_are_matched_to_the_commit_the_writer_made_just_before() {
        let (w, r, other) = (TaskId::new("build-1"), TaskId::new("verify-1"), TaskId::new("other-1"));
        let slot = SlotId::new("verdict");
        let sw = |seq, task: &TaskId, version| ev(seq, OrgEventKind::SlotWritten { task: task.clone(), slot: slot.clone(), version, bytes: 10 });
        let commit = |seq, task: &TaskId, c: &str| ev(seq, OrgEventKind::Committed { task: task.clone(), commit: c.into() });
        let events = vec![
            sw(1, &r, 1), // before any commit: no target is guessed
            commit(2, &w, "c1"),
            commit(3, &other, "zz"), // another task's commit never counts
            sw(4, &r, 2),
            commit(5, &w, "c2"),
            sw(6, &other, 9), // another task's slot write never counts
            sw(7, &r, 3),
        ];
        let rounds = review_rounds(&events, &w, &r, &slot);
        assert_eq!(rounds, vec![
            ReviewRound { round: 1, version: 1, commit: None },
            ReviewRound { round: 2, version: 2, commit: Some("c1".into()) },
            ReviewRound { round: 3, version: 3, commit: Some("c2".into()) },
        ]);
        assert!(review_rounds(&events, &w, &r, &SlotId::new("other")).is_empty());
        assert_eq!(latest_slot_version(&events, &r, &slot), Some(3));
        assert_eq!(latest_slot_version(&events, &w, &slot), None);
    }

    #[test]
    fn recorded_findings_are_per_send_back_of_that_writer_only() {
        let (w, o) = (TaskId::new("w"), TaskId::new("o"));
        let f = |seq, t: &TaskId, x: &str| ev(seq, OrgEventKind::FindingsRecorded { task: t.clone(), findings: vec![x.into()] });
        let events = vec![f(1, &w, "a"), f(2, &o, "other"), f(3, &w, "b")];
        assert_eq!(recorded_findings(&events, &w), vec![vec!["a".to_string()], vec!["b".to_string()]]);
        assert!(recorded_findings(&[], &w).is_empty());
    }

    #[test]
    fn stored_outputs_are_read_only_when_their_shape_matches() {
        let v = serde_json::json!({"verdict": "rework", "findings": ["a", "b"], "score": 3});
        assert_eq!(parse_review_value(&v), Some((OrgReviewVerdict::Rework, vec!["a".into(), "b".into()])));
        assert_eq!(parse_review_value(&serde_json::json!({"verdict": "replan", "findings": []})).unwrap().0, OrgReviewVerdict::Replan);
        assert_eq!(parse_review_value(&serde_json::json!({"verdict": "maybe", "findings": []})), None);
        assert_eq!(parse_review_value(&serde_json::json!({"verdict": "pass"})), None);
        assert_eq!(parse_review_value(&serde_json::json!({"verdict": "pass", "findings": [1]})), None);
        let r = serde_json::json!({"summary": "s", "changedFiles": ["a.rs"], "tests": {"ran": true, "passed": false}});
        assert_eq!(parse_report_value(&r), Some(OrgWorkerReportView { summary: "s".into(), changed_files: vec!["a.rs".into()], tests_ran: true, tests_passed: false }));
        assert_eq!(parse_report_value(&serde_json::json!({"title": "doc", "body": "b", "notes": []})), None);
    }

    #[test]
    fn the_list_hint_counts_unimported_terminal_writes_and_skips_ones_a_later_write_builds_on() {
        let t = |id: &str, deps: &[&str], commit: bool, phase| (id.to_string(), deps.iter().map(|d| d.to_string()).collect::<Vec<_>>(), commit, phase);
        let none = HashSet::new();
        assert_eq!(artifact_hint(&[], &none), None);
        assert_eq!(artifact_hint(&[t("w", &[], false, P::Completed)], &none), None, "no commit, nothing to show");
        assert_eq!(artifact_hint(&[t("w", &[], true, P::Running)], &none), None, "still working");
        let chain = [t("w1", &[], true, P::Completed), t("w2", &["w1"], true, P::Completed), t("r", &["w2"], false, P::Completed)];
        assert_eq!(artifact_hint(&chain, &none), Some(OrgArtifactHint { ready: 1, imported: 0 }));
        let applied = HashSet::from(["w2".to_string()]);
        assert_eq!(artifact_hint(&chain, &applied), Some(OrgArtifactHint { ready: 0, imported: 1 }));
        assert_eq!(artifact_hint(&[t("w", &[], true, P::Failed)], &none), Some(OrgArtifactHint { ready: 1, imported: 0 }));
    }

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xFF, 0xFE, 0x00]), "//4A");
    }

    #[test]
    fn limit_rows_cuts_at_a_line_end_and_says_so() {
        assert_eq!(limit_rows("a\nb\nc\n", 2), ("a\nb".to_string(), true));
        assert_eq!(limit_rows("a\nb", 5), ("a\nb".to_string(), false));
        assert_eq!(limit_rows("a\nb\n", 2), ("a\nb".to_string(), false));
    }

    #[test]
    fn report_and_diff_differences_are_listed_both_ways() {
        let (a, b) = report_vs_diff(&["src/a.rs".into(), "Docs\\x.md".into(), "gone.txt".into()], &["src/a.rs".into(), "docs/x.md".into(), "extra.txt".into()]);
        assert_eq!(a, ["gone.txt"]);
        assert_eq!(b, ["extra.txt"]);
        assert_eq!(report_vs_diff(&[], &[]), (vec![], vec![]));
        assert_eq!(mismatch_lines(&a, &b).len(), 2);
    }

    fn file(a: Known<u32>, d: Known<u32>, binary: bool) -> OrgArtifactFile {
        OrgArtifactFile { path: "p".into(), kind: ChangeKind::Modified, move_to: None, additions: a, deletions: d, binary, preview: OrgPreviewKind::None }
    }

    #[test]
    fn totals_do_not_count_unknown_lines_as_zero() {
        let t = totals(&[file(Known::direct(3), Known::direct(1), false), file(Known::Unsupported, Known::Unsupported, true), file(Known::NotFetched, Known::NotFetched, false)]);
        assert_eq!(t, OrgArtifactTotals { files: 3, additions: 3, deletions: 1, binary: 1, unknown: 1 });
    }

    #[test]
    fn numstat_z_reads_plain_binary_and_renamed_entries() {
        let out = "3\t1\tsrc/a.rs\0-\t-\timg.png\00\t0\t\0old name.txt\0new name.txt\02\t0\tz.md\0";
        let e = parse_numstat_z(out);
        assert_eq!(e.len(), 4);
        assert_eq!((e[0].path.as_str(), e[0].additions, e[0].deletions), ("src/a.rs", Some(3), Some(1)));
        assert!(e[1].binary && e[1].additions.is_none());
        assert_eq!((e[2].path.as_str(), e[2].old_path.as_deref()), ("new name.txt", Some("old name.txt")));
        assert_eq!(e[3].path, "z.md");
        assert!(parse_numstat_z("").is_empty());
        // 壊れた途中の名前変更は捨てる。
        assert!(parse_numstat_z("0\t0\t\0only-old\0").is_empty());
    }

    #[test]
    fn name_status_and_numstat_are_merged_with_moves_and_binaries() {
        let names = vec![
            ChangedFile { status: "A".into(), path: "new.md".into(), old_path: None },
            ChangedFile { status: "R100".into(), path: "b.txt".into(), old_path: Some("a.txt".into()) },
            ChangedFile { status: "M".into(), path: "pic.png".into(), old_path: None },
            ChangedFile { status: "D".into(), path: "gone".into(), old_path: None },
            ChangedFile { status: "M".into(), path: "unmatched".into(), old_path: None },
        ];
        let nums = parse_numstat_z("5\t0\tnew.md\00\t0\t\0a.txt\0b.txt\0-\t-\tpic.png\01\t4\tgone\0");
        let f = merge_files(&names, &nums);
        assert_eq!((f[0].kind, f[0].preview), (ChangeKind::Added, OrgPreviewKind::Markdown));
        assert_eq!((f[1].path.as_str(), f[1].move_to.as_deref(), f[1].kind), ("a.txt", Some("b.txt"), ChangeKind::Modified));
        assert!(f[2].binary && f[2].additions == Known::Unsupported);
        assert_eq!(f[3].kind, ChangeKind::Deleted);
        assert_eq!(f[4].additions, Known::NotFetched);
        assert_eq!(touched_paths(&f[1..2]), ["a.txt", "b.txt"]);
    }

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn merge_tree_output_is_clean_conflict_or_failed() {
        assert_eq!(parse_merge_tree(Some(0), &format!("{OID}\0")), MergeTreeResult::Clean { tree: OID.into() });
        assert_eq!(parse_merge_tree(Some(1), &format!("{OID}\0a.txt\0dir/b.txt\0a.txt\0")), MergeTreeResult::Conflict { files: vec!["a.txt".into(), "dir/b.txt".into()] });
        assert_eq!(parse_merge_tree(Some(128), ""), MergeTreeResult::Failed);
        assert_eq!(parse_merge_tree(Some(0), "not a tree"), MergeTreeResult::Failed);
        assert_eq!(parse_merge_tree(None, &format!("{OID}\0")), MergeTreeResult::Failed);
        assert_eq!(parse_merge_tree(Some(2), &format!("{OID}\0")), MergeTreeResult::Failed);
    }

    #[test]
    fn merge_tree_needs_git_238() {
        assert_eq!(merge_tree_supported("git version 2.37.1"), Some(false));
        assert_eq!(merge_tree_supported("git version 2.38.0\n"), Some(true));
        assert_eq!(merge_tree_supported("git version 2.45.2.windows.1"), Some(true));
        assert_eq!(merge_tree_supported("git version 3.0.0"), Some(true));
        assert_eq!(merge_tree_supported("git version 1.9.5"), Some(false));
        assert_eq!(merge_tree_supported("something else"), None);
    }

    #[test]
    fn operations_in_progress_are_named() {
        assert_eq!(operation_in_progress(|n| n == "MERGE_HEAD"), Some("マージ"));
        assert_eq!(operation_in_progress(|n| n == "rebase-apply"), Some("リベース"));
        assert_eq!(operation_in_progress(|n| n == "CHERRY_PICK_HEAD"), Some("チェリーピック"));
        assert_eq!(operation_in_progress(|_| false), None);
    }

    fn base_facts<'a>(selected: &'a [(String, OrgArtifactStatus)]) -> ImportFacts<'a> {
        ImportFacts { branch: Some("main"), in_progress: None, workflow_finished: true, delete_pending: false, selected, git_supported: Some(true) }
    }

    #[test]
    fn import_is_blocked_by_each_unsafe_precondition() {
        let ok = [("t1".to_string(), S::ReadyForReview), ("t2".to_string(), S::NotPassed)];
        assert!(import_blockers(&base_facts(&ok)).is_empty());
        let one = |f: ImportFacts| import_blockers(&f);
        assert_eq!(one(ImportFacts { branch: Some("HEAD"), ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { branch: Some("agentdock/org/w/t"), ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { branch: None, ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { in_progress: Some("マージ"), ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { workflow_finished: false, ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { delete_pending: true, ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { git_supported: Some(false), ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(ImportFacts { git_supported: None, ..base_facts(&ok) }).len(), 1);
        assert_eq!(one(base_facts(&[])).len(), 1);
        let bad = [("t3".to_string(), S::Imported)];
        assert!(import_blockers(&base_facts(&bad))[0].contains("t3"));
    }

    #[test]
    fn dirty_overlap_ignores_case_separators_and_catches_folder_clashes() {
        let touched = vec!["Docs/Plan.md".to_string(), "src/new.rs".to_string(), "assets/img.png".to_string(), "other.txt".to_string()];
        let dirty = vec!["docs\\plan.md".to_string(), "src".to_string(), "assets/img.png/".to_string()];
        assert_eq!(dirty_overlap(&touched, &dirty), ["Docs/Plan.md", "src/new.rs", "assets/img.png"]);
        assert!(dirty_overlap(&touched, &["unrelated.txt".to_string()]).is_empty());
        assert!(dirty_overlap(&[], &dirty).is_empty());
        // 移動の元の名前も、未コミットの変更と重なれば拾う。
        assert_eq!(dirty_overlap(&["old.txt".to_string(), "new.txt".to_string()], &["OLD.txt".to_string()]), ["old.txt"]);
    }

    #[test]
    fn import_outcome_is_applied_only_when_head_is_read_back_as_the_result() {
        assert_eq!(classify_import("H", "R", Some("R"), None, None), OrgImportOutcome::Applied { head: "R".into() });
        assert!(matches!(classify_import("H", "R", Some("H"), Some(&[]), Some("lock")), OrgImportOutcome::NotApplied { reason } if reason == "lock"));
        assert!(matches!(classify_import("H", "R", Some("H"), Some(&[]), None), OrgImportOutcome::NotApplied { .. }));
        assert_eq!(classify_import("H", "R", Some("H"), Some(&["a.txt".to_string()]), None), OrgImportOutcome::Partial { files: vec!["a.txt".into()] });
        assert!(matches!(classify_import("H", "R", Some("H"), None, None), OrgImportOutcome::Unknown { .. }));
        assert!(matches!(classify_import("H", "R", Some("X"), Some(&[]), None), OrgImportOutcome::Unknown { .. }));
        assert!(matches!(classify_import("H", "R", None, None, None), OrgImportOutcome::Unknown { head_now: Known::NotFetched }));
    }

    #[test]
    fn begin_without_end_is_reconciled_by_the_current_head() {
        assert_eq!(reconcile_import("H", "R", Some("R")), OrgImportOutcome::Applied { head: "R".into() });
        assert!(matches!(reconcile_import("H", "R", Some("H")), OrgImportOutcome::NotApplied { .. }));
        assert!(matches!(reconcile_import("H", "R", Some("Z")), OrgImportOutcome::Unknown { .. }));
        assert!(matches!(reconcile_import("H", "R", None), OrgImportOutcome::Unknown { .. }));
        assert_eq!(reconcile_undo("H", "R", Some("H")), Some(OrgImportState::Undone));
        assert_eq!(reconcile_undo("H", "R", Some("R")), Some(OrgImportState::Applied));
        assert_eq!(reconcile_undo("H", "R", Some("Z")), None);
    }

    fn undo_facts<'a>() -> UndoFacts<'a> {
        UndoFacts {
            state: OrgImportState::Applied,
            recorded_branch: "main",
            current_branch: Some("main"),
            head_now: Some("R"),
            before: "H",
            result: "R",
            remote_contains: Some(false),
            dirty_overlap: &[],
            partial_files: &[],
        }
    }

    #[test]
    fn undo_is_allowed_only_for_an_unpushed_untouched_import() {
        let d = undo_decision(&undo_facts());
        assert!(d.allowed && d.mode == OrgUndoMode::Reset && d.refusal.is_none());
        let why = |f: UndoFacts| undo_decision(&f).refusal.unwrap();
        assert!(why(UndoFacts { remote_contains: Some(true), ..undo_facts() }).contains("送信済み"));
        assert!(why(UndoFacts { remote_contains: None, ..undo_facts() }).contains("確認できない"));
        assert!(why(UndoFacts { head_now: Some("Z"), ..undo_facts() }).contains("進んだ"));
        assert!(why(UndoFacts { current_branch: Some("dev"), ..undo_facts() }).contains("ブランチ"));
        assert!(why(UndoFacts { current_branch: None, ..undo_facts() }).contains("ブランチ"));
        let dirty = ["a.txt".to_string()];
        assert!(why(UndoFacts { dirty_overlap: &dirty, ..undo_facts() }).contains("a.txt"));
        for s in [OrgImportState::Undone, OrgImportState::NotApplied, OrgImportState::Unknown] {
            assert!(!undo_decision(&UndoFacts { state: s, ..undo_facts() }).allowed);
        }
    }

    #[test]
    fn a_partial_import_is_restored_file_by_file_while_head_is_still_before() {
        let files = ["a.txt".to_string()];
        let p = UndoFacts { state: OrgImportState::Partial, head_now: Some("H"), partial_files: &files, ..undo_facts() };
        let d = undo_decision(&p);
        assert!(d.allowed && d.mode == OrgUndoMode::RestoreFiles && d.files == files);
        assert!(!undo_decision(&UndoFacts { head_now: Some("Z"), ..p }).allowed);
    }

    #[test]
    fn commit_message_lists_each_task_with_a_short_sha() {
        let m = import_commit_message("レポート作成\n詳細", &[("t1".into(), "概要".into(), "0123456789".into()), ("t2".into(), "付録".into(), "abc".into())]);
        assert!(m.starts_with("AgentDock: 組織実行の成果を取り込み（レポート作成）\n\n"));
        assert!(m.contains("- t1 概要 0123456\n") && m.contains("- t2 付録 abc\n"));
    }

    fn row(kind: LedgerKind, id: &str) -> LedgerRow {
        LedgerRow { schema_version: 1, kind, id: id.into(), at: UnixMillis(1), folder: Some("C:/r".into()), branch: Some("main".into()), before: Some("H".into()), result: Some("R".into()), tasks: vec![LedgerTask { task: "t1".into(), commit: "c1".into() }], outcome: None, undo_outcome: None }
    }

    fn text_of(rows: &[LedgerRow]) -> String {
        rows.iter().map(|r| serde_json::to_string(r).unwrap()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn ledger_rows_fold_into_records_and_states() {
        let begin = row(LedgerKind::Begin, "i1");
        // begin だけ: HEAD で見分ける。
        let recs = fold_ledger(&text_of(&[begin.clone()]));
        assert_eq!(recs.len(), 1);
        assert_eq!(record_state(&recs[0], Some("R")), OrgImportState::Applied);
        assert_eq!(record_state(&recs[0], Some("H")), OrgImportState::NotApplied);
        assert_eq!(record_state(&recs[0], Some("Z")), OrgImportState::Unknown);
        assert_eq!(record_state(&recs[0], None), OrgImportState::Unknown);
        // end: Applied は HEAD が進んでも Applied のまま。
        let end = LedgerRow { outcome: Some(OrgImportOutcome::Applied { head: "R".into() }), ..row(LedgerKind::End, "i1") };
        let recs = fold_ledger(&text_of(&[begin.clone(), end.clone()]));
        assert_eq!(record_state(&recs[0], Some("Z")), OrgImportState::Applied);
        assert_eq!(applied_tasks(&recs, Some("Z")), HashSet::from(["t1".to_string()]));
        // 戻した記録。
        let ub = row(LedgerKind::UndoBegin, "i1");
        let ue = LedgerRow { undo_outcome: Some(OrgUndoOutcome::Undone { head: "H".into() }), ..row(LedgerKind::UndoEnd, "i1") };
        let recs = fold_ledger(&text_of(&[begin.clone(), end.clone(), ub.clone(), ue]));
        assert_eq!(record_state(&recs[0], Some("H")), OrgImportState::Undone);
        assert!(applied_tasks(&recs, Some("H")).is_empty());
        // undoBegin だけ: HEAD で見分ける。
        let recs = fold_ledger(&text_of(&[begin.clone(), end, ub]));
        assert_eq!(record_state(&recs[0], Some("H")), OrgImportState::Undone);
        assert_eq!(record_state(&recs[0], Some("R")), OrgImportState::Applied);
        assert_eq!(record_state(&recs[0], Some("Z")), OrgImportState::Unknown);
    }

    #[test]
    fn ledger_skips_unreadable_lines_and_orphan_ends() {
        let end = LedgerRow { outcome: Some(OrgImportOutcome::Applied { head: "R".into() }), ..row(LedgerKind::End, "ghost") };
        let text = format!("{{ broken\n{}\n\n{}\n{}", text_of(&[end]), text_of(&[row(LedgerKind::Begin, "i1")]), text_of(&[row(LedgerKind::Begin, "i1")]));
        let recs = fold_ledger(&text);
        assert_eq!(recs.len(), 1, "orphan end dropped, duplicate begin ignored");
        assert_eq!(recs[0].id, "i1");
    }

    #[test]
    fn an_unknown_end_is_reconciled_with_the_head_again() {
        let end = LedgerRow { outcome: Some(OrgImportOutcome::Unknown { head_now: Known::NotFetched }), ..row(LedgerKind::End, "i1") };
        let recs = fold_ledger(&text_of(&[row(LedgerKind::Begin, "i1"), end]));
        assert_eq!(record_state(&recs[0], Some("R")), OrgImportState::Applied);
        assert_eq!(record_state(&recs[0], Some("Z")), OrgImportState::Unknown);
    }
}
