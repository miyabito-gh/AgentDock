//! Markdown 文書の読み書きの純粋関数: 文字コード（UTF-8・BOM）と改行（LF/CRLF）の判定と復元、画像参照の解決。
//! ファイルの入出力・パスの検証は `host/visual_markdown.rs` と `direct.rs`（リンク拒否・一時ファイル＋置換）が行う。
use serde::Serialize;

/// 編集欄で開ける最大サイズ。
pub const MAX_DOC_BYTES: usize = 4 * 1024 * 1024;
/// プレビューへ渡す画像の最大サイズ。
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// 1つの文書から拾う画像参照の上限。
const MAX_REFS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    Lf,
    Crlf,
}
impl Eol {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lf" => Some(Self::Lf),
            "crlf" => Some(Self::Crlf),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    /// 改行を LF にそろえた本文（BOM なし）。
    pub text: String,
    pub bom: bool,
    /// 多いほうの改行。保存時に復元する。
    pub eol: Eol,
    /// LF と CRLF（または単独の CR）が混在していた。保存すると `eol` にそろう。
    pub mixed: bool,
}

/// バイト列を読む。壊れた UTF-8・NUL を含むものは拒否する（開かない・上書きしない）。
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    if bytes.len() > MAX_DOC_BYTES {
        return Err("ファイルが大きすぎるため、編集欄では開けません（4MBまで）。".into());
    }
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    if body.contains(&0) {
        return Err("テキストではないデータ（NUL）が含まれているため、開きません。".into());
    }
    let text = std::str::from_utf8(body).map_err(|_| "UTF-8として読めない文字が含まれているため、開きません（保存すると文字化けするおそれがあるため）。".to_string())?;
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };
    let mixed = (crlf > 0 && lf > 0) || cr > 0;
    let text = if crlf + cr > 0 { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_string() };
    Ok(Decoded { text, bom, eol, mixed })
}

/// 本文（LF）を、判定した BOM・改行で書き出すバイト列にする。
pub fn encode(text: &str, bom: bool, eol: Eol) -> Vec<u8> {
    let lf = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_string() };
    let body = if eol == Eol::Crlf { lf.replace('\n', "\r\n") } else { lf };
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 文書内の相対参照を、ルートからの相対パスにする。外部URL・絶対パス・ルートの外へ出るものは None。
pub fn resolve_ref(doc_rel: &str, href: &str) -> Option<String> {
    let mut v = href.trim();
    if let Some(inner) = v.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        v = inner;
    }
    if v.is_empty() || v.starts_with(['#', '/', '\\']) || v.contains('\0') {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("data:") || lower.starts_with("mailto:") || lower.starts_with("javascript:") {
        return None;
    }
    let b = v.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return None;
    }
    let cut = v.find(['?', '#']).unwrap_or(v.len());
    let path = percent_decode(&v[..cut])?.replace('\\', "/");
    let mut parts: Vec<&str> = match doc_rel.rfind('/') {
        Some(at) => doc_rel[..at].split('/').collect(),
        None => Vec::new(),
    };
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// 画像として扱う拡張子と MIME。
pub fn image_mime(rel: &str) -> Option<&'static str> {
    let ext = rel.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// 本文から画像らしい参照先（`](…)` の括弧内と `src="…"`）を拾う（重複は除く）。
pub fn extract_refs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        if out.len() < MAX_REFS && !s.is_empty() && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        rest = &rest[at + 2..];
        let inner = rest.trim_start_matches([' ', '\t']);
        if let Some(angle) = inner.strip_prefix('<') {
            if let Some(end) = angle.find('>') {
                push(&angle[..end]);
            }
        } else {
            let end = inner.find(|c: char| c.is_whitespace() || c == ')').unwrap_or(inner.len());
            push(&inner[..end]);
        }
    }
    for quote in ['"', '\''] {
        let needle = format!("src={quote}");
        let mut rest = text;
        while let Some(at) = rest.find(&needle) {
            rest = &rest[at + needle.len()..];
            if let Some(end) = rest.find(quote) {
                push(&rest[..end]);
            }
        }
    }
    out
}

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() / 3 * 4 + 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lf_without_bom() {
        let d = decode("a\nb\n".as_bytes()).unwrap();
        assert_eq!((d.text.as_str(), d.bom, d.eol, d.mixed), ("a\nb\n", false, Eol::Lf, false));
    }
    #[test]
    fn bom_and_crlf_are_detected_and_restored() {
        let src = b"\xEF\xBB\xBF# t\r\nbody\r\n";
        let d = decode(src).unwrap();
        assert_eq!((d.text.as_str(), d.bom, d.eol, d.mixed), ("# t\nbody\n", true, Eol::Crlf, false));
        assert_eq!(encode(&d.text, d.bom, d.eol), src);
    }
    #[test]
    fn mixed_eol_follows_the_majority_and_is_flagged() {
        let d = decode(b"a\r\nb\r\nc\nd").unwrap();
        assert_eq!((d.eol, d.mixed, d.text.as_str()), (Eol::Crlf, true, "a\nb\nc\nd"));
        let d = decode(b"a\nb\nc\r\nd").unwrap();
        assert_eq!((d.eol, d.mixed), (Eol::Lf, true));
    }
    #[test]
    fn lone_cr_counts_as_mixed() {
        let d = decode(b"a\rb\n").unwrap();
        assert!(d.mixed);
        assert_eq!(d.text, "a\nb\n");
    }
    #[test]
    fn invalid_utf8_and_nul_are_refused() {
        assert!(decode(&[0x61, 0xFF, 0xFE, 0x62]).unwrap_err().contains("UTF-8"));
        assert!(decode(b"a\0b").is_err());
        assert!(decode(&vec![b'a'; MAX_DOC_BYTES + 1]).is_err());
    }
    #[test]
    fn empty_file_is_lf_no_bom() {
        let d = decode(b"").unwrap();
        assert_eq!((d.text.as_str(), d.bom, d.eol), ("", false, Eol::Lf));
    }
    #[test]
    fn encode_normalizes_stray_cr() {
        assert_eq!(encode("a\r\nb\rc\n", false, Eol::Lf), b"a\nb\nc\n");
        assert_eq!(encode("a\nb", true, Eol::Crlf), b"\xEF\xBB\xBFa\r\nb");
    }
    #[test]
    fn resolve_ref_stays_inside_the_root() {
        assert_eq!(resolve_ref("doc/a.md", "img/x.png").as_deref(), Some("doc/img/x.png"));
        assert_eq!(resolve_ref("a.md", "./x%20y.png?v=1#h").as_deref(), Some("x y.png"));
        assert_eq!(resolve_ref("doc/a.md", "../x.png").as_deref(), Some("x.png"));
        assert_eq!(resolve_ref("a.md", "../x.png"), None);
        assert_eq!(resolve_ref("a.md", "/x.png"), None);
        assert_eq!(resolve_ref("a.md", "C:/x.png"), None);
        assert_eq!(resolve_ref("a.md", "https://e.com/x.png"), None);
        assert_eq!(resolve_ref("a.md", "data:image/png;base64,AA"), None);
        assert_eq!(resolve_ref("a.md", "#top"), None);
        assert_eq!(resolve_ref("a.md", "<my pic.png>").as_deref(), Some("my pic.png"));
    }
    #[test]
    fn extract_refs_finds_markdown_and_html_images() {
        let t = "![a](img/a.png) text [l](b.md \"t\") <img src=\"c.jpg\"> ![d](<s p.png>) ![a](img/a.png)";
        assert_eq!(extract_refs(t), vec!["img/a.png", "b.md", "s p.png", "c.jpg"]);
    }
    #[test]
    fn image_mime_only_for_images() {
        assert_eq!(image_mime("a/b.PNG"), Some("image/png"));
        assert_eq!(image_mime("a.md"), None);
    }
    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }
}
