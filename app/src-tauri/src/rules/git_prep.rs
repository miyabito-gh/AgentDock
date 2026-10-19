//! Gitでないフォルダの準備（`git init`＋初回コミット）の純粋ロジック（DESIGN_ORG_INTAKE §2）。I/Oをしない。
//!
//! - 最初のコミットに入るファイルの集計・断る条件・外す候補（巨大ファイル・一時ファイル）
//! - 除外ファイル（`.git\info\exclude`）への追記は独立した小部品（追記のみ・既存は上書きしない・前後を照合）。初期作成では既定で書かない（U2）。
//! - 失敗時に「自分が作った `.git` だけを消してよいか」の判断
//! - プレビューの指紋（実行時に再計算して一致しなければ実行しない）

use sha2::{Digest, Sha256};

use crate::backend::git_ops::*;
use crate::backend::model::{FolderCheck, FolderReject};

/// これを超えるファイルは外す候補にする（バイト）。
pub const BIG_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// 準備を断る上限。
pub const MAX_FILES: usize = 20_000;
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const LARGEST_SHOWN: usize = 10;

/// 最初のコミットに入りうるファイル1件（フォルダからの相対パス、`/` 区切り）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepFile {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
}

/// 集計の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepPlan {
    /// 最初のコミットに入るファイル（外す指定のときは外した後）。
    pub included: Vec<PrepFile>,
    pub total_bytes: u64,
    pub largest: Vec<PrepFileView>,
    pub suggested: Vec<PrepExcludeView>,
    /// 追記する行（利用者に示す内容そのもの）。
    pub exclude_lines: Vec<String>,
    pub refusal: Option<String>,
    pub notes: Vec<String>,
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 一時ファイルとして外す候補か。理由と、除外に書く形（パターン）を返す。
pub fn junk_pattern(path: &str) -> Option<&'static str> {
    let name = file_name(path).to_lowercase();
    if name == "thumbs.db" {
        Some("Thumbs.db")
    } else if name == "desktop.ini" {
        Some("desktop.ini")
    } else if name == ".ds_store" {
        Some(".DS_Store")
    } else if name.starts_with("~$") {
        Some("~$*")
    } else if name.ends_with(".tmp") {
        Some("*.tmp")
    } else {
        None
    }
}

/// gitの除外パターンとして、そのファイルだけを指す形にする（先頭の `/` で場所を固定し、特殊文字を `\` で逃がす）。
pub fn exact_pattern(path: &str) -> String {
    let mut out = String::from("/");
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 同期フォルダ（OneDrive 等）の中か。`.git` の同期で不具合が出ることがある（注意だけで止めない）。
pub fn in_sync_folder(folder: &str) -> bool {
    folder.replace('/', "\\").split('\\').any(|seg| {
        let s = seg.to_lowercase();
        s == "onedrive" || s.starts_with("onedrive - ") || s.starts_with("onedrive-") || s == "dropbox" || s.starts_with("google drive") || s == "googledrive" || s == "icloud drive" || s == "iclouddrive"
    })
}

/// 作業フォルダの検査結果から、準備を断る理由を作る（ドライブ直下・ユーザーフォルダ直下・Windows・Program Files 配下など）。
pub fn folder_verdict(check: &FolderCheck) -> Option<String> {
    match check {
        FolderCheck::Ok { .. } => None,
        FolderCheck::NeedsStrongConfirm { .. } => Some("このフォルダ（ドライブ直下・ユーザーフォルダ直下など）は広すぎるため、Gitを準備できません。仕事用のフォルダを選んでください".to_string()),
        FolderCheck::Rejected { reason } => Some(match reason {
            FolderReject::SystemFolder => "システムのフォルダのため、Gitを準備できません".to_string(),
            FolderReject::NotFound => "フォルダが見つかりません".to_string(),
            FolderReject::NotDirectory => "フォルダではありません".to_string(),
            FolderReject::Unc | FolderReject::NetworkDrive => "ネットワーク上のフォルダには、Gitを準備できません".to_string(),
            _ => "このフォルダには、Gitを準備できません".to_string(),
        }),
    }
}

/// 集計して計画を作る。`files` は最初のコミットに入りうる全ファイル（既存の `.gitignore` 反映後）。
/// `apply_excludes` が偽なら、外す候補は示すだけで、すべてコミットに含める（初期作成では除外を書かない）。
pub fn plan_prep(kind: PrepKind, files: &[PrepFile], apply_excludes: bool, folder_refusal: Option<String>, synced: bool) -> PrepPlan {
    let mut suggested = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    for f in files {
        if let Some(p) = junk_pattern(&f.path) {
            suggested.push(PrepExcludeView { path: f.path.clone(), size: f.size, reason: "一時ファイル".to_string() });
            if !patterns.iter().any(|x| x == p) {
                patterns.push(p.to_string());
            }
        } else if f.size > BIG_FILE_BYTES {
            suggested.push(PrepExcludeView { path: f.path.clone(), size: f.size, reason: format!("{}MBを超える大きいファイル", BIG_FILE_BYTES / 1024 / 1024) });
            patterns.push(exact_pattern(&f.path));
        }
    }
    let excluded_paths: std::collections::HashSet<&str> = suggested.iter().map(|s| s.path.as_str()).collect();
    let included: Vec<PrepFile> = if apply_excludes { files.iter().filter(|f| !excluded_paths.contains(f.path.as_str())).cloned().collect() } else { files.to_vec() };
    let total_bytes: u64 = included.iter().map(|f| f.size).sum();
    let mut by_size: Vec<&PrepFile> = included.iter().collect();
    by_size.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    let largest = by_size.into_iter().take(LARGEST_SHOWN).map(|f| PrepFileView { path: f.path.clone(), size: f.size }).collect();

    let big_present = suggested.iter().any(|s| s.size > BIG_FILE_BYTES);
    let refusal = if let Some(r) = folder_refusal {
        Some(r)
    } else if included.is_empty() {
        Some("最初のコミットに入れるファイルがありません".to_string())
    } else if included.len() > MAX_FILES {
        Some(format!("ファイルが多すぎます（{}件。上限は{}件）。対象のフォルダを絞ってください", included.len(), MAX_FILES))
    } else if total_bytes > MAX_TOTAL_BYTES {
        Some(format!("合計サイズが大きすぎます（{}MB。上限は{}MB）。対象のフォルダを絞るか、大きいファイルを外してください", total_bytes / 1024 / 1024, MAX_TOTAL_BYTES / 1024 / 1024))
    } else if kind == PrepKind::NoCommit && big_present && !apply_excludes {
        Some(format!("{}MBを超えるファイルがあります。除外の追記を選ぶか、大きいファイルを外してから、もう一度開いてください", BIG_FILE_BYTES / 1024 / 1024))
    } else {
        None
    };
    let mut notes = Vec::new();
    if synced {
        notes.push("同期フォルダ（OneDrive 等）の中です。`.git` が同期されると不具合が出ることがあります".to_string());
    }
    PrepPlan { included, total_bytes, largest, suggested, exclude_lines: patterns, refusal, notes }
}

// ───────────────────────────── 除外ファイルへの追記（独立した小部品） ─────────────────────────────

const EXCLUDE_HEADER: &str = "# AgentDock: 最初のコミットから外したファイル";

/// 既存の内容に足す文字列。すでに同じ行があるものは足さない。足すものがなければ空（書かない）。既存の内容は変えない（追記のみ）。
pub fn exclude_block(existing: &str, lines: &[String]) -> String {
    let present: std::collections::HashSet<&str> = existing.lines().map(str::trim_end).collect();
    let new: Vec<&String> = lines.iter().filter(|l| !l.is_empty() && !present.contains(l.as_str())).collect();
    if new.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(EXCLUDE_HEADER);
    out.push('\n');
    for l in new {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// 追記の前後の照合。書き終えた内容が「前の内容＋足した文字列」と完全に一致すること。
pub fn exclude_append_verified(before: &str, block: &str, after: &str) -> bool {
    after.len() == before.len() + block.len() && after.starts_with(before) && after.ends_with(block)
}

// ───────────────────────────── 失敗時の巻き戻し ─────────────────────────────

/// 準備の手順の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepResult {
    Ok,
    /// 非0終了など、明確に失敗した。
    ClearFailure,
    /// タイムアウト・起動不能・出力が読めないなど、結果が分からない。
    Unknown,
}

/// `.git` の中の「AgentDockが作った」印。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// 自分の準備IDと一致する。
    Mine,
    /// 別の準備の印（古い・別の操作）。
    Other,
    /// 印がない。
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackDecision {
    /// 作った `.git` だけを取り除く。
    RemoveOwnGit,
    /// 取り除かない。
    Keep(&'static str),
}

/// 失敗後に `.git` を取り除いてよいか。取り除くのは、新しく作った `.git`（`NotRepo`）で、明確に失敗し、印が自分のものと一致するときだけ。
pub fn rollback_decision(kind: PrepKind, step: &StepResult, marker: Marker) -> RollbackDecision {
    match (kind, step, marker) {
        (PrepKind::NotRepo, StepResult::ClearFailure, Marker::Mine) => RollbackDecision::RemoveOwnGit,
        (PrepKind::NoCommit, _, _) => RollbackDecision::Keep("既存のリポジトリのため取り除きません"),
        (PrepKind::Interrupted, _, _) => RollbackDecision::Keep("前回の途中の準備のため、自動では取り除きません"),
        (_, StepResult::Unknown, _) => RollbackDecision::Keep("結果を確認できないため取り除きません"),
        (_, StepResult::Ok, _) => RollbackDecision::Keep("失敗していません"),
        (_, _, Marker::Other) => RollbackDecision::Keep("別の準備の印のため取り除きません"),
        (_, _, Marker::Missing) => RollbackDecision::Keep("AgentDockが作った印を確認できないため取り除きません"),
    }
}

/// 巻き戻しの判断に使う印。実行前に `.git` が無いことを確認済みの新規作成（`NotRepo`）では、印を書く前に失敗しても
/// 今ある `.git` は自分が作ったもの。別の準備の印があるときだけ自分のものとして扱わない。
pub fn effective_marker(kind: PrepKind, observed: Marker) -> Marker {
    match (kind, observed) {
        (PrepKind::NotRepo, Marker::Missing) => Marker::Mine,
        (_, m) => m,
    }
}

// ───────────────────────────── 指紋・作者 ─────────────────────────────

/// ファイル一覧（パス・サイズ・更新時刻）の SHA-256（16進）。並び順に依存しない。
pub fn preview_token(files: &[PrepFile]) -> String {
    let mut sorted: Vec<&PrepFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut h = Sha256::new();
    for f in sorted {
        h.update(f.path.as_bytes());
        h.update([0]);
        h.update(f.size.to_le_bytes());
        h.update(f.mtime_ms.to_le_bytes());
        h.update([b'\n']);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 指紋が一致するか（空は常に不一致）。
pub fn token_matches(expected: &str, actual: &str) -> bool {
    !expected.is_empty() && expected == actual
}

pub const AGENTDOCK_AUTHOR_NAME: &str = "AgentDock";
pub const AGENTDOCK_AUTHOR_EMAIL: &str = "agentdock@localhost";

/// 作者。利用者の Git 設定に user.name と user.email が両方あればそれ、なければ AgentDock 名義（設定は書かない）。
pub fn author_from_config(name: Option<&str>, email: Option<&str>) -> PrepAuthorView {
    let n = name.map(str::trim).filter(|s| !s.is_empty());
    let e = email.map(str::trim).filter(|s| !s.is_empty());
    match (n, e) {
        (Some(n), Some(e)) => PrepAuthorView { name: n.to_string(), email: e.to_string(), is_agentdock: false },
        _ => PrepAuthorView { name: AGENTDOCK_AUTHOR_NAME.to_string(), email: AGENTDOCK_AUTHOR_EMAIL.to_string(), is_agentdock: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, size: u64) -> PrepFile {
        PrepFile { path: path.to_string(), size, mtime_ms: 1 }
    }

    const MB: u64 = 1024 * 1024;

    #[test]
    fn junk_and_big_files_are_only_suggested_unless_excludes_are_applied() {
        let files = vec![f("a.docx", MB), f("sub/Thumbs.db", 10), f("~$a.docx", 5), f("video.mp4", 60 * MB), f("x.tmp", 1)];
        let off = plan_prep(PrepKind::NotRepo, &files, false, None, false);
        assert_eq!(off.included.len(), 5, "nothing is excluded by default");
        assert_eq!(off.suggested.len(), 4);
        assert!(off.refusal.is_none());
        let on = plan_prep(PrepKind::NotRepo, &files, true, None, false);
        assert_eq!(on.included.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), vec!["a.docx"]);
        assert_eq!(on.exclude_lines, vec!["Thumbs.db", "~$*", "/video.mp4", "*.tmp"]);
        assert_eq!(on.total_bytes, MB);
    }

    #[test]
    fn largest_is_top_ten_by_size_with_stable_order() {
        let files: Vec<PrepFile> = (0..15).map(|i| f(&format!("f{i:02}"), i as u64)).collect();
        let p = plan_prep(PrepKind::NotRepo, &files, false, None, false);
        assert_eq!(p.largest.len(), 10);
        assert_eq!(p.largest[0].path, "f14");
        assert_eq!(p.largest[9].path, "f05");
    }

    #[test]
    fn plan_refuses_empty_too_many_too_large_and_refused_folders() {
        assert!(plan_prep(PrepKind::NotRepo, &[], false, None, false).refusal.unwrap().contains("ありません"));
        let many: Vec<PrepFile> = (0..MAX_FILES + 1).map(|i| f(&format!("{i}"), 1)).collect();
        assert!(plan_prep(PrepKind::NotRepo, &many, false, None, false).refusal.unwrap().contains("多すぎ"));
        let exactly: Vec<PrepFile> = (0..MAX_FILES).map(|i| f(&format!("{i}"), 1)).collect();
        assert!(plan_prep(PrepKind::NotRepo, &exactly, false, None, false).refusal.is_none());
        let huge = vec![f("a", 600 * MB), f("b", 600 * MB)];
        assert!(plan_prep(PrepKind::NotRepo, &huge, false, None, false).refusal.unwrap().contains("大きすぎ"));
        // the same files pass when the big ones are excluded... except nothing is left to commit
        assert!(plan_prep(PrepKind::NotRepo, &huge, true, None, false).refusal.unwrap().contains("入れるファイルがありません"));
        assert_eq!(plan_prep(PrepKind::NotRepo, &[f("a", 1)], false, Some("広すぎます".into()), false).refusal.as_deref(), Some("広すぎます"));
    }

    #[test]
    fn existing_repo_with_a_big_file_needs_the_exclude_choice() {
        let files = vec![f("a", 1), f("big.bin", 51 * MB)];
        let p = plan_prep(PrepKind::NoCommit, &files, false, None, false);
        assert!(p.refusal.unwrap().contains("除外の追記"));
        assert!(plan_prep(PrepKind::NoCommit, &files, true, None, false).refusal.is_none());
        // a new .git does not refuse a single big file by itself
        assert!(plan_prep(PrepKind::NotRepo, &files, false, None, false).refusal.is_none());
    }

    #[test]
    fn folder_verdict_refuses_broad_and_system_folders() {
        assert!(folder_verdict(&FolderCheck::Ok { normalized: "C:\\w".into() }).is_none());
        assert!(folder_verdict(&FolderCheck::NeedsStrongConfirm { normalized: "C:\\".into(), reasons: vec![crate::backend::model::StrongReason::DriveRoot] }).unwrap().contains("広すぎ"));
        assert!(folder_verdict(&FolderCheck::Rejected { reason: FolderReject::SystemFolder }).unwrap().contains("システム"));
    }

    #[test]
    fn sync_folders_are_detected_by_segment() {
        assert!(in_sync_folder("C:\\Users\\a\\OneDrive - Corp\\docs"));
        assert!(in_sync_folder("C:/Users/a/Dropbox/x"));
        assert!(!in_sync_folder("C:\\Users\\a\\Documents\\onedrivelike"));
    }

    #[test]
    fn exact_pattern_pins_the_location_and_escapes_specials() {
        assert_eq!(exact_pattern("dir/a b.bin"), "/dir/a b.bin");
        assert_eq!(exact_pattern("#x"), "/#x");
        assert_eq!(exact_pattern("a*b[1]"), "/a\\*b\\[1]");
    }

    #[test]
    fn exclude_block_appends_only_new_lines_and_never_changes_existing_text() {
        let lines = vec!["*.tmp".to_string(), "/big.bin".to_string()];
        let b = exclude_block("", &lines);
        assert_eq!(b, "# AgentDock: 最初のコミットから外したファイル\n*.tmp\n/big.bin\n");
        // existing content without a trailing newline gets one before the block
        let existing = "# mine\n*.tmp";
        let b = exclude_block(existing, &lines);
        assert_eq!(b, "\n# AgentDock: 最初のコミットから外したファイル\n/big.bin\n");
        assert!(exclude_append_verified(existing, &b, &format!("{existing}{b}")));
        // nothing new: nothing to write
        assert_eq!(exclude_block("*.tmp\n/big.bin\n", &lines), "");
    }

    #[test]
    fn exclude_append_verification_catches_changed_or_truncated_files() {
        let before = "a\n";
        let block = "# x\n*.tmp\n";
        assert!(exclude_append_verified(before, block, "a\n# x\n*.tmp\n"));
        assert!(!exclude_append_verified(before, block, "A\n# x\n*.tmp\n"));
        assert!(!exclude_append_verified(before, block, "a\n# x\n"));
        assert!(!exclude_append_verified(before, block, "a\n# x\n*.tmp\nextra\n"));
    }

    #[test]
    fn rollback_removes_only_our_new_git_after_a_clear_failure() {
        use RollbackDecision::*;
        assert_eq!(rollback_decision(PrepKind::NotRepo, &StepResult::ClearFailure, Marker::Mine), RemoveOwnGit);
        // unknown result: keep and re-read
        assert!(matches!(rollback_decision(PrepKind::NotRepo, &StepResult::Unknown, Marker::Mine), Keep(_)));
        // somebody else's mark or no mark
        assert!(matches!(rollback_decision(PrepKind::NotRepo, &StepResult::ClearFailure, Marker::Other), Keep(_)));
        assert!(matches!(rollback_decision(PrepKind::NotRepo, &StepResult::ClearFailure, Marker::Missing), Keep(_)));
        // the user's existing repository and an interrupted earlier prep are never removed automatically
        assert!(matches!(rollback_decision(PrepKind::NoCommit, &StepResult::ClearFailure, Marker::Mine), Keep(_)));
        assert!(matches!(rollback_decision(PrepKind::Interrupted, &StepResult::ClearFailure, Marker::Mine), Keep(_)));
        assert!(matches!(rollback_decision(PrepKind::NotRepo, &StepResult::Ok, Marker::Mine), Keep(_)));
    }

    #[test]
    fn a_new_git_dir_without_a_mark_is_ours_so_init_or_mark_failures_are_rolled_back() {
        let m = effective_marker(PrepKind::NotRepo, Marker::Missing);
        assert_eq!(rollback_decision(PrepKind::NotRepo, &StepResult::ClearFailure, m), RollbackDecision::RemoveOwnGit);
        // another prep's mark, and non-new repositories, are not promoted
        assert_eq!(effective_marker(PrepKind::NotRepo, Marker::Other), Marker::Other);
        assert_eq!(effective_marker(PrepKind::NoCommit, Marker::Missing), Marker::Missing);
        assert!(matches!(rollback_decision(PrepKind::NoCommit, &StepResult::ClearFailure, effective_marker(PrepKind::NoCommit, Marker::Missing)), RollbackDecision::Keep(_)));
        // an unknown result is still kept
        assert!(matches!(rollback_decision(PrepKind::NotRepo, &StepResult::Unknown, m), RollbackDecision::Keep(_)));
    }

    #[test]
    fn preview_token_changes_with_size_mtime_or_path_but_not_order() {
        let a = vec![f("a", 1), f("b", 2)];
        let t = preview_token(&a);
        assert_eq!(t, preview_token(&[f("b", 2), f("a", 1)]));
        assert_ne!(t, preview_token(&[f("a", 1), f("b", 3)]));
        assert_ne!(t, preview_token(&[f("a", 1), PrepFile { mtime_ms: 2, ..f("b", 2) }]));
        assert_ne!(t, preview_token(&[f("a", 1), f("c", 2)]));
        assert!(token_matches(&t, &t));
        assert!(!token_matches("", ""));
        assert!(!token_matches(&t, "other"));
    }

    #[test]
    fn author_uses_the_users_identity_only_when_both_are_set() {
        let u = author_from_config(Some("Taro"), Some("t@example.com"));
        assert_eq!((u.name.as_str(), u.is_agentdock), ("Taro", false));
        assert!(author_from_config(Some("Taro"), None).is_agentdock);
        assert!(author_from_config(Some(" "), Some("t@example.com")).is_agentdock);
        assert!(author_from_config(None, None).is_agentdock);
    }
}
