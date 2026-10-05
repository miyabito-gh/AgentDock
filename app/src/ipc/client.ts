// ホスト（Rust）とのIPC。実接続用。仮データ版は `../mock/client.ts`（モック切替用）。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  HOST_EVENT_CHANNEL,
  type AgentHistory, type AgentKey, type AppSettings, type Chat, type ChatKey, type ChatModelSettings, type CommandMap, type CommandName,
  type HostEventEnvelope, type HostSnapshot, type InterruptChatResult, type IpcError, type IpcErrorCode, type LocalId,
  type ModelChoice, type ModelInfo, type MonitorScope, type Page, type ChatSummary, type PermissionPreset,
  type RequestAnswer, type RequestKey, type RespondOutcome, type SendAttempt, type SendIntent, type SourceInfo,
  type StartChatResult, type ResumeOutcome, type SaveScope, type SaveStatus, type ChatQueue, type QueueEntry, type SettingsImpact,
  type AttachmentEntry, type FileRef,
} from "./types";

/** 型付きinvoke。引数は `args` 1個で渡す（types.ts CommandMap の規約）。 */
export function invokeCmd<K extends CommandName>(name: K, args: CommandMap[K]["args"]): Promise<CommandMap[K]["result"]> {
  return invoke(name, { args });
}

/** invokeのrejectをIpcErrorへ正規化する（形が違えば通信系のioとして扱い、成功・失敗の断定はしない）。 */
export function asIpcError(e: unknown): IpcError {
  if (e && typeof e === "object" && "code" in e && "message" in e) return e as IpcError;
  return { code: "io" as IpcErrorCode, message: typeof e === "string" ? e : e instanceof Error ? e.message : "不明なエラー", blocked: null };
}

/** ホストイベントの購読（seqの欠番検出と再snapshotは呼び出し側）。 */
export function subscribeHostEvents(handler: (e: HostEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<HostEventEnvelope>(HOST_EVENT_CHANNEL, (ev) => handler(ev.payload));
}

export const getSnapshot = (): Promise<HostSnapshot> => invokeCmd("get_snapshot", {});
export const connectBackend = (executable: string | null): Promise<SourceInfo> =>
  invokeCmd("connect_backend", { backend: "codex", executable });
export const listChats = (cursor: string | null = null, includeArchived = false): Promise<Page<ChatSummary>> =>
  invokeCmd("list_chats", { cursor, limit: null, search: null, includeArchived });
export const openChat = (chat: ChatKey): Promise<AgentHistory> => invokeCmd("open_chat", { chat });
export const startChat = (cwd: string | null, model: ModelChoice | null, firstMessage: string | null, permission: PermissionPreset | null = null): Promise<StartChatResult> =>
  invokeCmd("start_chat", { backend: "codex", cwd, model, permission, firstMessage });
export const sendMessage = (chat: ChatKey, text: string, intent: SendIntent, attachments: LocalId[] = []): Promise<SendAttempt> =>
  invokeCmd("send_message", { chat, text, attachments, intent });
export const retrySend = (chat: ChatKey, attempt: LocalId): Promise<SendAttempt> => invokeCmd("retry_send", { chat, attempt });
export const respondRequest = (request: RequestKey, answer: RequestAnswer): Promise<RespondOutcome> =>
  invokeCmd("respond_request", { request, answer });
export const interruptChat = (chat: ChatKey): Promise<InterruptChatResult> => invokeCmd("interrupt_chat", { chat });
export const resumeChat = (chat: ChatKey): Promise<ResumeOutcome> => invokeCmd("resume_chat", { chat });
export const setPinned = (chat: ChatKey, pinned: boolean): Promise<Chat> => invokeCmd("set_pinned", { chat, pinned });
export const listModels = (): Promise<ModelInfo[]> => invokeCmd("list_models", { backend: "codex", includeHidden: false });
export const setChatModel = (chat: ChatKey, choice: ModelChoice): Promise<ChatModelSettings> => invokeCmd("set_chat_model", { chat, choice });
export const setMonitorScope = (scope: MonitorScope): Promise<null> => invokeCmd("set_monitor_scope", { scope });
/** 入力途中の文章をホストへ渡す（保存は500msまとめ、ホスト側）。復元しても送信しない。 */
export const setDraft = (chat: ChatKey, text: string): Promise<null> => invokeCmd("set_draft", { chat, text });
/** 保存の再試行（ユーザー操作）。失敗してもエラーにはならず、失敗の内容を含む状態が返る。 */
export const retrySave = (scope: SaveScope): Promise<SaveStatus> => invokeCmd("retry_save", { scope });
/** 完了後に送る依頼の登録（設定は送信時点のものを使う）。 */
export const enqueue = (chat: ChatKey, text: string, attachments: LocalId[] = []): Promise<QueueEntry> => invokeCmd("enqueue", { chat, text, attachments });
export const editQueueEntry = (chat: ChatKey, entry: LocalId, text: string, attachments: LocalId[] = []): Promise<QueueEntry> =>
  invokeCmd("edit_queue_entry", { chat, entry, text, attachments });
export const cancelQueueEntry = (chat: ChatKey, entry: LocalId): Promise<null> => invokeCmd("cancel_queue_entry", { chat, entry });
/** 「キューを再開」（ユーザー操作）。「確認済み」とは別。失敗した依頼そのものは再実行しない。 */
export const resumeQueue = (chat: ChatKey): Promise<ChatQueue> => invokeCmd("resume_queue", { chat });
/** 「履歴と照合」。読み取りのみで再送しない。 */
export const reconcileSend = (chat: ChatKey, attempt: LocalId): Promise<SendAttempt> => invokeCmd("reconcile_send", { chat, attempt });
export const setChatPermission = (chat: ChatKey, permission: PermissionPreset): Promise<SettingsImpact> =>
  invokeCmd("set_chat_permission", { chat, permission });
export const setChatCwd = (chat: ChatKey, cwd: string, queueRetargetConfirmed: boolean): Promise<SettingsImpact> =>
  invokeCmd("set_chat_cwd", { chat, cwd, queueRetargetConfirmed });
/** アプリ設定の保存（通知設定など。ホストが保存し、`settingsUpdated` で返る）。 */
export const setAppSettings = (settings: AppSettings): Promise<AppSettings> => invokeCmd("set_app_settings", { settings });
/** 「確認済み」にする。印を外すだけで、再実行・成功化はしない。agent省略＝チャット内の未確認の失敗すべて。 */
export const acknowledgeFailure = (chat: ChatKey, agent: AgentKey | null): Promise<null> => invokeCmd("acknowledge_failure", { chat, agent });
/** 選択中のチャットをホストへ伝える（通知の抑制判定だけに使う）。 */
export const setSelectedChat = (chat: ChatKey | null): Promise<null> => invokeCmd("set_selected_chat", { chat });
/** 診断ログのフォルダをエクスプローラーで開く（ユーザー操作）。戻り値はログファイルの場所。 */
export const openDiagDir = (): Promise<string> => invoke("open_diag_dir");

// ── 添付・成果物（§3.8）。コピーしたファイルは対象チャット用の領域にあり、元ファイルは変更されない。 ──
/** ファイル選択・ドロップで得たパスを添付する。フォルダは `blocked.folderIsWorkspace`（コピーしない）。 */
export const addAttachmentFile = (chat: ChatKey, path: string): Promise<AttachmentEntry> => invokeCmd("add_attachment_file", { chat, path });
/** クリップボード画像（PNG）を生バイトで送る。JSON配列にはしない。チャットと名前はヘッダーで渡す。 */
export const addAttachmentImageBytes = (chat: ChatKey, name: string, bytes: ArrayBuffer): Promise<AttachmentEntry> =>
  invoke("add_attachment_image_bytes", bytes, { headers: { "x-chat-backend": chat.backend, "x-chat-id": chat.id, "x-file-name": name } });
export const removeAttachment = (chat: ChatKey, attachment: LocalId): Promise<null> => invokeCmd("remove_attachment", { chat, attachment });
/** 関連アプリで開く（ユーザー操作のときだけ）。実体がなければ欠損として止まる。 */
export const openFile = (target: FileRef): Promise<null> => invokeCmd("open_file", { target });
/** 名前を付けて保存。同名があれば `blocked.targetExists`。確認のうえ `overwriteConfirmed=true` で再実行する。 */
export const saveFileAs = (target: FileRef, dest: string, overwriteConfirmed: boolean): Promise<null> =>
  invokeCmd("save_file_as", { target, dest, overwriteConfirmed });
/** 画像のアプリ内プレビュー用のバイト列（画像だけ。大きいものはエラー）。 */
export const readFilePreview = (target: FileRef): Promise<ArrayBuffer> => invoke("read_file_preview", { args: { target } });
