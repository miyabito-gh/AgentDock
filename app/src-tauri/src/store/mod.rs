//! 保存層（P2）。アプリ専用領域 `%LOCALAPPDATA%\com.agentdock.app\` への永続化（要件§3.12、D01・D02・D06）。
//!
//! - 方式: 1保存単位＝1 JSONファイル、原子的書込み（一時ファイル→`sync_all`→同一ディレクトリ内でrename置換）。
//!   監視活動履歴だけは追記型JSONL。SQLite等の依存は増やさない（データ量が小さく、ファイル単位で壊れても他へ波及しない）。
//! - Codexの保存履歴を正本とし、会話本文は保存しない（二重正本を作らない、§3.6）。
//! - 期限・容量による自動削除をしない（M39、D02）。読めないファイルは退避名へ改名して残し、黙って上書きしない。
//! - 書込み前に空き容量を確認し、足りなければその操作を止めて案内する（D02）。
//! - 保存状態は [`SaveStatus`] で単位ごとに持ち、失敗は再試行できる。保存成功を確認できないまま正常終了と表示しない（D06）。
//!
//! 実装はP2。ここは骨組み（シグネチャと契約）だけ。

pub mod atomic;
pub mod layout;
pub mod records;

use std::path::PathBuf;

use crate::backend::local::*;
use crate::backend::model::*;

use records::*;

/// 書込み時に、書く量に加えて残しておく空き（バイト）。下回る場合は書込みを止めて案内する。
pub const FREE_SPACE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;

/// 下書きの書込みを遅らせる時間（入力のたびに書かない）。
pub const DRAFT_DEBOUNCE_MS: i64 = 500;

/// 自動再試行の上限（以後はユーザーの「再試行」を待つ）。
pub const AUTO_RETRY_LIMIT: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("insufficient space: required {required}, available {available}")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// 読めなかったファイルは `moved_to` へ退避した（削除していない）。
    #[error("corrupt: {path} (moved to {moved_to:?})")]
    Corrupt { path: String, moved_to: Option<String> },
    /// 新しい版で書かれたファイル。上書きしない。
    #[error("unsupported schema version {found} in {path}")]
    NewerSchema { path: String, found: u32 },
}

/// 起動時に読み込んだ内容。ホストはこれでHostDataを初期化する（送信・キュー再開はしない）。
#[derive(Debug, Default)]
pub struct Restored {
    pub settings: Option<AppSettingsFile>,
    pub windows: Option<WindowsFile>,
    pub chats: Vec<RestoredChat>,
    /// 読めなかったファイル（警告表示用）。
    pub problems: Vec<StoreError>,
}

#[derive(Debug)]
pub struct RestoredChat {
    pub dir: PathBuf,
    pub local: ChatLocalFile,
    pub queue: Option<QueueFile>,
}

/// 保存層の窓口。同期I/O。ホストは `spawn_blocking` 経由で呼び、ロックを持ったまま呼ばない。
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// `root` は `app.path().app_local_data_dir()`（`%LOCALAPPDATA%\com.agentdock.app`）。
    pub fn open(root: PathBuf) -> Result<Self, StoreError> {
        let _ = &root;
        todo!("P2")
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 全ファイルを読み込む。`Copying` のまま残った添付は `CopyFailed{Interrupted}` にし、部分ファイル（`.partial`）を消す。
    /// 送信中（Sending）のまま残ったキュー項目は `AcceptanceUnknown` に変える（無条件再送しない）。
    pub fn load_all(&self) -> Restored {
        todo!("P2")
    }

    pub fn save_settings(&self, file: &AppSettingsFile) -> Result<UnixMillis, StoreError> {
        let _ = file;
        todo!("P2")
    }

    pub fn save_windows(&self, file: &WindowsFile) -> Result<UnixMillis, StoreError> {
        let _ = file;
        todo!("P2")
    }

    /// チャット領域を新規作成する（一般チャットはthread開始前に作業領域が要るので、ChatKeyより先に作る）。
    pub fn create_chat_dir(&self) -> Result<(LocalId, PathBuf), StoreError> {
        todo!("P2")
    }

    /// thread開始に失敗したとき、作成した領域が空なら消す（中身があれば残す）。
    pub fn discard_chat_dir_if_empty(&self, dir_id: &LocalId) -> Result<bool, StoreError> {
        let _ = dir_id;
        todo!("P2")
    }

    /// チャット領域を探す。なければ作成する（外部作成の会話に補足情報を付けるとき）。
    pub fn ensure_chat_dir(&self, chat: &ChatKey) -> Result<PathBuf, StoreError> {
        let _ = chat;
        todo!("P2")
    }

    pub fn save_chat_local(&self, file: &ChatLocalFile) -> Result<UnixMillis, StoreError> {
        let _ = file;
        todo!("P2")
    }

    pub fn save_queue(&self, file: &QueueFile) -> Result<UnixMillis, StoreError> {
        let _ = file;
        todo!("P2")
    }

    /// 監視活動を1行追記する（flushまで行う）。途中で切れた最終行は読込み時に無視する。
    pub fn append_activity(&self, chat: &ChatKey, line: &ActivityLine) -> Result<(), StoreError> {
        let _ = (chat, line);
        todo!("P2")
    }

    /// 監視活動を読む（エクスポート・再起動後の表示）。
    pub fn read_activity(&self, chat: &ChatKey) -> Result<Vec<ActivityLine>, StoreError> {
        let _ = chat;
        todo!("P2")
    }

    /// 書込み前の空き確認。`needed` に [`FREE_SPACE_MARGIN_BYTES`] を足して比べる。
    pub fn check_space(&self, needed: u64) -> Result<(), StoreError> {
        let _ = needed;
        todo!("P2")
    }

    /// 使用量の集計（重い。設定画面を開いたときなどに背景で実行）。
    pub fn usage(&self, chat: Option<&ChatKey>) -> UsageReport {
        let _ = chat;
        todo!("P7")
    }

    /// チャット領域の削除（M46・§3.6）。呼ぶ前にホストが停止確認・ユーザー確認・Codex側削除の成否を確かめる。
    /// 領域外（元ファイル・作業フォルダ・他チャット）には触れない。部分失敗はそのまま返す。
    pub fn remove_chat_dir(&self, chat: &ChatKey) -> Result<(), (Vec<String>, Vec<String>)> {
        let _ = chat;
        todo!("P7")
    }
}
