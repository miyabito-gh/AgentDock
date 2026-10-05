//! AIバックエンドtrait（要件§2.3・§3.1〜3.6・§3.9）。
//!
//! Codex App Server（stdio JSON-RPC）はこのtraitの実装の一つ（T2: 通信、T3: 変換）。ホスト側の
//! 監視・キュー・停止照合・保存・通知は、このtraitと [`super::model`] だけに依存させる。
//!
//! 型で守る約束:
//! - **受理不明は失敗ではない**: timeout・切断は [`SendOutcome::AcceptanceUnknown`] / [`BackendError::OutcomeUnknown`]。
//!   状態の `Done` / `Failed` に変換しない。
//! - **受理不明の送信を再送しない**: [`SendRequest`] と [`UnconfirmedSend`] は `Clone` できず、
//!   `UnconfirmedSend` から `SendRequest` へ戻す手段は [`reconcile_send`](AiBackend::reconcile_send) で
//!   「受理なし」を確定し、さらに [`UserConfirmed`] を添えた [`SendRequest::retry`] だけ。
//! - **中断は要求の受付のみ**: [`interrupt`](AiBackend::interrupt) は停止を返さない。停止は終端イベントで照合する。
//! - **監視のために作業を変えない**（M11）: `read` / `list_*` / `scan_descendants` は resume・送信・承認をしない。
//!   `resume` はユーザーの明示操作からだけ呼ぶ。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::model::*;

// ───────────────────────────── エラー ─────────────────────────────

/// バックエンド操作の失敗。`OutcomeUnknown` は「結果が分からない」であり失敗確定ではない。
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BackendError {
    #[error("not connected")]
    NotConnected,
    /// 能力宣言で非対応（または対象版で存在しない）。成功扱いしない。
    #[error("unsupported: {capability}")]
    Unsupported { capability: String },
    /// バックエンドが明示的に拒否した（エラー応答）。
    #[error("rejected: {message}")]
    Rejected { code: Option<i64>, message: String },
    /// 要求は送ったが応答がない（timeout・切断）。相手側で実行された可能性がある。
    #[error("outcome unknown: {message}")]
    OutcomeUnknown { message: String },
    /// 応答の形が想定外（版違い等）。プロセスは止めず警告に回す（M12）。
    #[error("protocol: {message}")]
    Protocol { message: String },
    #[error("io: {message}")]
    Io { message: String },
}

pub type BackendResult<T> = Result<T, BackendError>;

// ───────────────────────────── イベント ─────────────────────────────

/// バックエンドからホストへ流れる正規化イベント。到着順は保証されない前提で、ホストは
/// `TurnKey` と [`AgentStatus::turn`] を使って古いturnの後着を捨てる（§4.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BackendEvent {
    /// 接続状態の変化。`Disconnected` を受けたら全エージェントの鮮度を `Disconnected` にする（状態は変えない）。
    Connection { state: ConnectionState },
    /// 新規・既知のエージェントの発見（ルート含む）。子孫の動的生成もここ（M02）。
    AgentDiscovered { agent: Agent },
    /// エージェント状態の明示的な変化。
    AgentStatus { agent: AgentKey, status: AgentStatus },
    TurnStarted { turn: TurnKey, evidence: Evidence },
    /// turn終端の明示。中断照合（§3.4）の証拠になる。
    TurnEnded { turn: TurnKey, end: TurnEnd, error: Option<String>, evidence: Evidence },
    /// 子エージェントの担当（親のspawn依頼文の先頭）。対応が確定できたものだけ（推測で割り当てない）。
    AgentAssignment { agent: AgentKey, assignment: String },
    /// 活動itemの開始・更新・完了。
    Activity { activity: Activity },
    /// 逐次本文（agentMessage等のdelta）。順序は `seq` に従う。
    ActivityDelta { item: ItemKey, delta: String },
    /// 承認・質問の到着。自動回答しない。
    RequestOpened { request: PendingRequest },
    /// 要求がバックエンド側で解決済み（他経路の回答・取消・turn終了）。
    RequestResolved { request: RequestKey, evidence: Evidence },
    ChatMetaChanged { chat: ChatKey, change: ChatMetaChange },
    /// モデルの実効値が変わった（reroute等、§3.9）。
    ModelRerouted { agent: AgentKey, turn: Option<ExternalId>, effective: ModelChoice },
    /// 未知のイベント・正規化できない内容。破棄せず警告として表示（M12）。
    Unrecognized { raw_label: String, note: Option<String> },
    /// イベント欠落の可能性（キュー溢れ・lag・パース失敗）。対象の鮮度を要照合にする。
    Gap { scope: GapScope, reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChatMetaChange {
    Renamed { name: String },
    Archived,
    Unarchived,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GapScope {
    /// 監視元全体（どのエージェントか特定できない）。
    Source,
    Agent { agent: AgentKey },
}

/// 監視元・受信順を付けたイベント。`seq` は監視元内で単調増加（再接続で新しい `source` になる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub source: SourceId,
    pub seq: u64,
    pub received_at: UnixMillis,
    pub event: BackendEvent,
}

/// イベントの受け口。消費者はホストの監視層ただ一つ。アダプターは溢れたら黙って捨てず `Gap` を送る。
pub type EventReceiver = mpsc::Receiver<EventEnvelope>;

// ───────────────────────────── 接続 ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectConfig {
    /// 実行ファイルのパス。None なら既定（PATH、§11）。
    pub executable: Option<String>,
    /// 対象版。異なれば `VersionCheck::Mismatch` で警告するが接続は続ける（§3.1）。
    pub expected_version: Option<String>,
    /// experimental APIを有効化して接続するか（使用範囲は設計で決める、§11）。
    pub enable_experimental: bool,
    /// アプリ専用のデータ領域（一般チャットの作業領域の親など）。
    pub app_data_dir: String,
}

// ───────────────────────────── 会話 ─────────────────────────────

/// 権限の初期値（§3.3）。無断でフルアクセスへ変更しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum PermissionPreset {
    /// 作業領域内の操作を許可＋必要時に確認（初期値）。
    WorkspaceWriteOnRequest,
    ReadOnly,
    /// ユーザーの明示操作でのみ選ぶ。
    FullAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChatParams {
    pub kind: ChatKind,
    /// 解決済みcwd。一般チャットではホストがアプリ管理の作業領域を割り当てて渡す（M21）。
    pub cwd: String,
    pub model: Option<ModelChoice>,
    pub permission: PermissionPreset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStarted {
    pub chat: Chat,
    pub root: Agent,
    /// 開始時点で受理された設定（確認できなければ NotFetched）。
    pub accepted_model: Known<ModelChoice>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChatsQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub search: Option<String>,
    pub archived: bool,
    /// CLI・VS Code等の外部作成を含める（M36）。
    pub include_external: bool,
    /// 子孫（サブエージェント）を一覧に含める。
    pub include_descendants: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub chat: Chat,
    pub root: Agent,
    pub status: AgentStatus,
    pub preview: Known<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOptions {
    pub include_turns: bool,
}

/// 保存履歴の読み取り結果。readはlive購読を戻さないので、鮮度は呼び出し側で `HistoryOnly` とする。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AgentHistory {
    pub agent: Agent,
    pub chat: Option<Chat>,
    pub status: AgentStatus,
    pub turns: Vec<TurnRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ResumeOutcome {
    /// 同じ会話として再開し、live購読が戻った。
    Resumed { history: AgentHistory },
    /// 外部で実行中のため閲覧のみ（M36）。
    RunningElsewhere,
    /// 再開できない（理由を表示。別threadで代替しない）。
    Unavailable { reason: String },
}

/// 子孫の再発見結果。`complete=false` はページ途中の生成等で取りこぼしがあり得る（再走査が必要）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescendantScan {
    pub root: AgentKey,
    pub found: Vec<Agent>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ManageOp {
    Rename { name: String },
    Archive,
    Unarchive,
    /// 削除。停止未確認・所有不明があればホストが呼ばない（M46）。
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ManageOutcome {
    Done,
    /// 部分成功。完了と偽らない（§3.6）。
    Partial { done: Vec<String>, failed: Vec<String> },
}

// ───────────────────────────── 送信 ─────────────────────────────

/// ユーザーの明示操作があったことを示す証票。IPCコマンド処理からだけ作る（自動処理から作らない）。
#[derive(Debug)]
pub struct UserConfirmed {
    _private: (),
}

impl UserConfirmed {
    /// IPCのユーザー操作ハンドラ内でだけ呼ぶ。
    pub(crate) fn from_user_command() -> Self {
        UserConfirmed { _private: () }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SendMode {
    /// 新しいturnを開始する。
    NewTurn,
    /// 実行中turnへの追加指示（能力 `steer` がSupportedの時のみ）。
    Steer { turn: TurnKey },
}

/// 1回分の送信。`Clone` しない（同じ内容を二重送信しないため）。
#[derive(Debug)]
pub struct SendRequest {
    pub chat: ChatKey,
    pub mode: SendMode,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub model: Option<ModelChoice>,
    /// 照合用ID。ホストが試行ごとに新規発行する。
    pub client_message_id: String,
}

impl SendRequest {
    /// 照合で「受理なし」が確定した送信を、ユーザー確認のうえで作り直す唯一の経路。
    /// 新しい `client_message_id` が必要（同じIDで再送しない）。
    pub fn retry(not_accepted: NotAccepted, _confirmed: UserConfirmed, new_client_message_id: String) -> Self {
        let NotAccepted { request } = not_accepted;
        SendRequest { client_message_id: new_client_message_id, ..request }
    }
}

/// 受理不明の送信。`Clone` できず、`reconcile_send` に渡す以外の使い道を持たない。
#[derive(Debug)]
pub struct UnconfirmedSend {
    request: SendRequest,
    pub since: UnixMillis,
}

impl UnconfirmedSend {
    /// アダプター（T3）が受理不明を返すときに作る。
    pub fn new(request: SendRequest, since: UnixMillis) -> Self {
        UnconfirmedSend { request, since }
    }
    pub fn chat(&self) -> &ChatKey {
        &self.request.chat
    }
    pub fn client_message_id(&self) -> &str {
        &self.request.client_message_id
    }
    /// アダプターが照合で受理なしを確定したときにだけ使う。
    pub fn into_not_accepted(self) -> NotAccepted {
        NotAccepted { request: self.request }
    }
}

/// 受理されなかったことが確定した送信（明示拒否、または照合で痕跡なし）。
#[derive(Debug)]
pub struct NotAccepted {
    request: SendRequest,
}

impl NotAccepted {
    pub fn new(request: SendRequest) -> Self {
        NotAccepted { request }
    }
    pub fn text(&self) -> &str {
        &self.request.text
    }
}

#[derive(Debug)]
pub enum SendOutcome {
    Accepted { turn: TurnKey, accepted_model: Known<ModelChoice> },
    /// 明示的な拒否。`NotAccepted` からユーザー確認付きで作り直せる。
    Rejected { error: BackendError, request: NotAccepted },
    /// 受理不明。照合するまで再送しない。キューは `AcceptanceUnknown` で保留する。
    AcceptanceUnknown(UnconfirmedSend),
}

#[derive(Debug)]
pub enum ReconcileOutcome {
    Accepted { turn: TurnKey },
    /// 照合の結果、受理の痕跡なし。再送はユーザー確認後だけ。
    NotFound(NotAccepted),
    /// まだ判断できない（鮮度不足等）。
    StillUnknown(UnconfirmedSend),
}

// ───────────────────────────── 承認・質問・中断 ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RespondOutcome {
    Delivered,
    /// すでに解決済み・取消・期限切れ。回答は送っていない（再送しない）。
    AlreadyResolved,
    /// 送ったが受理が確認できない。要求状態は `AnswerUnconfirmed`。
    Unknown { message: String },
}

/// 中断要求の受付結果。停止の確認ではない（§3.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum InterruptAck {
    /// 要求を受け付けた。終端は `TurnEnded` で照合する。
    Requested,
    /// 対象turnは既に実行中でない（終端は別途確認）。
    NotRunning,
    /// 応答なし（timeout等）。保存はinterruptedになっている場合がある。
    Unknown { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelQuery {
    pub include_hidden: bool,
}

// ───────────────────────────── trait ─────────────────────────────

/// AIバックエンド（1監視元＝1プロセス）。Codex App Serverの実装はT3。
///
/// すべての操作は接続前なら `BackendError::NotConnected`、能力が `Unsupported` なら
/// `BackendError::Unsupported` を返す（成功を装わない）。
#[async_trait]
pub trait AiBackend: Send + Sync {
    fn kind(&self) -> BackendKind;

    /// 現在の能力宣言（接続前は保守的な値＝Unknown）。
    fn capabilities(&self) -> Capabilities;

    /// プロセス起動・版確認・初期化。起動失敗は `SourceInfo.connection = LaunchFailed` で返し、
    /// エージェントを作らない（M15）。
    async fn connect(&self, config: ConnectConfig) -> BackendResult<SourceInfo>;

    /// 接続を閉じる。実行中の作業の停止を意味しない。
    async fn disconnect(&self) -> BackendResult<()>;

    /// イベント受け口を一度だけ取り出す（2回目以降は None）。
    fn take_events(&self) -> Option<EventReceiver>;

    // ── 会話 ──
    async fn start_chat(&self, params: StartChatParams) -> BackendResult<ChatStarted>;
    async fn list_chats(&self, query: ListChatsQuery) -> BackendResult<Page<ChatSummary>>;
    /// 保存履歴の読み取り。resumeしない（§3.6）。
    async fn read(&self, agent: AgentKey, options: ReadOptions) -> BackendResult<AgentHistory>;
    /// ユーザーの明示操作でのみ呼ぶ再開（M11・M24）。
    async fn resume(&self, chat: ChatKey, confirmed: &UserConfirmed) -> BackendResult<ResumeOutcome>;
    async fn manage_chat(&self, chat: ChatKey, op: ManageOp, confirmed: &UserConfirmed) -> BackendResult<ManageOutcome>;

    // ── 監視 ──
    /// このプロセスでロード済みのエージェント。
    async fn list_loaded(&self) -> BackendResult<Vec<AgentKey>>;
    /// 子孫の再発見（能力 `descendant_search` 依存）。
    async fn scan_descendants(&self, root: AgentKey) -> BackendResult<DescendantScan>;

    // ── 送信 ──
    /// 送信。`Err` を返さず、受理／拒否／受理不明のいずれかに必ず分類する。
    async fn send(&self, request: SendRequest) -> SendOutcome;
    /// 受理不明の照合（履歴のclient_message_id等で確認）。再送はしない。
    async fn reconcile_send(&self, pending: UnconfirmedSend) -> ReconcileOutcome;

    // ── 承認・質問 ──
    async fn respond(&self, request: RequestKey, answer: RequestAnswer, confirmed: &UserConfirmed) -> BackendResult<RespondOutcome>;

    // ── 中断 ──
    async fn interrupt(&self, turn: TurnKey, confirmed: &UserConfirmed) -> BackendResult<InterruptAck>;

    // ── モデル ──
    async fn list_models(&self, query: ModelQuery) -> BackendResult<Vec<ModelInfo>>;
}
