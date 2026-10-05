// ホスト（Rust）とのIPC。実接続用。仮データ版は `../mock/client.ts`（モック切替用）。
import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  HOST_EVENT_CHANNEL,
  type AgentHistory, type AgentKey, type AppSettings, type Chat, type ChatKey, type ChatModelSettings, type CommandMap, type CommandName,
  type HostEventEnvelope, type HostSnapshot, type InterruptChatResult, type IpcError, type IpcErrorCode, type LocalId,
  type ModelChoice, type ModelInfo, type MonitorScope, type Page, type ChatSummary, type PermissionPreset,
  type RequestAnswer, type RequestKey, type RespondOutcome, type SendAttempt, type SendIntent, type SourceInfo,
  type StartChatResult, type ResumeOutcome, type SaveScope, type SaveStatus, type ChatQueue, type QueueEntry, type SettingsImpact,
  type ForceKillPreview, type MonitorWindowScope, type QuitDecision, type QuitPhase, type StopRecord, type WindowKind,
  type ChatLocalView, type DeleteOutcome, type DeletePreview, type UsageReport,
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
/** 完了後に送る依頼の登録（設定は送信時点のものを使う）。 */
export const enqueue = (chat: ChatKey, text: string): Promise<QueueEntry> => invokeCmd("enqueue", { chat, text, attachments: [] });
export const editQueueEntry = (chat: ChatKey, entry: LocalId, text: string): Promise<QueueEntry> =>
  invokeCmd("edit_queue_entry", { chat, entry, text, attachments: [] });
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
// ── 窓・常駐・終了（P6） ──
/** 完全終了を要求する。作業中なら確認の段階が返る。閉じる操作（トレイ格納）とは別。 */
export const requestQuit = (): Promise<QuitPhase> => invokeCmd("request_quit", {});
export const quitDecision = (decision: QuitDecision): Promise<QuitPhase> => invokeCmd("quit_decision", { decision });
/** 強制終了の確認内容（影響を受ける全チャット）。実行はしない。 */
export const previewForceKill = (source: string): Promise<ForceKillPreview> => invokeCmd("preview_force_kill", { source });
/** 強制終了（確認画面の後だけ）。停止を確認できるまでは停止未確認のまま。 */
export const forceKill = (source: string): Promise<StopRecord[]> => invokeCmd("force_kill", { source });
export const setAlwaysOnTop = (window: WindowKind, on: boolean): Promise<null> => invokeCmd("set_always_on_top", { window, on });
export const openMonitorWindow = (): Promise<null> => invokeCmd("open_monitor_window", {});
export const setMonitorWindowScope = (scope: MonitorWindowScope): Promise<null> => invokeCmd("set_monitor_window_scope", { scope });
/** 通常画面を前面に出す。chat があればそのチャットを開く（表示だけ）。 */
export const showMainWindow = (chat: ChatKey | null): Promise<null> => invokeCmd("show_main_window", { chat });
/** このウィンドウを閉じる。通常画面はトレイへ格納され、監視窓はその窓だけ閉じる（作業は続く）。 */
export const closeThisWindow = (): Promise<void> => getCurrentWindow().close();

// 通常画面で選択中のチャットを監視窓へ伝える（画面の表示状態だけ。ホストの状態・送信先は変えない）。
const SELECTED_CHAT_EVENT = "agentdock://selected-chat";
const SELECTED_CHAT_REQUEST = "agentdock://selected-chat-request";
export const publishSelectedChat = (id: string | null): Promise<void> => emit(SELECTED_CHAT_EVENT, { id });
export const subscribeSelectedChat = (handler: (id: string | null) => void): Promise<UnlistenFn> =>
  listen<{ id: string | null }>(SELECTED_CHAT_EVENT, (ev) => handler(ev.payload.id));
/** 監視窓の起動時に、現在の選択を通常画面へ尋ねる。 */
export const requestSelectedChat = (): Promise<void> => emit(SELECTED_CHAT_REQUEST);
export const onSelectedChatRequest = (handler: () => void): Promise<UnlistenFn> => listen(SELECTED_CHAT_REQUEST, () => handler());

// ── 削除・アーカイブ・エクスポート・使用量（P7） ──
/** 削除確認に出す内容（実行しない）。 */
export const previewDelete = (chat: ChatKey): Promise<DeletePreview> => invokeCmd("preview_delete", { chat });
/** 削除（確認画面の後だけ）。停止を確認できなければ pending、部分失敗は partial（完了とは言わない）。 */
export const deleteChat = (chat: ChatKey): Promise<DeleteOutcome> => invokeCmd("delete_chat", { chat });
/** アーカイブ。アプリの一覧からは即座に隠れる。Codexへの反映は作業終了・停止確認の後。 */
export const archiveChat = (chat: ChatKey): Promise<ChatLocalView> => invokeCmd("archive_chat", { chat });
export const unarchiveChat = (chat: ChatKey): Promise<ChatLocalView> => invokeCmd("unarchive_chat", { chat });
export const exportMarkdown = (chat: ChatKey, includeMonitorActivity: boolean, dest: string, overwriteConfirmed: boolean): Promise<null> =>
  invokeCmd("export_markdown", { chat, includeMonitorActivity, dest, overwriteConfirmed });
export const getUsage = (chat: ChatKey | null): Promise<UsageReport> => invokeCmd("get_usage", { chat });
/** codex.exe の「参照…」。選ぶだけで設定は変えない。キャンセルなら null。 */
export const pickCodexExecutable = (): Promise<string | null> => invokeCmd("pick_codex_executable", {});
/** 保存先の選択（同名があればOSが上書きを確認する）。キャンセルなら null。 */
export const pickSaveFile = (defaultName: string): Promise<string | null> => invokeCmd("pick_save_file", { defaultName });

/** 診断ログのフォルダをエクスプローラーで開く（ユーザー操作）。戻り値はログファイルの場所。 */
export const openDiagDir = (): Promise<string> => invoke("open_diag_dir");
