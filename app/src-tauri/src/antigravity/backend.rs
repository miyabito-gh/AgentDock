//! `AiBackend` の Antigravity（`agy` CLI）実装。1ターン=1プロセス（print モード）。
//!
//! 規則（DESIGN_T1・要件§3、Codex アダプターと同じ）:
//! - 送信は再送しない。プロセスを起動できたら受理、起動できなければ明示拒否（何も実行していない）。
//! - `read` / `list_*` は記録を読むだけ。プロセスを起動しない・resume しない（M11）。
//! - 承認は print モードで自動拒否される。承認要求は発生せず、拒否した操作は警告と記録に残す。
//! - 中断は要求の受付のみ。停止は読取り task がプロセス終了を観測して出す `TurnEnded` で照合する。
//! - 通信断・出力を読めない・プロセスの終了を確認できない場合は、完了にも失敗にもせず結果不明にする。
//! - イベントは有界キューへ。溢れたら黙って捨てず `Gap` を送る（[`Outbox`]）。
//! - `Connection` イベントは出さない（監視元は Codex の1つで、接続表示はホストの Mux が決める）。

use super::cmd::{self, TurnLaunch};
use super::store::{ChatStore, StoredChat, StoredTurn, TurnStatus};
use super::stream::{parse_line, ExitInfo, TurnStream};
use super::view::*;
use crate::backend::backend::*;
use crate::backend::model::*;
use crate::backend::parity::ParityOps;
use crate::codex::outbox::Outbox;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

const EVENT_QUEUE_CAPACITY: usize = 4096;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);
const MODELS_CACHE_TTL: Duration = Duration::from_secs(60);
const KILL_TIMEOUT: Duration = Duration::from_secs(10);
/// 出力が終わってから、プロセスの終了を待つ最大時間。
const EXIT_WAIT: Duration = Duration::from_secs(15);
/// `result` の後、残りの出力を捨てて終了を待つ最大時間。
const DRAIN_WAIT: Duration = Duration::from_secs(10);
/// stderr の末尾として保持する量。
const STDERR_KEEP_BYTES: usize = 4096;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static SOURCE_COUNTER: AtomicU64 = AtomicU64::new(1);
static CHAT_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ───────────────────────────── 能力宣言 ─────────────────────────────

/// 接続後に確定する能力（agy 1.5.x の print モードで確認した範囲）。
pub fn antigravity_capabilities() -> Capabilities {
    Capabilities {
        descendant_monitoring: Support::Unsupported,
        descendant_search: Support::Unsupported,
        history_read_without_resume: Support::Supported,
        resume: Support::Supported,
        list_loaded: Support::Unsupported,
        // print モードは承認が必要な操作を自動拒否する。承認要求は発生しない。
        approval_kinds: Vec::new(),
        steer: Support::Unsupported,
        interrupt: Support::Supported,
        model_selection: Support::Supported,
        effort_selection: Support::Unsupported,
        rename: Support::Supported,
        archive: Support::Supported,
        delete: Support::Supported,
        external_history: Support::Unsupported,
        attachment_kinds: Vec::new(),
        managed_exec_control: Support::Unsupported,
        projects: Support::Unsupported,
        workspace_roots: Support::Unsupported,
        // usage は取れるが、使用率の上限（文脈の大きさ）が分からない。
        context_usage: Support::Unsupported,
        structured_output: Support::Unsupported,
        ops: Vec::new(),
    }
}

fn unknown_capabilities() -> Capabilities {
    Capabilities {
        descendant_monitoring: Support::Unknown,
        descendant_search: Support::Unknown,
        history_read_without_resume: Support::Unknown,
        resume: Support::Unknown,
        list_loaded: Support::Unknown,
        approval_kinds: Vec::new(),
        steer: Support::Unknown,
        interrupt: Support::Unknown,
        model_selection: Support::Unknown,
        effort_selection: Support::Unknown,
        rename: Support::Unknown,
        archive: Support::Unknown,
        delete: Support::Unknown,
        external_history: Support::Unknown,
        attachment_kinds: Vec::new(),
        managed_exec_control: Support::Unknown,
        projects: Support::Unknown,
        workspace_roots: Support::Unknown,
        context_usage: Support::Unknown,
        structured_output: Support::Unknown,
        ops: Vec::new(),
    }
}

// ───────────────────────────── プロセス実行の補助 ─────────────────────────────

enum RunError {
    Spawn(std::io::Error),
    Timeout,
}

struct Captured {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// 短い読取り専用の実行（`--version`・`models`）。タイムアウトで子プロセスも止める。
async fn run_agy(exe: &str, args: &[&str], timeout: Duration) -> Result<Captured, RunError> {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => Err(RunError::Timeout),
        Ok(Err(e)) => Err(RunError::Spawn(e)),
        Ok(Ok(o)) => Ok(Captured {
            success: o.status.success(),
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
    }
}

fn head(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(max).collect::<String>())
    }
}

fn launch_failure(e: RunError) -> (LaunchFailure, String) {
    match e {
        RunError::Spawn(e) => {
            let kind = match e.kind() {
                std::io::ErrorKind::NotFound => LaunchFailure::NotFound,
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied => LaunchFailure::InvalidPath,
                _ => LaunchFailure::SpawnFailed,
            };
            let message = match kind {
                LaunchFailure::NotFound => format!("agy が見つかりません（%LOCALAPPDATA%\\agy\\bin\\agy.exe または PATH 上の agy が必要です）: {e}"),
                _ => format!("agy を起動できませんでした: {e}"),
            };
            (kind, message)
        }
        RunError::Timeout => (LaunchFailure::VersionCheckFailed, format!("agy --version が {} 秒以内に応答しませんでした", VERSION_TIMEOUT.as_secs())),
    }
}

/// プロセスツリーの強制終了の失敗。
struct KillError {
    /// 対象のプロセスが既に無かった。
    gone: bool,
    message: String,
}

/// プロセスとその子孫を強制終了する。Windows では `taskkill /PID <pid> /T /F`
/// （agy が起動した子が残らないよう、標準の kill ではなくツリーごと止める）。
async fn kill_tree(pid: u32) -> Result<(), KillError> {
    #[cfg(windows)]
    let mut cmd = {
        let system = std::env::var_os("SystemRoot").map(|r| std::path::PathBuf::from(r).join("System32").join("taskkill.exe")).filter(|p| p.is_file());
        let mut c = Command::new(system.unwrap_or_else(|| "taskkill".into()));
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("kill");
        c.args(["-KILL", &pid.to_string()]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    match tokio::time::timeout(KILL_TIMEOUT, cmd.output()).await {
        Err(_) => Err(KillError { gone: false, message: "プロセスの停止要求が時間内に完了しませんでした".into() }),
        Ok(Err(e)) => Err(KillError { gone: false, message: format!("プロセスの停止要求を実行できませんでした: {e}") }),
        Ok(Ok(o)) if o.status.success() => Ok(()),
        // taskkill の 128 は「そのプロセスは存在しない」。
        Ok(Ok(o)) => {
            let text = format!("{} {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            Err(KillError { gone: o.status.code() == Some(128), message: format!("プロセスの停止要求が失敗しました（終了コード {:?}）: {}", o.status.code(), head(&text, 300)) })
        }
    }
}

// ───────────────────────────── 共有状態 ─────────────────────────────

#[derive(Clone)]
struct ConnInfo {
    exe: String,
}

/// 実行中の turn（プロセス）。
struct LiveTurn {
    pid: Option<u32>,
    /// 中断を依頼してプロセスを止めた（終了の観測時に Interrupted として扱う）。
    interrupted: AtomicBool,
    /// 終端を確定した（以後の中断は NotRunning）。
    done: AtomicBool,
}

struct Shared {
    conn: Mutex<Option<ConnInfo>>,
    /// 直近の監視元ID。切断後も、実行中の turn のイベントはこの ID で出す。
    source: Mutex<Option<SourceId>>,
    caps: Mutex<Capabilities>,
    store: Mutex<Option<ChatStore>>,
    live: Mutex<HashMap<TurnKey, Arc<LiveTurn>>>,
    models: Mutex<Option<(Instant, Vec<ModelInfo>)>>,
    outbox: Arc<Outbox>,
    event_tx: mpsc::Sender<EventEnvelope>,
    event_rx: Mutex<Option<EventReceiver>>,
    forwarder_started: AtomicBool,
    /// connect / disconnect を直列化する。
    lifecycle: AsyncMutex<()>,
}

fn unknown_chat() -> BackendError {
    BackendError::Rejected { code: None, message: "この会話は AgentDock の Antigravity の記録にありません".into() }
}

fn rejected(message: impl Into<String>) -> BackendError {
    BackendError::Rejected { code: None, message: message.into() }
}

impl Shared {
    fn emit(&self, event: BackendEvent) {
        let source = self.source.lock().unwrap().clone();
        if let Some(source) = source {
            self.outbox.push(&source, event);
        }
    }

    fn ready(&self) -> BackendResult<ConnInfo> {
        self.conn.lock().unwrap().clone().ok_or(BackendError::NotConnected)
    }

    fn with_store<R>(&self, f: impl FnOnce(&mut ChatStore) -> R) -> BackendResult<R> {
        let mut g = self.store.lock().unwrap();
        g.as_mut().map(f).ok_or(BackendError::NotConnected)
    }

    /// 保存する（一時の会話は保存しない）。
    fn persist(store: &ChatStore, chat_id: &str) -> Result<(), String> {
        if store.get(chat_id).is_some_and(|c| c.ephemeral) {
            return Ok(());
        }
        store.save()
    }

    /// 最初の turn で判明した agy の会話IDを保存する。
    fn remember_conversation(&self, chat_id: &str, conversation_id: &str) {
        let r = self.with_store(|s| {
            if let Some(c) = s.get_mut(chat_id) {
                if c.conversation_id.is_none() {
                    c.conversation_id = Some(conversation_id.to_string());
                }
            }
            Self::persist(s, chat_id)
        });
        if let Ok(Err(e)) = r {
            crate::diag::log("antigravity", &format!("saving conversation id failed: {e}"));
            self.emit(BackendEvent::Unrecognized { raw_label: "antigravity.store".into(), note: Some(format!("会話IDを保存できませんでした（{e}）")) });
        }
    }

    /// turn の確定内容を保存する。保存に失敗したら理由を返す。
    fn record_final(&self, chat_id: &str, turn_id: &str, fin: &super::stream::Finished, ended_at: Option<i64>) -> Option<String> {
        let r = self.with_store(|s| {
            if let Some(t) = s.get_mut(chat_id).and_then(|c| c.turns.iter_mut().find(|t| t.turn_id == turn_id)) {
                t.status = fin.status;
                t.ended_at = ended_at;
                t.response_text = fin.response.clone();
                t.error = fin.error.clone();
                t.denied_actions = fin.denied.clone();
                t.usage = fin.usage.clone();
            }
            Self::persist(s, chat_id)
        });
        match r {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(e) => Some(e.to_string()),
        }
    }
}

// ───────────────────────────── アダプター ─────────────────────────────

pub struct AntigravityBackend {
    shared: Arc<Shared>,
}

impl Default for AntigravityBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// 一覧の絞り込み用にパスを正規化する（区切り・末尾の区切り・大文字小文字の違いを同じとみなす）。
fn norm_path(p: &str) -> String {
    let mut s = p.trim().replace('/', "\\").to_lowercase();
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

fn new_chat_id(store: &ChatStore) -> String {
    loop {
        let id = format!("agy-{:x}-{:x}", now_ms(), CHAT_COUNTER.fetch_add(1, Ordering::SeqCst));
        if !store.contains(&id) {
            return id;
        }
    }
}

impl AntigravityBackend {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(64);
        AntigravityBackend {
            shared: Arc::new(Shared {
                conn: Mutex::new(None),
                source: Mutex::new(None),
                caps: Mutex::new(unknown_capabilities()),
                store: Mutex::new(None),
                live: Mutex::new(HashMap::new()),
                models: Mutex::new(None),
                outbox: Arc::new(Outbox::new(EVENT_QUEUE_CAPACITY)),
                event_tx: tx,
                event_rx: Mutex::new(Some(rx)),
                forwarder_started: AtomicBool::new(false),
                lifecycle: AsyncMutex::new(()),
            }),
        }
    }

    /// 接続の前処理（送出 task の起動・監視元の発行・記録の読込み）。agy の起動可否とは独立。
    fn prepare(&self, app_data_dir: &str) -> BackendResult<SourceId> {
        if app_data_dir.trim().is_empty() {
            return Err(BackendError::Io { message: "app_data_dir is empty".into() });
        }
        if !self.shared.forwarder_started.swap(true, Ordering::SeqCst) {
            tokio::spawn(self.shared.outbox.clone().forward(self.shared.event_tx.clone()));
        }
        if self.shared.conn.lock().unwrap().is_some() {
            return Err(BackendError::Protocol { message: "already connected".into() });
        }
        let source = SourceId(format!("antigravity-{}", SOURCE_COUNTER.fetch_add(1, Ordering::SeqCst)));
        *self.shared.source.lock().unwrap() = Some(source.clone());
        // 記録は最初の1回だけ読む（再接続で読み直すと、実行中の turn を結果不明にしてしまう）。
        let mut store = self.shared.store.lock().unwrap();
        if store.is_none() {
            let dir = Path::new(app_data_dir).join("antigravity");
            let (loaded, warnings) = ChatStore::load(&dir, now_ms());
            *store = Some(loaded);
            drop(store);
            for w in warnings {
                crate::diag::log("antigravity", "chat store warning");
                self.shared.emit(BackendEvent::Unrecognized { raw_label: "antigravity.store".into(), note: Some(w) });
            }
        }
        Ok(source)
    }

    /// `agy models` の結果から既定のモデルを選ぶ。取れなければ `Io`。
    async fn default_model(&self) -> BackendResult<String> {
        let models = self.list_models(ModelQuery::default()).await?;
        models
            .iter()
            .find(|m| m.is_default)
            .or(models.first())
            .map(|m| m.id.clone())
            .ok_or_else(|| BackendError::Io { message: "利用できる Gemini 系モデルを agy から取得できませんでした".into() })
    }

    /// 指定 → 保存済み → 一覧の既定、の順でモデルを決める。指定が Gemini 系でなければ拒否。
    async fn resolve_model(&self, requested: Option<&str>, stored: Option<&str>) -> BackendResult<String> {
        if let Some(m) = requested {
            if !cmd::is_supported_model(m) {
                return Err(rejected(format!("Antigravity では Gemini 系のモデルだけを選べます（{m}）")));
            }
            return Ok(m.to_string());
        }
        if let Some(m) = stored {
            return Ok(m.to_string());
        }
        self.default_model().await
    }

    fn check_dirs(dirs: &[&str]) -> BackendResult<()> {
        for d in dirs {
            if d.trim().is_empty() || !Path::new(d).is_dir() {
                return Err(rejected(format!("作業フォルダが見つかりません: {d}")));
            }
        }
        Ok(())
    }

    /// 送信の記録を取り消す（プロセスを起動できなかったとき。何も実行していない）。
    fn rollback_send(&self, chat_id: &str, turn_id: &str, backup: StoredChat) {
        let r = self.shared.with_store(|s| {
            if let Some(c) = s.get_mut(chat_id) {
                c.turns.retain(|t| t.turn_id != turn_id);
                c.model = backup.model;
                c.permission = backup.permission;
                c.cwd = backup.cwd;
                c.extra_folders = backup.extra_folders;
                c.last_used_at = backup.last_used_at;
            }
            Shared::persist(s, chat_id)
        });
        if let Ok(Err(e)) = r {
            crate::diag::log("antigravity", &format!("rollback save failed: {e}"));
        }
    }

    /// テスト用: agy を実行せずに接続済みの状態にする（`exe` は偽の実行ファイル）。
    #[cfg(test)]
    pub(crate) fn attach_for_test(&self, exe: &str, app_data_dir: &str) {
        self.prepare(app_data_dir).expect("prepare");
        *self.shared.caps.lock().unwrap() = antigravity_capabilities();
        *self.shared.conn.lock().unwrap() = Some(ConnInfo { exe: exe.to_string() });
    }
}

#[async_trait]
impl AiBackend for AntigravityBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Antigravity
    }

    fn capabilities(&self) -> Capabilities {
        self.shared.caps.lock().unwrap().clone()
    }

    async fn connect(&self, config: ConnectConfig) -> BackendResult<SourceInfo> {
        let _lifecycle = self.shared.lifecycle.lock().await;
        let source = self.prepare(&config.app_data_dir)?;
        let started_at = UnixMillis(now_ms());
        let exe = cmd::resolve_executable(config.executable.as_deref());
        let failed = |reason: LaunchFailure, message: String, version: VersionCheck| SourceInfo {
            source: source.clone(),
            backend: BackendKind::Antigravity,
            pid: Known::Unsupported,
            started_at,
            version,
            capabilities: self.capabilities(),
            connection: ConnectionState::LaunchFailed { reason, message },
            sandbox_workaround: config.sandbox_workaround,
        };
        // 起動不能は LaunchFailed（作業の失敗ではない）。接続イベントは出さない。
        let out = match run_agy(&exe, &["--version"], VERSION_TIMEOUT).await {
            Ok(o) => o,
            Err(e) => {
                let (reason, message) = launch_failure(e);
                return Ok(failed(reason, message.clone(), VersionCheck::Unknown { message }));
            }
        };
        if !out.success {
            let message = format!("agy --version が終了コード {:?} で終了しました: {}", out.code, head(&out.stderr, 300));
            return Ok(failed(LaunchFailure::VersionCheckFailed, message.clone(), VersionCheck::Unknown { message }));
        }
        let version = cmd::classify_version(&out.stdout);
        let caps = antigravity_capabilities();
        *self.shared.caps.lock().unwrap() = caps.clone();
        *self.shared.conn.lock().unwrap() = Some(ConnInfo { exe });
        Ok(SourceInfo {
            source,
            backend: BackendKind::Antigravity,
            // 常駐プロセスを持たない（1ターン=1プロセス）。
            pid: Known::Unsupported,
            started_at,
            version,
            capabilities: caps,
            connection: ConnectionState::Connected,
            sandbox_workaround: config.sandbox_workaround,
        })
    }

    async fn disconnect(&self) -> BackendResult<()> {
        let _lifecycle = self.shared.lifecycle.lock().await;
        // 接続を閉じるだけ。実行中のプロセスは止めない（終端は読取り task が観測する）。
        *self.shared.conn.lock().unwrap() = None;
        Ok(())
    }

    fn take_events(&self) -> Option<EventReceiver> {
        self.shared.event_rx.lock().unwrap().take()
    }

    async fn start_chat(&self, params: StartChatParams) -> BackendResult<ChatStarted> {
        self.shared.ready()?;
        let model = self.resolve_model(params.model.as_ref().map(|m| m.model.as_str()), None).await?;
        let dirs: Vec<&str> = std::iter::once(params.cwd.as_str()).chain(params.extra_folders.iter().map(String::as_str)).collect();
        Self::check_dirs(&dirs)?;
        let extras: Vec<String> = params.extra_folders.iter().filter(|f| **f != params.cwd).cloned().collect();
        let now = now_ms();
        let chat = self
            .shared
            .with_store(|s| {
                let id = new_chat_id(s);
                s.insert(StoredChat {
                    id: id.clone(),
                    conversation_id: None,
                    cwd: params.cwd.clone(),
                    extra_folders: extras,
                    model: Some(model),
                    permission: params.permission,
                    kind: params.kind,
                    name: None,
                    created_at: now,
                    last_used_at: None,
                    archived: false,
                    turns: Vec::new(),
                    ephemeral: params.ephemeral,
                });
                match Shared::persist(s, &id) {
                    Ok(()) => Ok(s.get(&id).cloned()),
                    Err(e) => {
                        s.remove(&id);
                        Err(BackendError::Io { message: format!("チャットの記録を保存できませんでした: {e}") })
                    }
                }
            })??
            .ok_or_else(unknown_chat)?;
        // プロセスは起動していない。agy が受理した設定はまだ無いので NotFetched / 非対応。
        Ok(ChatStarted { chat: to_chat(&chat), root: to_agent(&chat), accepted_model: Known::NotFetched, accepted_folders: Known::Unsupported })
    }

    async fn list_chats(&self, query: ListChatsQuery) -> BackendResult<Page<ChatSummary>> {
        self.shared.ready()?;
        let offset = match query.cursor.as_deref() {
            None => 0,
            Some(c) => c.parse::<usize>().map_err(|_| rejected("cursor が不正です"))?,
        };
        let needle = query.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);
        let cwds: Option<Vec<String>> = query.cwd.as_ref().map(|v| v.iter().map(|c| norm_path(c)).collect());
        let now = UnixMillis(now_ms());
        self.shared.with_store(|s| {
            let mut found: Vec<&StoredChat> = s
                .chats()
                .iter()
                .filter(|c| !c.ephemeral && c.archived == query.archived)
                .filter(|c| cwds.as_ref().is_none_or(|v| v.contains(&norm_path(&c.cwd))))
                .filter(|c| {
                    needle.as_ref().is_none_or(|n| {
                        c.name.as_deref().is_some_and(|x| x.to_lowercase().contains(n.as_str()))
                            || c.turns.first().is_some_and(|t| t.user_text.to_lowercase().contains(n.as_str()))
                    })
                })
                .collect();
            found.sort_by(|a, b| {
                let key = |c: &StoredChat| (c.last_used_at.unwrap_or(c.created_at), c.created_at);
                key(b).cmp(&key(a)).then_with(|| a.id.cmp(&b.id))
            });
            let total = found.len();
            let page: Vec<&StoredChat> = match query.limit {
                Some(l) => found.into_iter().skip(offset).take(l as usize).collect(),
                None => found.into_iter().skip(offset).collect(),
            };
            let end = offset + page.len();
            Page { items: page.iter().map(|c| to_summary(c, now)).collect(), next_cursor: (end < total).then(|| end.to_string()) }
        })
    }

    async fn read(&self, agent: AgentKey, options: ReadOptions) -> BackendResult<AgentHistory> {
        self.shared.ready()?;
        let now = UnixMillis(now_ms());
        let found = self.shared.with_store(|s| {
            (agent.backend == BackendKind::Antigravity).then(|| s.get(&agent.id.0).map(|c| to_history(c, options.include_turns, EvidenceSource::HistoryRead, "antigravity.store", now))).flatten()
        })?;
        found.ok_or_else(unknown_chat)
    }

    async fn resume(&self, chat: ChatKey, _confirmed: &UserConfirmed, _overrides: Option<&ThreadOverrides>) -> BackendResult<ResumeOutcome> {
        self.shared.ready()?;
        let now = UnixMillis(now_ms());
        let history = self.shared.with_store(|s| {
            (chat.backend == BackendKind::Antigravity).then(|| s.get(&chat.id.0).map(|c| to_history(c, false, EvidenceSource::Response, "antigravity.store", now))).flatten()
        })?;
        // プロセスは起動しない（次の送信で `--conversation` を付けて続ける）。agy が受理したモデルの応答は無い。
        Ok(match history {
            Some(history) => ResumeOutcome::Resumed { history, accepted_model: Known::NotFetched },
            None => ResumeOutcome::Unavailable { reason: "AgentDock の記録に、この会話がありません".into() },
        })
    }

    async fn manage_chat(&self, chat: ChatKey, op: ManageOp, _confirmed: &UserConfirmed) -> BackendResult<ManageOutcome> {
        self.shared.ready()?;
        if chat.backend != BackendKind::Antigravity {
            return Err(unknown_chat());
        }
        let id = chat.id.0.clone();
        let change = self.shared.with_store(|s| -> BackendResult<ChatMetaChange> {
            let c = s.get(&id).ok_or_else(unknown_chat)?;
            if c.running_turn().is_some() {
                return Err(rejected("実行中の turn があるため、この操作はできません"));
            }
            let (change, backup) = match op {
                ManageOp::Rename { name } => {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return Err(rejected("名前が空です"));
                    }
                    let c = s.get_mut(&id).ok_or_else(unknown_chat)?;
                    let backup = c.clone();
                    c.name = Some(name.clone());
                    (ChatMetaChange::Renamed { name }, backup)
                }
                ManageOp::Archive | ManageOp::Unarchive => {
                    let archive = matches!(op, ManageOp::Archive);
                    let c = s.get_mut(&id).ok_or_else(unknown_chat)?;
                    let backup = c.clone();
                    c.archived = archive;
                    (if archive { ChatMetaChange::Archived } else { ChatMetaChange::Unarchived }, backup)
                }
                // AgentDock の記録だけを消す。agy 側の会話データには触れない。
                ManageOp::Delete => {
                    let backup = s.remove(&id).ok_or_else(unknown_chat)?;
                    if let Err(e) = Shared::persist(s, &id) {
                        s.insert(backup);
                        return Err(BackendError::Io { message: format!("記録を保存できませんでした: {e}") });
                    }
                    return Ok(ChatMetaChange::Deleted);
                }
            };
            if let Err(e) = Shared::persist(s, &id) {
                if let Some(c) = s.get_mut(&id) {
                    *c = backup;
                }
                return Err(BackendError::Io { message: format!("記録を保存できませんでした: {e}") });
            }
            Ok(change)
        })??;
        self.shared.emit(BackendEvent::ChatMetaChanged { chat, change });
        Ok(ManageOutcome::Done)
    }

    async fn list_projects(&self) -> BackendResult<Vec<ProjectSummary>> {
        Err(BackendError::Unsupported { capability: "projects".into() })
    }

    async fn list_loaded(&self) -> BackendResult<Vec<AgentKey>> {
        self.shared.ready()?;
        // 常駐するエージェントは無い（1ターン=1プロセス）。
        Ok(Vec::new())
    }

    async fn read_work_folders(&self, chat: &ChatKey) -> BackendResult<Known<WorkFolders>> {
        self.shared.ready()?;
        let id = chat.id.0.clone();
        self.shared.with_store(|s| match s.get(&id).filter(|_| chat.backend == BackendKind::Antigravity) {
            // AgentDock が agy の cwd・--add-dir に渡している値（agy の応答ではないので導出値）。
            Some(c) => Known::Value { value: WorkFolders { folders: std::iter::once(c.cwd.clone()).chain(c.extra_folders.iter().cloned()).collect() }, basis: Basis::Derived },
            None => Known::NotFetched,
        })
    }

    /// 子孫は追跡しない。agy の1ターンは1プロセスで、子の作業はそのターンの終了までに終わる。
    /// 「見つからなかった（完全）」を返す（ホストのキューは走査の完了を待つため、非対応エラーだと送信が保留され続ける）。
    async fn scan_descendants(&self, root: AgentKey) -> BackendResult<DescendantScan> {
        Ok(DescendantScan { root, found: Vec::new(), complete: true })
    }

    async fn send(&self, request: SendRequest) -> SendOutcome {
        let reject = |request: SendRequest, error: BackendError| SendOutcome::Rejected { error, request: NotAccepted::new(request) };
        let unsupported = |what: &str| BackendError::Unsupported { capability: what.into() };
        let conn = match self.shared.ready() {
            Ok(c) => c,
            Err(e) => return reject(request, e),
        };
        // 送れない指定は、黙って外さず何も実行せずに拒否する。
        if matches!(request.mode, SendMode::Steer { .. }) {
            return reject(request, unsupported("steer"));
        }
        if !request.attachments.is_empty() {
            return reject(request, unsupported("attachments"));
        }
        if request.output_schema.is_some() {
            return reject(request, unsupported("structuredOutput"));
        }
        if request.work_mode.is_some() {
            return reject(request, unsupported("workMode"));
        }
        if request.speed_tier.is_some() {
            return reject(request, unsupported("speedTier"));
        }
        if request.text.trim().is_empty() {
            return reject(request, rejected("送る本文が空です"));
        }
        if request.chat.backend != BackendKind::Antigravity {
            return reject(request, unknown_chat());
        }
        let chat_id = request.chat.id.0.clone();
        let snapshot = match self.shared.with_store(|s| s.get(&chat_id).cloned()) {
            Ok(Some(c)) => c,
            Ok(None) => return reject(request, unknown_chat()),
            Err(e) => return reject(request, e),
        };
        if snapshot.running_turn().is_some() {
            return reject(request, rejected("このチャットではまだ実行中の turn があります"));
        }
        let model = match self.resolve_model(request.model.as_ref().map(|m| m.model.as_str()), snapshot.model.as_deref()).await {
            Ok(m) => m,
            Err(e) => return reject(request, e),
        };
        // 作業フォルダ（先頭＝プライマリ＝プロセスの cwd、残りは --add-dir）。指定が無ければチャットの保存値。
        let (cwd, extras): (String, Vec<String>) = match &request.work_folders {
            Some(f) => match f.folders.split_first() {
                Some((primary, rest)) => (primary.clone(), rest.to_vec()),
                None => return reject(request, rejected("作業フォルダの指定が空です")),
            },
            None => (request.cwd.clone().unwrap_or_else(|| snapshot.cwd.clone()), snapshot.extra_folders.clone()),
        };
        let extras: Vec<String> = extras.into_iter().filter(|f| *f != cwd).collect();
        let dirs: Vec<&str> = std::iter::once(cwd.as_str()).chain(extras.iter().map(String::as_str)).collect();
        if let Err(e) = Self::check_dirs(&dirs) {
            return reject(request, e);
        }
        let permission = request.permission.unwrap_or(snapshot.permission);

        // ① 送る前に記録（running）して保存する。保存できなければ何も実行しない。
        let now = now_ms();
        let recorded = self.shared.with_store(|s| -> BackendResult<(String, Option<String>, StoredChat)> {
            let c = s.get_mut(&chat_id).ok_or_else(unknown_chat)?;
            if c.running_turn().is_some() {
                return Err(rejected("このチャットではまだ実行中の turn があります"));
            }
            let backup = c.clone();
            let mut n = c.turns.len() + 1;
            let turn_id = loop {
                let candidate = format!("{chat_id}-t{n}");
                if !c.turns.iter().any(|t| t.turn_id == candidate) {
                    break candidate;
                }
                n += 1;
            };
            c.model = Some(model.clone());
            c.permission = permission;
            c.cwd = cwd.clone();
            c.extra_folders = extras.clone();
            c.last_used_at = Some(now);
            c.turns.push(StoredTurn {
                turn_id: turn_id.clone(),
                client_message_id: request.client_message_id.clone(),
                user_text: request.text.clone(),
                response_text: String::new(),
                status: TurnStatus::Running,
                started_at: now,
                ended_at: None,
                denied_actions: Vec::new(),
                error: None,
                usage: None,
            });
            let conversation = c.conversation_id.clone();
            if let Err(e) = Shared::persist(s, &chat_id) {
                if let Some(c) = s.get_mut(&chat_id) {
                    *c = backup;
                }
                return Err(BackendError::Io { message: format!("送信前の記録を保存できなかったため、送りません: {e}") });
            }
            Ok((turn_id, conversation, backup))
        });
        let (turn_id, conversation, backup) = match recorded.and_then(|r| r) {
            Ok(x) => x,
            Err(e) => return reject(request, e),
        };

        // ② agy を起動する。起動できなければ記録を取り消して明示拒否（何も実行していない）。
        let args = cmd::turn_args(&TurnLaunch { text: &request.text, model: &model, conversation_id: conversation.as_deref(), permission, extra_dirs: &extras });
        let mut command = Command::new(&conn.exe);
        command.args(&args).current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(false);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.rollback_send(&chat_id, &turn_id, backup);
                return reject(request, rejected(format!("agy を起動できませんでした: {e}")));
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let live = Arc::new(LiveTurn { pid: child.id(), interrupted: AtomicBool::new(false), done: AtomicBool::new(false) });
        let turn = turn_key(&chat_id, &turn_id);
        self.shared.live.lock().unwrap().insert(turn.clone(), live.clone());
        tokio::spawn(run_turn(self.shared.clone(), child, stdout, stderr, turn.clone(), conversation, live));
        SendOutcome::Accepted { turn, accepted_model: Known::direct(ModelChoice { model, effort: None, speed_tier: None }) }
    }

    async fn reconcile_send(&self, pending: UnconfirmedSend) -> ReconcileOutcome {
        let chat_id = pending.chat().id.0.clone();
        let cmid = pending.client_message_id().to_string();
        let found = self.shared.with_store(|s| s.get(&chat_id).and_then(|c| c.turns.iter().find(|t| t.client_message_id == cmid).map(|t| t.turn_id.clone())));
        match found {
            Ok(Some(turn_id)) => ReconcileOutcome::Accepted { turn: turn_key(&chat_id, &turn_id) },
            // 記録（送る前に必ず保存する）に無ければ、起動していない。
            Ok(None) => ReconcileOutcome::NotFound(pending.into_not_accepted()),
            // 記録を読めていない間は判断しない。
            Err(_) => ReconcileOutcome::StillUnknown(pending),
        }
    }

    async fn respond(&self, _request: RequestKey, _answer: RequestAnswer, _confirmed: &UserConfirmed) -> BackendResult<RespondOutcome> {
        // print モードでは承認が必要な操作は自動拒否される。承認要求は発生しない。
        Err(BackendError::Unsupported { capability: "respond".into() })
    }

    async fn interrupt(&self, turn: TurnKey, _confirmed: &UserConfirmed) -> BackendResult<InterruptAck> {
        let live = self.shared.live.lock().unwrap().get(&turn).cloned();
        let Some(live) = live.filter(|l| !l.done.load(Ordering::SeqCst)) else { return Ok(InterruptAck::NotRunning) };
        let Some(pid) = live.pid else {
            return Ok(InterruptAck::Unknown { message: "プロセスIDを取得できていないため、停止を要求できません".into() });
        };
        // 先に印を付ける（終了を観測した task が、失敗ではなく中断として扱えるように）。
        live.interrupted.store(true, Ordering::SeqCst);
        match kill_tree(pid).await {
            // 受付のみ。停止は読取り task がプロセス終了を観測して出す TurnEnded で照合する。
            Ok(()) => Ok(InterruptAck::Requested),
            Err(e) => {
                live.interrupted.store(false, Ordering::SeqCst);
                if e.gone || live.done.load(Ordering::SeqCst) {
                    Ok(InterruptAck::NotRunning)
                } else {
                    Ok(InterruptAck::Unknown { message: e.message })
                }
            }
        }
    }

    async fn list_models(&self, _query: ModelQuery) -> BackendResult<Vec<ModelInfo>> {
        let conn = self.shared.ready()?;
        if let Some((at, models)) = self.shared.models.lock().unwrap().as_ref() {
            if at.elapsed() < MODELS_CACHE_TTL {
                return Ok(models.clone());
            }
        }
        let out = match run_agy(&conn.exe, &["models"], MODELS_TIMEOUT).await {
            Ok(o) => o,
            Err(RunError::Spawn(e)) => return Err(BackendError::Io { message: format!("agy models を実行できませんでした: {e}") }),
            Err(RunError::Timeout) => return Err(BackendError::Io { message: format!("agy models が {} 秒以内に応答しませんでした", MODELS_TIMEOUT.as_secs()) }),
        };
        if !out.success {
            return Err(BackendError::Io { message: format!("agy models が終了コード {:?} で終了しました: {}", out.code, head(&out.stderr, 300)) });
        }
        let models = cmd::parse_models(&out.stdout);
        if !models.is_empty() {
            *self.shared.models.lock().unwrap() = Some((Instant::now(), models.clone()));
        }
        Ok(models)
    }
}

/// 同等性の操作はすべて非対応（既定実装の `Unsupported`）。
impl ParityOps for AntigravityBackend {}

// ───────────────────────────── 読取り task ─────────────────────────────

/// stderr の末尾だけを保持する（読み捨てると、プロセスが書込みで詰まるため）。
async fn collect_tail(mut e: ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 2048];
    loop {
        match e.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut t = tail.lock().unwrap();
                t.extend_from_slice(&buf[..n]);
                if t.len() > STDERR_KEEP_BYTES {
                    let cut = t.len() - STDERR_KEEP_BYTES;
                    t.drain(..cut);
                }
            }
        }
    }
}

/// `result` を受けるか出力が終わるまで行を読み、イベントを流す。読み取りに失敗したら理由を返す。
async fn read_lines(shared: &Shared, reader: &mut BufReader<ChildStdout>, stream: &mut TurnStream, chat_id: &str) -> Option<String> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return None,
            Ok(_) => {
                let events = stream.on_line(parse_line(&String::from_utf8_lossy(&buf)), UnixMillis(now_ms()));
                if let Some(id) = stream.take_learned_conversation() {
                    shared.remember_conversation(chat_id, &id);
                }
                for e in events {
                    shared.emit(e);
                }
                if stream.has_result() {
                    return None;
                }
            }
            Err(e) => return Some(format!("標準出力の読み取りに失敗しました: {e}")),
        }
    }
}

/// 1ターン分のプロセスを見届ける。終端（またはその不明）を保存してからイベントを出す。
async fn run_turn(shared: Arc<Shared>, mut child: Child, stdout: Option<ChildStdout>, stderr: Option<ChildStderr>, turn: TurnKey, expected_conversation: Option<String>, live: Arc<LiveTurn>) {
    let chat_id = turn.agent.id.0.clone();
    let tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_task = stderr.map(|e| tokio::spawn(collect_tail(e, tail.clone())));
    let mut stream = TurnStream::new(turn.clone(), expected_conversation);
    for e in stream.start(UnixMillis(now_ms())) {
        shared.emit(e);
    }

    let mut reader = stdout.map(BufReader::new);
    let mut exit = ExitInfo::default();
    exit.lost = match reader.as_mut() {
        Some(r) => read_lines(&shared, r, &mut stream, &chat_id).await,
        None => Some("標準出力を取得できませんでした".into()),
    };
    let got_result = stream.has_result();
    if !got_result && exit.lost.is_none() {
        // 出力が終わった。プロセスの終了を見届ける（終了を確認できなければ結果不明）。
        match tokio::time::timeout(EXIT_WAIT, child.wait()).await {
            Ok(Ok(status)) => exit.exit_code = status.code(),
            Ok(Err(e)) => exit.lost = Some(format!("プロセスの終了を確認できませんでした: {e}")),
            Err(_) => exit.lost = Some("出力は終わりましたが、プロセスが終了しません".into()),
        }
    }
    if !got_result {
        if let Some(t) = stderr_task {
            let _ = tokio::time::timeout(Duration::from_secs(2), t).await;
        }
    }
    exit.stderr_tail = String::from_utf8_lossy(&tail.lock().unwrap()).into_owned();
    exit.interrupted = live.interrupted.load(Ordering::SeqCst);

    let now = now_ms();
    let (fin, events) = stream.finish(&exit, UnixMillis(now));
    // 先に保存し、実行中の印を外してから、終端を知らせる（受け取った側がすぐ次の送信をしても通るように）。
    let ended_at = (fin.status != TurnStatus::Unknown).then_some(now);
    let save_error = shared.record_final(&chat_id, &turn.turn_id.0, &fin, ended_at);
    live.done.store(true, Ordering::SeqCst);
    shared.live.lock().unwrap().remove(&turn);
    for e in events {
        shared.emit(e);
    }
    if let Some(e) = save_error {
        crate::diag::log("antigravity", &format!("saving turn result failed: {e}"));
        shared.emit(BackendEvent::Unrecognized { raw_label: "antigravity.store".into(), note: Some(format!("turn の結果を保存できませんでした（{e}）")) });
    }

    if got_result {
        // 残りの出力を捨てて（詰まらせない）、プロセスの終了を待つ。
        let drain = async {
            if let Some(r) = reader.as_mut() {
                let _ = tokio::io::copy(r, &mut tokio::io::sink()).await;
            }
            let _ = child.wait().await;
        };
        let _ = tokio::time::timeout(DRAIN_WAIT, drain).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;
    use std::path::PathBuf;

    fn request(chat: &str, text: &str) -> SendRequest {
        SendRequest {
            chat: ChatKey { backend: BackendKind::Antigravity, id: ExternalId(chat.into()) },
            mode: SendMode::NewTurn,
            text: text.into(),
            attachments: vec![],
            model: Some(ModelChoice { model: "gemini-test-1".into(), effort: None, speed_tier: None }),
            permission: None,
            cwd: None,
            work_folders: None,
            work_mode: None,
            speed_tier: None,
            client_message_id: format!("cm-{text}"),
            output_schema: None,
        }
    }

    fn params(cwd: &Path, model: Option<&str>) -> StartChatParams {
        StartChatParams {
            backend: BackendKind::Antigravity,
            kind: ChatKind::Development,
            cwd: cwd.to_string_lossy().into_owned(),
            model: model.map(|m| ModelChoice { model: m.into(), effort: None, speed_tier: None }),
            permission: PermissionPreset::WorkspaceWriteOnRequest,
            extra_folders: vec![],
            ephemeral: false,
            config: None,
            developer_instructions: None,
        }
    }

    fn setup(prefix: &str) -> (AntigravityBackend, PathBuf) {
        let dir = unique_temp_dir(prefix);
        let b = AntigravityBackend::new();
        b.attach_for_test("agy-not-used", dir.to_str().unwrap());
        (b, dir)
    }

    fn confirmed() -> UserConfirmed {
        UserConfirmed::from_user_command()
    }

    #[test]
    fn capabilities_are_conservative_before_connect_and_fixed_after() {
        let b = AntigravityBackend::new();
        assert_eq!(b.kind(), BackendKind::Antigravity);
        let before = b.capabilities();
        assert_eq!((before.resume, before.interrupt, before.model_selection), (Support::Unknown, Support::Unknown, Support::Unknown));
        let c = antigravity_capabilities();
        for s in [c.descendant_monitoring, c.descendant_search, c.list_loaded, c.steer, c.effort_selection, c.external_history, c.managed_exec_control, c.projects, c.workspace_roots, c.context_usage, c.structured_output] {
            assert_eq!(s, Support::Unsupported);
        }
        for s in [c.history_read_without_resume, c.resume, c.model_selection, c.interrupt, c.rename, c.archive, c.delete] {
            assert_eq!(s, Support::Supported);
        }
        assert!(c.approval_kinds.is_empty() && c.attachment_kinds.is_empty() && c.ops.is_empty());
    }

    #[tokio::test]
    async fn operations_before_connect_are_not_connected() {
        let b = AntigravityBackend::new();
        assert!(matches!(b.list_chats(ListChatsQuery::default()).await, Err(BackendError::NotConnected)));
        assert!(matches!(b.start_chat(params(Path::new("."), Some("gemini-x"))).await, Err(BackendError::NotConnected)));
        assert!(matches!(b.send(request("agy-1", "hi")).await, SendOutcome::Rejected { error: BackendError::NotConnected, .. }));
        assert!(matches!(b.interrupt(turn_key("agy-1", "t"), &confirmed()).await, Ok(InterruptAck::NotRunning)));
        assert!(b.take_events().is_some() && b.take_events().is_none());
    }

    #[tokio::test]
    async fn start_chat_records_without_starting_a_process_and_rejects_non_gemini() {
        let (b, dir) = setup("agentdock-agy-start");
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        assert!(matches!(b.start_chat(params(&work, Some("claude-sonnet-4-6"))).await, Err(BackendError::Rejected { .. })));
        assert!(matches!(b.start_chat(params(&work, Some("gpt-oss-120b-medium"))).await, Err(BackendError::Rejected { .. })));
        assert!(matches!(b.start_chat(params(&dir.join("missing"), Some("gemini-test-1"))).await, Err(BackendError::Rejected { .. })));
        let started = b.start_chat(params(&work, Some("gemini-test-1"))).await.unwrap();
        assert!(started.chat.key.id.0.starts_with("agy-") && started.chat.key.backend == BackendKind::Antigravity);
        assert_eq!(started.root.key.id, started.chat.key.id);
        assert_eq!((started.accepted_model.clone(), started.accepted_folders.clone()), (Known::NotFetched, Known::Unsupported));
        assert!(started.chat.no_history);
        // 記録は保存され、再読込みでも残る。
        let file = dir.join("antigravity").join("chats.json");
        assert!(file.is_file());
        let b2 = AntigravityBackend::new();
        b2.attach_for_test("agy-not-used", dir.to_str().unwrap());
        let page = b2.list_chats(ListChatsQuery::default()).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].chat.cwd, Known::direct(work.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ephemeral_chat_is_not_saved_or_listed() {
        let (b, dir) = setup("agentdock-agy-ephemeral");
        let mut p = params(&dir, Some("gemini-test-1"));
        p.ephemeral = true;
        let started = b.start_chat(p).await.unwrap();
        assert!(b.list_chats(ListChatsQuery::default()).await.unwrap().items.is_empty());
        assert!(b.read(agent_key(&started.chat.key.id.0), ReadOptions { include_turns: true }).await.is_ok());
        assert!(!dir.join("antigravity").join("chats.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn add_chat(b: &AntigravityBackend, id: &str, cwd: &str, name: Option<&str>, created: i64, used: Option<i64>, first_text: Option<&str>, archived: bool) {
        b.shared
            .with_store(|s| {
                s.insert(StoredChat {
                    id: id.into(),
                    conversation_id: None,
                    cwd: cwd.into(),
                    extra_folders: vec![],
                    model: Some("gemini-test-1".into()),
                    permission: PermissionPreset::ReadOnly,
                    kind: ChatKind::Development,
                    name: name.map(str::to_string),
                    created_at: created,
                    last_used_at: used,
                    archived,
                    turns: first_text
                        .map(|t| {
                            vec![StoredTurn {
                                turn_id: format!("{id}-t1"),
                                client_message_id: format!("cm-{id}"),
                                user_text: t.into(),
                                response_text: "ok".into(),
                                status: TurnStatus::Completed,
                                started_at: created,
                                ended_at: Some(created + 1),
                                denied_actions: vec![],
                                error: None,
                                usage: None,
                            }]
                        })
                        .unwrap_or_default(),
                    ephemeral: false,
                })
            })
            .unwrap();
    }

    #[tokio::test]
    async fn list_chats_filters_sorts_and_pages() {
        let (b, dir) = setup("agentdock-agy-list");
        add_chat(&b, "agy-a", "C:\\proj\\a", Some("Alpha"), 1, Some(300), Some("ビルドを直して"), false);
        add_chat(&b, "agy-b", "C:\\proj\\b", None, 2, Some(100), Some("README を書く"), false);
        add_chat(&b, "agy-c", "C:\\proj\\a", Some("Gamma"), 3, None, None, false);
        add_chat(&b, "agy-d", "C:\\proj\\a", Some("Old"), 4, Some(999), Some("x"), true);
        let ids = |p: &Page<ChatSummary>| p.items.iter().map(|i| i.chat.key.id.0.clone()).collect::<Vec<_>>();
        // last_used 降順（未使用は作成時刻）。アーカイブは既定で除く。
        assert_eq!(ids(&b.list_chats(ListChatsQuery::default()).await.unwrap()), ["agy-a", "agy-b", "agy-c"]);
        assert_eq!(ids(&b.list_chats(ListChatsQuery { archived: true, ..Default::default() }).await.unwrap()), ["agy-d"]);
        assert_eq!(ids(&b.list_chats(ListChatsQuery { search: Some("alpha".into()), ..Default::default() }).await.unwrap()), ["agy-a"]);
        assert_eq!(ids(&b.list_chats(ListChatsQuery { search: Some("readme".into()), ..Default::default() }).await.unwrap()), ["agy-b"]);
        assert_eq!(ids(&b.list_chats(ListChatsQuery { cwd: Some(vec!["c:/proj/a/".into()]), ..Default::default() }).await.unwrap()), ["agy-a", "agy-c"]);
        assert!(b.list_chats(ListChatsQuery { cwd: Some(vec!["C:\\proj".into()]), ..Default::default() }).await.unwrap().items.is_empty(), "exact match only");
        let p1 = b.list_chats(ListChatsQuery { limit: Some(2), ..Default::default() }).await.unwrap();
        assert_eq!((ids(&p1), p1.next_cursor.clone()), (vec!["agy-a".to_string(), "agy-b".to_string()], Some("2".to_string())));
        let p2 = b.list_chats(ListChatsQuery { limit: Some(2), cursor: p1.next_cursor, ..Default::default() }).await.unwrap();
        assert_eq!((ids(&p2), p2.next_cursor), (vec!["agy-c".to_string()], None));
        assert!(matches!(b.list_chats(ListChatsQuery { cursor: Some("zzz".into()), ..Default::default() }).await, Err(BackendError::Rejected { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn manage_chat_renames_archives_and_deletes_only_the_record() {
        let (b, dir) = setup("agentdock-agy-manage");
        add_chat(&b, "agy-a", "C:\\p", None, 1, None, Some("hello"), false);
        let key = chat_key("agy-a");
        let mut rx = b.take_events().unwrap();
        assert!(matches!(b.manage_chat(key.clone(), ManageOp::Rename { name: "  ".into() }, &confirmed()).await, Err(BackendError::Rejected { .. })));
        assert!(matches!(b.manage_chat(key.clone(), ManageOp::Rename { name: " 新名 ".into() }, &confirmed()).await, Ok(ManageOutcome::Done)));
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(first.event, BackendEvent::ChatMetaChanged { change: ChatMetaChange::Renamed { ref name }, .. } if name == "新名"));
        assert!(first.source.0.starts_with("antigravity-"));
        b.manage_chat(key.clone(), ManageOp::Archive, &confirmed()).await.unwrap();
        assert!(b.list_chats(ListChatsQuery::default()).await.unwrap().items.is_empty());
        b.manage_chat(key.clone(), ManageOp::Unarchive, &confirmed()).await.unwrap();
        assert_eq!(b.list_chats(ListChatsQuery::default()).await.unwrap().items[0].chat.name, Known::direct("新名".to_string()));
        // 実行中の turn があれば拒否する。
        b.shared.with_store(|s| s.get_mut("agy-a").unwrap().turns[0].status = TurnStatus::Running).unwrap();
        for op in [ManageOp::Delete, ManageOp::Archive, ManageOp::Rename { name: "x".into() }] {
            assert!(matches!(b.manage_chat(key.clone(), op, &confirmed()).await, Err(BackendError::Rejected { .. })));
        }
        b.shared.with_store(|s| s.get_mut("agy-a").unwrap().turns[0].status = TurnStatus::Completed).unwrap();
        b.manage_chat(key.clone(), ManageOp::Delete, &confirmed()).await.unwrap();
        assert!(b.list_chats(ListChatsQuery::default()).await.unwrap().items.is_empty());
        assert!(matches!(b.read(agent_key("agy-a"), ReadOptions::default()).await, Err(BackendError::Rejected { .. })));
        assert!(matches!(b.manage_chat(key, ManageOp::Delete, &confirmed()).await, Err(BackendError::Rejected { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_and_resume_use_the_record_only() {
        let (b, dir) = setup("agentdock-agy-read");
        add_chat(&b, "agy-a", "C:\\p", Some("N"), 1, None, Some("hello"), false);
        let h = b.read(agent_key("agy-a"), ReadOptions { include_turns: true }).await.unwrap();
        assert_eq!((h.turns.len(), h.status.state, h.status.evidence.source), (1, AgentState::Idle, EvidenceSource::HistoryRead));
        assert_eq!(h.turns[0].end, Some(TurnEnd::Completed));
        assert!(b.read(agent_key("agy-a"), ReadOptions::default()).await.unwrap().turns.is_empty());
        match b.resume(chat_key("agy-a"), &confirmed(), None).await.unwrap() {
            ResumeOutcome::Resumed { history, accepted_model } => assert_eq!((history.agent.key.id.0.as_str(), accepted_model), ("agy-a", Known::NotFetched)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(b.resume(chat_key("agy-none"), &confirmed(), None).await.unwrap(), ResumeOutcome::Unavailable { .. }));
        assert_eq!(
            b.read_work_folders(&chat_key("agy-a")).await.unwrap(),
            Known::Value { value: WorkFolders { folders: vec!["C:\\p".into()] }, basis: Basis::Derived }
        );
        assert_eq!(b.read_work_folders(&chat_key("agy-none")).await.unwrap(), Known::NotFetched);
        assert!(b.list_loaded().await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unsupported_operations_say_so() {
        let (b, dir) = setup("agentdock-agy-unsupported");
        assert!(matches!(b.list_projects().await, Err(BackendError::Unsupported { capability }) if capability == "projects"));
        let scan = b.scan_descendants(agent_key("agy-a")).await.unwrap();
        assert!(scan.found.is_empty() && scan.complete);
        let rk = RequestKey { backend: BackendKind::Antigravity, source: SourceId("s".into()), request_id: ExternalId("1".into()) };
        assert!(matches!(b.respond(rk, RequestAnswer::Decision { option_id: "x".into() }, &confirmed()).await, Err(BackendError::Unsupported { capability }) if capability == "respond"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn send_rejects_what_it_cannot_do_without_touching_the_record() {
        let (b, dir) = setup("agentdock-agy-reject");
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let id = b.start_chat(params(&work, Some("gemini-test-1"))).await.unwrap().chat.key.id.0;
        let code = |o: SendOutcome| match o {
            SendOutcome::Rejected { error, .. } => error,
            other => panic!("{other:?}"),
        };
        let mut r = request(&id, "hi");
        r.mode = SendMode::Steer { turn: turn_key(&id, "t") };
        assert!(matches!(code(b.send(r).await), BackendError::Unsupported { capability } if capability == "steer"));
        let mut r = request(&id, "hi");
        r.output_schema = Some(serde_json::json!({}));
        assert!(matches!(code(b.send(r).await), BackendError::Unsupported { .. }));
        let mut r = request(&id, "hi");
        r.attachments = vec![Attachment { id: LocalId("a".into()), kind: AttachmentKind::File, original_path: "C:\\f".into(), copy_path: None, attached_at: UnixMillis(0), exists: Known::NotFetched, owner_chat: chat_key(&id), name: None }];
        assert!(matches!(code(b.send(r).await), BackendError::Unsupported { .. }));
        let mut r = request(&id, "hi");
        r.model = Some(ModelChoice { model: "claude-sonnet-4-6".into(), effort: None, speed_tier: None });
        assert!(matches!(code(b.send(r).await), BackendError::Rejected { .. }));
        assert!(matches!(code(b.send(request("agy-none", "hi")).await), BackendError::Rejected { .. }));
        let mut r = request(&id, "hi");
        r.cwd = Some(dir.join("missing").to_string_lossy().into_owned());
        assert!(matches!(code(b.send(r).await), BackendError::Rejected { .. }));
        // 実行中の turn があれば拒否。
        b.shared.with_store(|s| s.get_mut(&id).unwrap().turns.push(StoredTurn { turn_id: "t-run".into(), client_message_id: "c".into(), user_text: "u".into(), response_text: String::new(), status: TurnStatus::Running, started_at: 1, ended_at: None, denied_actions: vec![], error: None, usage: None })).unwrap();
        assert!(matches!(code(b.send(request(&id, "hi")).await), BackendError::Rejected { .. }));
        let n = b.shared.with_store(|s| s.get(&id).unwrap().turns.len()).unwrap();
        assert_eq!(n, 1, "rejected sends leave no turn behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn spawn_failure_is_a_rejection_and_leaves_no_turn() {
        let dir = unique_temp_dir("agentdock-agy-spawnfail");
        let b = AntigravityBackend::new();
        b.attach_for_test(dir.join("no-such-agy.exe").to_str().unwrap(), dir.to_str().unwrap());
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let id = b.start_chat(params(&work, Some("gemini-test-1"))).await.unwrap().chat.key.id.0;
        assert!(matches!(b.send(request(&id, "hi")).await, SendOutcome::Rejected { error: BackendError::Rejected { .. }, .. }));
        assert!(b.shared.with_store(|s| s.get(&id).unwrap().turns.is_empty()).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reconcile_finds_recorded_messages_only() {
        let (b, dir) = setup("agentdock-agy-reconcile");
        add_chat(&b, "agy-a", "C:\\p", None, 1, None, Some("hello"), false);
        let found = UnconfirmedSend::restore(chat_key("agy-a"), "cm-agy-a".into(), "hello".into(), UnixMillis(0));
        assert!(matches!(b.reconcile_send(found).await, ReconcileOutcome::Accepted { turn } if turn.turn_id.0 == "agy-a-t1"));
        let missing = UnconfirmedSend::restore(chat_key("agy-a"), "cm-other".into(), "x".into(), UnixMillis(0));
        assert!(matches!(b.reconcile_send(missing).await, ReconcileOutcome::NotFound(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launch_failures_are_classified() {
        let nf = RunError::Spawn(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(launch_failure(nf).0, LaunchFailure::NotFound);
        assert_eq!(launch_failure(RunError::Spawn(std::io::Error::from(std::io::ErrorKind::PermissionDenied))).0, LaunchFailure::InvalidPath);
        assert_eq!(launch_failure(RunError::Spawn(std::io::Error::from(std::io::ErrorKind::Other))).0, LaunchFailure::SpawnFailed);
        assert_eq!(launch_failure(RunError::Timeout).0, LaunchFailure::VersionCheckFailed);
    }

    #[tokio::test]
    async fn running_a_missing_executable_reports_not_found() {
        let r = run_agy("C:\\definitely\\not\\here\\agy.exe", &["--version"], Duration::from_secs(5)).await;
        assert!(matches!(r, Err(RunError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound));
    }

    // ── 偽の agy（.cmd）でイベントの流れを確かめる（Windows。実機の agy は使わない） ──

    #[cfg(windows)]
    mod fake_agy {
        use super::*;

        const INIT: &str = r#"{"event":"init","conversation_id":"9db82f11-0000","init":{"model":"gemini-test-1","cwd":"C:\\work","permission_mode":"request-review"}}"#;
        const USER: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":0,"state":"DONE","step_type":"user_input"}}"#;
        const MSG1: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"PONG"}}"#;
        const MSG2: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"\n"}}"#;
        const RESULT: &str = r#"{"event":"result","result":{"conversation_id":"9db82f11-0000","status":"SUCCESS","response":"PONG\n","usage":{"total_tokens":10649}}}"#;

        /// 出力ファイルを `type` するだけの偽 agy。受け取った引数も記録する。
        fn fake(dir: &Path, lines: &[&str], tail: &str) -> String {
            std::fs::write(dir.join("out.ndjson"), lines.join("\r\n") + "\r\n").unwrap();
            let script = format!("@echo off\r\necho %* > \"%~dp0args.txt\"\r\ntype \"%~dp0out.ndjson\"\r\n{tail}\r\n");
            let path = dir.join("fake-agy.cmd");
            std::fs::write(&path, script).unwrap();
            path.to_string_lossy().into_owned()
        }

        async fn until(rx: &mut EventReceiver, stop: impl Fn(&BackendEvent) -> bool) -> Vec<EventEnvelope> {
            let mut out = Vec::new();
            loop {
                let env = tokio::time::timeout(Duration::from_secs(40), rx.recv()).await.expect("timed out waiting for events").expect("event channel closed");
                let done = stop(&env.event);
                out.push(env);
                if done {
                    return out;
                }
            }
        }

        fn is_end(e: &BackendEvent) -> bool {
            matches!(e, BackendEvent::TurnEnded { .. })
        }

        async fn new_chat(b: &AntigravityBackend, dir: &Path) -> String {
            let work = dir.join("work");
            std::fs::create_dir_all(&work).unwrap();
            b.start_chat(params(&work, Some("gemini-test-1"))).await.unwrap().chat.key.id.0
        }

        #[tokio::test]
        async fn successful_turn_streams_events_saves_and_continues_the_conversation() {
            let dir = unique_temp_dir("agentdock-agy-fake-ok");
            let exe = fake(&dir, &[INIT, USER, MSG1, MSG2, RESULT], "exit /b 0");
            let b = AntigravityBackend::new();
            b.attach_for_test(&exe, dir.to_str().unwrap());
            let mut rx = b.take_events().unwrap();
            let id = new_chat(&b, &dir).await;

            let mut req = request(&id, "hello");
            req.permission = Some(PermissionPreset::ReadOnly);
            let SendOutcome::Accepted { turn, accepted_model } = b.send(req).await else { panic!("not accepted") };
            assert_eq!(accepted_model, Known::direct(ModelChoice { model: "gemini-test-1".into(), effort: None, speed_tier: None }));
            let envs = until(&mut rx, is_end).await;
            let seqs: Vec<u64> = envs.iter().map(|e| e.seq).collect();
            assert!(seqs.windows(2).all(|w| w[0] < w[1]), "seq is strictly increasing: {seqs:?}");
            assert!(envs.iter().all(|e| e.source.0.starts_with("antigravity-")));
            let names: Vec<&str> = envs
                .iter()
                .map(|e| match &e.event {
                    BackendEvent::TurnStarted { .. } => "TurnStarted",
                    BackendEvent::AgentStatus { .. } => "AgentStatus",
                    BackendEvent::ModelAccepted { .. } => "ModelAccepted",
                    BackendEvent::Activity { .. } => "Activity",
                    BackendEvent::ActivityDelta { .. } => "ActivityDelta",
                    BackendEvent::TokenUsage { .. } => "TokenUsage",
                    BackendEvent::TurnEnded { .. } => "TurnEnded",
                    _ => "other",
                })
                .collect();
            assert_eq!(names, ["TurnStarted", "AgentStatus", "ModelAccepted", "Activity", "ActivityDelta", "ActivityDelta", "Activity", "TokenUsage", "TurnEnded"]);
            assert!(matches!(&envs.last().unwrap().event, BackendEvent::TurnEnded { turn: t, end: TurnEnd::Completed, error: None, .. } if *t == turn));

            // 終端の時点で保存済み・実行中の印は外れている。
            let (status, response, conv) = b.shared.with_store(|s| {
                let c = s.get(&id).unwrap();
                (c.turns[0].status, c.turns[0].response_text.clone(), c.conversation_id.clone())
            }).unwrap();
            assert_eq!((status, response.as_str(), conv.as_deref()), (TurnStatus::Completed, "PONG\n", Some("9db82f11-0000")));
            let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
            assert!(args.contains("--mode plan") && args.contains("--model gemini-test-1") && !args.contains("--conversation"), "{args}");

            // 2回目は会話を引き継ぐ。
            let SendOutcome::Accepted { .. } = b.send(request(&id, "again")).await else { panic!("second send not accepted") };
            until(&mut rx, is_end).await;
            let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
            assert!(args.contains("--conversation 9db82f11-0000"), "{args}");
            assert_eq!(b.shared.with_store(|s| s.get(&id).unwrap().turns.len()).unwrap(), 2);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[tokio::test]
        async fn exit_without_result_is_failed_with_stderr_tail() {
            let dir = unique_temp_dir("agentdock-agy-fake-fail");
            let exe = fake(&dir, &[INIT, "garbage line"], "echo auth required 1>&2\r\nexit /b 3");
            let b = AntigravityBackend::new();
            b.attach_for_test(&exe, dir.to_str().unwrap());
            let mut rx = b.take_events().unwrap();
            let id = new_chat(&b, &dir).await;
            let SendOutcome::Accepted { .. } = b.send(request(&id, "hello")).await else { panic!() };
            let envs = until(&mut rx, is_end).await;
            assert!(envs.iter().any(|e| matches!(e.event, BackendEvent::Gap { .. })), "the unparsable line is reported");
            let BackendEvent::TurnEnded { end, error, .. } = &envs.last().unwrap().event else { panic!() };
            assert_eq!(*end, TurnEnd::Failed);
            let msg = error.clone().unwrap();
            assert!(msg.contains("終了コード 3") && msg.contains("auth required"), "{msg}");
            let (status, err) = b.shared.with_store(|s| (s.get(&id).unwrap().turns[0].status, s.get(&id).unwrap().turns[0].error.clone())).unwrap();
            assert_eq!(status, TurnStatus::Failed);
            assert!(err.unwrap().contains("auth required"));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[tokio::test]
        async fn interrupt_stops_the_process_tree_and_ends_as_interrupted() {
            let dir = unique_temp_dir("agentdock-agy-fake-int");
            let exe = fake(&dir, &[INIT], "ping -n 60 127.0.0.1 > nul");
            let b = AntigravityBackend::new();
            b.attach_for_test(&exe, dir.to_str().unwrap());
            let mut rx = b.take_events().unwrap();
            let id = new_chat(&b, &dir).await;
            let SendOutcome::Accepted { turn, .. } = b.send(request(&id, "long job")).await else { panic!() };
            // 実行中の間は、同じチャットへの送信は拒否される。
            assert!(matches!(b.send(request(&id, "second")).await, SendOutcome::Rejected { error: BackendError::Rejected { .. }, .. }));
            until(&mut rx, |e| matches!(e, BackendEvent::TurnStarted { .. })).await;
            assert!(matches!(b.interrupt(turn.clone(), &confirmed()).await, Ok(InterruptAck::Requested)));
            let envs = until(&mut rx, is_end).await;
            assert!(matches!(&envs.last().unwrap().event, BackendEvent::TurnEnded { end: TurnEnd::Interrupted, error: None, .. }));
            assert_eq!(b.shared.with_store(|s| s.get(&id).unwrap().turns[0].status).unwrap(), TurnStatus::Interrupted);
            // 終わった後の中断は NotRunning。
            assert!(matches!(b.interrupt(turn, &confirmed()).await, Ok(InterruptAck::NotRunning)));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
