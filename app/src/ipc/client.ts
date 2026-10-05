// ホスト（Rust）とのIPC。実接続用。仮データ版は `../mock/client.ts`（モック切替用）。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  HOST_EVENT_CHANNEL,
  type AgentHistory, type Chat, type ChatKey, type ChatModelSettings, type CommandMap, type CommandName,
  type HostEventEnvelope, type HostSnapshot, type InterruptChatResult, type IpcError, type IpcErrorCode, type LocalId,
  type ModelChoice, type ModelInfo, type MonitorScope, type Page, type ChatSummary, type PermissionPreset,
  type RequestAnswer, type RequestKey, type RespondOutcome, type SendAttempt, type SendIntent, type SourceInfo,
  type StartChatResult, type ResumeOutcome, type SaveScope, type SaveStatus,
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
export const sendMessage = (chat: ChatKey, text: string, intent: SendIntent): Promise<SendAttempt> =>
  invokeCmd("send_message", { chat, text, attachments: [], intent });
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
/** 診断ログのフォルダをエクスプローラーで開く（ユーザー操作）。戻り値はログファイルの場所。 */
export const openDiagDir = (): Promise<string> => invoke("open_diag_dir");
