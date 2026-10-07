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
  type AttachmentEntry, type FileRef,
  type ForceKillPreview, type MonitorWindowScope, type QuitDecision, type QuitPhase, type StopRecord, type WindowKind,
  type ChatLocalView, type DeleteOutcome, type DeletePreview, type UsageReport, type ManageOutcome,
  type ChangeList, type ChangeScope, type ChangeSource, type ExternalId, type RevertPlan, type RevertResult, type UnifiedDiff,
  type BackendStatus, type Goal, type GoalUpdate, type Known, type MemoryStatus, type OpAck, type ResetMemoryResult, type WorkMode, type WorkModeInfo,
  type GitInfo, type WorktreeRecord, type WorktreeRemoveOutcome, type WorktreeRemovePreview,
  type HandoffText, type InstructionFiles, type InstructionTemplateResult, type OpenSideResult, type ReferenceBlock, type ReferenceTurns, type SideSessionMeta, type SideTranscript, type SkillList,
  type CompactChatResult, type CompactionSnapshot, type ForkReconcile, type ForkResult, type OpReconcile, type ParityOp, type ReviewChoices, type ReviewDelivery, type ReviewOutcome, type ReviewTarget,
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
/** worktree は、作業フォルダ（cwd）がAgentDockが作ったworktreeの場所のとき、その記録のID（チャットに結び付ける）。 */
export const startChat = (cwd: string | null, model: ModelChoice | null, firstMessage: string | null, permission: PermissionPreset | null = null, worktree: LocalId | null = null): Promise<StartChatResult> =>
  invokeCmd("start_chat", { backend: "codex", cwd, model, permission, firstMessage, worktree });
export const sendMessage = (chat: ChatKey, text: string, intent: SendIntent, attachments: LocalId[] = []): Promise<SendAttempt> =>
  invokeCmd("send_message", { chat, text, attachments, intent });
export const retrySend = (chat: ChatKey, attempt: LocalId): Promise<SendAttempt> => invokeCmd("retry_send", { chat, attempt });
export const respondRequest = (request: RequestKey, answer: RequestAnswer): Promise<RespondOutcome> =>
  invokeCmd("respond_request", { request, answer });
export const interruptChat = (chat: ChatKey): Promise<InterruptChatResult> => invokeCmd("interrupt_chat", { chat });
export const resumeChat = (chat: ChatKey): Promise<ResumeOutcome> => invokeCmd("resume_chat", { chat });
/** 名前の変更（ユーザー操作）。Codex が受け付けたことを確認できたときだけ成功になる。 */
export const renameChat = (chat: ChatKey, name: string): Promise<ManageOutcome> => invokeCmd("manage_chat", { chat, op: { kind: "rename", name } });
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
/** 「状態を再確認」。保留中の子孫の履歴を読み直す（読み取りのみ。resumeしない）。 */
export const recheckQueueState = (chat: ChatKey): Promise<ChatQueue> => invokeCmd("recheck_queue_state", { chat });
/** 「確認して今すぐ送る」。確認ダイアログで承認した後にだけ呼ぶ。先頭の送信待ち1件だけを送る。 */
export const sendQueueEntryNow = (chat: ChatKey, entry: LocalId): Promise<ChatQueue> => invokeCmd("send_queue_entry_now", { chat, entry });
/** 「履歴と照合」。読み取りのみで再送しない。 */
export const reconcileSend = (chat: ChatKey, attempt: LocalId): Promise<SendAttempt> => invokeCmd("reconcile_send", { chat, attempt });
export const setChatPermission = (chat: ChatKey, permission: PermissionPreset): Promise<SettingsImpact> =>
  invokeCmd("set_chat_permission", { chat, permission });
export const setChatCwd = (chat: ChatKey, cwd: string, queueRetargetConfirmed: boolean): Promise<SettingsImpact> =>
  invokeCmd("set_chat_cwd", { chat, cwd, queueRetargetConfirmed });
/** アプリ設定の保存（通知設定など。ホストが保存し、`settingsUpdated` で返る）。 */
export const setAppSettings = (settings: AppSettings): Promise<AppSettings> => invokeCmd("set_app_settings", { settings });
/** 起動時の保存データ警告を確認済みにする（reset=true で解除）。設定へ保存するだけで、領域のファイルは触らない。 */
export const acknowledgeWarnings = (reset: boolean): Promise<AppSettings> => invokeCmd("acknowledge_warnings", { reset });
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
/** ユーザーがチャットを開いた操作を記録する（最近利用時刻。一覧の並び用。背景の読み込みでは呼ばない）。 */
export const touchChatUsed = (chat: ChatKey): Promise<null> => invokeCmd("touch_chat_used", { chat });
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

// ── 添付・成果物（§3.8）。コピーしたファイルは対象チャット用の領域にあり、元ファイルは変更されない。 ──
/** ファイル選択・ドロップで得たパスを添付する。フォルダは `blocked.folderIsWorkspace`（コピーしない）。 */
export const addAttachmentFile = (chat: ChatKey, path: string): Promise<AttachmentEntry> => invokeCmd("add_attachment_file", { chat, path });
/** クリップボード画像（PNG）を生バイトで送る。JSON配列にはしない。チャットと名前はヘッダーで渡す。 */
export const addAttachmentImageBytes = (chat: ChatKey, name: string, bytes: ArrayBuffer): Promise<AttachmentEntry> =>
  invoke("add_attachment_image_bytes", bytes, { headers: { "x-chat-backend": chat.backend, "x-chat-id": chat.id, "x-file-name": name } });
export const removeAttachment = (chat: ChatKey, attachment: LocalId): Promise<null> => invokeCmd("remove_attachment", { chat, attachment });
/** 関連アプリで開く（ユーザー操作のときだけ）。実体がなければ欠損として止まる。 */
/** プロジェクト外・実行形式は `blocked.openNeedsConfirm`。確認のうえ `riskConfirmed=true` で再実行する。 */
export const openFile = (target: FileRef, riskConfirmed = false): Promise<null> => invokeCmd("open_file", { target, riskConfirmed });
/** 名前を付けて保存。同名があれば `blocked.targetExists`。確認のうえ `overwriteConfirmed=true` で再実行する。 */
export const saveFileAs = (target: FileRef, dest: string, overwriteConfirmed: boolean): Promise<null> =>
  invokeCmd("save_file_as", { target, dest, overwriteConfirmed });
/** 画像のアプリ内プレビュー用のバイト列（画像だけ。大きいものはエラー）。 */
export const readFilePreview = (target: FileRef): Promise<ArrayBuffer> => invoke("read_file_preview", { args: { target } });

// ── 変更ファイルと差分・変更を戻す（段階③ #1・#2）。一覧と差分は読取りのみ。 ──
/** 変更ファイルの一覧。scope が workingTree ならGit上の現在の差分、それ以外はCodexが報告した変更（出所は混ぜない）。 */
export const getChangeList = (chat: ChatKey, scope: ChangeScope): Promise<ChangeList> => invokeCmd("get_change_list", { chat, scope });
export const getFileDiff = (chat: ChatKey, path: string, source: ChangeSource, turn: ExternalId | null): Promise<UnifiedDiff> =>
  invokeCmd("get_file_diff", { chat, path, source, turn });
/** 戻す計画の作成（ファイルは変更しない）。ファイルごとに戻せるか・戻せない理由が返る。 */
export const previewRevert = (chat: ChatKey, turn: ExternalId | null, paths: string[] | null): Promise<RevertPlan> => invokeCmd("preview_revert", { chat, turn, paths });
/** 変更を戻す（確認画面の後だけ）。控えを保存できなければ何も変えない。部分成功は failed に出る。 */
export const revertChanges = (chat: ChatKey, planId: LocalId, paths: string[]): Promise<RevertResult> => invokeCmd("revert_changes", { chat, planId, paths });

// ── 計画／実行・Goal・状態・memories（段階③ P3-3） ──
/** 計画／実行の選択（次のturnから適用）。null＝選択を外す。受理されたかは、設定の更新通知を受けたときだけ「受理済み」に出る。 */
export const setWorkMode = (chat: ChatKey, mode: WorkMode | null): Promise<SettingsImpact> => invokeCmd("set_work_mode", { chat, mode });
export const listWorkModes = (): Promise<WorkModeInfo[]> => invokeCmd("list_work_modes", {});
/** 目標の取得（読取りのみ。resumeしない）。値なし＝目標は設定されていない、未取得＝読めなかった。 */
export const getGoal = (chat: ChatKey): Promise<Known<Goal>> => invokeCmd("get_goal", { chat });
/** 目標の設定・変更（ユーザー操作。必要なら会話を再開する）。accepted は要求の受付で、結果は画面の目標の更新で確認する。 */
export const setGoal = (chat: ChatKey, update: GoalUpdate): Promise<OpAck> => invokeCmd("set_goal", { chat, update });
export const clearGoal = (chat: ChatKey): Promise<OpAck> => invokeCmd("clear_goal", { chat });
/** Codex の状態（読取りのみ）。取れない項目は未取得のまま。 */
export const getBackendStatus = (): Promise<BackendStatus> => invokeCmd("get_backend_status", {});
export const getMemoryStatus = (): Promise<MemoryStatus> => invokeCmd("get_memory_status", {});
export const setMemoryMode = (chat: ChatKey, mode: string): Promise<OpAck> => invokeCmd("set_memory_mode", { chat, mode });
/** memoriesのリセット（記憶データを消す）。影響を表示して確認した後だけ confirmed=true で呼ぶ。 */
export const resetMemory = (confirmed: boolean): Promise<ResetMemoryResult> => invokeCmd("reset_memory", { confirmed });

// ── レビュー・分岐・圧縮（段階③ P3-2）。すべて確認画面の後のユーザー操作。結果は「受け付けた」までで、完了は会話の記録の観測で示す。 ──
/** レビュー対象の候補（読取りのみ）。Gitのリポジトリでなければ理由つきで空（指示だけ選べる）。 */
export const getReviewChoices = (chat: ChatKey): Promise<ReviewChoices> => invokeCmd("get_review_choices", { chat });
/** コードレビュー。newChat は新しい会話を作って実行する（作成後に始められなければ newChatCreatedReviewFailed）。 */
export const startReview = (chat: ChatKey, target: ReviewTarget, delivery: ReviewDelivery): Promise<ReviewOutcome> => invokeCmd("start_review", { chat, target, delivery });
/** 会話の分岐。throughTurn=null は最新の終端turnまで（実行中のチャットはturnの指定が必要）。unknown は再送せず reconcileFork で確認する。 */
export const forkChat = (chat: ChatKey, throughTurn: ExternalId | null): Promise<ForkResult> => invokeCmd("fork_chat", { chat, throughTurn });
/** 受理不明の分岐の照合（読取りのみ）。ちょうど1件に絞れたときだけ分岐先を採用する。 */
export const reconcileFork = (chat: ChatKey, attemptedAt: number): Promise<ForkReconcile> => invokeCmd("reconcile_fork", { chat, attemptedAt });
/** 文脈の圧縮。圧縮前の本文を控えに保存してから送る（保存できなければ送らない）。 */
export const compactChat = (chat: ChatKey): Promise<CompactChatResult> => invokeCmd("compact_chat", { chat });
/** 結果が未確認の操作（レビュー・圧縮）の照合（読取りのみ。再送しない）。 */
export const reconcileOp = (chat: ChatKey, op: ParityOp): Promise<OpReconcile> => invokeCmd("reconcile_op", { chat, op });
/** 圧縮前の控えの一覧（作成時刻。新しい順）と本文。AgentDockが保存した表示専用の控え。 */
export const listCompactionSnapshots = (chat: ChatKey): Promise<number[]> => invokeCmd("list_compaction_snapshots", { chat });
export const readCompactionSnapshot = (chat: ChatKey, snapshot: number): Promise<CompactionSnapshot> => invokeCmd("read_compaction_snapshot", { chat, snapshot });

// ── Git worktree（段階③ #14）。作成・削除はユーザー操作。削除はAgentDockが作った記録のあるものだけ。 ──
/** フォルダのGit情報（読取りのみ）。リポジトリでない・Gitなしは status に理由が入る。 */
export const gitInfo = (folder: string): Promise<GitInfo> => invokeCmd("git_info", { folder });
/** worktreeの作成。戻り値の state が ready のときだけ作成成功（failed は作られた分を自動削除していない）。 */
export const createWorktree = (repoRoot: string, branch: string | null, base: string | null, chatName: string | null): Promise<WorktreeRecord> =>
  invokeCmd("create_worktree", { repoRoot, branch, base, chatName });
/** 削除確認の内容（読取りのみ）。 */
export const previewRemoveWorktree = (id: LocalId): Promise<WorktreeRemovePreview> => invokeCmd("preview_remove_worktree", { id });
/** 削除。force は未コミット変更の破棄（2段目の確認の後だけ）。deleteBranch はマージ済みのときだけブランチも削除。 */
export const removeWorktree = (id: LocalId, force: boolean, deleteBranch: boolean): Promise<WorktreeRemoveOutcome> =>
  invokeCmd("remove_worktree", { id, force, deleteBranch });

// ── 過去会話の参考指定・Skills・指示ファイル・side相談（段階③ P3-4）。参考・一覧・確認は読取りのみ（resumeしない）。 ──
/** 参考にできるturnの一覧（古い順。読取りのみ。取得できなければエラー）。 */
export const listReferenceTurns = (chat: ChatKey): Promise<ReferenceTurns> => invokeCmd("list_reference_turns", { chat });
/** 選んだturnの抜粋（原文のまま。取得できなかったturnは missing と本文の「未取得」で示す）。入力欄へ入れるだけで、自動送信しない。 */
export const buildReference = (chat: ChatKey, turns: ExternalId[]): Promise<ReferenceBlock> => invokeCmd("build_reference", { chat, turns });
/** 作業フォルダのSkill一覧（読取りのみ）。forceReload はディスクの再走査（明示操作のときだけ）。 */
export const listSkills = (chat: ChatKey, forceReload: boolean): Promise<SkillList> => invokeCmd("list_skills", { chat, forceReload });
/** Skillを入力欄の指定（チップ）にする（コピーしない。送信時に明示呼出しの入力になる）。 */
export const addSkillAttachment = (chat: ChatKey, name: string, path: string): Promise<AttachmentEntry> => invokeCmd("add_skill_attachment", { chat, name, path });
/** 指示ファイル（AGENTS.md等）の確認（読取りのみ）。読み込み済みの一覧は、会話を開始・再開・分岐した応答にだけ載る（なければ未取得）。 */
export const getInstructionFiles = (chat: ChatKey): Promise<InstructionFiles> => invokeCmd("get_instruction_files", { chat });
/** AGENTS.md の雛形の作成（確認の後だけ。既存があれば blocked.alreadyExists で何も書かない）。 */
export const createInstructionTemplate = (chat: ChatKey): Promise<InstructionTemplateResult> => invokeCmd("create_instruction_template", { chat });
/** 指示ファイルを関連アプリで開く（ユーザー操作のときだけ。その会話の指示ファイルとして確認できたパスだけ）。 */
export const openInstructionFile = (chat: ChatKey, path: string): Promise<null> => invokeCmd("open_instruction_file", { chat, path });
/** side相談を開く（読取り専用の一時の分岐）。unknown は開けたか不明（再送しない）。 */
export const openSide = (chat: ChatKey): Promise<OpenSideResult> => invokeCmd("open_side", { chat });
/** sideへ送る（再送しない）。unknown は受理を確認できない。 */
export const sendSide = (side: LocalId, text: string): Promise<OpAck> => invokeCmd("send_side", { side, text });
/** 相談を閉じる。実行中は interrupt=true（確認のうえ）のときだけ閉じる。 */
export const closeSide = (side: LocalId, interrupt: boolean): Promise<SideSessionMeta> => invokeCmd("close_side", { side, interrupt });
/** 相談の記録（確定した発言。閉じた後は保存した記録を読み取り専用で返す）。 */
export const readSide = (side: LocalId): Promise<SideTranscript> => invokeCmd("read_side", { side });
/** 選んだ発言を主会話へ渡す引用ブロック（入力欄へ入れるだけで、自動送信しない）。entries は SideTranscript.entries の添字。 */
export const handoffSide = (side: LocalId, entries: number[]): Promise<HandoffText> => invokeCmd("handoff_side", { side, entries });
