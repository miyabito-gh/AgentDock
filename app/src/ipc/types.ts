// AgentDock IPC契約（手書き）。正本は app/src-tauri/src/backend/{model,backend,ipc}.rs。
// 規則: 構造体はcamelCase、データを持つenumは `kind` タグ、値なしenumは文字列リテラル。
// Rust側を変えたら必ずここも同時に変える。Codex固有の型はここに出さない。

// ───────────── 基本型 ─────────────

/** Unix時刻（ミリ秒） */
export type UnixMillis = number;
export type BackendKind = "codex";
/** バックエンド内部ID（不透明） */
export type ExternalId = string;
export type SourceId = string;
export type LocalId = string;

export interface ChatKey { backend: BackendKind; id: ExternalId }
export interface AgentKey { backend: BackendKind; id: ExternalId }
export interface TurnKey { agent: AgentKey; turnId: ExternalId }
export interface ItemKey { agent: AgentKey; turnId: ExternalId | null; itemId: ExternalId }
export interface RequestKey { backend: BackendKind; source: SourceId; requestId: ExternalId }

// ───────────── 不明値・根拠 ─────────────

export type Basis = "direct" | "derived" | "inferred";
/** 未取得・非対応・欠損を区別する値。notFetchedは「未確認」と表示する。 */
export type Known<T> =
  | { kind: "value"; value: T; basis: Basis }
  | { kind: "notFetched" }
  | { kind: "unsupported" }
  | { kind: "missing" };

export type EvidenceSource = "liveEvent" | "historyRead" | "statusQuery" | "response" | "hostReconcile";
export interface Evidence {
  source: EvidenceSource;
  rawLabel: string | null;
  sourceTime: UnixMillis | null;
  observedAt: UnixMillis;
}

// ───────────── 状態（§4.1）・鮮度（§4.2） ─────────────

export type AgentState =
  | "initializing" | "running" | "waiting" | "idle" | "done"
  | "failed" | "interrupted" | "closed" | "unknown";
export type StateScope = "agent" | "turn" | "tool";
export type WaitReason =
  | { kind: "approval"; request: RequestKey | null }
  | { kind: "userInput"; request: RequestKey | null }
  | { kind: "child"; child: AgentKey | null }
  | { kind: "other"; raw: string };
export interface WaitInfo { reason: WaitReason; started: Evidence }
export interface RawState { label: string }
export interface AgentStatus {
  state: AgentState;
  raw: RawState;
  scope: StateScope;
  turn: ExternalId | null;
  wait: WaitInfo | null;
  evidence: Evidence;
}
export type TurnEnd = "completed" | "failed" | "interrupted";

/** 鮮度。エージェント状態とは別軸。live以外はキュー自動送信を保留。 */
export type Freshness = "live" | "historyOnly" | "needsReconcile" | "disconnected" | "unsupported";

export type LaunchFailure = "notFound" | "invalidPath" | "versionCheckFailed" | "spawnFailed" | "handshakeFailed";
export type ConnectionState =
  | { kind: "notStarted" }
  | { kind: "starting" }
  | { kind: "connected" }
  | { kind: "launchFailed"; reason: LaunchFailure; message: string }
  | { kind: "disconnected"; message: string | null };
export type VersionCheck =
  | { kind: "match"; version: string }
  | { kind: "mismatch"; expected: string; actual: string }
  | { kind: "unknown"; message: string };

// ───────────── 能力 ─────────────

export type Support = "supported" | "experimental" | "unsupported" | "unknown";
export interface Capabilities {
  descendantMonitoring: Support;
  descendantSearch: Support;
  historyReadWithoutResume: Support;
  resume: Support;
  listLoaded: Support;
  approvalKinds: RequestKind[];
  steer: Support;
  interrupt: Support;
  modelSelection: Support;
  effortSelection: Support;
  rename: Support;
  archive: Support;
  delete: Support;
  externalHistory: Support;
  attachmentKinds: AttachmentKind[];
  managedExecControl: Support;
}

// ───────────── 論理データ（§5） ─────────────

export interface SourceInfo {
  source: SourceId;
  backend: BackendKind;
  pid: Known<number>;
  startedAt: UnixMillis;
  version: VersionCheck;
  capabilities: Capabilities;
  connection: ConnectionState;
}

export type ChatKind = "general" | "development";
export type ChatOrigin = "appManaged" | "external" | "unknown";
export interface Chat {
  key: ChatKey;
  kind: ChatKind;
  cwd: Known<string>;
  name: Known<string>;
  /** 最初の依頼の先頭（名前が無いときの仮表示用。確認済みの名前ではない） */
  preview: Known<string>;
  pinned: boolean;
  archived: Known<boolean>;
  origin: ChatOrigin;
  draft: string | null;
  createdAt: Known<UnixMillis>;
  lastUsedAt: UnixMillis | null;
}

export type ParentLink =
  | { kind: "root" }
  | { kind: "explicit"; parent: AgentKey }
  | { kind: "unknown" };
export interface Agent {
  key: AgentKey;
  chat: ChatKey;
  parent: ParentLink;
  forkedFrom: Known<AgentKey | null>;
  displayName: Known<string>;
  role: Known<string>;
  assignment: Known<string>;
  latestTurn: ExternalId | null;
}
export interface AgentView {
  agent: Agent;
  status: AgentStatus;
  freshness: Freshness;
  currentActivity: Activity | null;
}

export type ActivityKind =
  | { kind: "userMessage" } | { kind: "agentMessage" } | { kind: "reasoning" } | { kind: "plan" }
  | { kind: "command" } | { kind: "fileChange" } | { kind: "toolCall" } | { kind: "webSearch" }
  | { kind: "subAgent" } | { kind: "other"; raw: string };
export type ActivityPhase = "started" | "inProgress" | "completed" | "unconfirmed";
export interface Activity {
  key: ItemKey;
  kind: ActivityKind;
  phase: ActivityPhase;
  summary: Known<string>;
  evidence: Evidence;
}
export interface TranscriptEntry {
  key: ItemKey;
  kind: ActivityKind;
  text: Known<string>;
  phase: ActivityPhase;
}
export interface TurnRecord {
  key: TurnKey;
  /** null = 終端未確認 */
  end: TurnEnd | null;
  startedAt: Known<UnixMillis>;
  completedAt: Known<UnixMillis>;
  entries: TranscriptEntry[];
  complete: boolean;
}

// ───────────── 要求（§3.3） ─────────────

export type RequestKind =
  | { kind: "commandApproval" } | { kind: "fileChangeApproval" } | { kind: "permissionsApproval" }
  | { kind: "userInput" } | { kind: "toolElicitation" } | { kind: "other"; raw: string };
export type DecisionScope = "once" | "session" | "persistent" | "unknown";
export type DecisionEffect = "allow" | "deny" | "cancel" | "other";
export interface DecisionOption {
  id: string;
  label: string;
  effect: DecisionEffect;
  scope: DecisionScope;
  description: string | null;
}
export interface Question {
  id: string;
  header: string | null;
  text: string;
  options: DecisionOption[];
  allowsFreeText: boolean;
  secret: boolean;
}
export interface RequestDetail {
  summary: string;
  reason: Known<string>;
  command: Known<string>;
  cwd: Known<string>;
  files: Known<string[]>;
  extraPermissions: Known<string>;
  url: Known<string>;
  questions: Question[];
}
export type RequestState =
  | { kind: "pending" }
  | { kind: "answered"; optionId: string | null; at: UnixMillis }
  | { kind: "answerUnconfirmed"; at: UnixMillis }
  | { kind: "resolvedElsewhere"; at: UnixMillis }
  | { kind: "cancelled"; at: UnixMillis }
  | { kind: "expired"; at: UnixMillis };
export interface PendingRequest {
  key: RequestKey;
  chat: ChatKey;
  agent: AgentKey;
  turn: ExternalId | null;
  item: ExternalId | null;
  kind: RequestKind;
  detail: RequestDetail;
  options: DecisionOption[];
  state: RequestState;
  receivedAt: UnixMillis;
}
export interface QuestionAnswer { questionId: string; optionId: string | null; text: string | null }
export type RequestAnswer =
  | { kind: "decision"; optionId: string }
  | { kind: "answers"; answers: QuestionAnswer[] };

// ───────────── 送信・キュー ─────────────

/** acceptanceUnknownの間は再送ボタンを出さない。 */
export type SendState =
  | { kind: "sending" }
  | { kind: "accepted"; turn: TurnKey }
  | { kind: "rejected"; message: string }
  | { kind: "acceptanceUnknown"; since: UnixMillis }
  | { kind: "notFoundAfterReconcile" };
export interface SendAttempt {
  attemptId: LocalId;
  clientMessageId: string;
  at: UnixMillis;
  state: SendState;
}
export type QueueHoldReason =
  | { kind: "notLive"; freshness: Freshness }
  | { kind: "descendantsNotFinished" }
  | { kind: "stopUnconfirmed" }
  | { kind: "acceptanceUnknown" }
  | { kind: "userPaused" }
  | { kind: "other"; message: string };
export type QueueItemState =
  | { kind: "waiting" }
  | { kind: "held"; reason: QueueHoldReason }
  | { kind: "sending" }
  | { kind: "acceptanceUnknown" }
  | { kind: "sent"; turn: TurnKey }
  | { kind: "cancelled" };
export interface QueueItem {
  id: LocalId;
  chat: ChatKey;
  text: string;
  attachments: LocalId[];
  order: number;
  state: QueueItemState;
  attempts: SendAttempt[];
  settings: ChatModelSettings | null;
}

// ───────────── 添付 ─────────────

export type AttachmentKind = "image" | "file" | "audio";
export interface Attachment {
  id: LocalId;
  kind: AttachmentKind;
  originalPath: string;
  copyPath: string | null;
  attachedAt: UnixMillis;
  exists: Known<boolean>;
  ownerChat: ChatKey;
}

// ───────────── モデル（§3.9） ─────────────

export interface EffortOption { id: string; description: string | null }
export interface ModelInfo {
  id: string;
  displayName: string;
  description: string | null;
  efforts: EffortOption[];
  defaultEffort: string | null;
  isDefault: boolean;
  hidden: boolean;
  inputKinds: AttachmentKind[];
}
export interface ModelChoice { model: string; effort: string | null }
export type ApplyTiming = "now" | "nextTurn" | "afterResume" | "unknown";
export interface ChatModelSettings {
  selected: ModelChoice | null;
  accepted: Known<ModelChoice>;
  effective: Known<ModelChoice>;
  applies: ApplyTiming;
}

// ───────────── 停止・保存（§4.3） ─────────────

export interface TurnEndConfirmation { end: TurnEnd; at: UnixMillis }
export interface StopEvidence {
  interruptRequestedAt: UnixMillis | null;
  turnEndConfirmed: TurnEndConfirmation | null;
  managedExecEndedAt: UnixMillis | null;
  osGoneConfirmedAt: UnixMillis | null;
}
export type StopTargetRef =
  | { kind: "turn"; turn: TurnKey }
  | { kind: "managedExec"; agent: AgentKey; execId: ExternalId }
  | { kind: "osProcess"; pid: number; createdAt: Known<UnixMillis>; executable: Known<string> };
export type Ownership = "confirmed" | "unknown";
export type StopSummary =
  | "notRequested" | "interruptRequested" | "turnEndConfirmed" | "managedExecEndConfirmed"
  | "osGoneConfirmed" | "unconfirmed" | "ownershipUnknown";
export interface StopTarget {
  target: StopTargetRef;
  ownership: Ownership;
  evidence: StopEvidence;
  summary: StopSummary;
}
export interface StopRecord {
  id: LocalId;
  chat: ChatKey;
  startedAt: UnixMillis;
  targets: StopTarget[];
}
export type SaveState =
  | { kind: "saved"; at: UnixMillis }
  | { kind: "unsaved" }
  | { kind: "saveFailed"; message: string; savedPart: string | null; unsavedPart: string | null };

// ───────────── backend.rs 由来の戻り値型 ─────────────

export type PermissionPreset = "workspaceWriteOnRequest" | "readOnly" | "fullAccess";
export interface Page<T> { items: T[]; nextCursor: string | null }
export interface ChatSummary { chat: Chat; root: Agent; status: AgentStatus; preview: Known<string> }
export interface AgentHistory { agent: Agent; chat: Chat | null; status: AgentStatus; turns: TurnRecord[] }
export type ResumeOutcome =
  | { kind: "resumed"; history: AgentHistory }
  | { kind: "runningElsewhere" }
  | { kind: "unavailable"; reason: string };
export type ManageOp =
  | { kind: "rename"; name: string }
  | { kind: "archive" } | { kind: "unarchive" } | { kind: "delete" };
export type ManageOutcome =
  | { kind: "done" }
  | { kind: "partial"; done: string[]; failed: string[] };
export type RespondOutcome =
  | { kind: "delivered" }
  | { kind: "alreadyResolved" }
  | { kind: "unknown"; message: string };
/** 中断要求の受付結果。停止確認ではない。 */
export type InterruptAck =
  | { kind: "requested" }
  | { kind: "notRunning" }
  | { kind: "unknown"; message: string };

// ───────────── IPC（ipc.rs） ─────────────

export const HOST_EVENT_CHANNEL = "agentdock://host-event";

export type IpcErrorCode =
  | "notConnected" | "unsupported" | "rejected" | "outcomeUnknown" | "protocol" | "io"
  | "invalidArgs" | "notFound" | "blocked";
export type BlockedReason =
  | { kind: "stopUnconfirmed"; record: LocalId }
  | { kind: "acceptanceUnknown"; attempt: LocalId }
  | { kind: "runningElsewhere" }
  | { kind: "capabilityUnsupported"; capability: string }
  | { kind: "requestAlreadyResolved" };
/** コマンド失敗時にinvokeがrejectする値。outcomeUnknownは失敗確定ではない。 */
export interface IpcError { code: IpcErrorCode; message: string; blocked: BlockedReason | null }

export type MonitorScope =
  | { kind: "selectedChat"; chat: ChatKey | null }
  | { kind: "allChats"; showFinished: boolean };

export interface HostSnapshot {
  seq: number;
  sources: SourceInfo[];
  chats: Chat[];
  agents: AgentView[];
  requests: PendingRequest[];
  stops: StopRecord[];
  queue: QueueItem[];
  monitorScope: MonitorScope;
}

export type SendIntent = "newTurn" | "steer";

export interface ConnectBackendArgs { backend: BackendKind; executable: string | null }
export interface ListChatsArgs { cursor: string | null; limit: number | null; search: string | null; includeArchived: boolean }
export interface OpenChatArgs { chat: ChatKey }
export interface StartChatArgs {
  backend: BackendKind;
  cwd: string | null;
  model: ModelChoice | null;
  permission: PermissionPreset | null;
  firstMessage: string | null;
}
export interface StartChatResult { chat: Chat; firstSend: SendAttempt | null }
export interface SendMessageArgs { chat: ChatKey; text: string; attachments: LocalId[]; intent: SendIntent }
export interface RetrySendArgs { chat: ChatKey; attempt: LocalId }
export interface RespondRequestArgs { request: RequestKey; answer: RequestAnswer }
export interface InterruptChatArgs { chat: ChatKey }
export interface InterruptChatResult { ack: InterruptAck; record: StopRecord }
export interface ResumeChatArgs { chat: ChatKey }
export interface ManageChatArgs { chat: ChatKey; op: ManageOp }
export interface SetPinnedArgs { chat: ChatKey; pinned: boolean }
export interface ListModelsArgs { backend: BackendKind; includeHidden: boolean }
export interface SetChatModelArgs { chat: ChatKey; choice: ModelChoice }
export interface SetMonitorScopeArgs { scope: MonitorScope }

/**
 * コマンド名 → 引数・戻り値。Tauriのinvokeはトップレベル引数名で渡すため、
 * Rust側コマンドは `args: XxxArgs` を1引数で受け、TSは `invoke(name, { args })` で呼ぶ。
 */
export interface CommandMap {
  get_snapshot: { args: Record<string, never>; result: HostSnapshot };
  connect_backend: { args: ConnectBackendArgs; result: SourceInfo };
  list_chats: { args: ListChatsArgs; result: Page<ChatSummary> };
  open_chat: { args: OpenChatArgs; result: AgentHistory };
  start_chat: { args: StartChatArgs; result: StartChatResult };
  send_message: { args: SendMessageArgs; result: SendAttempt };
  retry_send: { args: RetrySendArgs; result: SendAttempt };
  respond_request: { args: RespondRequestArgs; result: RespondOutcome };
  interrupt_chat: { args: InterruptChatArgs; result: InterruptChatResult };
  resume_chat: { args: ResumeChatArgs; result: ResumeOutcome };
  manage_chat: { args: ManageChatArgs; result: ManageOutcome };
  set_pinned: { args: SetPinnedArgs; result: Chat };
  list_models: { args: ListModelsArgs; result: ModelInfo[] };
  set_chat_model: { args: SetChatModelArgs; result: ChatModelSettings };
  set_monitor_scope: { args: SetMonitorScopeArgs; result: null };
}
export type CommandName = keyof CommandMap;

// ───────────── ホスト→UIイベント ─────────────

export type HostEvent =
  | { kind: "sourceUpdated"; source: SourceInfo }
  | { kind: "chatUpdated"; chat: Chat }
  | { kind: "chatRemoved"; chat: ChatKey }
  | { kind: "agentUpdated"; view: AgentView }
  | { kind: "turnUpdated"; turn: TurnKey; end: TurnEnd | null }
  | { kind: "activityUpdated"; activity: Activity }
  | { kind: "activityDelta"; item: ItemKey; delta: string }
  | { kind: "requestUpdated"; request: PendingRequest }
  | { kind: "sendUpdated"; chat: ChatKey; attempt: SendAttempt }
  | { kind: "queueUpdated"; item: QueueItem }
  | { kind: "stopUpdated"; record: StopRecord }
  | { kind: "modelSettingsUpdated"; chat: ChatKey; settings: ChatModelSettings }
  | { kind: "warning"; source: SourceId | null; message: string; rawLabel: string | null };

/** seqに欠番があればget_snapshotで取り直す。 */
export interface HostEventEnvelope { seq: number; at: UnixMillis; event: HostEvent }
