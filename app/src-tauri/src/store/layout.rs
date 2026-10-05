//! 専用領域の配置（純粋関数。単体テスト対象）。
//!
//! ```text
//! %LOCALAPPDATA%\com.agentdock.app\
//!   settings.json                 アプリ設定（AppSettingsFile）
//!   windows.json                  窓の位置・サイズ（WindowsFile）
//!   diag\                         診断ログ
//!   chats\<dirId>\                チャット1件の専用領域（dirIdはAgentDockのLocalId。ChatKeyはchat.jsonに書く）
//!     chat.json                   補足情報（ChatLocalFile）
//!     queue.json                  キューと受理不明の送信記録（QueueFile）
//!     activity.jsonl              取得した監視活動履歴（追記）
//!     attachments\<attId>\<name>  添付のコピー（添付ごとに別フォルダ。同名・再添付でも衝突しない）
//!     attachments\<attId>\<name>.partial   コピー途中（完了まで利用不可）
//!     workspace\                  一般チャットの作業フォルダ（開発チャットは使わない）
//! ```
//!
//! - ディレクトリ名にthread IDを使わない（一般チャットはthread開始前に作業領域が要るため）。
//!   ChatKey→dirの対応は起動時に `chats\*\chat.json` を走査して作る（索引ファイルを持たない＝索引の破損で全体を失わない）。
//! - 段階①で `%APPDATA%\com.agentdock.app\chats\chat-*` に作った一般チャットの作業領域は、既存threadのcwdなので移動しない。

use std::path::{Component, Path, PathBuf, Prefix};

use crate::backend::model::{ChatKey, LocalId};

pub const SETTINGS_FILE: &str = "settings.json";
pub const WINDOWS_FILE: &str = "windows.json";
pub const CHATS_DIR: &str = "chats";
pub const CHAT_FILE: &str = "chat.json";
pub const QUEUE_FILE: &str = "queue.json";
pub const ACTIVITY_FILE: &str = "activity.jsonl";
pub const ATTACHMENTS_DIR: &str = "attachments";
pub const WORKSPACE_DIR: &str = "workspace";
pub const PARTIAL_SUFFIX: &str = ".partial";

pub fn chat_dir(root: &Path, dir_id: &LocalId) -> PathBuf {
    // 保存ファイルから読んだIDでも、パス区切りや `..` で領域の外へ出ないようファイル名として安全化する。
    root.join(CHATS_DIR).join(sanitize_file_name(&dir_id.0))
}

pub fn workspace_dir(chat_dir: &Path) -> PathBuf {
    chat_dir.join(WORKSPACE_DIR)
}

/// 添付コピーの最終パスと、コピー途中のパス。
pub fn attachment_paths(chat_dir: &Path, attachment: &LocalId, display_name: &str) -> (PathBuf, PathBuf) {
    let dir = chat_dir.join(ATTACHMENTS_DIR).join(sanitize_file_name(&attachment.0));
    let name = sanitize_file_name(display_name);
    let final_path = dir.join(&name);
    let partial = dir.join(format!("{name}{PARTIAL_SUFFIX}"));
    (final_path, partial)
}

const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4",
    "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MAX_NAME_CHARS: usize = 120;

/// ファイル名を安全化する（`<>:"/\|?*`・制御文字・末尾の空白とピリオド・予約名 CON/PRN/AUX/NUL/COM1..9/LPT1..9 を置換、
/// 長さを120文字以内に切る、空なら `file`）。拡張子は残す。
pub fn sanitize_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let trimmed = replaced.trim_end_matches([' ', '.']);
    let mut out = if trimmed.is_empty() { "file".to_string() } else { trimmed.to_string() };
    // 予約名は拡張子を除いた部分で判定する（`CON.txt` も不可）。
    let stem = out.split('.').next().unwrap_or("");
    if RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        out.insert(0, '_');
    }
    if out.chars().count() > MAX_NAME_CHARS {
        let (stem, ext) = match out.rfind('.') {
            Some(i) if i > 0 && out[i..].chars().count() <= 20 => (&out[..i], &out[i..]),
            _ => (out.as_str(), ""),
        };
        let keep = MAX_NAME_CHARS.saturating_sub(ext.chars().count()).max(1);
        let cut: String = stem.chars().take(keep).collect();
        let cut = cut.trim_end_matches([' ', '.']);
        out = format!("{}{ext}", if cut.is_empty() { "file" } else { cut });
    }
    out
}

/// クリップボード画像の表示名（`clipboard-YYYYMMDD-HHMMSS.png`、ローカル時刻）。
pub fn clipboard_image_name(at_local: (i32, u32, u32, u32, u32, u32)) -> String {
    let (y, mo, d, h, mi, sec) = at_local;
    format!("clipboard-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{sec:02}.png")
}

/// 退避名（読めないファイルを残すときの名前。`name.corrupt-<ms>`）。
pub fn corrupt_aside_name(file: &Path, now_ms: i64) -> PathBuf {
    let mut name = file.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".corrupt-{now_ms}"));
    file.with_file_name(name)
}

/// パスを字句的に正規化した部品列（`.` を除き `..` を解決、大文字小文字は区別しない＝Windows）。
fn normalized(p: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 接頭辞とルートは `..` で越えない。
    let mut floor = 0;
    for c in p.components() {
        match c {
            Component::Prefix(pre) => {
                // `\\?\C:` と `C:` は同じドライブとして扱う。
                let s = match pre.kind() {
                    Prefix::VerbatimDisk(d) | Prefix::Disk(d) => format!("{}:", (d as char).to_ascii_lowercase()),
                    _ => pre.as_os_str().to_string_lossy().to_lowercase(),
                };
                out.push(s);
                floor = out.len();
            }
            Component::RootDir => {
                out.push("\\".to_string());
                floor = out.len();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if out.len() > floor {
                    out.pop();
                }
            }
            Component::Normal(n) => out.push(n.to_string_lossy().to_lowercase()),
        }
    }
    out
}

/// パスがチャット領域の内側か（削除・成果物の `in_chat_area` 判定）。正規化後の前方一致で判定し、`..` を解決する。
/// 領域そのものも内側とみなす。どちらかが相対パスなら判定できないので false。
pub fn is_inside(base: &Path, path: &Path) -> bool {
    if !base.is_absolute() || !path.is_absolute() {
        return false;
    }
    let (b, p) = (normalized(base), normalized(path));
    p.len() >= b.len() && p[..b.len()] == b[..]
}

/// 一覧の索引用（ログ・診断で使う文字列）。ファイル名には使わない。
pub fn chat_label(chat: &ChatKey) -> String {
    format!("{:?}/{}", chat.backend, chat.id.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_paths_are_per_attachment_and_safe() {
        let base = Path::new(r"C:\data\chats\d1");
        let (fin, part) = attachment_paths(base, &LocalId("att-1".into()), "a/b.txt");
        assert_eq!(fin, base.join("attachments").join("att-1").join("a_b.txt"));
        assert_eq!(part, base.join("attachments").join("att-1").join("a_b.txt.partial"));
        let (other, _) = attachment_paths(base, &LocalId("att-2".into()), "a/b.txt");
        assert_ne!(fin, other, "re-attaching the same file gets a separate copy");
        let (esc, _) = attachment_paths(base, &LocalId("..\\x".into()), "f");
        assert!(is_inside(base, &esc));
    }

    #[test]
    fn clipboard_name_is_zero_padded() {
        assert_eq!(clipboard_image_name((2026, 1, 2, 3, 4, 5)), "clipboard-20260102-030405.png");
    }

    #[test]
    fn sanitize_replaces_forbidden_and_trims() {
        assert_eq!(sanitize_file_name("a<b>c:d\"e/f|g?h*i"), "a_b_c_d_e_f_g_h_i");
        assert_eq!(sanitize_file_name("report. "), "report");
        assert_eq!(sanitize_file_name("..."), "file");
        assert_eq!(sanitize_file_name(""), "file");
        assert_eq!(sanitize_file_name("a\tb\u{1}"), "a_b_");
        assert_eq!(sanitize_file_name("日本語 ファイル.txt"), "日本語 ファイル.txt");
    }

    #[test]
    fn sanitize_guards_reserved_names_and_length() {
        assert_eq!(sanitize_file_name("con"), "_con");
        assert_eq!(sanitize_file_name("NUL.txt"), "_NUL.txt");
        assert_eq!(sanitize_file_name("com1"), "_com1");
        assert_eq!(sanitize_file_name("console.txt"), "console.txt");
        let long = format!("{}.png", "x".repeat(300));
        let s = sanitize_file_name(&long);
        assert_eq!(s.chars().count(), MAX_NAME_CHARS);
        assert!(s.ends_with(".png"));
        // 区切りはすべて置換されるので、ディレクトリ名として使っても領域の外へ出ない。
        assert_eq!(sanitize_file_name("..\\..\\x"), ".._.._x");
    }

    #[test]
    fn chat_dir_stays_under_chats() {
        let root = Path::new(r"C:\data\app");
        let d = chat_dir(root, &LocalId("dir-1-2".into()));
        assert_eq!(d, Path::new(r"C:\data\app\chats\dir-1-2"));
        assert!(is_inside(&root.join(CHATS_DIR), &chat_dir(root, &LocalId("..\\..\\evil".into()))));
        assert_eq!(workspace_dir(&d), Path::new(r"C:\data\app\chats\dir-1-2\workspace"));
    }

    #[test]
    fn is_inside_resolves_dots_and_ignores_case() {
        let base = Path::new(r"C:\Data\Chat1");
        assert!(is_inside(base, Path::new(r"c:\data\chat1\workspace\a.txt")));
        assert!(is_inside(base, base));
        assert!(!is_inside(base, Path::new(r"C:\Data\Chat10\a.txt")));
        assert!(!is_inside(base, Path::new(r"C:\Data\Chat1\..\Chat2\a.txt")));
        assert!(is_inside(base, Path::new(r"C:\Data\Chat1\sub\..\a.txt")));
        assert!(!is_inside(base, Path::new(r"D:\Data\Chat1\a.txt")));
        assert!(!is_inside(base, Path::new(r"relative\a.txt")));
        assert!(is_inside(base, Path::new(r"\\?\C:\Data\Chat1\a.txt")));
    }

    #[test]
    fn corrupt_name_keeps_the_original_name() {
        let p = corrupt_aside_name(Path::new(r"C:\d\chat.json"), 1234);
        assert_eq!(p, Path::new(r"C:\d\chat.json.corrupt-1234"));
    }
}
