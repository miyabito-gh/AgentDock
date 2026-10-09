//! 統一diffの分割・行数集計と、`git status --porcelain=v2` の解析（P3-1、P3B）。純粋ロジック。
//!
//! - [`split_files`]: 統一diff（`diff --git` 区切り、なければ `---`/`+++` 区切り）をファイル別に分ける。
//! - [`parse_status_v2`]: Git上の現在の差分の一覧に使う。差分の逆適用はP3Bで廃止した（戻す処理は `rules::baseline`）。

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
    /// Gitがバイナリ扱いにした（`Binary files … differ`／`GIT binary patch`）。行数は数えられない（0ではない）。
    pub binary: bool,
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
    let binary = first_hunk == lines.len() && lines.iter().any(|l| (l.starts_with("Binary files ") && l.ends_with(" differ")) || l.starts_with("GIT binary patch"));
    FileDiff { path, old_path, kind, additions, deletions, binary, text: text.to_string() }
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

/// `-z` 区切りの出力にある無視ファイル（`! ` の行）の数。ディレクトリごと無視されたものは1件と数える。
pub fn count_ignored_v2(out: &str) -> u32 {
    out.split('\0').filter(|s| s.starts_with("! ")).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_FILES: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..333\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";

    #[test]
    fn binary_files_are_flagged_not_counted_as_zero() {
        let d = "diff --git a/img.png b/img.png\nindex 111..222 100644\nBinary files a/img.png and b/img.png differ\ndiff --git a/a.txt b/a.txt\nindex 1..2 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let files = split_files(d);
        assert_eq!(files.len(), 2);
        assert!(files[0].binary);
        assert!(!files[1].binary);
        assert_eq!((files[1].additions, files[1].deletions), (1, 1));
        let p = "diff --git a/b.bin b/b.bin\nindex 1..2 100644\nGIT binary patch\nliteral 0\nHcmV?d00001\n";
        assert!(split_files(p)[0].binary);
    }

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
    fn counts_ignore_headers() {
        assert_eq!(count_changes("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n"), (1, 1));
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
        // 無視ファイルは変更に数えず、別に数える。
        assert_eq!(count_ignored_v2(&out), 1);
        assert_eq!(count_ignored_v2("! a.log\0! build/\0? x\01 .M N... 100644 100644 100644 a b !c.rs\0"), 2);
        assert_eq!(count_ignored_v2(""), 0);
    }
}
