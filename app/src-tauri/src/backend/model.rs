//! バックエンド共通の正規化データモデル（要件§4・§5）。
//!
//! 規則:
//! - Codex固有の型（Thread / ThreadStatus / ThreadItem 等）をここに持ち込まない。変換はアダプター（T3）の責務。
//! - 識別子は必ず「バックエンド種別＋内部ID」の組で持つ（§2.3）。生の文字列IDを単独で比較しない。
//! - エージェント状態（§4.1）と鮮度・収集状態（§4.2）は別の型・別のフィールド。片方からもう片方を導かない。
//! - 不明値は [`Known`] で「未取得／非対応／欠損」と「値あり（直接取得／機械的導出／推定）」を区別する。
//!   0・空文字・`Done` で代用しない（§5）。
//!
//! serde: 構造体は camelCase、データを持つ enum は `kind` タグ（内部タグ）、値なし enum は camelCase 文字列。
//! TS側 `app/src/ipc/types.ts` と同じ形にする。

use serde::{Deserialize, Serialize};

// ───────────────────────────── 基本型 ─────────────────────────────

/// Unix時刻（ミリ秒）。タイムゾーン変換は表示側で行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixMillis(pub i64);

/// AIバックエンドの種別。初期版はCodexのみ（§2.3）。追加時にvariantを足す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum BackendKind {
    Codex,
}

/// バックエンド内部のID（Codexならthread ID・turn ID等）。中身はアダプターだけが解釈する不透明値。
/// 数値IDを持つバックエンドは、アダプターが可逆な文字列表現に符号化する。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalId(pub String);

/// 監視元（起動単位）のID。App Serverプロセスを起動するたびに新規発行する（§5「監視元」）。
/// JSON-RPCのサーバー要求IDは接続単位でしか一意でないため、要求の識別に必ず含める。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub String);

/// AgentDock自身が発行するID（キュー項目・添付・停止記録・送信試行など）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocalId(pub String);

/// チャット（ユーザーが選ぶ会話のルート）。Codexではルートthread。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatKey {
    pub backend: BackendKind,
    pub id: ExternalId,
}

/// エージェント（ルート自身も含む）。Codexではthread。ルートのAgentKeyとChatKeyは同じidを持ち得るが型で区別する。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKey {
    pub backend: BackendKind,
    pub id: ExternalId,
}

/// turn。所属エージェントとの組で識別する。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnKey {
    pub agent: AgentKey,
    pub turn_id: ExternalId,
}

/// 活動item。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemKey {
    pub agent: AgentKey,
    /// item が属するturn。未取得なら None（推定で埋めない）。
    pub turn_id: Option<ExternalId>,
    pub item_id: ExternalId,
}

/// バックエンドからの要求（承認・質問）。要求IDは接続単位でのみ一意なので監視元を含める。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestKey {
    pub backend: BackendKind,
    pub source: SourceId,
    pub request_id: ExternalId,
}

// ───────────────────────────── 不明値・根拠区分（§5） ─────────────────────────────

/// 値の根拠区分（§5「直接取得／機械的導出／内容からの推定」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Basis {
    /// バックエンドが明示した値をそのまま使った。
    Direct,
    /// 明示値から機械的に導出した（例: 親IDの連鎖からルートID）。
    Derived,
    /// 本文などの内容から推定した。表示で推定と分かるようにする。
    Inferred,
}

/// 取得できるか分からない値。`Option` の代わりに使い、欠け方の理由を保持する（§5「不明値」）。
/// UI表示では NotFetched を「未確認」として扱う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Known<T> {
    Value { value: T, basis: Basis },
    /// まだ取得していない／取得を試みていない。
    NotFetched,
    /// 対象バックエンド・対象版では取得できない（能力宣言に基づく）。
    Unsupported,
    /// 取得したが値が欠けていた（schema上はあるはずの項目がnull等）。
    Missing,
}

impl<T> Known<T> {
    pub fn direct(value: T) -> Self {
        Known::Value { value, basis: Basis::Direct }
    }
    pub fn value(&self) -> Option<&T> {
        match self {
            Known::Value { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// 状態や活動の根拠（どのイベント／取得結果から得たか）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// 根拠の種類（バックエンド非依存の短いラベル。例: "statusNotification", "historyRead", "turnEnded"）。
    pub source: EvidenceSource,
    /// 元のイベント名など、診断用の原文ラベル（例: Codexの "thread/status/changed"）。UIでは補助表示のみ。
    pub raw_label: Option<String>,
    /// 元イベントに含まれた時刻（あれば）。
    pub source_time: Option<UnixMillis>,
    /// ホストが受信・観測した時刻。
    pub observed_at: UnixMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceSource {
    /// live購読中の通知。
    LiveEvent,
    /// 保存履歴の読み取り（resumeしない）。
    HistoryRead,
    /// ロード済み一覧など状態照会。
    StatusQuery,
    /// 要求に対する応答。
    Response,
    /// ホスト内の照合処理で導出。
    HostReconcile,
}

// ───────────────────────────── 状態（§4.1） ─────────────────────────────

/// 正規化したエージェント状態（§4.1）。鮮度とは別軸（[`Freshness`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentState {
    Initializing,
    Running,
    Waiting,
    Idle,
    /// 当該turn／委任の完了。永久終了ではない。
    Done,
    /// 当該turn／agentの失敗の明示。単一ツールの失敗で設定しない。
    Failed,
    Interrupted,
    Closed,
    /// 裏付けなし。notLoaded・notFound・通信断・timeoutはここ（doneにしない）。
    Unknown,
}

/// 状態の変換元scope（§4.1「原状態と変換元のscope」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StateScope {
    Agent,
    Turn,
    Tool,
}

/// 待機理由（§4.1 waiting）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WaitReason {
    Approval { request: Option<RequestKey> },
    UserInput { request: Option<RequestKey> },
    Child { child: Option<AgentKey> },
    /// バックエンドが示した、正規化できない理由（原文）。
    Other { raw: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitInfo {
    pub reason: WaitReason,
    pub started: Evidence,
}

/// バックエンドの原状態（診断・根拠表示用）。UIの判定ロジックはこれを解釈しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawState {
    /// 原文の状態ラベル（例: "active[waitingOnApproval]", "notLoaded", "completed"）。
    pub label: String,
}

/// エージェント状態の1観測（§5「状態」）。鮮度は含めない（[`AgentView`] で並置する）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub state: AgentState,
    pub raw: RawState,
    pub scope: StateScope,
    /// この状態が対応するturn。古いturnの後着完了で新しいturnを上書きしないために使う。
    pub turn: Option<ExternalId>,
    pub wait: Option<WaitInfo>,
    pub evidence: Evidence,
}

/// turnの終端（§3.4 終端照合）。終端の明示がない限り作らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnEnd {
    Completed,
    Failed,
    Interrupted,
}

// ───────────────────────────── 鮮度・収集状態（§4.2） ─────────────────────────────

/// 鮮度・収集状態。エージェント状態とは別軸。Live以外の間はキュー自動送信を保留する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Freshness {
    Live,
    /// 履歴取得済み・live未復旧。
    HistoryOnly,
    /// 要照合（切断・wake・取得失敗の後）。
    NeedsReconcile,
    Disconnected,
    Unsupported,
}

impl Freshness {
    /// キューの自動送信を許可してよいか（§4.2）。
    pub fn allows_auto_send(self) -> bool {
        matches!(self, Freshness::Live)
    }
}

/// ホストとバックエンドプロセスの接続状態（監視元単位）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConnectionState {
    NotStarted,
    Starting,
    Connected,
    /// 起動失敗（未導入・パス不正・版違い等）。agentの作業失敗とは別（M15）。
    LaunchFailed { reason: LaunchFailure, message: String },
    /// 接続後に切れた。状態は全エージェントで要照合になる（doneにしない）。
    Disconnected { message: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchFailure {
    NotFound,
    InvalidPath,
    VersionCheckFailed,
    SpawnFailed,
    HandshakeFailed,
}

/// 版確認の結果（§3.1）。対象版以外は警告だが接続は止めない設計。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum VersionCheck {
    Match { version: String },
    Mismatch { expected: String, actual: String },
    Unknown { message: String },
}

// ───────────────────────────── 能力宣言（§2.3） ─────────────────────────────

/// 能力の対応区分。Unknownは非対応と断定しない（正本§0.2「未確認」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    Supported,
    /// experimental API前提で利用可能（接続時の有効化が必要）。
    Experimental,
    Unsupported,
    Unknown,
}

/// バックエンドの能力一覧。UIはUnsupportedの操作を隠すか「このAIでは非対応」と表示する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub descendant_monitoring: Support,
    /// 祖先・親での一覧絞り込み（Codexではexperimental）。
    pub descendant_search: Support,
    pub history_read_without_resume: Support,
    pub resume: Support,
    pub list_loaded: Support,
    pub approval_kinds: Vec<RequestKind>,
    /// 実行中turnへの追加指示（§3.7、steer相当）。
    pub steer: Support,
    pub interrupt: Support,
    pub model_selection: Support,
    pub effort_selection: Support,
    pub rename: Support,
    pub archive: Support,
    pub delete: Support,
    /// CLI・他クライアント作成の履歴を一覧・再開できるか（M36）。
    pub external_history: Support,
    pub attachment_kinds: Vec<AttachmentKind>,
    /// 管理実行（バックグラウンド端末等）の一覧・終了。
    pub managed_exec_control: Support,
}

// ───────────────────────────── 論理データ（§5） ─────────────────────────────

/// 監視元（§5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub source: SourceId,
    pub backend: BackendKind,
    pub pid: Known<u32>,
    pub started_at: UnixMillis,
    pub version: VersionCheck,
    pub capabilities: Capabilities,
    pub connection: ConnectionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatKind {
    /// 作業フォルダ未選択（アプリ管理のチャット用作業領域を内部cwdにする、M21）。
    General,
    Development,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatOrigin {
    AppManaged,
    /// CLI・VS Code等で作成された保存履歴（M36）。元ファイルは参照扱い。
    External,
    Unknown,
}

/// チャット（§5）。本文の正本はバックエンドの保存履歴。ここはメタデータと補足情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub key: ChatKey,
    pub kind: ChatKind,
    pub cwd: Known<String>,
    pub name: Known<String>,
    /// 最初の依頼の先頭（バックエンドが示す一覧用の要約）。名前が無いときの仮表示に使う。確認済みの名前ではない。
    pub preview: Known<String>,
    /// ピンはアプリ側で保持する（バックエンドに対応項目なし）。
    pub pinned: bool,
    pub archived: Known<bool>,
    pub origin: ChatOrigin,
    pub draft: Option<String>,
    pub created_at: Known<UnixMillis>,
    /// ユーザーの利用で更新する最近利用時刻（背景活動では更新しない、§3.6）。
    pub last_used_at: Option<UnixMillis>,
}

/// 直接親の関係（M03）。深さから推定しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ParentLink {
    /// ルート（チャット本体）。
    Root,
    /// 明示された直接親ID。
    Explicit { parent: AgentKey },
    /// 子であることは分かるが親が不明。
    Unknown,
}

/// エージェント（§5）。ルート自身もエージェントとして持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub key: AgentKey,
    pub chat: ChatKey,
    pub parent: ParentLink,
    /// fork元（spawnの親子とは区別する）。
    pub forked_from: Known<Option<AgentKey>>,
    pub display_name: Known<String>,
    /// role。未知のroleも原文のまま保持（固定スロットにしない、M02）。
    pub role: Known<String>,
    /// 具体的な担当（M05）。
    pub assignment: Known<String>,
    pub latest_turn: Option<ExternalId>,
}

/// UIに渡すエージェントの表示単位。状態と鮮度を並置する（変換しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub agent: Agent,
    pub status: AgentStatus,
    pub freshness: Freshness,
    pub current_activity: Option<Activity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ActivityKind {
    UserMessage,
    AgentMessage,
    Reasoning,
    Plan,
    Command,
    FileChange,
    ToolCall,
    WebSearch,
    /// 子エージェントの生成・送受信・待機（spawn/send/wait）。
    SubAgent,
    /// 未知・未正規化のitem。原文の種別名を保持して表示する（M12）。
    Other { raw: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityPhase {
    Started,
    InProgress,
    Completed,
    /// 終端が確認できない（切断等）。完了扱いしない。
    Unconfirmed,
}

/// 活動（§5）。本文全文ではなく短い表示本文。全文は履歴取得で別に扱う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub key: ItemKey,
    pub kind: ActivityKind,
    pub phase: ActivityPhase,
    pub summary: Known<String>,
    pub evidence: Evidence,
}

/// 会話本文の1要素（履歴表示用）。Markdown等は文字列データとしてのみ扱う（§2.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    pub key: ItemKey,
    pub kind: ActivityKind,
    pub text: Known<String>,
    pub phase: ActivityPhase,
}

/// 1turn分の履歴。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRecord {
    pub key: TurnKey,
    /// None = 終端未確認（進行中・切断で不明）。
    pub end: Option<TurnEnd>,
    pub started_at: Known<UnixMillis>,
    pub completed_at: Known<UnixMillis>,
    pub entries: Vec<TranscriptEntry>,
    /// 部分取得か（items未ロード等）。
    pub complete: bool,
}

// ───────────────────────────── 要求（承認・質問、§3.3） ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RequestKind {
    CommandApproval,
    FileChangeApproval,
    PermissionsApproval,
    UserInput,
    /// 外部ツール（MCP等）からの入力要求。
    ToolElicitation,
    /// 未知の要求種別。自動回答せず原文で表示する。
    Other { raw: String },
}

/// 選択肢の効力範囲（M30）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionScope {
    Once,
    Session,
    /// 恒久的な規則追加（実行ポリシー変更等）。
    Persistent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionEffect {
    Allow,
    Deny,
    /// 拒否してturn自体も止める等。
    Cancel,
    Other,
}

/// バックエンドが提供する選択肢（固定一覧を決め打ちしない）。`id` はアダプターだけが解釈する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    pub effect: DecisionEffect,
    pub scope: DecisionScope,
    pub description: Option<String>,
}

/// 質問1件（UserInput／Elicitation）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub header: Option<String>,
    pub text: String,
    /// 選択肢がない場合は自由入力。
    pub options: Vec<DecisionOption>,
    pub allows_free_text: bool,
    /// 秘密入力（表示・保存しない）。
    pub secret: bool,
}

/// 要求の表示内容（取得できる範囲、§3.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDetail {
    pub summary: String,
    pub reason: Known<String>,
    pub command: Known<String>,
    pub cwd: Known<String>,
    pub files: Known<Vec<String>>,
    /// 追加権限の説明（原文要約）。
    pub extra_permissions: Known<String>,
    pub url: Known<String>,
    pub questions: Vec<Question>,
}

/// 要求の状態（§5）。AnswerUnconfirmedは「送ったが受理が確認できない」。再送しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RequestState {
    Pending,
    Answered { option_id: Option<String>, at: UnixMillis },
    AnswerUnconfirmed { at: UnixMillis },
    /// バックエンド側で解決済み通知（他クライアント回答・turn終了など）。
    ResolvedElsewhere { at: UnixMillis },
    Cancelled { at: UnixMillis },
    Expired { at: UnixMillis },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub key: RequestKey,
    pub chat: ChatKey,
    pub agent: AgentKey,
    pub turn: Option<ExternalId>,
    pub item: Option<ExternalId>,
    pub kind: RequestKind,
    pub detail: RequestDetail,
    pub options: Vec<DecisionOption>,
    pub state: RequestState,
    pub received_at: UnixMillis,
}

/// 要求への回答（ユーザー操作からのみ作る。自動回答しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RequestAnswer {
    Decision { option_id: String },
    Answers { answers: Vec<QuestionAnswer> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswer {
    pub question_id: String,
    pub option_id: Option<String>,
    pub text: Option<String>,
}

// ───────────────────────────── 送信・キュー（§3.2・§3.7・§5） ─────────────────────────────

/// 送信の結果状態。AcceptanceUnknownは「受理不明」で、照合以外の手段で解消しない（無条件再送しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SendState {
    Sending,
    Accepted { turn: TurnKey },
    /// 明確に受理されなかった（エラー応答）。ユーザーが再送を選べる。
    Rejected { message: String },
    /// 受理不明（timeout・切断）。照合で Accepted / NotFound を確定するまで再送しない。
    AcceptanceUnknown { since: UnixMillis },
    /// 照合の結果、受理された痕跡がない。再送はユーザー確認後のみ。
    NotFoundAfterReconcile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAttempt {
    pub attempt_id: LocalId,
    /// 照合用にバックエンドへ渡すクライアント側メッセージID。
    pub client_message_id: String,
    pub at: UnixMillis,
    pub state: SendState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueHoldReason {
    NotLive { freshness: Freshness },
    DescendantsNotFinished,
    StopUnconfirmed,
    AcceptanceUnknown,
    UserPaused,
    Other { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueItemState {
    Waiting,
    Held { reason: QueueHoldReason },
    Sending,
    AcceptanceUnknown,
    Sent { turn: TurnKey },
    Cancelled,
}

/// キュー項目（§5）。§3.7の動作はT1範囲外。データ形だけ定める。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: LocalId,
    pub chat: ChatKey,
    pub text: String,
    pub attachments: Vec<LocalId>,
    pub order: u64,
    pub state: QueueItemState,
    pub attempts: Vec<SendAttempt>,
    pub settings: Option<ChatModelSettings>,
}

// ───────────────────────────── 添付（§5） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentKind {
    Image,
    File,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: LocalId,
    pub kind: AttachmentKind,
    /// 元パス（参照のみ。削除しない）。
    pub original_path: String,
    pub copy_path: Option<String>,
    pub attached_at: UnixMillis,
    pub exists: Known<bool>,
    pub owner_chat: ChatKey,
}

// ───────────────────────────── モデル・effort（§3.9） ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffortOption {
    pub id: String,
    pub description: Option<String>,
}

/// バックエンドから取得したモデル（固定一覧を持たない）。一覧にあることは利用資格の証明ではない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub efforts: Vec<EffortOption>,
    pub default_effort: Option<String>,
    pub is_default: bool,
    pub hidden: bool,
    pub input_kinds: Vec<AttachmentKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    pub model: String,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplyTiming {
    Now,
    NextTurn,
    AfterResume,
    Unknown,
}

/// チャット単位のモデル設定。選択値・受理値・実効値を区別する（§3.9）。
/// サブエージェントの実効値とは扱わない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatModelSettings {
    pub selected: Option<ModelChoice>,
    /// バックエンドが受理した設定。受理を確認できなければ NotFetched のまま（適用済み表示しない）。
    pub accepted: Known<ModelChoice>,
    /// 観測できる実効値（reroute通知等）。
    pub effective: Known<ModelChoice>,
    pub applies: ApplyTiming,
}

// ───────────────────────────── 停止・保存（§4.3） ─────────────────────────────

/// 停止の各証拠（§4.3）。別々に記録し、1つの証拠から他を推定しない。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopEvidence {
    pub interrupt_requested_at: Option<UnixMillis>,
    pub turn_end_confirmed: Option<TurnEndConfirmation>,
    pub managed_exec_ended_at: Option<UnixMillis>,
    pub os_gone_confirmed_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndConfirmation {
    pub end: TurnEnd,
    pub at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StopTargetRef {
    Turn { turn: TurnKey },
    /// 管理実行（バックエンドの実行ID）。
    ManagedExec { agent: AgentKey, exec_id: ExternalId },
    /// OSプロセス。PID再利用に備え生成時刻・実行ファイルで照合する。
    OsProcess { pid: u32, created_at: Known<UnixMillis>, executable: Known<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ownership {
    Confirmed,
    /// 所有不明。削除保留の条件（§4.3）。強制終了の対象にしない。
    Unknown,
}

/// 停止の要約状態（表示用）。StopEvidenceから機械的に導出する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopSummary {
    NotRequested,
    InterruptRequested,
    TurnEndConfirmed,
    ManagedExecEndConfirmed,
    OsGoneConfirmed,
    /// 停止未確認（10秒経過時の案内対象、§3.4）。停止・失敗の判定ではない。
    Unconfirmed,
    OwnershipUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopTarget {
    pub target: StopTargetRef,
    pub ownership: Ownership,
    pub evidence: StopEvidence,
    pub summary: StopSummary,
}

/// 停止記録（§5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopRecord {
    pub id: LocalId,
    pub chat: ChatKey,
    pub started_at: UnixMillis,
    pub targets: Vec<StopTarget>,
}

impl StopRecord {
    /// 削除保留の条件（停止未確認または所有不明が残る、M46）。
    pub fn blocks_deletion(&self) -> bool {
        self.targets.iter().any(|t| {
            matches!(t.summary, StopSummary::Unconfirmed | StopSummary::OwnershipUnknown | StopSummary::InterruptRequested)
                || t.ownership == Ownership::Unknown
        })
    }
}

/// 保存の状態（§4.3）。保存失敗は保存済みと未保存を分けて示す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SaveState {
    Saved { at: UnixMillis },
    Unsaved,
    SaveFailed { message: String, saved_part: Option<String>, unsaved_part: Option<String> },
}
