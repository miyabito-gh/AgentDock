//! 会話内のファイルパスの扱い（読取りのメタデータ確認だけ。内容は読まない・書かない）。
//! ここは純粋な文字列処理（正規化・基準フォルダの解決・相対パス化）と、存在確認の1件分。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend::model::ChatKey;

/// 1回に確認するパスの上限。
pub const MAX_PATHS: usize = 50;
/// 長すぎるパスは確認しない（未確認として返す）。
pub const MAX_PATH_CHARS: usize = 400;
/// 存在確認全体の待ち時間（ネットワーク・UNCで遅いときは未確認にする）。
pub const STAT_TIMEOUT_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StatPathsArgs {
    pub chat: ChatKey,
    /// 確認するパス（絶対、または作業フォルダ基準の相対）。上限 `MAX_PATHS` 件を超える分は未確認で返す。
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OpenPathArgs {
    pub chat: ChatKey,
    pub path: String,
    /// 開く前の確認（作業フォルダ外・実行形式）をユーザーが了承したとき true。
    #[serde(default)]
    pub risk_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevealPathArgs {
    pub chat: ChatKey,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum PathKind {
    File,
    Folder,
    /// 存在しないと確認できた。
    Missing,
    /// 確認できなかった（遅い・権限・長すぎる・基準が不明など）。存在しないとは限らない。
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PathInfo {
    /// 問い合わせたパス（そのまま）。
    pub input: String,
    pub kind: PathKind,
    /// 正規化した絶対パス（存在を確認できたときだけ）。
    pub absolute: Option<String>,
    /// 作業フォルダ基準の相対パス（`/` 区切り）。作業フォルダの外・基準不明なら None。
    pub relative: Option<String>,
}

pub fn is_drive_abs(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// `\\server\share`（`\\?\` の長いパス形式は含めない）。
pub fn is_unc(p: &str) -> bool {
    let q = p.replace('/', "\\");
    q.starts_with("\\\\") && !q.starts_with("\\\\?\\") && !q.starts_with("\\\\.\\")
}

/// ネットワーク上の場所（`\\server\share`・`//server/`・`\\?\UNC\`・`\\.\` のデバイス名前空間）。
/// 触るとSMB接続でWindowsの認証情報が相手に送られうるので、自動の存在確認では一切触らない。
pub fn is_network_path(p: &str) -> bool {
    let q = p.trim().replace('/', "\\");
    if let Some(rest) = q.strip_prefix("\\\\?\\") {
        return rest.len() >= 4 && rest.is_char_boundary(4) && rest[..4].eq_ignore_ascii_case("UNC\\");
    }
    q.starts_with("\\\\")
}

pub fn is_absolute(p: &str) -> bool {
    is_drive_abs(p) || is_unc(p)
}

/// `.`・`..` を字句だけで畳み、区切りを `\` にそろえる（実在・リンクは見ない）。ルートより上へは出ない。
pub fn lexical_normalize(p: &str) -> String {
    let q = p.replace('/', "\\");
    let (prefix, rest) = if is_drive_abs(&q) {
        (q[..2].to_string(), &q[2..])
    } else if q.starts_with("\\\\") {
        // \\server\share までをルートとする
        let mut it = q[2..].splitn(3, '\\');
        let (s, sh, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
        let root = format!("\\\\{s}\\{sh}");
        let mut parts: Vec<&str> = Vec::new();
        for c in rest.split('\\') {
            match c {
                "" | "." => {}
                ".." => { parts.pop(); }
                c => parts.push(c),
            }
        }
        return if parts.is_empty() { root } else { format!("{root}\\{}", parts.join("\\")) };
    } else {
        (String::new(), q.as_str())
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in rest.split('\\') {
        match c {
            "" | "." => {}
            ".." => { parts.pop(); }
            c => parts.push(c),
        }
    }
    if prefix.is_empty() {
        return parts.join("\\");
    }
    format!("{prefix}\\{}", parts.join("\\"))
}

/// 絶対パスにする。相対パスは作業フォルダ基準（基準がなければ None）。
pub fn resolve(input: &str, cwd: Option<&str>) -> Option<String> {
    let input = input.trim();
    if input.is_empty() || input.contains('\0') {
        return None;
    }
    if is_absolute(input) {
        return Some(lexical_normalize(input));
    }
    // `\foo` `/foo`（ドライブなしのルート相対）は曖昧なので扱わない。
    if input.starts_with('\\') || input.starts_with('/') || input.contains(':') {
        return None;
    }
    let root = cwd.filter(|c| is_absolute(c))?;
    Some(lexical_normalize(&format!("{}\\{}", root.trim_end_matches(['\\', '/']), input)))
}

/// `\\?\C:\x` → `C:\x`、`\\?\UNC\s\x` → `\\s\x`。
pub fn strip_verbatim(p: &str) -> String {
    if let Some(r) = p.strip_prefix("\\\\?\\UNC\\") {
        return format!("\\\\{r}");
    }
    p.strip_prefix("\\\\?\\").unwrap_or(p).to_string()
}

/// `abs` が `root` の中なら `/` 区切りの相対パス。同じ場所・外・`..` を含むときは None（大文字小文字は区別しない）。
pub fn relative_to(abs: &str, root: &str) -> Option<String> {
    let (a, r) = (lexical_normalize(&strip_verbatim(abs)), lexical_normalize(&strip_verbatim(root)));
    if r.is_empty() || a.split('\\').any(|c| c == "..") {
        return None;
    }
    let (al, rl) = (a.to_lowercase(), r.to_lowercase());
    let pre = format!("{}\\", rl.trim_end_matches('\\'));
    if !al.starts_with(&pre) {
        return None;
    }
    // 小文字化で長さが変わる文字があっても、元の文字列で切るため文字数で数える。
    let skip = pre.chars().count();
    let rel: String = a.chars().skip(skip).collect::<String>().replace('\\', "/");
    if rel.is_empty() { None } else { Some(rel) }
}

/// 存在確認1件（ブロッキング）。メタデータだけ読む。
pub fn stat_blocking(abs: &str) -> (PathKind, Option<String>) {
    let p = Path::new(abs);
    match std::fs::metadata(p) {
        Ok(m) => {
            let canon = std::fs::canonicalize(p).map(|c| strip_verbatim(&c.to_string_lossy())).unwrap_or_else(|_| abs.to_string());
            (if m.is_dir() { PathKind::Folder } else { PathKind::File }, Some(canon))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (PathKind::Missing, None),
        Err(_) => (PathKind::Unconfirmed, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_dots_and_slashes() {
        assert_eq!(lexical_normalize("C:/a/./b/../c.txt"), "C:\\a\\c.txt");
        assert_eq!(lexical_normalize("C:\\a\\..\\..\\x"), "C:\\x");
        assert_eq!(lexical_normalize("\\\\srv\\sh\\a\\..\\b"), "\\\\srv\\sh\\b");
    }

    #[test]
    fn resolve_relative_and_absolute() {
        assert_eq!(resolve("src/foo.rs", Some("C:\\work\\p")).as_deref(), Some("C:\\work\\p\\src\\foo.rs"));
        assert_eq!(resolve("./a.md", Some("C:\\work\\p\\")).as_deref(), Some("C:\\work\\p\\a.md"));
        assert_eq!(resolve("D:/x/y.txt", None).as_deref(), Some("D:\\x\\y.txt"));
        assert_eq!(resolve("src/foo.rs", None), None);
        assert_eq!(resolve("/etc/passwd", Some("C:\\w")), None);
        assert_eq!(resolve("E:foo", Some("C:\\w")), None);
    }

    #[test]
    fn relative_inside_only() {
        assert_eq!(relative_to("C:\\Work\\p\\src\\a.rs", "c:\\work\\p").as_deref(), Some("src/a.rs"));
        assert_eq!(relative_to("C:\\work\\p", "C:\\work\\p"), None);
        assert_eq!(relative_to("C:\\work\\pp\\a.rs", "C:\\work\\p"), None);
        assert_eq!(relative_to("\\\\?\\C:\\work\\p\\a.rs", "C:\\work\\p").as_deref(), Some("a.rs"));
    }

    #[test]
    fn network_path_detection() {
        for p in ["\\\\host\\share\\a", "//host/share/a", "\\\\?\\UNC\\host\\share", "\\\\.\\pipe\\x", "  \\\\host\\s"] {
            assert!(is_network_path(p), "{p}");
        }
        for p in ["\\\\?\\C:\\x", "C:\\x", "C:/x", "src/a.rs", "./a"] {
            assert!(!is_network_path(p), "{p}");
        }
    }

    #[test]
    fn unc_detection_and_verbatim() {
        assert!(is_unc("//srv/share/x"));
        assert!(!is_unc("\\\\?\\C:\\x"));
        assert_eq!(strip_verbatim("\\\\?\\UNC\\srv\\sh\\a"), "\\\\srv\\sh\\a");
    }
}
