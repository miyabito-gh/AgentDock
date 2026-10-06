//! `AiBackend` のCodex App Server実装（T3）。traitの各操作をApp Serverの要求へ写像する。
//!
//! 規則（DESIGN_T1・要件§3）:
//! - 送信は再送しない。結果は受理／明示拒否／受理不明の3値。受理不明は照合（`reconcile_send`）でしか解消しない。
//! - `read` / `list_*` / `scan_descendants` は resume・送信・承認をしない（M11）。resumeは `resume()` のみ。
//! - 承認・質問は自動回答しない。回答は `respond()`（ユーザー操作の証票つき）からだけ送る。
//! - 中断は要求の受付のみ。停止は `TurnEnded` で照合する。
//! - イベントは有界キューへ。溢れたら黙って捨てず、空き次第 `Gap` を送る。

use super::convert::*;
use super::events::{build_reply, notification_to_events, server_request_to_event, EventCtx, Reply, StoredRequest};
use super::outbox::Outbox;
use super::process::{check_version, CodexProcess};
use crate::win::job::ProcessJob;
use super::rpc::{RpcClient, RpcEvent};
use super::tree::assemble;
use super::wire::{WireThread, WireTurn};
use crate::backend::backend::*;
use crate::backend::model::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex};

const EVENT_QUEUE_CAPACITY: usize = 4096;
const RPC_EVENT_CAPACITY: usize = 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const PAGE_LIMIT: u32 = 100;
const MAX_PAGES: usize = 100;

const SUBAGENT_KINDS: [&str; 5] = ["subAgent", "subAgentReview", "subAgentCompact", "subAgentThreadSpawn", "subAgentOther"];

static SOURCE_COUNTER: AtomicU64 = AtomicU64::new(1);
static CHILD_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);
static ITEM_SHAPES_LOGGED: StdMutex<Vec<String>> = StdMutex::new(Vec::new());

fn now_ms() -> UnixMillis {
    UnixMillis(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
}

/// 対象版（0.160.0）で確認できた能力。`enable_experimental` が偽なら実験的な操作は使えない。
pub fn codex_capabilities(experimental: bool) -> Capabilities {
    let exp = if experimental { Support::Supported } else { Support::Experimental };
    Capabilities {
        descendant_monitoring: Support::Supported,
        // 祖先絞り込みはexperimental。無効時は安定側のsourceKinds一覧で代替する（重い）。
        descendant_search: exp,
        history_read_without_resume: Support::Supported,
        resume: Support::Supported,
        list_loaded: Support::Supported,
        approval_kinds: vec![
            RequestKind::CommandApproval,
            RequestKind::FileChangeApproval,
            RequestKind::PermissionsApproval,
            RequestKind::UserInput,
            RequestKind::ToolElicitation,
        ],
        steer: Support::Supported,
        interrupt: Support::Supported,
        model_selection: Support::Supported,
        effort_selection: Support::Supported,
        rename: Support::Supported,
        archive: Support::Supported,
        delete: Support::Supported,
        external_history: Support::Supported,
        attachment_kinds: vec![AttachmentKind::Image, AttachmentKind::Audio, AttachmentKind::File],
        // thread/backgroundTerminals/* はexperimental。
        managed_exec_control: exp,
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
    }
}

// ───────────────────────────── 要求パラメータの組立て（純粋） ─────────────────────────────

/// 添付 → `UserInput`。画像は `localImage`、その他のファイルはコピー先のパスを本文の後ろの `text` 入力に列挙する
/// （`mention` は使わない。モデルがファイルの中身を読めるかは未確認、V09）。
pub fn build_turn_input(text: &str, attachments: &[Attachment]) -> BackendResult<Vec<Value>> {
    let mut input = vec![json!({"type": "text", "text": text, "text_elements": []})];
    let mut files: Vec<String> = Vec::new();
    for a in attachments {
        let path = a.copy_path.clone().unwrap_or_else(|| a.original_path.clone());
        match a.kind {
            AttachmentKind::Image => input.push(json!({"type": "localImage", "path": path})),
            AttachmentKind::Audio => input.push(json!({"type": "localAudio", "path": path})),
            AttachmentKind::File => {
                let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                files.push(format!("- {name} — {path}"));
            }
        }
    }
    if !files.is_empty() {
        input.push(json!({"type": "text", "text": format!("添付ファイル:\n{}", files.join("\n")), "text_elements": []}));
    }
    Ok(input)
}

/// `turn/start`（新しいturn）または `turn/steer`（実行中turnへの追加指示）のメソッドとparams。
pub fn build_send_call(req: &SendRequest) -> BackendResult<(&'static str, Value)> {
    let input = build_turn_input(&req.text, &req.attachments)?;
    match &req.mode {
        SendMode::NewTurn => {
            let mut p = json!({"threadId": req.chat.id.0, "clientUserMessageId": req.client_message_id, "input": input});
            if let Some(m) = &req.model {
                p["model"] = json!(m.model);
                if let Some(e) = &m.effort {
                    p["effort"] = json!(e);
                }
            }
            // 送信時点のチャット設定（モデルと同じく、このturnと以後のturnに及ぶ）。指定がなければ上書きしない。
            if let Some(perm) = req.permission {
                p["approvalPolicy"] = json!(permission_params(perm).0);
                p["sandboxPolicy"] = sandbox_policy_json(perm);
            }
            if let Some(cwd) = &req.cwd {
                p["cwd"] = json!(cwd);
            }
            Ok(("turn/start", p))
        }
        SendMode::Steer { turn } => {
            // 照合（reconcile_send）はチャットのスレッドを読む。別スレッドへの追加指示は照合できないので送らない。
            if turn.agent.id != req.chat.id {
                return Err(BackendError::Unsupported { capability: "steer on a non-root agent".into() });
            }
            Ok((
                "turn/steer",
                json!({"threadId": turn.agent.id.0, "clientUserMessageId": req.client_message_id, "input": input, "expectedTurnId": turn.turn_id.0}),
            ))
        }
    }
}

/// 送信応答から受理されたturn idを取り出す。取れなければ受理不明。
fn accepted_turn_id(mode: &SendMode, result: &Value) -> Option<String> {
    match mode {
        SendMode::NewTurn => result.get("turn")?.get("id")?.as_str().map(str::to_string),
        SendMode::Steer { .. } => result.get("turnId")?.as_str().map(str::to_string),
    }
}

fn source_kinds(query: &ListChatsQuery) -> Vec<&'static str> {
    let mut v = if query.include_external { vec!["cli", "vscode", "exec", "appServer", "unknown"] } else { vec!["appServer"] };
    if query.include_descendants {
        v.extend(SUBAGENT_KINDS);
    }
    v
}

// ───────────────────────────── 共有状態 ─────────────────────────────

struct Conn {
    client: RpcClient,
    source: SourceId,
    process: Arc<Mutex<CodexProcess>>,
    experimental: bool,
    /// 強制終了用のJob（P6）。
    job: Option<Arc<ProcessJob>>,
}

struct Shared {
    conn: StdMutex<Option<Conn>>,
    caps: StdMutex<Capabilities>,
    app_data_dir: StdMutex<Option<String>>,
    /// エージェント→所属チャット（要求・通知で使う近似キャッシュ）。
    chat_cache: StdMutex<HashMap<AgentKey, ChatKey>>,
    /// 回答待ちのサーバー要求の原文（回答JSONの組立てに使う）。
    pending: StdMutex<HashMap<RequestKey, StoredRequest>>,
    outbox: Arc<Outbox>,
    event_tx: mpsc::Sender<EventEnvelope>,
    event_rx: StdMutex<Option<EventReceiver>>,
    forwarder_started: AtomicBool,
    /// connect / disconnect を直列化する（二重起動・起動中の切断を防ぐ）。
    lifecycle: Mutex<()>,
}

impl Shared {
    /// 送出バッファへ積む。重要イベントは落とさず、落とした分は `Gap` で必ず知らせる（[`Outbox`]）。
    fn emit(&self, source: &SourceId, event: BackendEvent) {
        self.outbox.push(source, event);
    }

    /// 該当監視元の切断後処理。別の監視元（再接続後の新しい接続）の状態は消さない。
    fn on_disconnected(&self, source: &SourceId) {
        {
            let mut conn = self.conn.lock().unwrap();
            if should_clear_conn(conn.as_ref().map(|c| &c.source), source) {
                *conn = None;
            }
        }
        retain_other_sources(&mut self.pending.lock().unwrap(), source);
    }

    fn chat_of(&self, a: &AgentKey) -> ChatKey {
        self.chat_cache.lock().unwrap().get(a).cloned().unwrap_or(ChatKey { backend: BackendKind::Codex, id: a.id.clone() })
    }

    fn remember(&self, agent: &Agent) {
        self.chat_cache.lock().unwrap().insert(agent.key.clone(), agent.chat.clone());
    }
}

/// 切断された監視元が、現在の接続のものか。
fn should_clear_conn(current: Option<&SourceId>, disconnected: &SourceId) -> bool {
    current == Some(disconnected)
}

/// 指定した監視元の回答待ち要求だけを消す。
fn retain_other_sources<V>(pending: &mut HashMap<RequestKey, V>, source: &SourceId) {
    pending.retain(|k, _| &k.source != source);
}

/// 未対応・変換できなかった通知／itemを、メソッド名とキー構造だけ診断ログへ残す（本文・値は残さない）。
fn log_unsupported(method: &str, params: &Value, e: &BackendEvent) {
    match e {
        BackendEvent::Unrecognized { raw_label, .. } => {
            crate::diag::log("unrecognized-notification", &format!("{raw_label} {}", crate::diag::shape(params)));
        }
        BackendEvent::Activity { activity } if matches!(activity.kind, ActivityKind::Other { .. }) => {
            let item = params.get("item").map(crate::diag::shape).unwrap_or_default();
            crate::diag::log("unsupported-item", &format!("{method} {item}"));
        }
        _ => {}
    }
}

/// サブエージェント関連itemの形を種別ごとに1回だけ診断ログへ（キー構造・件数のみ。ID・本文は書かない）。
fn log_item_shape_once(params: &Value) {
    let Some(item) = params.get("item") else { return };
    let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
    let (key, text) = match ty {
        "collabAgentToolCall" => {
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("?");
            let receivers = item.get("receiverThreadIds").and_then(Value::as_array).map(Vec::len);
            let states = item.get("agentsStates").and_then(Value::as_object).map(|o| o.len());
            (
                format!("{ty}/{tool}"),
                format!(
                    "tool={tool} receivers={receivers:?} agentsStatesKeys={states:?} promptPresent={} status={}",
                    item.get("prompt").is_some_and(|p| p.as_str().is_some_and(|s| !s.is_empty())),
                    item.get("status").and_then(Value::as_str).unwrap_or("?")
                ),
            )
        }
        "subAgentActivity" => (ty.to_string(), crate::diag::shape(item)),
        "dynamicToolCall" => {
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("?");
            (format!("{ty}/{tool}"), format!("tool={tool} args={}", crate::diag::shape(item.get("arguments").unwrap_or(&Value::Null))))
        }
        _ => return,
    };
    let mut seen = ITEM_SHAPES_LOGGED.lock().unwrap();
    if seen.contains(&key) {
        return;
    }
    seen.push(key.clone());
    crate::diag::log("item-shape", &format!("{key} {text}"));
}

/// thread/start・thread/resume の応答が示す、会話に設定されているモデル・推論の強さ（無ければ未取得）。
fn accepted_model_of(resp: &Value) -> Known<ModelChoice> {
    match resp.get("model").and_then(Value::as_str) {
        Some(m) => Known::direct(ModelChoice { model: m.to_string(), effort: resp.get("reasoningEffort").and_then(Value::as_str).map(str::to_string) }),
        None => Known::NotFetched,
    }
}

fn log_source(label: &str, resp: &Value) {
    let kind = crate::diag::source_kind(resp.get("thread").and_then(|t| t.get("source")));
    crate::diag::log("thread-source", &format!("{label} sourceKind={kind}"));
}

/// 受信側の中継。通知・サーバー要求を共通イベントへ変換して `emit` する。
async fn pump(shared: Arc<Shared>, source: SourceId, mut rx: mpsc::Receiver<RpcEvent>) {
    while let Some(ev) = rx.recv().await {
        match ev {
            RpcEvent::Notification { method, params, .. } => {
                let chat_of = |a: &AgentKey| shared.chat_of(a);
                let ctx = EventCtx { source: &source, now: now_ms(), chat_of: &chat_of };
                for e in notification_to_events(&method, &params, &ctx) {
                    log_unsupported(&method, &params, &e);
                    if matches!(e, BackendEvent::Activity { .. }) {
                        log_item_shape_once(&params);
                    }
                    match &e {
                        BackendEvent::AgentDiscovered { agent } => shared.remember(agent),
                        BackendEvent::RequestResolved { request, .. } => {
                            shared.pending.lock().unwrap().remove(request);
                        }
                        _ => {}
                    }
                    shared.emit(&source, e);
                }
            }
            RpcEvent::ServerRequest { key, method, params, .. } => {
                let chat_of = |a: &AgentKey| shared.chat_of(a);
                let ctx = EventCtx { source: &source, now: now_ms(), chat_of: &chat_of };
                let conv = server_request_to_event(&key, &method, &params, &ctx);
                if let BackendEvent::RequestOpened { request } = &conv.event {
                    if matches!(request.kind, RequestKind::Other { .. }) {
                        crate::diag::log("unsupported-server-request", &format!("{method} {}", crate::diag::shape(&params)));
                    }
                }
                if let Some(stored) = conv.stored {
                    shared.pending.lock().unwrap().insert(key, stored);
                }
                shared.emit(&source, conv.event);
            }
            RpcEvent::UnmatchedResponse { id, .. } => shared.emit(
                &source,
                BackendEvent::Unrecognized { raw_label: "response/unmatched".into(), note: Some(format!("response for unknown or timed-out request id {id}")) },
            ),
            RpcEvent::Malformed { note, .. } => {
                shared.emit(&source, BackendEvent::Gap { scope: GapScope::Source, reason: format!("malformed message: {note}") });
            }
            RpcEvent::Overflow { .. } => {
                shared.emit(&source, BackendEvent::Gap { scope: GapScope::Source, reason: "rpc receive backlog overflowed; notifications were dropped".into() });
            }
            RpcEvent::Disconnected { reason } => {
                shared.on_disconnected(&source);
                shared.emit(&source, BackendEvent::Connection { state: ConnectionState::Disconnected { message: Some(reason) } });
                break;
            }
        }
    }
}

// ───────────────────────────── アダプター ─────────────────────────────

pub struct CodexBackend {
    shared: Arc<Shared>,
}

impl Default for CodexBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexBackend {
    /// 現在の接続のJob（強制終了用）。接続がない・Jobを割り当てられなかったときは None。
    pub fn current_job(&self) -> Option<(SourceId, Arc<ProcessJob>)> {
        let conn = self.shared.conn.lock().unwrap();
        conn.as_ref().and_then(|c| c.job.clone().map(|j| (c.source.clone(), j)))
    }

    pub fn new() -> Self {
        Self::with_queue_capacity(EVENT_QUEUE_CAPACITY)
    }

    fn with_queue_capacity(cap: usize) -> Self {
        let (tx, rx) = mpsc::channel(64);
        CodexBackend {
            shared: Arc::new(Shared {
                conn: StdMutex::new(None),
                caps: StdMutex::new(unknown_capabilities()),
                app_data_dir: StdMutex::new(None),
                chat_cache: StdMutex::new(HashMap::new()),
                pending: StdMutex::new(HashMap::new()),
                outbox: Arc::new(Outbox::new(cap)),
                event_tx: tx,
                event_rx: StdMutex::new(Some(rx)),
                forwarder_started: AtomicBool::new(false),
                lifecycle: Mutex::new(()),
            }),
        }
    }

    fn client(&self) -> BackendResult<(RpcClient, bool)> {
        let g = self.shared.conn.lock().unwrap();
        match g.as_ref() {
            Some(c) if !c.client.is_closed() => Ok((c.client.clone(), c.experimental)),
            _ => Err(BackendError::NotConnected),
        }
    }

    async fn call(&self, method: &str, params: Value, timeout: Duration) -> BackendResult<Value> {
        let (client, _) = self.client()?;
        client.request(method, params, Some(timeout)).await
    }

    fn app_dir(&self) -> Option<String> {
        self.shared.app_data_dir.lock().unwrap().clone()
    }

    fn ev(&self, source: EvidenceSource, label: &str) -> Evidence {
        evidence(source, label, None, now_ms())
    }

    /// 一覧系の共通ページング（上限ページ数で打ち切り、残りがあれば `truncated=true`）。
    async fn collect_pages(&self, method: &str, base: Value, data_key: &str) -> BackendResult<(Vec<Value>, bool)> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut p = base.clone();
            p["limit"] = json!(PAGE_LIMIT);
            if let Some(c) = &cursor {
                p["cursor"] = json!(c);
            }
            let r = self.call(method, p, READ_TIMEOUT).await?;
            let data = r
                .get(data_key)
                .and_then(Value::as_array)
                .ok_or_else(|| BackendError::Protocol { message: format!("{method}: missing {data_key}") })?;
            out.extend(data.iter().cloned());
            match r.get("nextCursor").and_then(Value::as_str) {
                Some(c) => cursor = Some(c.to_string()),
                None => return Ok((out, false)),
            }
        }
        Ok((out, true))
    }

    /// 履歴のturnを `thread/turns/list`（古い順・itemsView=full）でページ取得する。
    /// `thread/read` の `includeTurns` は非推奨（全履歴の一括展開）なので使わない。
    async fn fetch_turns(&self, thread_id: &str) -> BackendResult<Vec<WireTurn>> {
        let base = json!({"threadId": thread_id, "sortDirection": "asc", "itemsView": "full"});
        let (data, truncated) = self.collect_pages("thread/turns/list", base, "data").await?;
        if truncated {
            return Err(BackendError::Protocol { message: "thread/turns/list: too many pages".into() });
        }
        data.iter().map(|v| WireTurn::from_value(v).map_err(|m| BackendError::Protocol { message: m })).collect()
    }

    /// メタデータのみの `thread/read` ＋（必要なら）ページングでturn取得。
    async fn read_thread(&self, thread_id: &str, include_turns: bool) -> BackendResult<WireThread> {
        let r = self.call("thread/read", json!({"threadId": thread_id, "includeTurns": false}), READ_TIMEOUT).await?;
        let mut t = thread_of(&r)?;
        if include_turns {
            t.turns = self.fetch_turns(thread_id).await?;
        }
        Ok(t)
    }

    fn history_from_thread(&self, t: &WireThread, include_turns: bool, src: EvidenceSource, label: &str) -> AgentHistory {
        let agent_chat = self.shared.chat_of(&agent_key(&t.id));
        let is_root = matches!(parent_link(t), ParentLink::Root);
        let chat = if is_root { chat_key(&t.id) } else if agent_chat.id.0 != t.id { agent_chat } else { default_chat_for(t) };
        let agent = thread_to_agent(t, chat);
        self.shared.remember(&agent);
        AgentHistory {
            chat: is_root.then(|| thread_to_chat(t, self.app_dir().as_deref(), Known::NotFetched)),
            status: derive_status(&t.status, t.latest_turn(), self.ev(src, label)),
            turns: if include_turns { t.turns.iter().map(|x| turn_to_record(&t.id, x)).collect() } else { Vec::new() },
            agent,
        }
    }
}

fn flatten(nodes: &[super::tree::TreeNode], out: &mut Vec<Agent>) {
    for n in nodes {
        out.push(n.agent.clone());
        flatten(&n.children, out);
    }
}

fn parse_thread(v: &Value) -> BackendResult<WireThread> {
    WireThread::from_value(v).map_err(|m| BackendError::Protocol { message: m })
}

fn thread_of(resp: &Value) -> BackendResult<WireThread> {
    parse_thread(resp.get("thread").ok_or_else(|| BackendError::Protocol { message: "response without thread".into() })?)
}

#[async_trait]
impl AiBackend for CodexBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    fn capabilities(&self) -> Capabilities {
        self.shared.caps.lock().unwrap().clone()
    }

    async fn connect(&self, config: ConnectConfig) -> BackendResult<SourceInfo> {
        let _lifecycle = self.shared.lifecycle.lock().await;
        if !self.shared.forwarder_started.swap(true, Ordering::SeqCst) {
            tokio::spawn(self.shared.outbox.clone().forward(self.shared.event_tx.clone()));
        }
        if self.shared.conn.lock().unwrap().is_some() {
            return Err(BackendError::Protocol { message: "already connected".into() });
        }
        let source = SourceId(format!("codex-{}-{}", now_ms().0, SOURCE_COUNTER.fetch_add(1, Ordering::SeqCst)));
        let path = config.executable.clone().unwrap_or_else(|| "codex".to_string());
        let started_at = now_ms();
        let failed = |reason: LaunchFailure, message: String, version: VersionCheck| {
            self.shared.emit(&source, BackendEvent::Connection { state: ConnectionState::LaunchFailed { reason, message: message.clone() } });
            SourceInfo {
                source: source.clone(),
                backend: BackendKind::Codex,
                pid: Known::NotFetched,
                started_at,
                version,
                capabilities: self.capabilities(),
                connection: ConnectionState::LaunchFailed { reason, message },
            }
        };

        // 版違いは警告のみ。起動できない場合は LaunchFailed（agentの作業失敗ではない）。
        let version = match check_version(&path, config.expected_version.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(failed(e.kind, e.message.clone(), VersionCheck::Unknown { message: e.message })),
        };
        *self.shared.app_data_dir.lock().unwrap() = Some(config.app_data_dir.clone());
        self.shared.emit(&source, BackendEvent::Connection { state: ConnectionState::Starting });
        let (process, rx) = match CodexProcess::launch(&path, source.clone(), config.enable_experimental, RPC_EVENT_CAPACITY).await {
            Ok(x) => x,
            Err(e) => return Ok(failed(e.kind, e.message, version)),
        };
        crate::diag::log(
            "initialize",
            &format!(
                "shape={} userAgent={} versionCheck={}",
                crate::diag::shape(&process.init_response),
                crate::diag::user_agent_version(process.init_response.get("userAgent").and_then(Value::as_str)),
                crate::diag::version_check_kind(&version)
            ),
        );
        let caps = codex_capabilities(config.enable_experimental);
        *self.shared.caps.lock().unwrap() = caps.clone();
        let pid = match process.pid {
            Some(p) => Known::direct(p),
            None => Known::Missing,
        };
        let process_job = process.job.clone();
        *self.shared.conn.lock().unwrap() = Some(Conn {
            client: process.client.clone(),
            source: source.clone(),
            process: Arc::new(Mutex::new(process)),
            experimental: config.enable_experimental,
            job: process_job,
        });
        self.shared.emit(&source, BackendEvent::Connection { state: ConnectionState::Connected });
        tokio::spawn(pump(self.shared.clone(), source.clone(), rx));
        Ok(SourceInfo { source, backend: BackendKind::Codex, pid, started_at, version, capabilities: caps, connection: ConnectionState::Connected })
    }

    async fn disconnect(&self) -> BackendResult<()> {
        let _lifecycle = self.shared.lifecycle.lock().await;
        let conn = self.shared.conn.lock().unwrap().take();
        if let Some(c) = conn {
            // 接続を閉じるだけ。実行中作業の停止確認ではない。切断イベントは受信側が送る。
            c.process.lock().await.shutdown(SHUTDOWN_GRACE).await;
            retain_other_sources(&mut self.shared.pending.lock().unwrap(), &c.source);
        }
        Ok(())
    }

    fn take_events(&self) -> Option<EventReceiver> {
        self.shared.event_rx.lock().unwrap().take()
    }

    async fn start_chat(&self, params: StartChatParams) -> BackendResult<ChatStarted> {
        let (approval, sandbox) = permission_params(params.permission);
        let mut p = json!({"cwd": params.cwd, "approvalPolicy": approval, "sandbox": sandbox});
        if let Some(m) = &params.model {
            p["model"] = json!(m.model);
        }
        let r = self.call("thread/start", p, WRITE_TIMEOUT).await?;
        log_source("thread/start", &r);
        let t = thread_of(&r)?;
        let chat = thread_to_chat(&t, self.app_dir().as_deref(), Known::Value { value: false, basis: Basis::Derived });
        let root = thread_to_agent(&t, chat_key(&t.id));
        self.shared.remember(&root);
        let accepted_model = accepted_model_of(&r);
        Ok(ChatStarted { chat, root, accepted_model })
    }

    async fn list_chats(&self, query: ListChatsQuery) -> BackendResult<Page<ChatSummary>> {
        let mut p = json!({"sourceKinds": source_kinds(&query), "archived": query.archived});
        if let Some(c) = &query.cursor {
            p["cursor"] = json!(c);
        }
        if let Some(l) = query.limit {
            p["limit"] = json!(l);
        }
        if let Some(s) = &query.search {
            p["searchTerm"] = json!(s);
        }
        let r = self.call("thread/list", p, READ_TIMEOUT).await?;
        let data = r.get("data").and_then(Value::as_array).ok_or_else(|| BackendError::Protocol { message: "thread/list: missing data".into() })?;
        let app_dir = self.app_dir();
        let mut items = Vec::with_capacity(data.len());
        for v in data {
            let t = parse_thread(v)?;
            let chat = thread_to_chat(&t, app_dir.as_deref(), Known::Value { value: query.archived, basis: Basis::Derived });
            let root = thread_to_agent(&t, default_chat_for(&t));
            self.shared.remember(&root);
            items.push(ChatSummary {
                chat,
                status: derive_status(&t.status, t.latest_turn(), self.ev(EvidenceSource::StatusQuery, "thread/list")),
                preview: known_str(&t.preview),
                root,
            });
        }
        Ok(Page { items, next_cursor: r.get("nextCursor").and_then(Value::as_str).map(str::to_string) })
    }

    async fn read(&self, agent: AgentKey, options: ReadOptions) -> BackendResult<AgentHistory> {
        let t = self.read_thread(&agent.id.0, options.include_turns).await?;
        Ok(self.history_from_thread(&t, options.include_turns, EvidenceSource::HistoryRead, "thread/read"))
    }

    async fn resume(&self, chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<ResumeOutcome> {
        match self.call("thread/resume", json!({"threadId": chat.id.0}), WRITE_TIMEOUT).await {
            Ok(r) => {
                log_source("thread/resume", &r);
                let t = thread_of(&r)?;
                Ok(ResumeOutcome::Resumed { history: self.history_from_thread(&t, true, EvidenceSource::Response, "thread/resume"), accepted_model: accepted_model_of(&r) })
            }
            // 明示的に拒否された。別threadで代替せず理由を返す。
            Err(BackendError::Rejected { message, .. }) => Ok(ResumeOutcome::Unavailable { reason: message }),
            Err(e) => Err(e),
        }
    }

    async fn manage_chat(&self, chat: ChatKey, op: ManageOp, _confirmed: &UserConfirmed) -> BackendResult<ManageOutcome> {
        let id = chat.id.0;
        let (method, params) = match op {
            ManageOp::Rename { name } => ("thread/name/set", json!({"threadId": id, "name": name})),
            ManageOp::Archive => ("thread/archive", json!({"threadId": id})),
            ManageOp::Unarchive => ("thread/unarchive", json!({"threadId": id})),
            ManageOp::Delete => ("thread/delete", json!({"threadId": id})),
        };
        self.call(method, params, WRITE_TIMEOUT).await?;
        Ok(ManageOutcome::Done)
    }

    async fn list_loaded(&self) -> BackendResult<Vec<AgentKey>> {
        let (data, truncated) = self.collect_pages("thread/loaded/list", json!({}), "data").await?;
        if truncated {
            return Err(BackendError::Protocol { message: "thread/loaded/list: too many pages".into() });
        }
        Ok(data.iter().filter_map(Value::as_str).map(agent_key).collect())
    }

    async fn scan_descendants(&self, root: AgentKey) -> BackendResult<DescendantScan> {
        let (_, experimental) = self.client()?;
        let mut base = json!({"sourceKinds": SUBAGENT_KINDS});
        if experimental {
            base["ancestorThreadId"] = json!(root.id.0);
        }
        let chat = self.shared.chat_cache.lock().unwrap().get(&root).cloned().unwrap_or_else(|| chat_key(&root.id.0));
        let mut all: Vec<Agent> = Vec::new();
        let mut truncated = false;
        // 親が未発見の子がいれば（走査中の生成の可能性）、experimentalのときだけ1回再走査して統合する。
        for _ in 0..2 {
            let (data, trunc) = self.collect_pages("thread/list", base.clone(), "data").await?;
            truncated = trunc;
            for v in &data {
                let t = parse_thread(v)?;
                if !CHILD_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
                    // 子のsourceのキー構造を1回だけ記録（値は書かない）。
                    crate::diag::log("child-source", &crate::diag::shape(v.get("source").unwrap_or(&Value::Null)));
                }
                if t.id != root.id.0 {
                    all.push(thread_to_agent(&t, chat.clone()));
                }
            }
            if !(experimental && assemble(&root, &all).needs_rescan()) {
                break;
            }
        }
        let tree = assemble(&root, &all);
        let mut found = Vec::new();
        flatten(&tree.children, &mut found);
        // 祖先絞り込み（experimental）の結果は全員がこのルートの子孫なので、親が見つからない子も
        // 「親不明」として含める。安定側の代替一覧は他チャットの子も混ざるため、ルートに連ならないものは含めない。
        if experimental {
            found.extend(tree.orphans.iter().map(|o| o.agent.clone()));
        }
        for a in &found {
            self.shared.remember(a);
        }
        // 完全性はページを読み切れたかだけで決める。親が見つからない子が残っても再走査は終わらせる（無限に未完にしない）。
        Ok(DescendantScan { root, found, complete: !truncated })
    }

    async fn send(&self, request: SendRequest) -> SendOutcome {
        // 送る前に確定できる失敗は、何も送っていないので明示拒否。
        let (method, params) = match build_send_call(&request) {
            Ok(x) => x,
            Err(error) => return SendOutcome::Rejected { error, request: NotAccepted::new(request) },
        };
        let result = self.call(method, params, WRITE_TIMEOUT).await;
        match result {
            Ok(v) => match accepted_turn_id(&request.mode, &v) {
                Some(turn_id) => {
                    let thread = match &request.mode {
                        SendMode::NewTurn => request.chat.id.0.clone(),
                        SendMode::Steer { turn } => turn.agent.id.0.clone(),
                    };
                    // turn/startの応答は受理した設定を返さないので NotFetched（適用済み表示しない）。
                    SendOutcome::Accepted { turn: turn_key(&thread, &turn_id), accepted_model: Known::NotFetched }
                }
                // 応答は来たがturn idが読めない: 受理されたか分からない。
                None => SendOutcome::AcceptanceUnknown(UnconfirmedSend::new(request, now_ms())),
            },
            Err(error @ BackendError::Rejected { .. }) | Err(error @ BackendError::NotConnected) => {
                SendOutcome::Rejected { error, request: NotAccepted::new(request) }
            }
            // timeout・切断・書込み失敗・応答不正: 相手が実行した可能性がある。再送しない。
            Err(_) => SendOutcome::AcceptanceUnknown(UnconfirmedSend::new(request, now_ms())),
        }
    }

    async fn reconcile_send(&self, pending: UnconfirmedSend) -> ReconcileOutcome {
        let thread_id = pending.chat().id.0.clone();
        // 履歴取得に失敗・部分取得なら判断しない（再送の根拠にしない）。
        let Ok(t) = self.read_thread(&thread_id, true).await else { return ReconcileOutcome::StillUnknown(pending) };
        match find_client_message(&t, pending.client_message_id(), now_ms().0 - pending.since.0) {
            ClientMessageLookup::Found { turn_id } => ReconcileOutcome::Accepted { turn: turn_key(&thread_id, &turn_id) },
            ClientMessageLookup::NotFound => ReconcileOutcome::NotFound(pending.into_not_accepted()),
            ClientMessageLookup::Undetermined => ReconcileOutcome::StillUnknown(pending),
        }
    }

    async fn respond(&self, request: RequestKey, answer: RequestAnswer, _confirmed: &UserConfirmed) -> BackendResult<RespondOutcome> {
        let (client, _) = self.client()?;
        let stored = self.shared.pending.lock().unwrap().get(&request).cloned();
        let Some(stored) = stored else { return Ok(RespondOutcome::AlreadyResolved) };
        // 提示していない選択肢などはここで拒否（何も送らず、要求は保留のまま）。
        let reply = build_reply(&stored, &answer)?;
        let res = match reply {
            Reply::Result(v) => client.respond(&request, v).await,
            Reply::Error { code, message } => client.respond_error(&request, code, &message).await,
        };
        match res {
            Ok(()) => {
                self.shared.pending.lock().unwrap().remove(&request);
                Ok(RespondOutcome::Delivered)
            }
            Err(BackendError::Protocol { .. }) => {
                // rpc側で未知・回答済み。再送しない。
                self.shared.pending.lock().unwrap().remove(&request);
                Ok(RespondOutcome::AlreadyResolved)
            }
            Err(BackendError::OutcomeUnknown { message }) => {
                self.shared.pending.lock().unwrap().remove(&request);
                Ok(RespondOutcome::Unknown { message })
            }
            Err(e) => Err(e),
        }
    }

    async fn interrupt(&self, turn: TurnKey, _confirmed: &UserConfirmed) -> BackendResult<InterruptAck> {
        match self.call("turn/interrupt", json!({"threadId": turn.agent.id.0, "turnId": turn.turn_id.0}), WRITE_TIMEOUT).await {
            // 受付のみ。停止は turn/completed（TurnEnded）で照合する。
            Ok(_) => Ok(InterruptAck::Requested),
            Err(BackendError::OutcomeUnknown { message }) => Ok(InterruptAck::Unknown { message }),
            Err(e) => Err(e),
        }
    }

    async fn list_models(&self, query: ModelQuery) -> BackendResult<Vec<ModelInfo>> {
        let (data, truncated) = self.collect_pages("model/list", json!({"includeHidden": query.include_hidden}), "data").await?;
        if truncated {
            return Err(BackendError::Protocol { message: "model/list: too many pages".into() });
        }
        Ok(data.iter().filter_map(model_to_info).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: SendMode) -> SendRequest {
        SendRequest {
            chat: chat_key("th"),
            mode,
            text: "hi".into(),
            attachments: vec![],
            model: Some(ModelChoice { model: "m".into(), effort: Some("low".into()) }),
            permission: None,
            cwd: None,
            client_message_id: "cm1".into(),
        }
    }

    fn att(kind: AttachmentKind) -> Attachment {
        Attachment {
            id: LocalId("a".into()),
            kind,
            original_path: "C:/o.png".into(),
            copy_path: Some("C:/copy.png".into()),
            attached_at: UnixMillis(0),
            exists: Known::NotFetched,
            owner_chat: chat_key("th"),
        }
    }

    #[test]
    fn new_turn_params_carry_client_message_id_and_model() {
        let (m, p) = build_send_call(&req(SendMode::NewTurn)).unwrap();
        assert_eq!(m, "turn/start");
        assert_eq!(p["threadId"], "th");
        assert_eq!(p["clientUserMessageId"], "cm1");
        assert_eq!((p["model"].as_str(), p["effort"].as_str()), (Some("m"), Some("low")));
        assert_eq!(p["input"][0], json!({"type": "text", "text": "hi", "text_elements": []}));
    }

    #[test]
    fn new_turn_carries_permission_and_cwd_only_when_set_and_steer_never_does() {
        let (_, p) = build_send_call(&req(SendMode::NewTurn)).unwrap();
        assert!(p.get("sandboxPolicy").is_none() && p.get("approvalPolicy").is_none() && p.get("cwd").is_none());
        let mut r = req(SendMode::NewTurn);
        r.permission = Some(PermissionPreset::ReadOnly);
        r.cwd = Some("C:/w".into());
        let (_, p) = build_send_call(&r).unwrap();
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["sandboxPolicy"], json!({"type": "readOnly", "networkAccess": false}));
        assert_eq!(p["cwd"], "C:/w");
        let mut s = req(SendMode::Steer { turn: turn_key("th", "t9") });
        s.permission = Some(PermissionPreset::FullAccess);
        s.cwd = Some("C:/w".into());
        let (m, p) = build_send_call(&s).unwrap();
        assert_eq!(m, "turn/steer");
        assert!(p.get("sandboxPolicy").is_none() && p.get("cwd").is_none());
    }

    #[test]
    fn steer_requires_same_thread_and_sends_expected_turn() {
        let ok = req(SendMode::Steer { turn: turn_key("th", "t9") });
        let (m, p) = build_send_call(&ok).unwrap();
        assert_eq!((m, p["expectedTurnId"].as_str()), ("turn/steer", Some("t9")));
        let other = req(SendMode::Steer { turn: turn_key("child", "t9") });
        assert!(matches!(build_send_call(&other), Err(BackendError::Unsupported { .. })));
    }

    #[test]
    fn attachments_map_images_to_local_image_and_files_to_text() {
        let i = build_turn_input("x", &[att(AttachmentKind::Image), att(AttachmentKind::Audio)]).unwrap();
        assert_eq!(i[1], json!({"type": "localImage", "path": "C:/copy.png"}));
        assert_eq!(i[2]["type"], "localAudio");
        let f = build_turn_input("x", &[att(AttachmentKind::Image), att(AttachmentKind::File), att(AttachmentKind::File)]).unwrap();
        assert_eq!(f.len(), 3, "files are listed in one text input after the images");
        assert_eq!(f[2], json!({"type": "text", "text": "添付ファイル:\n- copy.png — C:/copy.png\n- copy.png — C:/copy.png", "text_elements": []}));
        assert!(f.iter().all(|v| v["type"] != "mention"));
        // 添付がなければ本文だけ。
        assert_eq!(build_turn_input("x", &[]).unwrap().len(), 1);
    }

    #[test]
    fn accepted_turn_id_requires_turn_in_response() {
        assert_eq!(accepted_turn_id(&SendMode::NewTurn, &json!({"turn": {"id": "t1"}})).as_deref(), Some("t1"));
        assert_eq!(accepted_turn_id(&SendMode::NewTurn, &json!({})), None);
        assert_eq!(accepted_turn_id(&SendMode::Steer { turn: turn_key("a", "b") }, &json!({"turnId": "t2"})).as_deref(), Some("t2"));
    }

    #[test]
    fn list_source_kinds_are_explicit() {
        let q = ListChatsQuery::default();
        assert_eq!(source_kinds(&q), vec!["appServer"]);
        let q = ListChatsQuery { include_external: true, include_descendants: true, ..Default::default() };
        let k = source_kinds(&q);
        assert!(k.contains(&"cli") && k.contains(&"vscode") && k.contains(&"subAgentThreadSpawn"));
    }

    #[test]
    fn capabilities_depend_on_experimental_flag() {
        assert_eq!(codex_capabilities(false).descendant_search, Support::Experimental);
        assert_eq!(codex_capabilities(true).descendant_search, Support::Supported);
        assert_eq!(codex_capabilities(false).steer, Support::Supported);
    }

    #[tokio::test]
    async fn operations_before_connect_are_not_connected_and_send_is_rejected_not_unknown() {
        let b = CodexBackend::new();
        assert!(b.take_events().is_some());
        assert!(b.take_events().is_none());
        assert_eq!(b.capabilities().resume, Support::Unknown);
        assert!(matches!(b.list_loaded().await, Err(BackendError::NotConnected)));
        match b.send(req(SendMode::NewTurn)).await {
            SendOutcome::Rejected { error: BackendError::NotConnected, .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn disconnect_only_clears_the_matching_source() {
        let (old, new) = (SourceId("old".into()), SourceId("new".into()));
        assert!(should_clear_conn(Some(&old), &old));
        assert!(!should_clear_conn(Some(&new), &old), "a late disconnect of the old source must not drop the new connection");
        assert!(!should_clear_conn(None, &old));
        let key = |s: &SourceId, n: &str| RequestKey { backend: BackendKind::Codex, source: s.clone(), request_id: ExternalId(n.into()) };
        let mut pending: HashMap<RequestKey, u8> = HashMap::new();
        pending.insert(key(&old, "n:1"), 1);
        pending.insert(key(&new, "n:1"), 2);
        retain_other_sources(&mut pending, &old);
        assert_eq!(pending.len(), 1);
        assert!(pending.contains_key(&key(&new, "n:1")));
    }

    #[tokio::test]
    async fn connect_to_missing_executable_is_launch_failed_and_reported_as_event() {
        let b = CodexBackend::new();
        let mut rx = b.take_events().unwrap();
        let cfg = || ConnectConfig {
            executable: Some("Z:/definitely/not/here/codex.exe".into()),
            expected_version: None,
            enable_experimental: false,
            app_data_dir: "C:/tmp".into(),
        };
        // 直列化されているので同時に呼んでも壊れない（どちらも LaunchFailed）。
        let (a, c) = tokio::join!(b.connect(cfg()), b.connect(cfg()));
        for r in [a, c] {
            let info = r.unwrap();
            assert!(matches!(info.connection, ConnectionState::LaunchFailed { .. }));
            assert_eq!(info.pid, Known::NotFetched);
        }
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(ev.event, BackendEvent::Connection { state: ConnectionState::LaunchFailed { .. } }));
        assert!(matches!(b.list_loaded().await, Err(BackendError::NotConnected)));
    }
}
