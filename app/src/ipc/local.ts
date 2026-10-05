// 段階②（P1設計）のアプリ側データとIPC追加分。Rust `src-tauri/src/backend/local.rs` と1対1。
// P2〜P7で types.ts の HostEvent・CommandMap に統合する（統合時に live.ts の applyHostEvent に分岐を足す）。
// 設計メモは app/DESIGN_P2.md。

import type {
  AgentKey, AgentState, AttachmentKind, ChatKey, Freshness, ItemKey, Known, LocalId, ModelChoice,
  PermissionPreset, SaveState, SendAttempt, SourceId, StopRecord, TurnKey, UnixMillis,
} from "./types";

// ───────────── 保存状態 ─────────────

export type SaveScope =
  | { kind: "appSettings" }
  | { kind: "windowBounds" }
  | { kind: "chatLocal"; chat: ChatKey }
  | { kind: "queue"; chat: ChatKey }
  | { kind: "activity"; chat: ChatKey };
export interface SaveStatus { scope: SaveScope; state: SaveState; lastAttemptAt: UnixMillis | null; retries: number }

// ───────────── チャット別の補足情報 ─────────────

export interface Draft { text: string; attachments: LocalId[]; updatedAt: UnixMillis | null }
export type ArchiveSync =
  | { kind: "waitingForWorkEnd" }
  | { kind: "sending" }
  | { kind: "synced"; at: UnixMillis }
  | { kind: "failed"; message: string; at: UnixMillis }
  | { kind: "unsupported" };
export type ListVisibility =
  | { kind: "visible" }
  | { kind: "archived"; at: UnixMillis; sync: ArchiveSync }
  | { kind: "removedFromList"; at: UnixMillis };
export type DeletePendingReason =
  | { kind: "stopUnconfirmed" }
  | { kind: "ownershipUnknown" }
  | { kind: "partialFailure"; done: string[]; failed: string[] }
  | { kind: "readyForUserRetry" };
export interface DeletePending { requestedAt: UnixMillis; reason: DeletePendingReason; stopRecord: LocalId | null }
export interface ChatMarks { awaitingAnswer: boolean; unacknowledgedFailure: boolean }
export interface ChatLocalView {
  chat: ChatKey;
  pinned: boolean;
  lastUsedAt: UnixMillis | null;
  model: ModelChoice | null;
  permission: PermissionPreset | null;
  nextCwd: string | null;
  draft: Draft;
  visibility: ListVisibility;
  deletePending: DeletePending | null;
  marks: ChatMarks;
  save: SaveState;
}

// ───────────── 添付・成果物 ─────────────

export type AttachmentSource = { kind: "file"; originalPath: string } | { kind: "clipboardImage" };
export type CopyFailure =
  | { kind: "insufficientSpace"; required: number; available: number }
  | { kind: "sourceUnreadable" }
  | { kind: "writeFailed" }
  | { kind: "interrupted" }
  | { kind: "sizeMismatch" };
export type AttachmentState =
  | { kind: "copying" }
  | { kind: "ready" }
  | { kind: "copyFailed"; reason: CopyFailure; message: string }
  | { kind: "missing" };
export interface AttachmentEntry {
  id: LocalId;
  chat: ChatKey;
  kind: AttachmentKind;
  displayName: string;
  source: AttachmentSource;
  copyPath: string | null;
  size: Known<number>;
  attachedAt: UnixMillis;
  state: AttachmentState;
  usedBy: LocalId[];
}
export interface ArtifactEntry {
  id: LocalId;
  chat: ChatKey;
  agent: AgentKey;
  item: ItemKey | null;
  path: string;
  observedAt: UnixMillis;
  exists: Known<boolean>;
  checkedAt: UnixMillis | null;
  inChatArea: boolean;
}
export type FileRef = { kind: "attachment"; chat: ChatKey; id: LocalId } | { kind: "artifact"; chat: ChatKey; id: LocalId };

// ───────────── キュー ─────────────

export interface AppliedSettings { model: ModelChoice | null; permission: PermissionPreset; cwd: Known<string>; decidedAt: UnixMillis }
export type QueueStopCause =
  | { kind: "parentFailed"; turn: TurnKey }
  | { kind: "parentInterrupted"; turn: TurnKey }
  | { kind: "descendantFailed"; agent: AgentKey }
  | { kind: "descendantInterrupted"; agent: AgentKey }
  | { kind: "sendRejected"; entry: LocalId }
  | { kind: "notAcceptedAfterReconcile"; entry: LocalId };
export type QueueRun =
  | { kind: "active" }
  | { kind: "stopped"; cause: QueueStopCause; at: UnixMillis }
  | { kind: "pausedAfterRestart" };
export interface HoldTarget { agent: AgentKey; state: AgentState; freshness: Freshness }
export type QueueHold =
  | { kind: "parentWorking" }
  | { kind: "descendantsActive"; targets: HoldTarget[] }
  | { kind: "stateUnknown"; targets: HoldTarget[]; descendantScanIncomplete: boolean }
  | { kind: "notLive"; targets: HoldTarget[] }
  | { kind: "stopUnconfirmed"; record: LocalId }
  | { kind: "acceptanceUnknown"; attempt: LocalId }
  | { kind: "deletePending" }
  | { kind: "disconnected" };
export type QueueEntryState =
  | { kind: "waiting" }
  | { kind: "sending"; attempt: LocalId }
  | { kind: "acceptanceUnknown"; attempt: LocalId }
  | { kind: "sent"; turn: TurnKey }
  | { kind: "notAccepted"; attempt: LocalId; message: string }
  | { kind: "cancelled" };
export interface QueueEntry {
  id: LocalId;
  chat: ChatKey;
  text: string;
  attachments: LocalId[];
  order: number;
  registeredAt: UnixMillis;
  state: QueueEntryState;
  attempts: SendAttempt[];
  applied: AppliedSettings | null;
}
export interface ChatQueue {
  chat: ChatKey;
  run: QueueRun;
  hold: QueueHold | null;
  baselineAt: UnixMillis | null;
  entries: QueueEntry[];
  nextOrder: number;
}

// ───────────── 通知・窓・常駐 ─────────────

export interface NotificationSettings {
  enabled: boolean;
  approvalAndQuestion: boolean;
  completed: boolean;
  failed: boolean;
  sound: boolean;
  showChatName: boolean;
}
export type WindowKind = "main" | "monitor";
export type MonitorWindowScope = "selectedChat" | "allChats";
export interface WindowBounds { x: number; y: number; width: number; height: number; maximized: boolean }
export interface WindowPrefs { alwaysOnTop: boolean; bounds: WindowBounds | null }
export type SendKey = "ctrlEnter" | "enter";
export interface AppSettings {
  codexExecutable: string | null;
  autostart: boolean;
  notifications: NotificationSettings;
  mainWindow: WindowPrefs;
  monitorWindow: WindowPrefs;
  monitorScope: MonitorWindowScope;
  defaultModel: ModelChoice | null;
  sendKey: SendKey;
}

export type QuitPhase =
  | { kind: "idle" }
  | { kind: "confirming"; busy: ChatKey[] }
  | { kind: "stopping"; records: LocalId[]; startedAt: UnixMillis }
  | { kind: "stopUnconfirmed"; records: LocalId[] }
  | { kind: "flushing" }
  | { kind: "saveFailed"; failed: SaveScope[] };
export interface ForceKillPreview { source: SourceId; affected: ChatKey[]; activeProcesses: Known<number> }

// ───────────── 使用量・削除 ─────────────

export interface UsageBreakdown { attachments: number; artifacts: number; workspace: number; activity: number; metadata: number }
export interface ChatUsage { chat: ChatKey; breakdown: UsageBreakdown }
export interface UsageReport {
  total: UsageBreakdown;
  chats: ChatUsage[];
  freeSpace: Known<number>;
  measuredAt: UnixMillis;
  unreadable: string[];
}
export interface DeletePreview {
  chat: ChatKey;
  deletesBackendHistory: boolean;
  descendants: Known<number>;
  attachments: number;
  artifactsInChatArea: number;
  chatAreaBytes: Known<number>;
  requiresStop: boolean;
}
export type DeleteOutcome =
  | { kind: "deleted" }
  | { kind: "pending"; pending: DeletePending }
  | { kind: "partial"; done: string[]; failed: string[] };

// ───────────── コマンド ─────────────

export interface ChatArgs { chat: ChatKey }
export interface SetAppSettingsArgs { settings: AppSettings }
export interface RetrySaveArgs { scope: SaveScope }
export interface SetDraftArgs { chat: ChatKey; text: string }
export interface AddAttachmentFileArgs { chat: ChatKey; path: string }
export interface AttachmentArgs { chat: ChatKey; attachment: LocalId }
export interface OpenFileArgs { target: FileRef }
export interface SaveFileAsArgs { target: FileRef; dest: string; overwriteConfirmed: boolean }
export interface EnqueueArgs { chat: ChatKey; text: string; attachments: LocalId[] }
export interface EditQueueEntryArgs { chat: ChatKey; entry: LocalId; text: string; attachments: LocalId[] }
export interface QueueEntryArgs { chat: ChatKey; entry: LocalId }
export interface ReconcileSendArgs { chat: ChatKey; attempt: LocalId }
export interface SetChatPermissionArgs { chat: ChatKey; permission: PermissionPreset }
export interface SettingsImpact { local: ChatLocalView; affectedEntries: LocalId[] }
export interface SetChatCwdArgs { chat: ChatKey; cwd: string; queueRetargetConfirmed: boolean }
export interface AcknowledgeFailureArgs { chat: ChatKey; agent: AgentKey | null }
export interface SetSelectedChatArgs { chat: ChatKey | null }
export interface ExportMarkdownArgs { chat: ChatKey; includeMonitorActivity: boolean; dest: string; overwriteConfirmed: boolean }
export interface GetUsageArgs { chat: ChatKey | null }
export type QuitDecision = "cancel" | "stopAndQuit" | "wait" | "retryInterrupt";
export interface QuitDecisionArgs { decision: QuitDecision }
export interface ForceKillArgs { source: SourceId }
export interface SetAlwaysOnTopArgs { window: WindowKind; on: boolean }
export interface SetMonitorWindowScopeArgs { scope: MonitorWindowScope }

/** 追加コマンド。`add_attachment_image_bytes` は invoke(name, Uint8Array, { headers }) の生バイトで呼ぶため含めない。 */
export interface LocalCommandMap {
  get_app_settings: { args: Record<string, never>; result: AppSettings };
  set_app_settings: { args: SetAppSettingsArgs; result: AppSettings };
  get_chat_locals: { args: Record<string, never>; result: ChatLocalView[] };
  retry_save: { args: RetrySaveArgs; result: SaveStatus };
  set_draft: { args: SetDraftArgs; result: null };
  add_attachment_file: { args: AddAttachmentFileArgs; result: AttachmentEntry };
  remove_attachment: { args: AttachmentArgs; result: null };
  open_file: { args: OpenFileArgs; result: null };
  save_file_as: { args: SaveFileAsArgs; result: null };
  enqueue: { args: EnqueueArgs; result: QueueEntry };
  edit_queue_entry: { args: EditQueueEntryArgs; result: QueueEntry };
  cancel_queue_entry: { args: QueueEntryArgs; result: null };
  resume_queue: { args: ChatArgs; result: ChatQueue };
  reconcile_send: { args: ReconcileSendArgs; result: SendAttempt };
  set_chat_permission: { args: SetChatPermissionArgs; result: SettingsImpact };
  set_chat_cwd: { args: SetChatCwdArgs; result: SettingsImpact };
  acknowledge_failure: { args: AcknowledgeFailureArgs; result: null };
  set_selected_chat: { args: SetSelectedChatArgs; result: null };
  preview_delete: { args: ChatArgs; result: DeletePreview };
  delete_chat: { args: ChatArgs; result: DeleteOutcome };
  archive_chat: { args: ChatArgs; result: ChatLocalView };
  unarchive_chat: { args: ChatArgs; result: ChatLocalView };
  export_markdown: { args: ExportMarkdownArgs; result: null };
  get_usage: { args: GetUsageArgs; result: UsageReport };
  request_quit: { args: Record<string, never>; result: QuitPhase };
  quit_decision: { args: QuitDecisionArgs; result: QuitPhase };
  preview_force_kill: { args: ForceKillArgs; result: ForceKillPreview };
  force_kill: { args: ForceKillArgs; result: StopRecord[] };
  set_always_on_top: { args: SetAlwaysOnTopArgs; result: null };
  open_monitor_window: { args: Record<string, never>; result: null };
  set_monitor_window_scope: { args: SetMonitorWindowScopeArgs; result: null };
}

export type LocalBlockedReason =
  | { kind: "insufficientSpace"; required: number; available: number }
  | { kind: "folderIsWorkspace"; path: string }
  | { kind: "targetExists"; path: string }
  | { kind: "deletePending" }
  | { kind: "chatBusy" }
  | { kind: "queueRetargetUnconfirmed"; waiting: number }
  | { kind: "queueEntryNotEditable" }
  | { kind: "saveFailed"; scope: SaveScope }
  | { kind: "attachmentNotReady"; attachment: LocalId }
  | { kind: "modelLacksInput"; input: AttachmentKind };

/** 追加イベント。統合時に HostEvent へ入れる（seqは共通）。 */
export type LocalHostEvent =
  | { kind: "chatLocalUpdated"; local: ChatLocalView }
  | { kind: "chatQueueUpdated"; queue: ChatQueue }
  | { kind: "attachmentUpdated"; entry: AttachmentEntry }
  | { kind: "artifactUpdated"; entry: ArtifactEntry }
  | { kind: "saveStatusUpdated"; status: SaveStatus }
  | { kind: "settingsUpdated"; settings: AppSettings }
  | { kind: "quitUpdated"; phase: QuitPhase }
  | { kind: "navigateToChat"; chat: ChatKey }
  | { kind: "systemResumed"; at: UnixMillis }
  | { kind: "usageUpdated"; report: UsageReport };
