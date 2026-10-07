//! 統一diffの分割・解析・逆適用（P3-1、`app/DESIGN_P3.md` §1 #1・#2）。純粋ロジック。
//!
//! - [`split_files`]: turn集約のdiff（`diff --git` 区切り、なければ `---`/`+++` 区切り）をファイル別に分ける。
//! - [`parse_hunks`]: 1ファイル分の統一diffの変更箇所（hunk）を読む。行数（`@@ -a,b +c,d @@`）に従って読み、本文の `---` 等と混同しない。
//! - [`apply_reverse`]: 変更後の内容から変更前の内容を作る。文脈は完全一致だけを許し、曖昧な一致・ずれの補正（fuzz）はしない。

use crate::backend::changes::ChangeKind;

/// ファイル別に分けた差分。`text` はそのファイル分の原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// 移動・名前変更の元（なければ None）。
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub additions: u32,
    pub deletions: u32,
    pub text: String,
}

fn unquote(s: &str) -> String {
    let t = s.trim_end_matches('\r');
    let Some(inner) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else { return t.to_string() };
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// `---`/`+++` 行のパス部分。`/dev/null` は None。時刻などの後続（タブ以降）は捨てる。
fn marker_path(rest: &str) -> Option<String> {
    let rest = rest.split('\t').next().unwrap_or(rest);
    let p = unquote(rest);
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).map(str::to_string).unwrap_or(p))
}

/// `diff --git a/X b/Y` から (X, Y)。空白を含むパスは、前後が同じ長さに分かれるときだけ確定する。
fn git_header_paths(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim_end_matches('\r');
    if let Some((a, b)) = rest.split_once(" b/") {
        let a = a.strip_prefix("a/").unwrap_or(a);
        if a == b || !b.contains(" b/") {
            return Some((unquote(a), unquote(b)));
        }
    }
    let inner = rest.strip_prefix("a/")?;
    let half = (inner.len().checked_sub(3)?) / 2;
    if inner.len() == half * 2 + 3 && inner.is_char_boundary(half) && inner[half..].starts_with(" b/") && inner[..half] == inner[half + 3..] {
        return Some((unquote(&inner[..half]), unquote(&inner[..half])));
    }
    None
}

fn count_body(lines: &[&str]) -> (u32, u32) {
    let (mut add, mut del) = (0u32, 0u32);
    for l in lines {
        match l.as_bytes().first() {
            Some(b'+') => add += 1,
            Some(b'-') => del += 1,
            _ => {}
        }
    }
    (add, del)
}

/// 本文（最初の `@@` 以降）の追加・削除の行数。ヘッダー（`---`/`+++`）は数えない。
pub fn count_changes(text: &str) -> (u32, u32) {
    let lines: Vec<&str> = text.lines().collect();
    match lines.iter().position(|l| l.starts_with("@@")) {
        Some(i) => count_body(&lines[i..]),
        None => (0, 0),
    }
}

fn parse_block(text: &str) -> FileDiff {
    let lines: Vec<&str> = text.lines().collect();
    let first_hunk = lines.iter().position(|l| l.starts_with("@@")).unwrap_or(lines.len());
    let head = &lines[..first_hunk];
    let (mut added, mut deleted) = (false, false);
    let (mut rename_from, mut rename_to): (Option<String>, Option<String>) = (None, None);
    let (mut minus, mut plus): (Option<Option<String>>, Option<Option<String>>) = (None, None);
    let mut git_paths = None;
    for l in head {
        if let Some(r) = l.strip_prefix("diff --git ") {
            git_paths = git_header_paths(r);
        } else if l.starts_with("new file mode") {
            added = true;
        } else if l.starts_with("deleted file mode") {
            deleted = true;
        } else if let Some(r) = l.strip_prefix("rename from ") {
            rename_from = Some(unquote(r));
        } else if let Some(r) = l.strip_prefix("rename to ") {
            rename_to = Some(unquote(r));
        } else if let Some(r) = l.strip_prefix("--- ") {
            minus = Some(marker_path(r));
        } else if let Some(r) = l.strip_prefix("+++ ") {
            plus = Some(marker_path(r));
        }
    }
    if matches!(minus, Some(None)) {
        added = true;
    }
    if matches!(plus, Some(None)) {
        deleted = true;
    }
    let kind = if added {
        ChangeKind::Added
    } else if deleted {
        ChangeKind::Deleted
    } else {
        ChangeKind::Modified
    };
    let path = plus
        .clone()
        .flatten()
        .or_else(|| minus.clone().flatten())
        .or_else(|| rename_to.clone())
        .or_else(|| git_paths.as_ref().map(|(_, b)| b.clone()))
        .unwrap_or_default();
    let old = rename_from.or_else(|| git_paths.as_ref().map(|(a, _)| a.clone())).or_else(|| minus.flatten());
    let old_path = old.filter(|o| !o.is_empty() && *o != path && kind == ChangeKind::Modified);
    let (additions, deletions) = count_body(&lines[first_hunk..]);
    FileDiff { path, old_path, kind, additions, deletions, text: text.to_string() }
}

/// turn集約のdiffをファイル別に分ける。空のdiffは空の一覧。
pub fn split_files(diff: &str) -> Vec<FileDiff> {
    if diff.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = diff.split_inclusive('\n').collect();
    let mut starts: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.starts_with("diff --git ")).map(|(i, _)| i).collect();
    if starts.is_empty() {
        // `--- X` の次の行が `+++ Y` のものを区切りにする（本文の `--- ` と混同しないよう、直後の `+++ ` を条件にする）。
        for i in 0..lines.len().saturating_sub(1) {
            if lines[i].starts_with("--- ") && lines[i + 1].starts_with("+++ ") {
                starts.push(i);
            }
        }
    }
    let mut out = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let e = starts.get(n + 1).copied().unwrap_or(lines.len());
        let block = lines[s..e].concat();
        let f = parse_block(&block);
        if !f.path.is_empty() {
            out.push(f);
        }
    }
    out
}

// ───────────────────────────── Gitの状態（porcelain v2） ─────────────────────────────

/// `git status --porcelain=v2 -z` の1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// 名前変更・コピーの元。
    pub old_path: Option<String>,
    /// 索引側・作業ツリー側の状態文字（`.` は変更なし）。
    pub x: char,
    pub y: char,
    pub untracked: bool,
    pub unmerged: bool,
}

impl StatusEntry {
    pub fn kind(&self) -> ChangeKind {
        if self.untracked || self.x == 'A' {
            ChangeKind::Added
        } else if self.x == 'D' || self.y == 'D' {
            ChangeKind::Deleted
        } else {
            ChangeKind::Modified
        }
    }
}

/// `-z` 区切りの出力を読む。無視ファイル（`!`）とヘッダー行（`#`）は含めない。読めない行は捨てる（推定で補わない）。
pub fn parse_status_v2(out: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(rec) = it.next() {
        let mut ch = rec.chars();
        match ch.next() {
            Some('1') => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let mut xy = f[1].chars();
                    entries.push(StatusEntry { path: f[8].to_string(), old_path: None, x: xy.next().unwrap_or('.'), y: xy.next().unwrap_or('.'), untracked: false, unmerged: false });
                }
            }
            Some('2') => {
                // 2 XY sub mH mI mW hH hI Xscore path <NUL> origPath
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = it.next();
                if f.len() == 10 {
                    let mut xy = f[1].chars();
                    entries.push(StatusEntry { path: f[9].to_string(), old_path: orig.map(str::to_string), x: xy.next().unwrap_or('.'), y: xy.next().unwrap_or('.'), untracked: false, unmerged: false });
                }
            }
            Some('u') => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    let mut xy = f[1].chars();
                    entries.push(StatusEntry { path: f[10].to_string(), old_path: None, x: xy.next().unwrap_or('U'), y: xy.next().unwrap_or('U'), untracked: false, unmerged: true });
                }
            }
            Some('?') => {
                if let Some(p) = rec.strip_prefix("? ") {
                    entries.push(StatusEntry { path: p.to_string(), old_path: None, x: '?', y: '?', untracked: true, unmerged: false });
                }
            }
            _ => {}
        }
    }
    entries
}

// ───────────────────────────── 変更箇所（hunk） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub kind: LineKind,
    /// 改行を除いた本文。
    pub text: String,
    /// 原文の行が CRLF だった。
    pub cr: bool,
    /// 直後に `\ No newline at end of file` があった（この行は末尾の改行なし）。
    pub no_eol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<HunkLine>,
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let (os, ol) = parse_range(old)?;
    let (ns, nl) = parse_range(new)?;
    Some((os, ol, ns, nl))
}

/// 統一diff（ヘッダー行があってもよい）の変更箇所を読む。変更箇所がない・途中で切れている場合は `Err`。
pub fn parse_hunks(text: &str) -> Result<Vec<Hunk>, String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((os, ol, ns, nl)) = parse_hunk_header(lines[i].trim_end_matches('\r')) else {
            i += 1;
            continue;
        };
        i += 1;
        let (mut old_left, mut new_left) = (ol, nl);
        let mut body: Vec<HunkLine> = Vec::new();
        while old_left > 0 || new_left > 0 {
            let Some(raw) = lines.get(i) else { return Err("差分が途中で切れています".into()) };
            i += 1;
            let (line, cr) = match raw.strip_suffix('\r') {
                Some(l) => (l, true),
                None => (*raw, false),
            };
            let (kind, text) = match line.chars().next() {
                Some(' ') => (LineKind::Context, &line[1..]),
                Some('-') => (LineKind::Del, &line[1..]),
                Some('+') => (LineKind::Add, &line[1..]),
                // 空白だけの文脈行が空行として書かれている場合。
                None => (LineKind::Context, ""),
                Some('\\') => {
                    if let Some(prev) = body.last_mut() {
                        prev.no_eol = true;
                    }
                    continue;
                }
                Some(_) => return Err("差分の行の形式が想定と異なります".into()),
            };
            match kind {
                LineKind::Context => {
                    if old_left == 0 || new_left == 0 {
                        return Err("差分の行数が合いません".into());
                    }
                    old_left -= 1;
                    new_left -= 1;
                }
                LineKind::Del => {
                    if old_left == 0 {
                        return Err("差分の行数が合いません".into());
                    }
                    old_left -= 1;
                }
                LineKind::Add => {
                    if new_left == 0 {
                        return Err("差分の行数が合いません".into());
                    }
                    new_left -= 1;
                }
            }
            body.push(HunkLine { kind, text: text.to_string(), cr, no_eol: false });
        }
        // 最後の行の直後にある「改行なし」の印。
        while let Some(next) = lines.get(i) {
            if next.starts_with('\\') {
                if let Some(prev) = body.last_mut() {
                    prev.no_eol = true;
                }
                i += 1;
            } else {
                break;
            }
        }
        hunks.push(Hunk { old_start: os, old_len: ol, new_start: ns, new_len: nl, lines: body });
    }
    if hunks.is_empty() {
        return Err("差分に変更箇所がありません".into());
    }
    Ok(hunks)
}

/// 統一diffの形をしているか（`@@ -a,b +c,d @@` の行がある）。追加・削除の報告が本文そのものかの判別に使う。
pub fn looks_like_unified(text: &str) -> bool {
    text.lines().any(|l| parse_hunk_header(l.trim_end_matches('\r')).is_some())
}

struct FileLine<'a> {
    text: &'a str,
    raw: &'a str,
    has_eol: bool,
    cr: bool,
}

fn file_lines(content: &str) -> Vec<FileLine<'_>> {
    content
        .split_inclusive('\n')
        .map(|raw| {
            let has_eol = raw.ends_with('\n');
            let body = raw.strip_suffix('\n').unwrap_or(raw);
            let (text, cr) = match body.strip_suffix('\r') {
                Some(t) => (t, true),
                None => (body, false),
            };
            FileLine { text, raw, has_eol, cr }
        })
        .collect()
}

/// 変更後の内容 `content` に差分を逆向きに当て、変更前の内容を返す。
/// 変更後の側の行（文脈と追加行）が現在の内容と完全に一致しなければ `Err`（位置のずれの補正はしない）。
pub fn apply_reverse(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let lines = file_lines(content);
    let file_crlf = lines.iter().any(|l| l.cr);
    let mut out = String::with_capacity(content.len());
    let mut pos = 0usize;
    for (n, h) in hunks.iter().enumerate() {
        let start = if h.new_len == 0 { h.new_start } else { h.new_start.saturating_sub(1) };
        if start < pos || start > lines.len() {
            return Err(format!("変更箇所 {} の位置が現在の内容と一致しません", n + 1));
        }
        for l in &lines[pos..start] {
            out.push_str(l.raw);
        }
        pos = start;
        for hl in &h.lines {
            match hl.kind {
                LineKind::Context | LineKind::Add => {
                    let Some(l) = lines.get(pos) else { return Err(format!("変更箇所 {} の内容が現在のファイルにありません", n + 1)) };
                    if l.text != hl.text || (hl.no_eol && l.has_eol) {
                        return Err(format!("変更箇所 {} の{}行が現在の内容と一致しません", n + 1, pos + 1));
                    }
                    if hl.kind == LineKind::Context {
                        out.push_str(l.raw);
                    }
                    pos += 1;
                }
                LineKind::Del => {
                    out.push_str(&hl.text);
                    if !hl.no_eol {
                        out.push_str(if hl.cr || file_crlf { "\r\n" } else { "\n" });
                    }
                }
            }
        }
    }
    for l in &lines[pos..] {
        out.push_str(l.raw);
    }
    Ok(out)
}

/// 削除された変更から、削除前の内容を作る。報告が統一diffなら削除行を集め、そうでなければ本文そのもの。
pub fn deleted_content(diff: &str) -> Result<String, String> {
    if !looks_like_unified(diff) {
        return Ok(diff.to_string());
    }
    let hunks = parse_hunks(diff)?;
    let mut out = String::new();
    for h in &hunks {
        for l in &h.lines {
            if l.kind == LineKind::Del {
                out.push_str(&l.text);
                if !l.no_eol {
                    out.push_str(if l.cr { "\r\n" } else { "\n" });
                }
            }
        }
    }
    Ok(out)
}

/// 追加・削除の報告の行数（統一diffでなければ本文の行数）。
pub fn count_for(kind: ChangeKind, diff: &str) -> (u32, u32) {
    if looks_like_unified(diff) {
        return count_changes(diff);
    }
    let n = diff.lines().count() as u32;
    match kind {
        ChangeKind::Added => (n, 0),
        ChangeKind::Deleted => (0, n),
        ChangeKind::Modified => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_FILES: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..333\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";

    #[test]
    fn split_files_separates_git_style_blocks_with_kinds_and_counts() {
        let files = split_files(TWO_FILES);
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].path.as_str(), files[0].kind, files[0].additions, files[0].deletions), ("src/a.rs", ChangeKind::Modified, 1, 1));
        assert_eq!((files[1].path.as_str(), files[1].kind, files[1].additions, files[1].deletions), ("new.txt", ChangeKind::Added, 2, 0));
        assert!(files[0].text.starts_with("diff --git a/src/a.rs"));
        assert!(!files[0].text.contains("new.txt"));
    }

    #[test]
    fn split_files_handles_delete_rename_and_marker_only_diffs() {
        let d = "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/x.txt b/y.txt\nsimilarity index 100%\nrename from x.txt\nrename to y.txt\n";
        let files = split_files(d);
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].path.as_str(), files[0].kind, files[0].deletions), ("old.txt", ChangeKind::Deleted, 1));
        assert_eq!((files[1].path.as_str(), files[1].old_path.as_deref(), files[1].kind), ("y.txt", Some("x.txt"), ChangeKind::Modified));
        let plain = "--- a/p.txt\n+++ b/p.txt\n@@ -1 +1 @@\n--- not a header\n+++ also body\n--- q.txt\n+++ q.txt\n@@ -1 +1 @@\n-a\n+b\n";
        // 本文の `--- ` `+++ ` が続くだけの行は、直後が `+++ ` のとき区切りと見なす既知の限界（git形式を推奨）。
        assert!(!split_files(plain).is_empty());
        assert!(split_files("").is_empty());
        assert!(split_files("   \n").is_empty());
    }

    #[test]
    fn counts_ignore_headers_and_unified_detection_distinguishes_raw_content() {
        assert_eq!(count_changes("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n"), (1, 1));
        assert!(looks_like_unified("@@ -1 +1 @@\n-a\n+b\n"));
        assert!(!looks_like_unified("line one\nline two\n"));
        assert_eq!(count_for(ChangeKind::Added, "x\ny\n"), (2, 0));
        assert_eq!(count_for(ChangeKind::Deleted, "x\n"), (0, 1));
    }

    #[test]
    fn parse_hunks_reads_by_line_counts_and_marks_missing_newline() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- body line starting with dashes\n+++ body line starting with pluses\n@@ -9 +9 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        let h = parse_hunks(d).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].lines.len(), 2);
        assert_eq!(h[0].lines[0].text, "-- body line starting with dashes");
        assert!(h[1].lines.iter().all(|l| l.no_eol));
        assert!(parse_hunks("no hunks here").is_err());
        assert!(parse_hunks("@@ -1,3 +1,3 @@\n a\n").is_err(), "truncated");
    }

    #[test]
    fn apply_reverse_restores_old_content() {
        let new = "fn a() {\n    2\n}\nend\n";
        let h = parse_hunks("@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\n").unwrap();
        assert_eq!(apply_reverse(new, &h).unwrap(), "fn a() {\n    1\n}\nend\n");
    }

    #[test]
    fn apply_reverse_handles_multiple_hunks_insertions_and_deletions() {
        // 旧: a b c d e f g → 新: a c d e F g h（b削除、f→F、h追加）
        let new = "a\nc\nd\ne\nF\ng\nh\n";
        let d = "@@ -1,3 +1,2 @@\n a\n-b\n c\n@@ -5,3 +4,4 @@\n e\n-f\n+F\n g\n+h\n";
        let h = parse_hunks(d).unwrap();
        assert_eq!(apply_reverse(new, &h).unwrap(), "a\nb\nc\nd\ne\nf\ng\n");
    }

    #[test]
    fn apply_reverse_blocks_on_context_mismatch_and_wrong_position() {
        let h = parse_hunks("@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\n").unwrap();
        assert!(apply_reverse("fn a() {\n    3\n}\n", &h).is_err(), "added line was edited");
        assert!(apply_reverse("x\nfn a() {\n    2\n}\n", &h).is_err(), "shifted: no fuzz");
        assert!(apply_reverse("", &h).is_err());
    }

    #[test]
    fn apply_reverse_keeps_crlf_and_missing_final_newline() {
        let h = parse_hunks("@@ -1,2 +1,2 @@\n keep\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n").unwrap();
        assert_eq!(apply_reverse("keep\r\nnew", &h).unwrap(), "keep\r\nold");
        // 新側が末尾に改行を持つのに、差分は持たないと言っている → 一致しない
        assert!(apply_reverse("keep\r\nnew\r\n", &h).is_err());
        let crlf = parse_hunks("@@ -1,2 +1,2 @@\r\n keep\r\n-old\r\n+new\r\n").unwrap();
        assert_eq!(apply_reverse("keep\r\nnew\r\n", &crlf).unwrap(), "keep\r\nold\r\n");
    }

    #[test]
    fn apply_reverse_of_pure_insertion_and_pure_deletion() {
        let ins = parse_hunks("@@ -2,0 +3,1 @@\n+x\n").unwrap();
        assert_eq!(apply_reverse("a\nb\nx\nc\n", &ins).unwrap(), "a\nb\nc\n");
        let del = parse_hunks("@@ -1,1 +0,0 @@\n-gone\n").unwrap();
        assert_eq!(apply_reverse("rest\n", &del).unwrap(), "gone\nrest\n");
    }

    #[test]
    fn status_v2_is_parsed_with_renames_untracked_and_unmerged() {
        let out = "# branch.oid abc\0" .to_owned() + "1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0" + "2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs\0old.rs\0" + "1 A. N... 000000 100644 100644 000 bbb added.rs\0" + "1 .D N... 100644 100644 000000 aaa 000 gone.rs\0" + "? untracked file.txt\0" + "! ignored.log\0" + "u UU N... 1 2 3 4 aaa bbb ccc conflict.rs\0";
        let v = parse_status_v2(&out);
        assert_eq!(v.len(), 6);
        assert_eq!((v[0].path.as_str(), v[0].kind()), ("src/a b.rs", ChangeKind::Modified));
        assert_eq!((v[1].path.as_str(), v[1].old_path.as_deref()), ("new name.rs", Some("old.rs")));
        assert_eq!(v[2].kind(), ChangeKind::Added);
        assert_eq!(v[3].kind(), ChangeKind::Deleted);
        assert!(v[4].untracked && v[4].path == "untracked file.txt" && v[4].kind() == ChangeKind::Added);
        assert!(v[5].unmerged);
        assert!(parse_status_v2("").is_empty());
    }

    #[test]
    fn deleted_content_from_unified_or_raw() {
        assert_eq!(deleted_content("@@ -1,2 +0,0 @@\n-a\n-b\n").unwrap(), "a\nb\n");
        assert_eq!(deleted_content("raw a\nraw b\n").unwrap(), "raw a\nraw b\n");
    }
}
