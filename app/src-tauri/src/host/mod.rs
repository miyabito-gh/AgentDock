//! ホスト（Tauri側の業務層）。バックエンド（`AiBackend`）とUIの間で状態を持ち、IPCコマンドの中身を実装する。
//!
//! 規則（DESIGN_T1・要件§3）:
//! - 状態を変える操作（送信・回答・中断・再開・管理）は `UserConfirmed` を要求する。証票はコマンド層（`crate::commands`）
//!   だけが作る。このモジュールの自動処理（イベント処理・走査・照合）は証票を作らない。
//! - イベントの消費は専用taskで常にdrainし、イベント処理中に要求（`request`）をawaitしない。追加作業は別taskへ逃がす。
//! - 切断・受理不明で自動再送しない。照合（読み取りのみ）は自動で行ってよい。
//! - 監視のためにresume・承認・停止をしない。resumeはユーザーの送信・再開操作の中でだけ行う。

pub mod notifier;
pub mod persist;
pub mod queue_driver;
pub mod state;
pub mod stop;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::model::*;
use crate::codex::{CodexBackend, TARGET_VERSION};
use persist::Persist;
use state::{agent_key_of, Followup, HostData};

pub type Emitter = Arc<dyn Fn(HostEventEnvelope) + Send + Sync>;

/// 既定のcodex.exe（Codexデスクトップ版の配置）。`~/.codex` は共有し、このアプリからは変更しない。
pub const DEFAULT_CODEX_EXE: &str = r"C:\Users\wmasa\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe";

const LIST_DEFAULT_LIMIT: u32 = 50;
const SCAN_READ_LIMIT: usize = 50;
const SCAN_INTERVAL: Duration = Duration::from_secs(3);
const SCAN_MAX_ROUNDS: usize = 600;
const NAME_MAX_CHARS: usize = 40;

pub fn now_ms() -> UnixMillis {
    UnixMillis(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
}

// ───────────────────────────── エラー変換 ─────────────────────────────

fn err(code: IpcErrorCode, message: impl Into<String>) -> IpcError {
    IpcError { code, message: message.into(), blocked: None }
}

fn blocked(reason: BlockedReason, message: impl Into<String>) -> IpcError {
    IpcError { code: IpcErrorCode::Blocked, message: message.into(), blocked: Some(reason) }
}

impl From<BackendError> for IpcError {
    fn from(e: BackendError) -> Self {
        let message = e.to_string();
        match e {
            BackendError::NotConnected => err(IpcErrorCode::NotConnected, message),
            BackendError::Unsupported { capability } => {
                IpcError { code: IpcErrorCode::Unsupported, message, blocked: Some(BlockedReason::CapabilityUnsupported { capability }) }
            }
            BackendError::Rejected { .. } => err(IpcErrorCode::Rejected, message),
            BackendError::OutcomeUnknown { .. } => err(IpcErrorCode::OutcomeUnknown, message),
            BackendError::Protocol { .. } => err(IpcErrorCode::Protocol, message),
            BackendError::Io { .. } => err(IpcErrorCode::Io, message),
        }
    }
}

/// 走査ループ内で覚えておく状態（ループが終われば捨てる）。
#[derive(Default)]
pub struct ScanMemo {
    /// 一度読んで、親が明示されていない（このチャットの子孫でない）と分かったスレッド。
    irrelevant: HashSet<AgentKey>,
    /// 親は明示されているが、その親がまだ見つかっていない子（読み取り済みの内容）。
    waiting: HashMap<AgentKey, (Agent, AgentStatus)>,
    off_fresh: usize,
    off_loaded: usize,
    off_stale: usize,
}

/// ラウンドロビン。`offset` から最大 `limit` 件を（末尾で折り返して）取り、offset を進める。全件を順に回る。
pub fn take_window<T: Clone>(items: &[T], offset: &mut usize, limit: usize) -> Vec<T> {
    if items.is_empty() || limit == 0 {
        return Vec::new();
    }
    let n = items.len().min(limit);
    let start = *offset % items.len();
    let out: Vec<T> = (0..n).map(|i| items[(start + i) % items.len()].clone()).collect();
    *offset = (start + n) % items.len();
    out
}

/// ロード済み一覧のうち、まだ読んでいない候補（既知でなく、無関係と判明済みでも待機中でもないもの）。
pub fn pick_unread(loaded: &[AgentKey], is_known: &dyn Fn(&AgentKey) -> bool, memo: &ScanMemo) -> Vec<AgentKey> {
    loaded.iter().filter(|k| !is_known(k) && !memo.irrelevant.contains(*k) && !memo.waiting.contains_key(*k)).cloned().collect()
}

/// 最初の依頼文からチャット名を機械的に切り出す（先頭行・上限文字数）。
pub fn derive_chat_name(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut name: String = line.chars().take(NAME_MAX_CHARS).collect();
    if line.chars().count() > NAME_MAX_CHARS {
        name.push('…');
    }
    Some(name)
}

/// 受理不明で未解決の送信。照合中（`unconfirmed` から取り出している間）も解決するまで残し、チャットの送信を止める。
#[derive(Default)]
pub struct UnresolvedSends {
    by_attempt: HashMap<LocalId, ChatKey>,
}

impl UnresolvedSends {
    pub fn add(&mut self, attempt: LocalId, chat: ChatKey) {
        self.by_attempt.insert(attempt, chat);
    }
    /// 受理または受理なしが確定したときだけ呼ぶ。
    pub fn resolve(&mut self, attempt: &LocalId) {
        self.by_attempt.remove(attempt);
    }
    pub fn contains(&self, attempt: &LocalId) -> bool {
        self.by_attempt.contains_key(attempt)
    }
    pub fn blocking(&self, chat: &ChatKey) -> Option<LocalId> {
        self.by_attempt.iter().find(|(_, c)| *c == chat).map(|(id, _)| id.clone())
    }
}

// ───────────────────────────── Host ─────────────────────────────

pub struct Host {
    backend: Arc<CodexBackend>,
    data: Mutex<HostData>,
    emitter: OnceLock<Emitter>,
    executable: Mutex<String>,
    app_data_dir: PathBuf,
    /// 保存の呼出し（`persist.rs`）。
    persist: Persist,
    /// OS通知の配信（`notifier.rs`）。
    notifier: notifier::Notifier,
    /// 受理不明の送信（照合以外で解消しない）。
    unconfirmed: Mutex<HashMap<LocalId, UnconfirmedSend>>,
    /// 受理不明のまま未解決の送信（照合で取り出している間も含む）。送信・再送を止める根拠。
    unresolved: Mutex<UnresolvedSends>,
    /// 受理なしが確定した送信（ユーザー確認つきの再送だけ可能）。
    rejected: Mutex<HashMap<LocalId, NotAccepted>>,
    counter: AtomicU64,
    /// 走査ループが動いているルート（重複起動しない）。
    scanning: Mutex<HashSet<AgentKey>>,
    /// 完了後キューの実行状態（`queue_driver.rs`）。
    queue_rt: queue_driver::QueueRuntime,
}

impl Host {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Host {
            backend: Arc::new(CodexBackend::new()),
            data: Mutex::new(HostData::default()),
            emitter: OnceLock::new(),
            executable: Mutex::new(DEFAULT_CODEX_EXE.to_string()),
            app_data_dir,
            persist: Persist::new(None),
            notifier: notifier::Notifier::default(),
            unconfirmed: Mutex::new(HashMap::new()),
            unresolved: Mutex::new(UnresolvedSends::default()),
            rejected: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(1),
            scanning: Mutex::new(HashSet::new()),
            queue_rt: queue_driver::QueueRuntime::default(),
        }
    }

    pub fn set_emitter(&self, e: Emitter) {
        let _ = self.emitter.set(e);
    }

    fn local_id(&self, prefix: &str) -> LocalId {
        LocalId(format!("{prefix}-{}-{}", now_ms().0, self.counter.fetch_add(1, Ordering::SeqCst)))
    }

    /// 状態を更新し、返したイベントを採番して発行する（ロック内で発行し、順序を保つ）。
    fn mutate<R>(&self, f: impl FnOnce(&mut HostData) -> (R, Vec<HostEvent>)) -> R {
        let mut d = self.data.lock().unwrap();
        let (r, events) = f(&mut d);
        for event in events {
            d.seq += 1;
            if let Some(emit) = self.emitter.get() {
                emit(HostEventEnvelope { seq: d.seq, at: now_ms(), event });
            }
        }
        let inbox = std::mem::take(&mut d.notify_inbox);
        drop(d);
        if !inbox.is_empty() {
            self.notify_submit(inbox);
        }
        r
    }

    fn read<R>(&self, f: impl FnOnce(&HostData) -> R) -> R {
        f(&self.data.lock().unwrap())
    }

    fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        self.mutate(|_| ((), vec![HostEvent::Warning { source: None, message, raw_label: None }]));
    }

    pub fn snapshot(&self) -> HostSnapshot {
        self.read(|d| d.snapshot())
    }

    // ── イベント処理 ──

    /// バックエンドのイベントを常にdrainする専用task。ここでは要求をawaitしない。
    pub fn start_event_pump(self: &Arc<Self>) {
        self.start_queue_driver();
        let Some(mut rx) = self.backend.take_events() else { return };
        let host = self.clone();
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                host.handle_backend_event(&env);
            }
        });
    }

    pub fn handle_backend_event(self: &Arc<Self>, env: &EventEnvelope) {
        let caps = self.backend.capabilities();
        let follow = self.mutate(|d| {
            let (events, follow) = d.apply_event(env, &caps);
            (follow, events)
        });
        if !matches!(env.event, BackendEvent::Activity { .. } | BackendEvent::ActivityDelta { .. }) {
            self.kick_queue();
        }
        for f in follow {
            match f {
                Followup::ScanDescendants(root) => {
                    let host = self.clone();
                    host.start_scan(root);
                }
            }
        }
    }

    /// 子孫の監視を始める（同じルートに1つだけ。読み取りのみで resume・送信・承認をしない）。
    /// 実行中は一定間隔で走査と状態の再読込みを続け、止まったら終わる。
    fn start_scan(self: &Arc<Self>, root: AgentKey) {
        if !self.scanning.lock().unwrap().insert(root.clone()) {
            return;
        }
        self.queue_rt.reset_scan(&root);
        let host = self.clone();
        tokio::spawn(async move {
            let chat = ChatKey { backend: root.backend, id: root.id.clone() };
            let mut quiet = 0;
            let mut memo = ScanMemo::default();
            for _ in 0..SCAN_MAX_ROUNDS {
                if host.scan_once(root.clone(), &mut memo).await.is_err() {
                    break;
                }
                let running = host.read(|d| d.agents.iter().any(|v| v.agent.chat == chat && d.running_turn.contains_key(&v.agent.key)));
                quiet = if running { 0 } else { quiet + 1 };
                if quiet >= 2 {
                    break;
                }
                tokio::time::sleep(SCAN_INTERVAL).await;
            }
            host.scanning.lock().unwrap().remove(&root);
        });
    }

    /// 1回分の走査。①祖先／sourceKindsでの検索 ②ロード済み一覧からの発見（親の明示が手掛かり）③既知の子の状態の再読込み。
    /// 読取りは1周期あたり各段階 `SCAN_READ_LIMIT` 件まで。件数が多いときは周回ごとにずらして全件を順に回す。
    /// 診断ログには引数・件数・完全性・親不明件数だけを残す。接続がなければ Err（ループ終了）。
    async fn scan_once(self: &Arc<Self>, root: AgentKey, memo: &mut ScanMemo) -> Result<(), BackendError> {
        let chat = ChatKey { backend: root.backend, id: root.id.clone() };
        let (mut found_n, mut complete, mut orphans, mut added) = (0usize, false, 0usize, 0usize);
        let (mut explicit, mut applied, mut dockable) = (0usize, 0usize, 0usize);
        match self.backend.scan_descendants(root.clone()).await {
            Ok(scan) => {
                found_n = scan.found.len();
                complete = scan.complete;
                self.queue_rt.note_scan(&root, scan.complete);
                orphans = scan.found.iter().filter(|a| matches!(a.parent, ParentLink::Unknown)).count();
                explicit = scan.found.iter().filter(|a| matches!(a.parent, ParentLink::Explicit { .. })).count();
                // 未登録、または所属不明の仮登録のままのものを、見つかった正しい所属・親で反映する。
                let todo: Vec<Agent> = self.read(|d| scan.found.iter().filter(|a| d.needs_adoption(&a.key)).cloned().collect());
                for a in take_window(&todo, &mut memo.off_fresh, SCAN_READ_LIMIT) {
                    match self.backend.read(a.key.clone(), ReadOptions { include_turns: false }).await {
                        Ok(h) => {
                            self.mutate(|d| ((), d.upsert_history_agent(a, h.status)));
                        }
                        // 状態を読めなくても、所属・親は正しく直す（状態は据え置き）。登録が無ければ何もしない。
                        Err(_) => {
                            self.mutate(|d| ((), d.adopt_agent(a)));
                        }
                    }
                    applied += 1;
                }
                dockable = self.read(|d| scan.found.iter().filter(|a| d.view(&a.key).is_some_and(|v| v.agent.chat == chat)).count());
            }
            Err(BackendError::NotConnected) => return Err(BackendError::NotConnected),
            Err(e) => crate::diag::log("scan", &format!("scan_descendants failed root={} kind={}", root.id.0, crate::diag::error_kind(&e))),
        }
        // ロード済み一覧: 親が既知のエージェントの子として明示されているものを加える。
        // 親が明示されていない（無関係な）スレッドは一度読んだら覚えて再読込みしない。親が未発見の子は待機に置き、親が現れたら加える。
        let mut loaded_n = 0usize;
        match self.backend.list_loaded().await {
            Ok(loaded) => {
                loaded_n = loaded.len();
                let candidates = self.read(|d| pick_unread(&loaded, &|k| !d.needs_adoption(k), memo));
                for k in take_window(&candidates, &mut memo.off_loaded, SCAN_READ_LIMIT) {
                    let Ok(h) = self.backend.read(k.clone(), ReadOptions { include_turns: false }).await else { continue };
                    match &h.agent.parent {
                        ParentLink::Explicit { .. } => {
                            memo.waiting.insert(k, (h.agent, h.status));
                        }
                        _ => {
                            memo.irrelevant.insert(k);
                        }
                    }
                }
                loop {
                    let ready: Vec<AgentKey> = self.read(|d| {
                        memo.waiting
                            .iter()
                            .filter(|(_, (a, _))| matches!(&a.parent, ParentLink::Explicit { parent } if d.view(parent).is_some()))
                            .map(|(k, _)| k.clone())
                            .collect()
                    });
                    if ready.is_empty() {
                        break;
                    }
                    for k in ready {
                        let Some((mut agent, status)) = memo.waiting.remove(&k) else { continue };
                        if let ParentLink::Explicit { parent } = &agent.parent {
                            if let Some(pchat) = self.read(|d| d.view(parent).map(|v| v.agent.chat.clone())) {
                                agent.chat = pchat;
                            }
                        }
                        self.mutate(|d| ((), d.upsert_history_agent(agent, status)));
                        added += 1;
                    }
                }
            }
            Err(BackendError::NotConnected) => return Err(BackendError::NotConnected),
            Err(e) => crate::diag::log("scan", &format!("list_loaded failed kind={}", crate::diag::error_kind(&e))),
        }
        // 既知の子（live購読でないもの）の状態を読み直す。購読が無くても実行中・完了が追随する。
        let stale: Vec<(AgentKey, ChatKey)> = self.read(|d| {
            d.agents
                .iter()
                .filter(|v| v.agent.chat == chat && !matches!(v.agent.parent, ParentLink::Root) && v.freshness != Freshness::Live)
                .map(|v| (v.agent.key.clone(), v.agent.chat.clone()))
                .collect()
        });
        for (k, c) in take_window(&stale, &mut memo.off_stale, SCAN_READ_LIMIT) {
            if let Ok(h) = self.backend.read(k, ReadOptions { include_turns: false }).await {
                let mut agent = h.agent;
                agent.chat = c;
                self.mutate(|d| ((), d.upsert_history_agent(agent, h.status)));
            }
        }
        crate::diag::log(
            "scan",
            &format!(
                "root={} found={found_n} explicitParent={explicit} applied={applied} dockable={dockable} complete={complete} orphans={orphans} loaded={loaded_n} added={added} irrelevant={} waiting={}",
                root.id.0,
                memo.irrelevant.len(),
                memo.waiting.len()
            ),
        );
        Ok(())
    }

    // ── 接続 ──

    pub async fn connect(self: &Arc<Self>, args: ConnectBackendArgs) -> Result<SourceInfo, IpcError> {
        let exe = {
            let mut cur = self.executable.lock().unwrap();
            if let Some(e) = args.executable.filter(|e| !e.trim().is_empty()) {
                *cur = e.trim().to_string();
            }
            cur.clone()
        };
        let config = ConnectConfig {
            executable: Some(exe),
            expected_version: Some(TARGET_VERSION.to_string()),
            enable_experimental: true,
            app_data_dir: self.app_data_dir.to_string_lossy().into_owned(),
        };
        let info = self.backend.connect(config).await?;
        self.mutate(|d| ((), d.set_source(info.clone())));
        Ok(info)
    }

    pub async fn shutdown(self: &Arc<Self>) {
        self.flush_pending().await;
        let _ = self.backend.disconnect().await;
    }

    // ── 一覧・履歴 ──

    pub async fn list_chats(&self, args: ListChatsArgs) -> Result<Page<ChatSummary>, IpcError> {
        let query = ListChatsQuery {
            cursor: args.cursor,
            limit: Some(args.limit.unwrap_or(LIST_DEFAULT_LIMIT)),
            search: args.search.filter(|s| !s.is_empty()),
            archived: args.include_archived,
            include_external: true,
            include_descendants: false,
        };
        let page = self.backend.list_chats(query).await?;
        self.mutate(|d| {
            let mut ev = Vec::new();
            for s in &page.items {
                ev.extend(d.upsert_chat(s.chat.clone()));
                ev.extend(d.upsert_history_agent(s.root.clone(), s.status.clone()));
            }
            ((), ev)
        });
        Ok(page)
    }

    pub async fn open_chat(self: &Arc<Self>, args: OpenChatArgs) -> Result<AgentHistory, IpcError> {
        let history = self.backend.read(agent_key_of(&args.chat), ReadOptions { include_turns: true }).await?;
        self.mutate(|d| {
            let mut ev = Vec::new();
            if let Some(c) = &history.chat {
                ev.extend(d.upsert_chat(c.clone()));
            }
            ev.extend(d.upsert_history_agent(history.agent.clone(), history.status.clone()));
            ((), ev)
        });
        let host = self.clone();
        let root = agent_key_of(&args.chat);
        host.start_scan(root);
        Ok(history)
    }

    // ── 会話の開始・再開 ──

    pub async fn start_chat(self: &Arc<Self>, args: StartChatArgs, confirmed: &UserConfirmed) -> Result<StartChatResult, IpcError> {
        let mut area: Option<LocalId> = None;
        let (kind, cwd) = match args.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => {
                if !std::path::Path::new(c).is_dir() {
                    return Err(err(IpcErrorCode::InvalidArgs, format!("作業フォルダが見つかりません: {c}")));
                }
                (ChatKind::Development, c.to_string())
            }
            None => {
                // 一般チャット: アプリ管理の作業領域（専用領域のチャット別フォルダ内）を割り当てる（M21）。
                let (dir, id) = self.new_general_workspace()?;
                area = id;
                (ChatKind::General, dir)
            }
        };
        let params = StartChatParams {
            kind,
            cwd,
            model: args.model.clone(),
            permission: args.permission.unwrap_or(PermissionPreset::WorkspaceWriteOnRequest),
        };
        let started = match self.backend.start_chat(params).await {
            Ok(s) => s,
            Err(e) => {
                self.discard_general_workspace(&area);
                return Err(e.into());
            }
        };
        let key = started.chat.key.clone();
        self.adopt_general_workspace(&key, &area);
        if area.is_some() || args.model.is_some() || args.permission.is_some() {
            // 作業領域とチャットの結び付き、チャット別のモデル・権限を保存する。
            self.update_local(&key, false, Duration::ZERO, |f| {
                f.model = args.model.clone();
                f.permission = args.permission;
            });
        }
        let now = now_ms();
        let settings = ChatModelSettings {
            selected: args.model.clone(),
            accepted: started.accepted_model.clone(),
            effective: Known::NotFetched,
            applies: ApplyTiming::NextTurn,
        };
        // 作成直後でturnがないので待機中（Idle）。応答由来であることをevidenceに残す。
        let status = AgentStatus {
            state: AgentState::Idle,
            raw: RawState { label: "thread/start".into() },
            scope: StateScope::Agent,
            turn: None,
            wait: None,
            evidence: Evidence { source: EvidenceSource::Response, raw_label: Some("thread/start".into()), source_time: None, observed_at: now },
        };
        let chat = self.mutate(|d| {
            d.hosted.insert(key.clone());
            let mut ev = d.upsert_chat(started.chat.clone());
            ev.extend(d.set_live(started.root.clone(), status));
            d.model_settings.insert(key.clone(), settings.clone());
            ev.push(HostEvent::ModelSettingsUpdated { chat: key.clone(), settings });
            (d.chat(&key).cloned().unwrap_or(started.chat.clone()), ev)
        });
        let mut first_send = None;
        if let Some(text) = args.first_message.filter(|t| !t.trim().is_empty()) {
            if let Some(name) = derive_chat_name(&text) {
                if self.backend.manage_chat(key.clone(), ManageOp::Rename { name: name.clone() }, confirmed).await.is_ok() {
                    self.mutate(|d| {
                        if let Some(c) = d.chats.iter_mut().find(|c| c.key == key) {
                            c.name = Known::direct(name);
                            let c = c.clone();
                            return ((), vec![HostEvent::ChatUpdated { chat: c }]);
                        }
                        ((), vec![])
                    });
                }
            }
            let request = SendRequest {
                chat: key.clone(),
                mode: SendMode::NewTurn,
                text,
                attachments: Vec::new(),
                model: args.model.clone(),
                // スレッド開始時に権限・作業フォルダを渡し済み。
                permission: None,
                cwd: None,
                client_message_id: self.local_id("cm").0,
            };
            first_send = Some(self.dispatch_send(request).await);
        }
        Ok(StartChatResult { chat: self.read(|d| d.chat(&key).cloned()).unwrap_or(chat), first_send })
    }

    /// 送信・再送の前提（live購読）を整える。保存履歴だけの会話は、ユーザー操作の中でだけresumeする。
    async fn ensure_live(self: &Arc<Self>, chat: &ChatKey, confirmed: &UserConfirmed) -> Result<(), IpcError> {
        match self.read(|d| d.root_view(chat).map(|v| v.freshness)) {
            Some(Freshness::Live) => return Ok(()),
            None => return Err(err(IpcErrorCode::NotFound, "チャットが一覧にありません")),
            _ => {}
        }
        match self.backend.resume(chat.clone(), confirmed).await? {
            ResumeOutcome::Resumed { history } => {
                self.apply_resumed(history);
                Ok(())
            }
            ResumeOutcome::RunningElsewhere => Err(blocked(BlockedReason::RunningElsewhere, "外部で実行中のため、この会話には送信できません")),
            ResumeOutcome::Unavailable { reason } => Err(err(IpcErrorCode::Rejected, format!("会話を再開できません: {reason}"))),
        }
    }

    fn apply_resumed(&self, history: AgentHistory) {
        self.mutate(|d| {
            // 再開した外部会話の origin は変えない（UIは鮮度で送信可否を判断し、バナーを「再開済み」にする）。
            let mut ev = Vec::new();
            if let Some(c) = &history.chat {
                ev.extend(d.upsert_chat(c.clone()));
            }
            ev.extend(d.set_live(history.agent, history.status));
            ((), ev)
        });
    }

    pub async fn resume_chat(self: &Arc<Self>, args: ResumeChatArgs, confirmed: &UserConfirmed) -> Result<ResumeOutcome, IpcError> {
        let out = self.backend.resume(args.chat.clone(), confirmed).await?;
        if let ResumeOutcome::Resumed { history } = &out {
            self.apply_resumed(history.clone());
            let host = self.clone();
            let root = agent_key_of(&args.chat);
            host.start_scan(root);
        }
        Ok(out)
    }

    // ── 送信 ──

    /// 送信を止める条件（停止未確認・受理不明・外部実行の可能性）。
    fn check_sendable(&self, chat: &ChatKey) -> Result<(), IpcError> {
        if let Some(rec) = self.read(|d| d.open_stop(chat).map(|r| r.id.clone())) {
            return Err(blocked(BlockedReason::StopUnconfirmed { record: rec }, "停止を確認できるまで、このチャットへの新しい送信は止めています"));
        }
        if let Some(attempt) = self.unknown_attempt(chat) {
            return Err(blocked(BlockedReason::AcceptanceUnknown { attempt }, "前の送信の受理を確認できるまで、再送・新しい送信は止めています"));
        }
        let ext_not_live = self.read(|d| {
            d.chat(chat).is_some_and(|c| state::external_send_locked(c.origin, d.root_view(chat).map(|v| v.freshness)))
        });
        if ext_not_live {
            return Err(blocked(BlockedReason::RunningElsewhere, "外部で作成された会話です。外部の実行が終わったことを確認して再開してください"));
        }
        Ok(())
    }

    fn unknown_attempt(&self, chat: &ChatKey) -> Option<LocalId> {
        self.unresolved.lock().unwrap().blocking(chat)
    }

    pub async fn send_message(self: &Arc<Self>, args: SendMessageArgs, confirmed: &UserConfirmed) -> Result<SendAttempt, IpcError> {
        if args.text.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "メッセージが空です"));
        }
        if !args.attachments.is_empty() {
            return Err(err(IpcErrorCode::Unsupported, "添付の送信は段階②で対応します"));
        }
        self.check_sendable(&args.chat)?;
        self.ensure_live(&args.chat, confirmed).await?;
        let root = agent_key_of(&args.chat);
        let (running, active, model) = self.read(|d| {
            (
                d.running_turn.get(&root).cloned(),
                d.root_view(&args.chat).is_some_and(|v| matches!(v.status.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing)),
                d.model_settings.get(&args.chat).and_then(|s| s.selected.clone()),
            )
        });
        let mode = match (args.intent, running) {
            (SendIntent::Steer, Some(t)) => SendMode::Steer { turn: TurnKey { agent: root, turn_id: t } },
            (SendIntent::Steer, None) => return Err(err(IpcErrorCode::InvalidArgs, "追加指示の対象turnを特定できません")),
            (SendIntent::NewTurn, Some(_)) => return Err(err(IpcErrorCode::InvalidArgs, "実行中です。追加指示として送るか、完了後に送ってください")),
            (SendIntent::NewTurn, None) if active => {
                return Err(err(IpcErrorCode::InvalidArgs, "実行中の可能性があり、turnを特定できないため新しいturnを開始しません"))
            }
            (SendIntent::NewTurn, None) => SendMode::NewTurn,
        };
        // 権限・作業フォルダは新しいturnにだけ付ける。追加指示（turn/steer）では設定を変えない。
        let (permission, cwd) = if matches!(mode, SendMode::NewTurn) { self.send_overrides(&args.chat) } else { (None, None) };
        let request = SendRequest {
            chat: args.chat,
            mode,
            text: args.text,
            attachments: Vec::new(),
            model,
            permission,
            cwd,
            client_message_id: self.local_id("cm").0,
        };
        Ok(self.dispatch_send(request).await)
    }

    pub async fn retry_send(self: &Arc<Self>, args: RetrySendArgs, confirmed: UserConfirmed) -> Result<SendAttempt, IpcError> {
        if self.unresolved.lock().unwrap().contains(&args.attempt) {
            return Err(blocked(BlockedReason::AcceptanceUnknown { attempt: args.attempt }, "受理を確認できるまで再送しません"));
        }
        self.check_sendable(&args.chat)?;
        // 取り出す前に前提（live）を整える。失敗したら再送権を失わないよう、先に確認する。
        if !self.rejected.lock().unwrap().contains_key(&args.attempt) {
            return Err(err(IpcErrorCode::NotFound, "再送できる送信がありません（受理なしが確定した送信だけ再送できます）"));
        }
        self.ensure_live(&args.chat, &confirmed).await?;
        let Some(not_accepted) = self.rejected.lock().unwrap().remove(&args.attempt) else {
            return Err(err(IpcErrorCode::NotFound, "再送できる送信がありません"));
        };
        let request = SendRequest::retry(not_accepted, confirmed, self.local_id("cm").0);
        // キューの依頼だった場合は、項目の状態（送信中→受理など）も更新する。
        if let Some((chat, entry)) = self.queue_entry_for_attempt(&args.attempt) {
            return self.retry_entry_send(&chat, entry, request).await;
        }
        Ok(self.dispatch_send(request).await)
    }

    /// 送信を1回だけ行い、結果を `SendUpdated` で通知する。Errを返さず3値に分類する。
    async fn dispatch_send(self: &Arc<Self>, request: SendRequest) -> SendAttempt {
        let attempt_id = self.local_id("att");
        let chat = request.chat.clone();
        let mut attempt = SendAttempt { attempt_id: attempt_id.clone(), client_message_id: request.client_message_id.clone(), at: now_ms(), state: SendState::Sending };
        self.emit_send(&chat, &attempt);
        let sent_cwd = if matches!(request.mode, SendMode::NewTurn) { request.cwd.clone() } else { None };
        let state = match self.backend.send(request).await {
            SendOutcome::Accepted { turn, .. } => SendState::Accepted { turn },
            SendOutcome::Rejected { error, request } => {
                self.rejected.lock().unwrap().insert(attempt_id.clone(), request);
                SendState::Rejected { message: error.to_string() }
            }
            SendOutcome::AcceptanceUnknown(u) => {
                let since = u.since;
                // 登録・保存・照合の開始（切断・再起動をまたいで続ける）は queue_driver.rs。
                self.register_unknown(&chat, &attempt_id, None, None, u);
                SendState::AcceptanceUnknown { since }
            }
        };
        attempt.state = state;
        self.emit_send(&chat, &attempt);
        if matches!(attempt.state, SendState::Accepted { .. }) {
            // 新しいturnで子孫が生まれ得る。キュー後続の判定に前の走査結果（完了）を残さず、読み取りだけの再走査を始める。
            let root = agent_key_of(&chat);
            self.queue_rt.reset_scan(&root);
            self.start_scan(root);
            if let Some(cwd) = sent_cwd {
                self.apply_cwd_after_accept(&chat, cwd);
            }
            // ユーザーの利用として最近利用時刻を更新する（背景活動では更新しない、§3.6）。
            self.mutate(|d| {
                if let Some(c) = d.chats.iter_mut().find(|c| c.key == chat) {
                    c.last_used_at = Some(now_ms());
                    let c = c.clone();
                    return ((), vec![HostEvent::ChatUpdated { chat: c }]);
                }
                ((), vec![])
            });
            self.update_local(&chat, true, Duration::ZERO, |f| f.last_used_at = Some(now_ms()));
        }
        attempt
    }

    fn emit_send(&self, chat: &ChatKey, attempt: &SendAttempt) {
        let (chat, attempt) = (chat.clone(), attempt.clone());
        self.mutate(|_| ((), vec![HostEvent::SendUpdated { chat, attempt }]));
    }

    // ── 承認・質問 ──

    pub async fn respond_request(&self, args: RespondRequestArgs, confirmed: &UserConfirmed) -> Result<RespondOutcome, IpcError> {
        match self.read(|d| d.requests.iter().find(|r| r.key == args.request).map(|r| r.state.clone())) {
            None => return Err(err(IpcErrorCode::NotFound, "要求が見つかりません")),
            Some(RequestState::Pending) => {}
            Some(_) => return Err(blocked(BlockedReason::RequestAlreadyResolved, "すでに回答済み、または解決済みです")),
        }
        let out = self.backend.respond(args.request.clone(), args.answer.clone(), confirmed).await?;
        let option_id = match &args.answer {
            RequestAnswer::Decision { option_id } => Some(option_id.clone()),
            RequestAnswer::Answers { .. } => None,
        };
        let at = now_ms();
        // UIの状態はこの通知で確定する（UI側で先取りしない）。
        self.mutate(|d| {
            if let Some(r) = d.requests.iter_mut().find(|r| r.key == args.request) {
                r.state = match &out {
                    RespondOutcome::Delivered => RequestState::Answered { option_id, at },
                    RespondOutcome::AlreadyResolved => RequestState::ResolvedElsewhere { at },
                    RespondOutcome::Unknown { .. } => RequestState::AnswerUnconfirmed { at },
                };
                let request = r.clone();
                let marks = d.marks_event(&request.chat);
                return ((), std::iter::once(HostEvent::RequestUpdated { request }).chain(marks).collect());
            }
            ((), vec![])
        });
        Ok(out)
    }

    // ── 中断 ──

    /// 中断要求を送る。受付は停止ではない。停止は終端の観測で確認し、10秒で確認できなければ案内する。
    pub async fn interrupt_chat(self: &Arc<Self>, args: InterruptChatArgs, confirmed: &UserConfirmed) -> Result<InterruptChatResult, IpcError> {
        let chat = args.chat;
        let root = agent_key_of(&chat);
        let mut targets: Vec<TurnKey> = self.read(|d| {
            d.agents
                .iter()
                .filter(|v| v.agent.chat == chat)
                .filter_map(|v| d.running_turn.get(&v.agent.key).map(|t| TurnKey { agent: v.agent.key.clone(), turn_id: t.clone() }))
                .collect()
        });
        targets.sort_by_key(|t| t.agent != root); // ルートを先に
        if targets.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "中断できる実行中のturnを特定できません"));
        }
        let mut results = Vec::new();
        for t in targets {
            let r = self.backend.interrupt(t.clone(), confirmed).await;
            results.push((t, r));
        }
        let now = now_ms();
        let mut stop_targets = Vec::new();
        let mut first_err = None;
        let mut ack = None;
        for (i, (turn, r)) in results.into_iter().enumerate() {
            match r {
                Ok(a @ (InterruptAck::Requested | InterruptAck::Unknown { .. })) => {
                    if i == 0 {
                        ack = Some(a);
                    }
                    stop_targets.push(stop::new_target(turn, Some(now), now));
                }
                Ok(InterruptAck::NotRunning) => {
                    if i == 0 {
                        ack = Some(InterruptAck::NotRunning);
                    }
                }
                Err(e) => {
                    self.warn(format!("中断要求を送れませんでした（{}）: {e}", turn.agent.id.0));
                    if i == 0 {
                        first_err = Some(e.clone());
                    }
                    stop_targets.push(stop::new_target(turn, None, now));
                }
            }
        }
        let requested_any = stop_targets.iter().any(|t| t.evidence.interrupt_requested_at.is_some());
        if !requested_any {
            if let Some(e) = first_err.clone() {
                return Err(e.into());
            }
        }
        let ack = ack.unwrap_or_else(|| InterruptAck::Unknown { message: first_err.map(|e| e.to_string()).unwrap_or_default() });
        let record = StopRecord { id: self.local_id("stop"), chat, started_at: now, targets: stop_targets };
        let record = self.mutate(|d| {
            let ev = d.add_stop_record(record.clone(), now_ms());
            (d.stops.iter().find(|r| r.id == record.id).cloned().unwrap_or(record), ev)
        });
        if requested_any {
            let host = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(stop::STOP_CONFIRM_DEADLINE_MS as u64 + 100)).await;
                host.mutate(|d| ((), d.refresh_stops(now_ms())));
            });
        }
        Ok(InterruptChatResult { ack, record })
    }

    // ── 管理・設定 ──

    pub async fn manage_chat(&self, args: ManageChatArgs, confirmed: &UserConfirmed) -> Result<ManageOutcome, IpcError> {
        if args.op == ManageOp::Delete {
            if let Some(rec) = self.read(|d| d.open_stop(&args.chat).map(|r| r.id.clone())) {
                return Err(blocked(BlockedReason::StopUnconfirmed { record: rec }, "停止を確認できるまで削除しません"));
            }
        }
        let out = self.backend.manage_chat(args.chat.clone(), args.op.clone(), confirmed).await?;
        if out == ManageOutcome::Done {
            self.mutate(|d| match &args.op {
                ManageOp::Delete => ((), d.remove_chat(&args.chat)),
                op => {
                    if let Some(c) = d.chats.iter_mut().find(|c| c.key == args.chat) {
                        match op {
                            ManageOp::Rename { name } => c.name = Known::direct(name.clone()),
                            ManageOp::Archive => c.archived = Known::direct(true),
                            ManageOp::Unarchive => c.archived = Known::direct(false),
                            ManageOp::Delete => {}
                        }
                        let c = c.clone();
                        return ((), vec![HostEvent::ChatUpdated { chat: c }]);
                    }
                    ((), vec![])
                }
            });
        }
        Ok(out)
    }

    /// ピンはアプリ側の保持（`chat.json` へ保存し、再起動後も戻す）。
    pub fn set_pinned(self: &Arc<Self>, args: SetPinnedArgs) -> Result<Chat, IpcError> {
        self.precheck_space()?;
        let res = self.mutate(|d| {
            if args.pinned {
                d.pinned.insert(args.chat.clone());
            } else {
                d.pinned.remove(&args.chat);
            }
            match d.chats.iter_mut().find(|c| c.key == args.chat) {
                Some(c) => {
                    c.pinned = args.pinned;
                    let c = c.clone();
                    (Ok(c.clone()), vec![HostEvent::ChatUpdated { chat: c }])
                }
                None => (Err(err(IpcErrorCode::NotFound, "チャットが見つかりません")), vec![]),
            }
        });
        if res.is_ok() {
            self.update_local(&args.chat, true, Duration::ZERO, |f| f.pinned = args.pinned);
        }
        res
    }

    pub async fn list_models(&self, args: ListModelsArgs) -> Result<Vec<ModelInfo>, IpcError> {
        Ok(self.backend.list_models(ModelQuery { include_hidden: args.include_hidden }).await?)
    }

    /// チャット単位の選択を保持する。送信時に適用し、受理応答までは `accepted` を更新しない（適用済みと表示しない）。
    pub fn set_chat_model(self: &Arc<Self>, args: SetChatModelArgs) -> Result<ChatModelSettings, IpcError> {
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.precheck_space()?;
        let settings = self.mutate(|d| {
            let s = d.model_settings.entry(args.chat.clone()).or_insert(ChatModelSettings {
                selected: None,
                accepted: Known::NotFetched,
                effective: Known::NotFetched,
                applies: ApplyTiming::NextTurn,
            });
            s.selected = Some(args.choice.clone());
            s.applies = ApplyTiming::NextTurn;
            let s = s.clone();
            (s.clone(), vec![HostEvent::ModelSettingsUpdated { chat: args.chat.clone(), settings: s }])
        });
        // 選択値だけを保存する（受理値・実効値は保存しない）。
        self.update_local(&args.chat, true, Duration::ZERO, |f| f.model = Some(args.choice.clone()));
        Ok(settings)
    }

    pub fn set_monitor_scope(&self, args: SetMonitorScopeArgs) {
        self.mutate(|d| {
            d.monitor_scope = args.scope;
            ((), vec![])
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_builds_without_tauri_and_returns_an_empty_snapshot() {
        let host = Arc::new(Host::new(std::env::temp_dir()));
        host.start_event_pump();
        let snap = host.snapshot();
        assert!(snap.chats.is_empty());
        assert!(snap.agents.is_empty());
    }

    #[test]
    fn round_robin_window_covers_everything_and_respects_the_limit() {
        let items: Vec<u32> = (0..7).collect();
        let mut off = 0;
        let mut seen = Vec::new();
        for _ in 0..3 {
            let w = take_window(&items, &mut off, 3);
            assert!(w.len() <= 3);
            seen.extend(w);
        }
        // 3周（9件分）で0..6が全部1回以上出て、順番に折り返す。
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5, 6, 0, 1]);
        let mut off = 5;
        assert_eq!(take_window(&items, &mut off, 100), vec![5, 6, 0, 1, 2, 3, 4], "small sets are read once per cycle");
        assert_eq!(take_window::<u32>(&[], &mut off, 3), Vec::<u32>::new());
    }

    #[test]
    fn irrelevant_and_waiting_threads_are_not_read_again() {
        let k = |s: &str| AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) };
        let mut memo = ScanMemo::default();
        memo.irrelevant.insert(k("other-root"));
        let loaded = vec![k("known"), k("other-root"), k("new")];
        let got = pick_unread(&loaded, &|a| a == &k("known"), &memo);
        assert_eq!(got, vec![k("new")]);
    }

    #[test]
    fn chat_name_is_first_nonempty_line_truncated() {
        assert_eq!(derive_chat_name("\n  hello world \nsecond").as_deref(), Some("hello world"));
        assert_eq!(derive_chat_name("   \n"), None);
        let long = "あ".repeat(50);
        let n = derive_chat_name(&long).unwrap();
        assert_eq!(n.chars().count(), NAME_MAX_CHARS + 1);
        assert!(n.ends_with('…'));
    }

    #[test]
    fn unresolved_send_keeps_blocking_while_reconciling_until_resolved() {
        let chat = ChatKey { backend: BackendKind::Codex, id: ExternalId("c".into()) };
        let other = ChatKey { backend: BackendKind::Codex, id: ExternalId("o".into()) };
        let id = LocalId("a1".into());
        let mut u = UnresolvedSends::default();
        u.add(id.clone(), chat.clone());
        // 照合のために別の入れ物から取り出されても、ここには残る（送信・再送を止め続ける）。
        assert_eq!(u.blocking(&chat), Some(id.clone()));
        assert!(u.contains(&id));
        assert_eq!(u.blocking(&other), None);
        u.resolve(&id);
        assert_eq!(u.blocking(&chat), None);
        assert!(!u.contains(&id));
    }

    #[test]
    fn backend_errors_map_without_turning_unknown_into_failure() {
        let e: IpcError = BackendError::OutcomeUnknown { message: "t".into() }.into();
        assert_eq!(e.code, IpcErrorCode::OutcomeUnknown);
        let e: IpcError = BackendError::Unsupported { capability: "x".into() }.into();
        assert!(matches!(e.blocked, Some(BlockedReason::CapabilityUnsupported { .. })));
    }
}
