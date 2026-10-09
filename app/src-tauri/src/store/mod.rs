//! 保存層（P2）。アプリ専用領域 `%LOCALAPPDATA%\com.agentdock.app\` への永続化（要件§3.12、D01・D02・D06）。
//!
//! - 方式: 1保存単位＝1 JSONファイル、原子的書込み（一時ファイル→`sync_all`→同一ディレクトリ内でrename置換）。
//!   監視活動履歴だけは追記型JSONL。SQLite等の依存は増やさない（データ量が小さく、ファイル単位で壊れても他へ波及しない）。
//! - Codexの保存履歴を正本とし、会話本文は保存しない（二重正本を作らない、§3.6）。
//! - 期限・容量による自動削除をしない（M39、D02）。読めないファイルは退避名へ改名して残し、黙って上書きしない。
//! - 書込み前に空き容量を確認し、足りなければその操作を止めて案内する（D02）。
//! - 保存状態は [`SaveStatus`] で単位ごとに持ち、失敗は再試行できる。保存成功を確認できないまま正常終了と表示しない（D06）。
//!
//! 同期I/O。`Store` 自体は書込みを直列化しない（ホスト側 `host::persist` が1本のロックで直列化する）。

pub mod atomic;
pub mod layout;
pub mod records;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::backend::baseline::{BaselineEntry, BaselineLine, BASELINE_SCHEMA_VERSION};
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
    /// 領域・ファイルに紐づく通知（文にパスを含む。確認済みにできる）。
    #[error("{message}")]
    Notice { message: String },
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
    /// worktree台帳（P3-7）。
    pub worktrees: Vec<WorktreeRecord>,
    /// クラウド委任の記録（P3-6）。
    pub cloud_tasks: Vec<CloudTaskRecord>,
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

/// 上書きしてはいけないファイル（読めなかった／新しい版）。
#[derive(Debug, Clone, Copy)]
enum Protection {
    Newer(u32),
    /// 退避に失敗して、読めないファイルがその場に残っている。
    Corrupt,
}

/// 書込み量に余白を足して、空きと比べる（純粋関数）。
pub fn space_check(needed: u64, available: u64) -> Result<(), StoreError> {
    let required = needed.saturating_add(FREE_SPACE_MARGIN_BYTES);
    if available < required {
        Err(StoreError::InsufficientSpace { required, available })
    } else {
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn shown(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// 保存層の窓口。同期I/O。ホストは `spawn_blocking` 経由で呼び、ロックを持ったまま呼ばない。
pub struct Store {
    root: PathBuf,
    /// ChatKey→dirId（起動時の走査と新規作成で埋める。索引ファイルは持たない）。
    index: Mutex<HashMap<ChatKey, LocalId>>,
    protected: Mutex<HashMap<PathBuf, Protection>>,
    counter: AtomicU64,
}

impl Store {
    /// `root` は `app.path().app_local_data_dir()`（`%LOCALAPPDATA%\com.agentdock.app`）。
    pub fn open(root: PathBuf) -> Result<Self, StoreError> {
        std::fs::create_dir_all(root.join(layout::CHATS_DIR))?;
        Ok(Store { root, index: Mutex::new(HashMap::new()), protected: Mutex::new(HashMap::new()), counter: AtomicU64::new(1) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn new_dir_id(&self) -> LocalId {
        LocalId(format!("dir-{}-{}", now_ms(), self.counter.fetch_add(1, Ordering::SeqCst)))
    }

    /// 読む。無ければ None。壊れていれば `*.corrupt-<ms>` へ退避して Corrupt を返す（削除・上書きしない）。
    /// 新しい版は NewerSchema を返し、以後このパスへの書込みを拒否する。
    fn read_file<T: serde::de::DeserializeOwned>(&self, path: &Path) -> Result<Option<T>, StoreError> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::Io(e)),
        };
        match parse_versioned(&bytes) {
            Ok(v) => Ok(Some(v)),
            Err(ParseError::Newer(found)) => {
                self.protected.lock().unwrap().insert(path.to_path_buf(), Protection::Newer(found));
                Err(StoreError::NewerSchema { path: shown(path), found })
            }
            Err(ParseError::Corrupt(_)) => {
                let aside = layout::corrupt_aside_name(path, now_ms());
                let moved_to = std::fs::rename(path, &aside).ok().map(|_| shown(&aside));
                if moved_to.is_none() {
                    self.protected.lock().unwrap().insert(path.to_path_buf(), Protection::Corrupt);
                }
                Err(StoreError::Corrupt { path: shown(path), moved_to })
            }
        }
    }

    fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<UnixMillis, StoreError> {
        if let Some(p) = self.protected.lock().unwrap().get(path).copied() {
            return Err(match p {
                Protection::Newer(found) => StoreError::NewerSchema { path: shown(path), found },
                Protection::Corrupt => StoreError::Corrupt { path: shown(path), moved_to: None },
            });
        }
        let bytes = to_bytes(value).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(bytes.len() as u64)?;
        atomic::write_atomic(path, &bytes)?;
        Ok(UnixMillis(now_ms()))
    }

    /// 全ファイルを読み込む。`Copying` のまま残った添付は `CopyFailed{Interrupted}` にし、部分ファイル（`.partial`）を消す。
    /// 送信中（Sending）のまま残ったキュー項目は `AcceptanceUnknown` に変える（無条件再送しない）。
    /// 読めなかったファイルは退避して `problems` に入れる（その単位だけ既定値で始まる）。
    pub fn load_all(&self) -> Restored {
        let mut r = Restored::default();
        match self.read_file::<AppSettingsFile>(&self.root.join(layout::SETTINGS_FILE)) {
            Ok(v) => {
                r.settings = v.map(|mut f| {
                    f.settings.migrate_legacy();
                    f
                })
            }
            Err(e) => r.problems.push(e),
        }
        match self.read_file::<WindowsFile>(&self.root.join(layout::WINDOWS_FILE)) {
            Ok(v) => r.windows = v,
            Err(e) => r.problems.push(e),
        }
        match self.read_file::<WorktreesFile>(&self.root.join(layout::WORKTREES_FILE)) {
            Ok(v) => r.worktrees = v.map(|f| f.worktrees).unwrap_or_default(),
            Err(e) => r.problems.push(e),
        }
        match self.read_file::<CloudTasksFile>(&self.root.join(layout::CLOUD_TASKS_FILE)) {
            Ok(v) => r.cloud_tasks = v.map(|f| f.tasks).unwrap_or_default(),
            Err(e) => r.problems.push(e),
        }
        let Ok(rd) = std::fs::read_dir(self.root.join(layout::CHATS_DIR)) else { return r };
        let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        let mut seen: HashSet<ChatKey> = HashSet::new();
        for dir in dirs {
            let mut local = match self.read_file::<ChatLocalFile>(&dir.join(layout::CHAT_FILE)) {
                Ok(Some(l)) => l,
                // chat.json の無い領域。監視活動だけの領域や段階①の作業領域は対象外。
                // 作業領域（workspace）や送信待ちが残っているのに記録が無いときは、ピン・下書き・送信待ちなどを復元できないので警告する。
                Ok(None) => {
                    if layout::workspace_dir(&dir).exists() || dir.join(layout::QUEUE_FILE).exists() {
                        r.problems.push(StoreError::Notice {
                            message: format!(
                                "チャットの記録（chat.json）が見つかりません。ピン・下書き・送信待ちを復元できません（元のファイルは変更していません）: {}",
                                shown(&dir)
                            ),
                        });
                    }
                    continue;
                }
                Err(e) => {
                    r.problems.push(e);
                    continue;
                }
            };
            let mut changed = false;
            // ディレクトリ名を正とする（保存内容と食い違ってもパスが領域の外へ向かないように）。
            let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if local.dir_id.0 != dir_name {
                local.dir_id = LocalId(dir_name);
                changed = true;
            }
            if let Some(key) = &local.chat {
                if !seen.insert(key.clone()) {
                    r.problems.push(StoreError::Notice { message: format!("同じチャットの記録が複数あります（先に読んだ方を使います）: {}", shown(&dir)) });
                    continue;
                }
                self.index.lock().unwrap().insert(key.clone(), local.dir_id.clone());
            }
            // 開いたままのside相談（一時の分岐）は、再起動で消えている。終了（再開不可）の記録に変える。
            if crate::rules::side::end_open_after_restart(&mut local.side_sessions) {
                changed = true;
            }
            for att in &mut local.attachments {
                if att.state == AttachmentState::Copying {
                    att.state = AttachmentState::CopyFailed {
                        reason: CopyFailure::Interrupted,
                        message: "コピー中にアプリが終了したため、このコピーは使えません。もう一度添付してください。".into(),
                    };
                    att.copy_path = None;
                    remove_partials(&dir.join(layout::ATTACHMENTS_DIR).join(layout::sanitize_file_name(&att.id.0)));
                    changed = true;
                }
            }
            if changed {
                if let Err(e) = self.save_chat_local(&local) {
                    r.problems.push(e);
                }
            }
            let queue = match self.read_file::<QueueFile>(&dir.join(layout::QUEUE_FILE)) {
                Ok(Some(mut q)) => {
                    let before = q.clone();
                    crate::rules::queue::restore_after_restart(&mut q.queue);
                    if q != before {
                        // 変換した状態を保存し直す（もう一度落ちても、Sending を未確認のまま再送へ戻さない）。
                        if let Err(e) = self.save_queue(&q) {
                            r.problems.push(e);
                        }
                    }
                    Some(q)
                }
                Ok(None) => None,
                Err(e) => {
                    r.problems.push(e);
                    None
                }
            };
            r.chats.push(RestoredChat { dir, local, queue });
        }
        r
    }

    pub fn save_settings(&self, file: &AppSettingsFile) -> Result<UnixMillis, StoreError> {
        self.write_json(&self.root.join(layout::SETTINGS_FILE), file)
    }

    pub fn save_windows(&self, file: &WindowsFile) -> Result<UnixMillis, StoreError> {
        self.write_json(&self.root.join(layout::WINDOWS_FILE), file)
    }

    /// worktree台帳を書く（空き確認つき。読めなかった・新しい版のファイルは上書きしない）。
    pub fn save_worktrees(&self, file: &WorktreesFile) -> Result<UnixMillis, StoreError> {
        self.write_json(&self.root.join(layout::WORKTREES_FILE), file)
    }

    /// クラウド委任の記録を書く（空き確認つき。読めなかった・新しい版のファイルは上書きしない）。
    pub fn save_cloud_tasks(&self, file: &CloudTasksFile) -> Result<UnixMillis, StoreError> {
        self.write_json(&self.root.join(layout::CLOUD_TASKS_FILE), file)
    }

    /// worktreeの置き場所（`<root>\worktrees`）。フォルダは作らない。
    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join(layout::WORKTREES_DIR)
    }

    /// チャット領域を新規作成する（一般チャットはthread開始前に作業領域が要るので、ChatKeyより先に作る）。
    /// 戻り値のパスはチャット領域。作業フォルダは `layout::workspace_dir` で、ここで作成済み。
    pub fn create_chat_dir(&self) -> Result<(LocalId, PathBuf), StoreError> {
        let id = self.new_dir_id();
        let dir = layout::chat_dir(&self.root, &id);
        std::fs::create_dir_all(layout::workspace_dir(&dir))?;
        Ok((id, dir))
    }

    /// thread開始に失敗したとき、作成した領域が空なら消す（中身があれば残す）。
    pub fn discard_chat_dir_if_empty(&self, dir_id: &LocalId) -> Result<bool, StoreError> {
        let dir = layout::chat_dir(&self.root, dir_id);
        if !dir.exists() {
            return Ok(false);
        }
        if !has_files(&dir)? {
            std::fs::remove_dir_all(&dir)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// ChatKeyに結び付けるチャット領域のID（既にあればそれ、なければ新しいIDを割り当てる。ディスクには触れない）。
    pub fn ensure_dir_id(&self, chat: &ChatKey) -> LocalId {
        let mut index = self.index.lock().unwrap();
        index.entry(chat.clone()).or_insert_with(|| self.new_dir_id()).clone()
    }

    /// `create_chat_dir` で作った領域を、thread開始後に ChatKey へ結び付ける。
    pub fn register_dir(&self, chat: &ChatKey, dir_id: &LocalId) {
        self.index.lock().unwrap().insert(chat.clone(), dir_id.clone());
    }

    /// チャット領域を探す。なければ作成する（外部作成の会話に補足情報を付けるとき）。
    pub fn ensure_chat_dir(&self, chat: &ChatKey) -> Result<PathBuf, StoreError> {
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn save_chat_local(&self, file: &ChatLocalFile) -> Result<UnixMillis, StoreError> {
        let path = layout::chat_dir(&self.root, &file.dir_id).join(layout::CHAT_FILE);
        let at = self.write_json(&path, file)?;
        if let Some(chat) = &file.chat {
            self.register_dir(chat, &file.dir_id);
        }
        Ok(at)
    }

    pub fn save_queue(&self, file: &QueueFile) -> Result<UnixMillis, StoreError> {
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(&file.queue.chat));
        self.write_json(&dir.join(layout::QUEUE_FILE), file)
    }

    /// 監視活動を1行追記する（flushまで行う）。途中で切れた最終行は読込み時に無視する。
    pub fn append_activity(&self, chat: &ChatKey, line: &ActivityLine) -> Result<(), StoreError> {
        let text = serde_json::to_string(line).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(text.len() as u64 + 1)?;
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat));
        atomic::append_line(&dir.join(layout::ACTIVITY_FILE), &text)?;
        Ok(())
    }

    /// 監視活動を読む（エクスポート・再起動後の表示）。読めない行（途中で切れた最終行など）は飛ばす。
    pub fn read_activity(&self, chat: &ChatKey) -> Result<Vec<ActivityLine>, StoreError> {
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat));
        let text = match std::fs::read_to_string(dir.join(layout::ACTIVITY_FILE)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        Ok(text.lines().filter_map(|l| serde_json::from_str::<ActivityLine>(l).ok()).collect())
    }

    /// 拡張・ツールサーバーの管理操作の記録を1行追記する（ルートの `extension-ops.jsonl`。flushまで行う。空き確認つき）。
    pub fn append_extension_op(&self, line: &crate::backend::ipc::ExtensionOpLine) -> Result<(), StoreError> {
        let text = serde_json::to_string(line).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(text.len() as u64 + 1)?;
        atomic::append_line(&self.root.join(layout::EXTENSION_OPS_FILE), &text)?;
        Ok(())
    }

    /// 管理操作の記録を古い順に読む。読めない行（途中で切れた最終行など）は飛ばす。
    pub fn read_extension_ops(&self) -> Result<Vec<crate::backend::ipc::ExtensionOpLine>, StoreError> {
        let text = match std::fs::read_to_string(self.root.join(layout::EXTENSION_OPS_FILE)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
    }

    /// 変更の控えの記録を1行追記する（`baselines\segments.jsonl`。flushまで行う。空き確認つき）。
    pub fn append_baseline(&self, chat: &ChatKey, line: &BaselineLine) -> Result<(), StoreError> {
        let text = serde_json::to_string(&BaselineEntry::new(line.clone())).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(text.len() as u64 + 1)?;
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat));
        atomic::append_line(&layout::baseline_segments_file(&dir), &text)?;
        Ok(())
    }

    /// このチャットの控えの記録を古い順に読む。読めない行（途中で切れた最終行など）と、新しい版の行は飛ばす（推測で解釈しない）。
    /// 旧方式の `changes.jsonl`・`revert-backup` は読まない。
    pub fn read_baselines(&self, chat: &ChatKey) -> Result<Vec<BaselineLine>, StoreError> {
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat));
        let text = match std::fs::read_to_string(layout::baseline_segments_file(&dir)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<BaselineEntry>(l).ok())
            .filter(|e| e.schema_version <= BASELINE_SCHEMA_VERSION)
            .map(|e| e.line)
            .collect())
    }

    /// 控え専用のgitオブジェクト置き場（`chats\<dirId>\baselines\objects`）。フォルダは作らない。
    pub fn baseline_objects_dir(&self, chat: &ChatKey) -> PathBuf {
        layout::baseline_objects_dir(&layout::chat_dir(&self.root, &self.ensure_dir_id(chat)))
    }

    /// 一時インデックスの置き場（`chats\<dirId>\baselines\tmp`）。フォルダは作らない。
    pub fn baseline_tmp_dir(&self, chat: &ChatKey) -> PathBuf {
        layout::baseline_tmp_dir(&layout::chat_dir(&self.root, &self.ensure_dir_id(chat)))
    }

    /// 控えの置き場（objects・tmp）を作る。
    pub fn ensure_baseline_dirs(&self, chat: &ChatKey) -> Result<(PathBuf, PathBuf), StoreError> {
        let (objects, tmp) = (self.baseline_objects_dir(chat), self.baseline_tmp_dir(chat));
        std::fs::create_dir_all(&objects)?;
        std::fs::create_dir_all(&tmp)?;
        Ok((objects, tmp))
    }

    /// このチャットの控え（`baselines\`）をすべて消す。ユーザーの明示操作（確認済み）と、作業中でないことの確認は呼出し側が行う。
    /// 読み取り専用のgitオブジェクトも消す。部分失敗は消せなかったものを返す。`revert-backup`・`changes.jsonl` には触れない。
    pub fn delete_baselines(&self, chat: &ChatKey) -> Result<(), Vec<String>> {
        let Some(dir_id) = self.index.lock().unwrap().get(chat).cloned() else { return Ok(()) };
        let dir = layout::baselines_dir(&layout::chat_dir(&self.root, &dir_id));
        match remove_tree(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(vec![format!("{}: {e}", shown(&dir))]),
        }
    }

    /// 戻す前の控えの置き場所（`chats/<dirId>/revert-backup/<ms>`）。フォルダは作らない。
    pub fn revert_backup_dir(&self, chat: &ChatKey, at: UnixMillis) -> PathBuf {
        layout::chat_dir(&self.root, &self.ensure_dir_id(chat)).join(layout::REVERT_BACKUP_DIR).join(at.0.to_string())
    }

    /// 圧縮前の控えを保存する（`chats/<dirId>/compactions/<ms>.json`）。空き確認つき。同じ時刻のファイルがあれば上書きせず失敗にする。
    pub fn write_compaction(&self, file: &CompactionFile) -> Result<PathBuf, StoreError> {
        let bytes = to_bytes(file).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(bytes.len() as u64 + 64 * 1024)?;
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(&file.chat)).join(layout::COMPACTIONS_DIR);
        let path = dir.join(format!("{}.json", file.created_at.0));
        if path.exists() {
            return Err(StoreError::Io(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "同じ時刻の控えがすでにあります")));
        }
        atomic::write_atomic(&path, &bytes)?;
        Ok(path)
    }

    /// 圧縮前の控えの一覧（作成時刻の新しい順）。ファイル名（数値のミリ秒）から導き、中身は読まない。
    pub fn list_compactions(&self, chat: &ChatKey) -> Result<Vec<UnixMillis>, StoreError> {
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(chat)).join(layout::COMPACTIONS_DIR);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        let mut at: Vec<i64> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<i64>().ok()))
            .collect();
        at.sort_unstable_by(|a, b| b.cmp(a));
        Ok(at.into_iter().map(UnixMillis).collect())
    }

    /// 圧縮前の控えを1件読む。壊れている・新しい版のファイルは `Err`（推測で補わない）。
    pub fn read_compaction(&self, chat: &ChatKey, at: UnixMillis) -> Result<CompactionFile, StoreError> {
        let path = layout::chat_dir(&self.root, &self.ensure_dir_id(chat)).join(layout::COMPACTIONS_DIR).join(format!("{}.json", at.0));
        let bytes = std::fs::read(&path)?;
        parse_versioned::<CompactionFile>(&bytes).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("控えを読めません: {e:?}"))))
    }

    /// side相談の記録を保存する（`chats/<dirId>/side/<id>.json`）。空き確認つき。同じIDは最新の内容で置き換える（原子的）。
    pub fn write_side(&self, file: &SideFile) -> Result<(), StoreError> {
        let bytes = to_bytes(file).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        self.check_space(bytes.len() as u64 + 16 * 1024)?;
        let dir = layout::chat_dir(&self.root, &self.ensure_dir_id(&file.main)).join(layout::SIDE_DIR);
        atomic::write_atomic(&dir.join(format!("{}.json", layout::sanitize_file_name(&file.id.0))), &bytes)?;
        Ok(())
    }

    /// side相談の記録を読む。壊れている・新しい版のファイルは `Err`（推測で補わない）。
    pub fn read_side(&self, main: &ChatKey, id: &LocalId) -> Result<SideFile, StoreError> {
        let path = layout::chat_dir(&self.root, &self.ensure_dir_id(main)).join(layout::SIDE_DIR).join(format!("{}.json", layout::sanitize_file_name(&id.0)));
        let bytes = std::fs::read(&path)?;
        parse_versioned::<SideFile>(&bytes).map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("side相談の記録を読めません: {e:?}"))))
    }

    /// 書込み前の空き確認。`needed` に [`FREE_SPACE_MARGIN_BYTES`] を足して比べる。
    /// 空きを取得できないときは止めない（書込み自体が失敗すればその結果を報告する）。
    pub fn check_space(&self, needed: u64) -> Result<(), StoreError> {
        match atomic::free_space(&self.root) {
            Ok(available) => space_check(needed, available),
            Err(_) => Ok(()),
        }
    }

    /// 使用量の集計（重い。設定画面を開いたときなどに背景で実行）。
    /// `chat` 指定ならそのチャットの領域だけ（設定ファイル等は含めない）。`legacy_area` は呼出し側（ホスト）が埋める。
    pub fn usage(&self, chat: Option<&ChatKey>) -> UsageReport {
        let mut total = UsageBreakdown::default();
        let mut chats: Vec<ChatUsage> = Vec::new();
        let mut unreadable: Vec<String> = Vec::new();
        let chats_root = self.root.join(layout::CHATS_DIR);
        let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&chats_root) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => {
                unreadable.push(shown(&chats_root));
                Vec::new()
            }
        };
        dirs.sort();
        for dir in dirs {
            // チャットの特定は chat.json から。読めない・thread開始前のものは合計にだけ入れる。
            let local = std::fs::read(dir.join(layout::CHAT_FILE)).ok().and_then(|b| parse_versioned::<ChatLocalFile>(&b).ok());
            let key = local.as_ref().and_then(|l| l.chat.clone());
            if let Some(want) = chat {
                if key.as_ref() != Some(want) {
                    continue;
                }
            }
            let (breakdown, mut bad) = chat_area_usage(&dir, local.as_ref());
            unreadable.append(&mut bad);
            add_breakdown(&mut total, &breakdown);
            if let Some(chat) = key {
                chats.push(ChatUsage { chat, breakdown });
            }
        }
        if chat.is_none() {
            // 設定・窓・診断ログ（チャット領域の外のアプリ専用領域）。
            let mut meta = 0u64;
            let Ok(rd) = std::fs::read_dir(&self.root) else {
                unreadable.push(shown(&self.root));
                return self.finish_usage(total, chats, unreadable);
            };
            for e in rd.filter_map(|e| e.ok()) {
                if e.file_name() == layout::CHATS_DIR {
                    continue;
                }
                let (n, mut bad) = atomic::dir_size(&e.path());
                meta = meta.saturating_add(n);
                unreadable.append(&mut bad);
            }
            total.metadata = total.metadata.saturating_add(meta);
        }
        self.finish_usage(total, chats, unreadable)
    }

    fn finish_usage(&self, total: UsageBreakdown, chats: Vec<ChatUsage>, unreadable: Vec<String>) -> UsageReport {
        let free_space = match atomic::free_space(&self.root) {
            Ok(n) => Known::direct(n),
            Err(_) => Known::NotFetched,
        };
        UsageReport { total, chats, legacy_area: 0, free_space, measured_at: UnixMillis(now_ms()), unreadable }
    }

    /// チャット領域の合計サイズ（削除確認の表示用）。領域がなければ 0、読めなかったパスがあれば None（0で代用しない）。
    pub fn chat_area_bytes(&self, chat: &ChatKey) -> Option<u64> {
        let Some(dir_id) = self.index.lock().unwrap().get(chat).cloned() else { return Some(0) };
        let (n, bad) = atomic::dir_size(&layout::chat_dir(&self.root, &dir_id));
        bad.is_empty().then_some(n)
    }

    /// チャット領域の削除（M46・§3.6）。呼ぶ前にホストが停止確認・ユーザー確認・Codex側削除の成否を確かめる。
    /// 領域外（元ファイル・作業フォルダ・他チャット）には触れない。部分失敗はそのまま返す（`(削除できたもの, できなかったもの)`）。
    /// 領域内のシンボリックリンク・ジャンクションは、リンク自体だけを消し、辿らない。
    pub fn remove_chat_dir(&self, chat: &ChatKey) -> Result<(), (Vec<String>, Vec<String>)> {
        let Some(dir_id) = self.index.lock().unwrap().get(chat).cloned() else { return Ok(()) };
        let chats_root = self.root.join(layout::CHATS_DIR);
        let dir = layout::chat_dir(&self.root, &dir_id);
        // 領域そのもの（`chats` 直下の1フォルダ）でなければ触らない。
        if dir.parent() != Some(chats_root.as_path()) {
            return Err((Vec::new(), vec![format!("チャット領域の場所が不正なため削除しません: {}", shown(&dir))]));
        }
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        let children = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.index.lock().unwrap().remove(chat);
                return Ok(());
            }
            Err(e) => return Err((done, vec![format!("{}: {e}", shown(&dir))])),
        };
        for child in children {
            let name = child.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match remove_tree(&child) {
                Ok(()) => done.push(name),
                Err(e) => failed.push(format!("{}: {e}", shown(&child))),
            }
        }
        if failed.is_empty() {
            match std::fs::remove_dir(&dir) {
                Ok(()) => done.push(shown(&dir)),
                Err(e) => failed.push(format!("{}: {e}", shown(&dir))),
            }
        }
        if !failed.is_empty() {
            return Err((done, failed));
        }
        self.index.lock().unwrap().remove(chat);
        Ok(())
    }
}

/// 段階①の一般チャット作業領域（旧領域。`<Roaming>\...\chats\chat-*`）の合計。移動も削除もしない。読めなかったパスは別に返す。
pub fn legacy_area_usage(legacy_chats_dir: &Path) -> (u64, Vec<String>) {
    let mut total = 0u64;
    let mut unreadable = Vec::new();
    match std::fs::read_dir(legacy_chats_dir) {
        Ok(rd) => {
            for e in rd.filter_map(|e| e.ok()) {
                if !e.file_name().to_string_lossy().starts_with("chat-") {
                    continue;
                }
                let (n, mut bad) = atomic::dir_size(&e.path());
                total = total.saturating_add(n);
                unreadable.append(&mut bad);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => unreadable.push(shown(legacy_chats_dir)),
    }
    (total, unreadable)
}

fn add_breakdown(total: &mut UsageBreakdown, b: &UsageBreakdown) {
    total.attachments = total.attachments.saturating_add(b.attachments);
    total.artifacts = total.artifacts.saturating_add(b.artifacts);
    total.workspace = total.workspace.saturating_add(b.workspace);
    total.activity = total.activity.saturating_add(b.activity);
    total.metadata = total.metadata.saturating_add(b.metadata);
    total.baselines = total.baselines.saturating_add(b.baselines);
    total.revert_backups = total.revert_backups.saturating_add(b.revert_backups);
}

/// チャット領域1件の内訳。添付・作業領域・監視活動はそれぞれの場所、残り（chat.json・queue.json・その他）はメタデータ。
/// 台帳にある成果物のうち作業領域内の実在ファイルは、作業領域から差し引いて成果物に数える（二重に数えない）。
fn chat_area_usage(dir: &Path, local: Option<&ChatLocalFile>) -> (UsageBreakdown, Vec<String>) {
    let (all, mut bad) = atomic::dir_size(dir);
    let (attachments, mut b1) = atomic::dir_size(&dir.join(layout::ATTACHMENTS_DIR));
    let workspace_dir = layout::workspace_dir(dir);
    let (workspace_all, mut b2) = atomic::dir_size(&workspace_dir);
    let (activity, mut b3) = atomic::dir_size(&dir.join(layout::ACTIVITY_FILE));
    let (baselines, mut b4) = atomic::dir_size(&layout::baselines_dir(dir));
    let (revert_backups, mut b5) = atomic::dir_size(&dir.join(layout::REVERT_BACKUP_DIR));
    bad.append(&mut b1);
    bad.append(&mut b2);
    bad.append(&mut b3);
    bad.append(&mut b4);
    bad.append(&mut b5);
    bad.sort();
    bad.dedup();
    let mut artifacts = 0u64;
    if let Some(l) = local {
        let mut seen: HashSet<String> = HashSet::new();
        for a in l.artifacts.iter().filter(|a| a.in_chat_area) {
            let p = Path::new(&a.path);
            if !layout::is_inside(&workspace_dir, p) || !seen.insert(a.path.to_lowercase()) {
                continue;
            }
            if let Ok(m) = std::fs::symlink_metadata(p) {
                if m.is_file() {
                    artifacts = artifacts.saturating_add(m.len());
                }
            }
        }
    }
    let artifacts = artifacts.min(workspace_all);
    let workspace = workspace_all - artifacts;
    // 変更の控え・戻す前の控えは独立に計上し、メタデータから差し引く（二重に数えない）。旧 `changes.jsonl` などは従来どおりメタデータ。
    let metadata = all
        .saturating_sub(attachments)
        .saturating_sub(workspace_all)
        .saturating_sub(activity)
        .saturating_sub(baselines)
        .saturating_sub(revert_backups);
    (UsageBreakdown { attachments, artifacts, workspace, activity, metadata, baselines, revert_backups }, bad)
}

/// ファイル・フォルダを消す。リンク（シンボリックリンク・ジャンクション）は辿らずリンクだけ消す。読み取り専用は解除して1回だけやり直す。
fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let ft = meta.file_type();
    if ft.is_dir() && !ft.is_symlink() {
        for e in std::fs::read_dir(path)? {
            remove_tree(&e?.path())?;
        }
        return remove_with_retry(path, |p| std::fs::remove_dir(p));
    }
    if ft.is_symlink() && meta.is_dir() {
        // ディレクトリへのリンク（ジャンクション等）。中身を辿らず、リンクだけ消す。
        return remove_with_retry(path, |p| std::fs::remove_dir(p));
    }
    remove_with_retry(path, |p| std::fs::remove_file(p))
}

fn remove_with_retry(path: &Path, f: impl Fn(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    match f(path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            if let Ok(m) = std::fs::symlink_metadata(path) {
                let mut perm = m.permissions();
                if perm.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = std::fs::set_permissions(path, perm);
                }
            }
            f(path)
        }
        other => other,
    }
}

/// 中にファイルが1つでもあるか（空のディレクトリだけなら false）。
fn has_files(dir: &Path) -> std::io::Result<bool> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            if has_files(&e.path())? {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

fn remove_partials(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        if e.file_name().to_string_lossy().ends_with(layout::PARTIAL_SUFFIX) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::backend::PermissionPreset;
    use std::io::Write;

    fn temp_root(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!("agentdock-store-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn key(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    #[test]
    fn space_check_requires_the_margin() {
        assert!(space_check(10, FREE_SPACE_MARGIN_BYTES + 10).is_ok());
        match space_check(10, FREE_SPACE_MARGIN_BYTES + 9) {
            Err(StoreError::InsufficientSpace { required, available }) => {
                assert_eq!(required, FREE_SPACE_MARGIN_BYTES + 10);
                assert_eq!(available, FREE_SPACE_MARGIN_BYTES + 9);
            }
            other => panic!("expected InsufficientSpace, got {other:?}"),
        }
        assert!(matches!(space_check(u64::MAX, u64::MAX - 1), Err(StoreError::InsufficientSpace { .. })));
    }

    #[test]
    fn saved_chat_and_settings_come_back_after_reopen() {
        let root = temp_root("restore");
        let store = Store::open(root.clone()).unwrap();
        let (dir_id, _) = (LocalId("dir-test-1".into()), ());
        let mut f = ChatLocalFile::new(dir_id.clone(), Some(key("t1")));
        f.pinned = true;
        f.model = Some(ModelChoice { model: "m".into(), effort: None, speed_tier: None });
        f.permission = Some(PermissionPreset::ReadOnly);
        f.draft.text = "途中の文章".into();
        store.save_chat_local(&f).unwrap();
        let mut s = AppSettings::default();
        s.autostart = true;
        store.save_settings(&AppSettingsFile::new(s.clone())).unwrap();

        let store2 = Store::open(root.clone()).unwrap();
        let r = store2.load_all();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.settings.unwrap().settings, s);
        assert_eq!(r.chats.len(), 1);
        assert_eq!(r.chats[0].local, f);
        // 復元後、同じチャットは同じ領域に結び付く。
        assert_eq!(store2.ensure_dir_id(&key("t1")), dir_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_file_is_moved_aside_and_not_overwritten() {
        let root = temp_root("corrupt");
        let store = Store::open(root.clone()).unwrap();
        let file = root.join(layout::SETTINGS_FILE);
        std::fs::write(&file, b"{broken").unwrap();
        let r = store.load_all();
        assert!(matches!(r.problems.as_slice(), [StoreError::Corrupt { moved_to: Some(_), .. }]));
        assert!(!file.exists(), "the unreadable file is moved away");
        let aside: Vec<_> = std::fs::read_dir(&root).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("settings.json.corrupt-")).collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read(aside[0].path()).unwrap(), b"{broken", "contents are kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newer_schema_file_is_left_untouched_and_writes_are_refused() {
        let root = temp_root("newer");
        let store = Store::open(root.clone()).unwrap();
        let file = root.join(layout::SETTINGS_FILE);
        let newer = format!("{{\"schemaVersion\":{},\"settings\":{{}}}}", SCHEMA_VERSION + 1);
        std::fs::write(&file, &newer).unwrap();
        let r = store.load_all();
        assert!(matches!(r.problems.as_slice(), [StoreError::NewerSchema { .. }]));
        assert!(matches!(store.save_settings(&AppSettingsFile::new(AppSettings::default())), Err(StoreError::NewerSchema { .. })));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupted_copy_and_sending_entry_are_not_trusted_after_restart() {
        let root = temp_root("interrupted");
        let store = Store::open(root.clone()).unwrap();
        let mut f = ChatLocalFile::new(LocalId("dir-x".into()), Some(key("t2")));
        f.attachments.push(AttachmentEntry {
            id: LocalId("att-1".into()),
            chat: key("t2"),
            kind: AttachmentKind::File,
            display_name: "a.txt".into(),
            source: AttachmentSource::ClipboardImage,
            copy_path: Some("x".into()),
            size: Known::NotFetched,
            attached_at: UnixMillis(1),
            state: AttachmentState::Copying,
            used_by: vec![],
        });
        store.save_chat_local(&f).unwrap();
        let att_dir = layout::chat_dir(&root, &f.dir_id).join(layout::ATTACHMENTS_DIR).join("att-1");
        std::fs::create_dir_all(&att_dir).unwrap();
        std::fs::write(att_dir.join("a.txt.partial"), b"half").unwrap();
        let entry = QueueEntry {
            id: LocalId("q1".into()),
            chat: key("t2"),
            text: "go".into(),
            attachments: vec![],
            order: 0,
            registered_at: UnixMillis(1),
            state: QueueEntryState::Sending { attempt: LocalId("a1".into()) },
            attempts: vec![],
            applied: None,
        };
        let q = QueueFile {
            schema_version: SCHEMA_VERSION,
            queue: ChatQueue { chat: key("t2"), run: QueueRun::Active, hold: None, baseline_at: None, awaiting: None, entries: vec![entry], next_order: 1 },
            unresolved_sends: vec![],
        };
        store.save_queue(&q).unwrap();

        let r = Store::open(root.clone()).unwrap().load_all();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let c = &r.chats[0];
        assert!(matches!(c.local.attachments[0].state, AttachmentState::CopyFailed { reason: CopyFailure::Interrupted, .. }));
        assert_eq!(c.local.attachments[0].copy_path, None);
        assert!(!att_dir.join("a.txt.partial").exists());
        let queue = &c.queue.as_ref().unwrap().queue;
        assert_eq!(queue.run, QueueRun::PausedAfterRestart);
        assert_eq!(queue.entries[0].state, QueueEntryState::AcceptanceUnknown { attempt: LocalId("a1".into()) });
        // 変換結果は保存し直されている（再度の起動でも Sending に戻らない）。
        let again = Store::open(root.clone()).unwrap().load_all();
        assert_eq!(again.chats[0].queue.as_ref().unwrap().queue.entries[0].state, QueueEntryState::AcceptanceUnknown { attempt: LocalId("a1".into()) });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_chat_area_is_under_chats_and_empty_ones_are_discarded() {
        let root = temp_root("area");
        let store = Store::open(root.clone()).unwrap();
        let (id, dir) = store.create_chat_dir().unwrap();
        assert!(layout::is_inside(&root.join(layout::CHATS_DIR), &layout::workspace_dir(&dir)));
        assert!(layout::workspace_dir(&dir).is_dir());
        assert!(store.discard_chat_dir_if_empty(&id).unwrap());
        assert!(!dir.exists());
        let (id2, dir2) = store.create_chat_dir().unwrap();
        std::fs::write(layout::workspace_dir(&dir2).join("keep.txt"), b"x").unwrap();
        assert!(!store.discard_chat_dir_if_empty(&id2).unwrap(), "non-empty areas are kept");
        assert!(dir2.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn activity_lines_append_and_a_cut_last_line_is_ignored() {
        let root = temp_root("activity");
        let store = Store::open(root.clone()).unwrap();
        let k = key("t3");
        store.append_activity(&k, &ActivityLine::Freshness { at: UnixMillis(2), agent: None, freshness: Freshness::Live }).unwrap();
        let path = layout::chat_dir(&root, &store.ensure_dir_id(&k)).join(layout::ACTIVITY_FILE);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"kind\":\"freshn").unwrap();
        let got = store.read_activity(&k).unwrap();
        assert_eq!(got.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// チャット領域を2つと、領域の外のファイルを用意する。
    fn two_areas(name: &str) -> (PathBuf, Store, PathBuf, PathBuf) {
        let root = temp_root(name);
        let store = Store::open(root.clone()).unwrap();
        let mut dirs = Vec::new();
        for (id, chat) in [("dir-a", "ta"), ("dir-b", "tb")] {
            let f = ChatLocalFile::new(LocalId(id.into()), Some(key(chat)));
            store.save_chat_local(&f).unwrap();
            let dir = layout::chat_dir(&root, &f.dir_id);
            std::fs::create_dir_all(dir.join(layout::ATTACHMENTS_DIR).join("att-1")).unwrap();
            std::fs::write(dir.join(layout::ATTACHMENTS_DIR).join("att-1").join("copy.txt"), b"12345").unwrap();
            std::fs::create_dir_all(layout::workspace_dir(&dir)).unwrap();
            std::fs::write(layout::workspace_dir(&dir).join("out.txt"), b"abc").unwrap();
            dirs.push(dir);
        }
        (root, store, dirs[0].clone(), dirs[1].clone())
    }

    #[test]
    fn remove_chat_dir_removes_only_that_area_and_nothing_outside() {
        let (root, store, a, b) = two_areas("remove");
        // 領域の外: 元ファイル相当・設定・他チャット。
        let outside = root.join("original-elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("src.txt"), b"keep").unwrap();
        std::fs::write(root.join("settings.json"), b"{}").unwrap();
        store.remove_chat_dir(&key("ta")).unwrap();
        assert!(!a.exists(), "the chat area is gone");
        assert!(b.join(layout::CHAT_FILE).exists() && b.join(layout::ATTACHMENTS_DIR).join("att-1").join("copy.txt").exists(), "other chats are untouched");
        assert_eq!(std::fs::read(outside.join("src.txt")).unwrap(), b"keep");
        assert!(root.join("settings.json").exists());
        // 結び付けも外れる。もう一度呼んでも他に触れず成功する。
        store.remove_chat_dir(&key("ta")).unwrap();
        assert!(b.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_chat_dir_for_an_unknown_chat_touches_nothing() {
        let (root, store, a, b) = two_areas("remove-unknown");
        store.remove_chat_dir(&key("never-seen")).unwrap();
        assert!(a.exists() && b.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_saved_dir_id_that_tries_to_leave_the_area_is_confined_to_chats() {
        let root = temp_root("escape");
        let store = Store::open(root.clone()).unwrap();
        let victim = root.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), b"k").unwrap();
        store.register_dir(&key("evil"), &LocalId(r"..\victim".into()));
        // dirId は安全化され `chats\.._victim` になる。`victim` には届かない。
        let _ = store.remove_chat_dir(&key("evil"));
        assert!(victim.join("keep.txt").exists());
        assert!(root.join(layout::CHATS_DIR).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_only_files_are_removed_and_a_failure_is_reported_not_hidden() {
        let (root, store, a, _b) = two_areas("readonly");
        let ro = layout::workspace_dir(&a).join("out.txt");
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&ro, perm).unwrap();
        store.remove_chat_dir(&key("ta")).unwrap();
        assert!(!a.exists());
        // 開いたままのファイルは削除できない。完了と偽らず、残ったものを返し、結び付けを残す。
        let f = ChatLocalFile::new(LocalId("dir-c".into()), Some(key("tc")));
        store.save_chat_local(&f).unwrap();
        let c = layout::chat_dir(&root, &f.dir_id);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::OpenOptions::new().write(true).create(true).share_mode(0).open(c.join("held.bin")).unwrap();
            let r = store.remove_chat_dir(&key("tc"));
            let (done, failed) = r.expect_err("a file held open cannot be deleted");
            assert!(failed.iter().any(|f| f.contains("held.bin")), "{failed:?}");
            assert!(done.iter().any(|d| d == "chat.json"), "{done:?}");
            assert!(c.join("held.bin").exists());
            drop(held);
            store.remove_chat_dir(&key("tc")).unwrap();
            assert!(!c.exists(), "retry after release completes the deletion");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn baselines_append_read_usage_and_delete() {
        use crate::backend::baseline::*;
        let (root, store, a, b) = two_areas("baselines");
        let seg = LocalId("seg-1".into());
        store.append_baseline(&key("ta"), &BaselineLine::Bound { seg: seg.clone(), turn: ExternalId("t1".into()) }).unwrap();
        store.append_baseline(&key("ta"), &BaselineLine::Abandoned { seg: seg.clone(), reason: "r".into() }).unwrap();
        // 途中で切れた最終行と、新しい版の行は読み飛ばす。
        let file = layout::baseline_segments_file(&a);
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("{\"schemaVersion\":2,\"kind\":\"bound\",\"seg\":\"x\",\"turn\":\"y\"}\n{\"schemaVersion\":1,\"kind\":\"bou");
        std::fs::write(&file, text).unwrap();
        let lines = store.read_baselines(&key("ta")).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], BaselineLine::Bound { seg: seg.clone(), turn: ExternalId("t1".into()) });
        assert!(store.read_baselines(&key("tb")).unwrap().is_empty(), "chats are separate");

        // 旧方式のファイル（changes.jsonl・revert-backup）は読まず、消さない。使用量には別枠で数える。
        std::fs::write(a.join(layout::CHANGES_FILE), vec![0u8; 13]).unwrap();
        std::fs::create_dir_all(a.join(layout::REVERT_BACKUP_DIR).join("1")).unwrap();
        std::fs::write(a.join(layout::REVERT_BACKUP_DIR).join("1").join("f.bin"), vec![0u8; 7]).unwrap();
        let (objects, tmp) = store.ensure_baseline_dirs(&key("ta")).unwrap();
        assert_eq!(objects, layout::baseline_objects_dir(&a));
        assert_eq!(tmp, layout::baseline_tmp_dir(&a));
        let obj = objects.join("ab").join("cdef");
        std::fs::create_dir_all(obj.parent().unwrap()).unwrap();
        std::fs::write(&obj, vec![0u8; 100]).unwrap();
        let mut perm = std::fs::metadata(&obj).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&obj, perm).unwrap();

        let u = store.usage(Some(&key("ta"))).chats[0].breakdown;
        assert_eq!(u.revert_backups, 7);
        assert_eq!(u.baselines, std::fs::metadata(&file).unwrap().len() + 100);
        let chat_json = std::fs::metadata(a.join(layout::CHAT_FILE)).unwrap().len();
        let queue_json = std::fs::metadata(a.join(layout::QUEUE_FILE)).map(|m| m.len()).unwrap_or(0);
        assert_eq!(u.metadata, chat_json + queue_json + 13, "the old changes.jsonl stays in metadata and nothing is counted twice");

        // 明示の削除で baselines だけが消える（読み取り専用のオブジェクトも）。
        store.delete_baselines(&key("ta")).unwrap();
        assert!(!layout::baselines_dir(&a).exists());
        assert!(a.join(layout::CHANGES_FILE).exists() && a.join(layout::REVERT_BACKUP_DIR).exists());
        assert!(store.read_baselines(&key("ta")).unwrap().is_empty());
        store.delete_baselines(&key("ta")).unwrap();

        // チャット削除は baselines（読み取り専用のオブジェクトを含む）も一緒に消す。
        let (objects_b, _) = store.ensure_baseline_dirs(&key("tb")).unwrap();
        let obj_b = objects_b.join("12").join("3456");
        std::fs::create_dir_all(obj_b.parent().unwrap()).unwrap();
        std::fs::write(&obj_b, b"x").unwrap();
        let mut perm = std::fs::metadata(&obj_b).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&obj_b, perm).unwrap();
        store.remove_chat_dir(&key("tb")).unwrap();
        assert!(!b.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_counts_each_part_once_and_chat_scope_excludes_other_chats_and_settings() {
        let (root, store, a, _b) = two_areas("usage");
        std::fs::write(root.join("settings.json"), vec![0u8; 7]).unwrap();
        std::fs::write(a.join(layout::ACTIVITY_FILE), vec![0u8; 11]).unwrap();
        // 成果物の台帳（作業領域内の out.txt=3バイトは、作業領域ではなく成果物として数える）。
        let mut f = ChatLocalFile::new(LocalId("dir-a".into()), Some(key("ta")));
        f.artifacts.push(ArtifactEntry {
            id: LocalId("art-1".into()),
            chat: key("ta"),
            agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("ta".into()) },
            item: None,
            path: layout::workspace_dir(&a).join("out.txt").to_string_lossy().into_owned(),
            observed_at: UnixMillis(1),
            exists: Known::direct(true),
            checked_at: None,
            in_chat_area: true,
        });
        store.save_chat_local(&f).unwrap();
        let all = store.usage(None);
        assert!(all.unreadable.is_empty(), "{:?}", all.unreadable);
        let ua = all.chats.iter().find(|c| c.chat == key("ta")).unwrap().breakdown;
        assert_eq!((ua.attachments, ua.artifacts, ua.workspace, ua.activity), (5, 3, 0, 11));
        let ub = all.chats.iter().find(|c| c.chat == key("tb")).unwrap().breakdown;
        assert_eq!((ub.attachments, ub.artifacts, ub.workspace, ub.activity), (5, 0, 3, 0));
        let sum = |b: &UsageBreakdown| b.attachments + b.artifacts + b.workspace + b.activity + b.metadata + b.baselines + b.revert_backups;
        // 合計 = 各チャットの合計 + チャット領域の外（settings.json の7バイト）。
        assert_eq!(sum(&all.total), sum(&ua) + sum(&ub) + 7);
        let one = store.usage(Some(&key("ta")));
        assert_eq!(one.chats.len(), 1);
        assert_eq!(one.total, one.chats[0].breakdown, "the chat scope does not include settings or other chats");
        assert_eq!(one.legacy_area, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_area_counts_only_chat_dirs() {
        let root = temp_root("legacy");
        let legacy = root.join("chats");
        std::fs::create_dir_all(legacy.join("chat-1")).unwrap();
        std::fs::write(legacy.join("chat-1").join("a.txt"), b"1234").unwrap();
        std::fs::create_dir_all(legacy.join("other")).unwrap();
        std::fs::write(legacy.join("other").join("b.txt"), b"99999999").unwrap();
        assert_eq!(legacy_area_usage(&legacy), (4, vec![]));
        assert_eq!(legacy_area_usage(&root.join("missing")), (0, vec![]));
        let _ = std::fs::remove_dir_all(&root);
    }

}
