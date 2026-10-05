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

use std::path::{Path, PathBuf};

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
    let _ = (root, dir_id);
    todo!("P2")
}

pub fn workspace_dir(chat_dir: &Path) -> PathBuf {
    let _ = chat_dir;
    todo!("P2")
}

/// 添付コピーの最終パスと、コピー途中のパス。
pub fn attachment_paths(chat_dir: &Path, attachment: &LocalId, display_name: &str) -> (PathBuf, PathBuf) {
    let _ = (chat_dir, attachment, display_name);
    todo!("P4")
}

/// ファイル名を安全化する（`<>:"/\|?*`・制御文字・末尾の空白とピリオド・予約名 CON/PRN/AUX/NUL/COM1..9/LPT1..9 を置換、
/// 長さを120文字以内に切る、空なら `file`）。拡張子は残す。
pub fn sanitize_file_name(name: &str) -> String {
    let _ = name;
    todo!("P4")
}

/// クリップボード画像の表示名（`clipboard-YYYYMMDD-HHMMSS.png`、ローカル時刻）。
pub fn clipboard_image_name(at_local: (i32, u32, u32, u32, u32, u32)) -> String {
    let _ = at_local;
    todo!("P4")
}

/// 退避名（読めないファイルを残すときの名前。`name.corrupt-<ms>`）。
pub fn corrupt_aside_name(file: &Path, now_ms: i64) -> PathBuf {
    let _ = (file, now_ms);
    todo!("P2")
}

/// パスがチャット領域の内側か（削除・成果物の `in_chat_area` 判定）。正規化後の前方一致で判定し、`..` を解決する。
pub fn is_inside(base: &Path, path: &Path) -> bool {
    let _ = (base, path);
    todo!("P2")
}

/// 一覧の索引用（ログ・診断で使う文字列）。ファイル名には使わない。
pub fn chat_label(chat: &ChatKey) -> String {
    let _ = chat;
    todo!("P2")
}
