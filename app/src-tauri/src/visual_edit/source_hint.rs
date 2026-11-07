//! 利用者がプレビューで選んだ要素に対応するソース位置の目安（行番号付きの抜粋）。読み取り専用。
//! 選択の文字列はページ由来で信頼しない。検索キーと引用にだけ使い、指示としては扱わない。
use super::{FileAccess, FileScope, ProjectRoot, RelativePath, VisualResult};
use serde::Deserialize;
use std::io::Read;

const MAX_SELECTIONS: usize = 12;
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 6000;
const CONTEXT_LINES: usize = 20;
const CANDIDATES_PER_SELECTION: usize = 2;
const MAX_LINE_CHARS: usize = 240;
const TOC_TOP: usize = 20;
const HEADER: &str = "ソースの該当箇所（目安）";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HintSelection {
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub text: String,
}

/// 編集対象HTML（スコープで Edit のもの）だけを読む。範囲外・存在しない・大きすぎる・UTF-8でない場合は None。
pub(crate) fn load_edit_source(root: &ProjectRoot, scope: &FileScope, html_path: &str) -> VisualResult<Option<String>> {
    let rel = RelativePath::parse(html_path)?;
    if !scope.files().any(|(p, a)| *p == rel && a == FileAccess::Edit) {
        return Ok(None);
    }
    let Some(held) = root.open_relative(&rel)? else { return Ok(None) };
    if held.file.metadata()?.len() > MAX_SOURCE_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    (&held.file).take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES || bytes.contains(&0) {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// セレクタから id と class の名前を取り出す（`#main`、`.card.wide`。目安なので完全な構文解析はしない）。
fn selector_names(selector: &str) -> (Vec<String>, Vec<String>) {
    let (mut ids, mut classes) = (Vec::new(), Vec::new());
    let chars: Vec<char> = selector.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' || c == '.' {
            let mut j = i + 1;
            while j < chars.len() && is_ident(chars[j]) {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            // 数字で始まる名前は属性値の一部（`:nth-child(2)` や `0.5` など）の可能性が高いので使わない。
            if !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) {
                let list = if c == '#' { &mut ids } else { &mut classes };
                if !list.contains(&name) && list.len() < 4 {
                    list.push(name);
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    (ids, classes)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 行の中の ` name="value"` / ` name='value'` の値を順に返す（複数行にまたがる属性は対象外）。
fn attr_values<'a>(line: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let key = format!("{name}=");
    let mut from = 0;
    while let Some(pos) = line[from..].find(&key) {
        let at = from + pos;
        from = at + key.len();
        let boundary = line[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || c == '<' || c == '"' || c == '\'');
        if !boundary {
            continue;
        }
        let rest = &line[from..];
        let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
        if let Some(end) = rest[1..].find(q) {
            out.push(&rest[1..1 + end]);
        }
    }
    out
}

fn line_has_id(line: &str, id: &str) -> bool {
    attr_values(line, "id").iter().any(|v| v.trim() == id)
}

fn line_has_class(line: &str, class: &str) -> bool {
    attr_values(line, "class").iter().any(|v| v.split_whitespace().any(|c| c == class))
}

#[derive(Debug)]
struct Candidate {
    line: usize,
    /// 長い行を切り詰めるとき、この文字列の位置を中心にする。
    needle: String,
}

/// 昇順の `sorted` に、`i` から `dist` 行以内の値があるか（二分探索）。
fn near_any(sorted: &[usize], i: usize, dist: usize) -> bool {
    let at = sorted.partition_point(|v| *v < i.saturating_sub(dist));
    sorted.get(at).is_some_and(|v| *v <= i + dist)
}

fn find_candidates(lines: &[&str], norm: &[String], sel: &HintSelection) -> Vec<Candidate> {
    let (ids, classes) = selector_names(&sel.selector);
    let fragment: String = collapse(&sel.text).chars().take(40).collect();
    let fragment = if fragment.chars().count() >= 2 { fragment } else { String::new() };
    let tag = sel.tag.trim().to_ascii_lowercase();
    let tag_ok = !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let open_tag = format!("<{tag}");
    let mut out: Vec<Candidate> = Vec::new();
    let push = |out: &mut Vec<Candidate>, line: usize, needle: &str| {
        // 採用するのは先頭の数件だけなので、満杯になったら以後は追加しない（候補が多い巨大ファイルで二乗にならないように）。
        if out.len() < CANDIDATES_PER_SELECTION && !out.iter().any(|c| c.line == line) {
            out.push(Candidate { line, needle: needle.to_string() });
        }
    };
    for id in &ids {
        for (i, l) in lines.iter().enumerate() {
            if line_has_id(l, id) {
                push(&mut out, i, id);
                if out.len() >= CANDIDATES_PER_SELECTION {
                    return out;
                }
            }
        }
    }
    let text_lines: Vec<usize> = if fragment.is_empty() { Vec::new() } else { (0..lines.len()).filter(|i| norm[*i].contains(&fragment)).collect() };
    let first_word = fragment.split(' ').next().unwrap_or("").to_string();
    // タグも一致する行を先に、次に class と本文が近い行、最後に本文だけ・class だけの行。
    if tag_ok {
        for i in &text_lines {
            if lines[*i].to_ascii_lowercase().contains(&open_tag) {
                push(&mut out, *i, &first_word);
                if out.len() >= CANDIDATES_PER_SELECTION {
                    return out;
                }
            }
        }
    }
    for class in &classes {
        for (i, l) in lines.iter().enumerate() {
            if line_has_class(l, class) && near_any(&text_lines, i, 3) {
                push(&mut out, i, class);
                if out.len() >= CANDIDATES_PER_SELECTION {
                    return out;
                }
            }
        }
    }
    for i in &text_lines {
        push(&mut out, *i, &first_word);
        if out.len() >= CANDIDATES_PER_SELECTION {
            return out;
        }
    }
    for class in &classes {
        for (i, l) in lines.iter().enumerate() {
            if line_has_class(l, class) {
                push(&mut out, i, class);
                if out.len() >= CANDIDATES_PER_SELECTION {
                    return out;
                }
            }
        }
    }
    out
}

fn clip_line(line: &str, needle: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= MAX_LINE_CHARS {
        return line.trim_end().to_string();
    }
    // 圧縮された1行HTMLなどは、探した位置の前後だけを出す。
    let center = if needle.is_empty() { 0 } else { line.find(needle).map(|b| line[..b].chars().count()).unwrap_or(0) };
    let start = center.saturating_sub(MAX_LINE_CHARS / 3).min(chars.len() - MAX_LINE_CHARS);
    let body: String = chars[start..start + MAX_LINE_CHARS].iter().collect();
    format!("{}{body}{}", if start > 0 { "…" } else { "" }, if start + MAX_LINE_CHARS < chars.len() { "…" } else { "" })
}

/// 引用する選択の文字列は1行にまとめて短くする（指示としては扱わない）。
fn quote(s: &str, max: usize) -> String {
    let c = collapse(s);
    if c.chars().count() > max { format!("{}…", c.chars().take(max).collect::<String>()) } else { c }
}

fn table_of_contents(lines: &[&str]) -> String {
    let mut out = format!("目次: {}行\n", lines.len());
    for (open, close, name) in [("<style", "</style>", "style"), ("<script", "</script>", "script")] {
        let mut ranges: Vec<String> = Vec::new();
        let mut start: Option<usize> = None;
        for (i, l) in lines.iter().enumerate() {
            let lower = l.to_ascii_lowercase();
            if start.is_none() && lower.contains(open) {
                start = Some(i);
            }
            if let Some(s) = start {
                if lower.contains(close) {
                    ranges.push(if s == i { format!("{}", s + 1) } else { format!("{}-{}", s + 1, i + 1) });
                    start = None;
                }
            }
        }
        if let Some(s) = start {
            ranges.push(format!("{}-", s + 1));
        }
        if !ranges.is_empty() {
            out.push_str(&format!("{name}: 行 {}\n", ranges.join(", ")));
        }
    }
    // 出現順を保つ Vec と、重複確認用の HashSet / HashMap（添字）を併用する（線形探索で二乗にならないように）。
    let mut ids: Vec<String> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut classes: Vec<(String, usize)> = Vec::new();
    let mut class_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for l in lines {
        for v in attr_values(l, "id") {
            let v = v.trim().to_string();
            if !v.is_empty() && seen_ids.insert(v.clone()) {
                ids.push(v);
            }
        }
        for v in attr_values(l, "class") {
            for c in v.split_whitespace() {
                match class_index.get(c) {
                    Some(k) => classes[*k].1 += 1,
                    None => {
                        class_index.insert(c.to_string(), classes.len());
                        classes.push((c.to_string(), 1));
                    }
                }
            }
        }
    }
    classes.sort_by(|a, b| b.1.cmp(&a.1)); // 安定ソート: 同数なら出現順
    if !ids.is_empty() {
        out.push_str(&format!("id: {}\n", ids.iter().take(TOC_TOP).map(|v| quote(v, 60)).collect::<Vec<_>>().join(", ")));
    }
    if !classes.is_empty() {
        out.push_str(&format!("class: {}\n", classes.iter().take(TOC_TOP).map(|(v, n)| format!("{} x{n}", quote(v, 60))).collect::<Vec<_>>().join(", ")));
    }
    out
}

fn truncate_chars(mut s: String, max: usize) -> String {
    if s.chars().count() > max {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        s.truncate(end);
        s.push_str("\n…（長いため省略）\n");
    }
    s
}

/// ソース全文と選択から、ヘッダー付きの抜粋を作る。該当がなければ空文字。
pub(crate) fn build_hint(source: &str, selection: &[HintSelection]) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut body = String::new();
    if selection.is_empty() {
        body = table_of_contents(&lines);
    } else {
        let norm: Vec<String> = lines.iter().map(|l| collapse(l)).collect();
        for (n, sel) in selection.iter().take(MAX_SELECTIONS).enumerate() {
            let found = find_candidates(&lines, &norm, sel);
            if found.is_empty() {
                continue;
            }
            body.push_str(&format!("## 選択{}: {} <{}>\n", n + 1, quote(&sel.selector, 120), quote(&sel.tag, 24)));
            for c in found.iter().take(CANDIDATES_PER_SELECTION) {
                let from = c.line.saturating_sub(CONTEXT_LINES);
                let to = (c.line + CONTEXT_LINES).min(lines.len() - 1);
                body.push_str(&format!("--- 候補: {}行目付近 ---\n", c.line + 1));
                for (i, l) in lines.iter().enumerate().take(to + 1).skip(from) {
                    let needle = if i == c.line { c.needle.as_str() } else { "" };
                    body.push_str(&format!("{}: {}\n", i + 1, clip_line(l, needle)));
                }
            }
        }
    }
    if body.is_empty() {
        return String::new();
    }
    truncate_chars(format!("{HEADER}\n{body}"), MAX_OUTPUT_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual_edit::SelectedFile;

    fn sel(selector: &str, tag: &str, text: &str) -> HintSelection {
        HintSelection { selector: selector.into(), tag: tag.into(), text: text.into() }
    }
    fn sample() -> String {
        let mut s = String::from("<!doctype html>\n<html>\n<head>\n<style>\n.card { color: red; }\n</style>\n</head>\n<body>\n");
        for i in 0..60 {
            s.push_str(&format!("<p class=\"row\">row {i}</p>\n"));
        }
        s.push_str("<div id=\"hero\">\n  <h1 class=\"title wide\">ようこそ   AgentDock へ</h1>\n</div>\n<script>\nlet a = 1;\n</script>\n</body>\n</html>\n");
        s
    }

    #[test]
    fn id_match_returns_numbered_lines_with_header() {
        let out = build_hint(&sample(), &[sel("div#hero", "div", "")]);
        assert!(out.starts_with("ソースの該当箇所（目安）\n"));
        assert!(out.contains("<div id=\"hero\">"));
        assert!(out.lines().any(|l| l.starts_with("69: <div id=\"hero\">")), "{out}");
    }

    #[test]
    fn class_match_finds_the_line() {
        let out = build_hint(&sample(), &[sel("h1.title", "h1", "")]);
        assert!(out.contains("<h1 class=\"title wide\">"), "{out}");
        let out = build_hint(&sample(), &[sel(".wide", "", "")]);
        assert!(out.contains("<h1 class=\"title wide\">"), "{out}");
    }

    #[test]
    fn text_match_ignores_whitespace_differences() {
        let out = build_hint(&sample(), &[sel("main > *", "h1", "ようこそ AgentDock")]);
        assert!(out.contains("ようこそ   AgentDock へ"), "{out}");
    }

    #[test]
    fn empty_selection_returns_table_of_contents() {
        let out = build_hint(&sample(), &[]);
        assert!(out.starts_with("ソースの該当箇所（目安）\n目次: "), "{out}");
        assert!(out.contains("style: 行 4-6"), "{out}");
        assert!(out.contains("script: 行 "), "{out}");
        assert!(out.contains("id: hero"), "{out}");
        assert!(out.contains("row x60"), "{out}");
    }

    #[test]
    fn no_match_returns_empty() {
        assert_eq!(build_hint(&sample(), &[sel("#nothing", "section", "存在しない文章")]), "");
    }

    #[test]
    fn output_is_truncated_to_the_limit() {
        let many: Vec<HintSelection> = (0..12).map(|i| sel(".row", "p", &format!("row {i}"))).collect();
        let out = build_hint(&sample(), &many);
        assert!(out.chars().count() <= MAX_OUTPUT_CHARS + 40, "{}", out.chars().count());
        assert!(out.contains("省略"));
    }

    #[test]
    fn long_single_line_is_clipped_around_the_match() {
        let src = format!("<div>{}<span id=\"target\">x</span>{}</div>", "a".repeat(3000), "b".repeat(3000));
        let out = build_hint(&src, &[sel("#target", "span", "")]);
        assert!(out.contains("id=\"target\"") && out.chars().count() < 600, "{}", out.chars().count());
    }

    #[test]
    fn large_file_with_short_text_and_common_class_stays_fast() {
        let mut src = String::new();
        while src.len() < 2 * 1024 * 1024 - 64 {
            src.push_str("<li class=\"item common\">ab</li>
");
        }
        let many: Vec<HintSelection> = (0..12).map(|_| sel(".common.item", "li", "ab")).collect();
        let start = std::time::Instant::now();
        let out = build_hint(&src, &many);
        let toc = build_hint(&src, &[]);
        assert!(out.starts_with("ソースの該当箇所（目安）") && toc.contains("class: "));
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    }

    #[test]
    fn only_the_edit_html_inside_the_scope_is_read() {
        let base = std::env::temp_dir().join(format!("agentdock-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("index.html"), "<p id=\"a\">x</p>").unwrap();
        std::fs::write(base.join("data.json"), "{}").unwrap();
        std::fs::write(base.join("other.html"), "<p>y</p>").unwrap();
        std::fs::write(base.join("big.html"), vec![b'a'; (MAX_SOURCE_BYTES + 1) as usize]).unwrap();
        std::fs::write(base.join("bin.html"), [0xff_u8, 0xfe, 0x00]).unwrap();
        let root = ProjectRoot::open(&base).unwrap();
        let sf = |p: &str, a| SelectedFile { path: p.into(), access: a };
        let scope = FileScope::new(vec![
            sf("index.html", FileAccess::Edit),
            sf("data.json", FileAccess::ReadOnly),
            sf("big.html", FileAccess::Edit),
            sf("bin.html", FileAccess::Edit),
        ])
        .unwrap();
        let ok = load_edit_source(&root, &scope, "index.html").unwrap();
        let read_only = load_edit_source(&root, &scope, "data.json").unwrap();
        let unlisted = load_edit_source(&root, &scope, "other.html").unwrap();
        let escaped = load_edit_source(&root, &scope, "../index.html");
        let big = load_edit_source(&root, &scope, "big.html").unwrap();
        let bin = load_edit_source(&root, &scope, "bin.html").unwrap();
        drop(root);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(ok.as_deref(), Some("<p id=\"a\">x</p>"));
        assert!(read_only.is_none() && unlisted.is_none() && big.is_none() && bin.is_none());
        assert!(escaped.is_err());
    }
}
