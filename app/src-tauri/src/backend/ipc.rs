//! UI⇔ホストのTauri IPC契約（コマンド引数・戻り値・イベント）。
//!
//! - TS側 `app/src/ipc/types.ts` と1対1。名前・タグ（`kind`）・camelCaseを揃える。
//! - UIは [`super::model`] の正規化型だけを見る。Codex固有の型・メソッド名は流さない（原文ラベルは診断表示のみ）。
//! - 取得・保存・キュー判断・通知集約はホストが持つ。UIはコマンドで意図を伝え、結果はイベントで受ける。
//! - rendererの再読込み後は `get_snapshot` で全体を取り直し、以後 `HostEventEnvelope.seq` で差分を適用する。
//!   seqが飛んだら再度 `get_snapshot` する。
//! - 状態を変える操作（送信・承認回答・中断・再開・管理）はユーザー操作のコマンドからだけ実行する。
//!   ホスト内部の自動処理からは呼ばない（`UserConfirmed` はこのコマンド層でだけ発行する）。

use serde::{Deserialize, Serialize};

use super::backend::{
    ChatSummary, InterruptAck, ManageOp, ManageOutcome, Page, PermissionPreset, RespondOutcome, ResumeOutcome,
    AgentHistory,
};
use super::local::{AppSettings, ArtifactEntry, AttachmentEntry, ChatLocalView, ChatQueue, QuitPhase, SaveScope, SaveStatus};
use super::model::*;

/// ホスト→UIのイベント名（Tauri `emit` のチャネル）。
pub const HOST_EVENT_CHANNEL: &str = "agentdock://host-event";

/// コマンド名（`#[tauri::command]` の関数名）。TSの `CommandMap` のキーと一致させる。
pub mod command_names {
    pub const GET_SNAPSHOT: &str = "get_snapshot";
    pub const CONNECT_BACKEND: &str = "connect_backend";
    pub const LIST_CHATS: &str = "list_chats";
    pub const OPEN_CHAT: &str = "open_chat";
    pub const START_CHAT: &str = "start_chat";
    pub const SEND_MESSAGE: &str = "send_message";
    pub const RETRY_SEND: &str = "retry_send";
    pub const RESPOND_REQUEST: &str = "respond_request";
    pub const INTERRUPT_CHAT: &str = "interrupt_chat";
    pub const RESUME_CHAT: &str = "resume_chat";
    pub const MANAGE_CHAT: &str = "manage_chat";
    pub const SET_PINNED: &str = "set_pinned";
    pub const LIST_MODELS: &str = "list_models";
    pub const SET_CHAT_MODEL: &str = "set_chat_model";
    pub const SET_MONITOR_SCOPE: &str = "set_monitor_scope";
    pub const ACKNOWLEDGE_WARNINGS: &str = "acknowledge_warnings";
    pub const GET_APP_SETTINGS: &str = "get_app_settings";
    pub const SET_APP_SETTINGS: &str = "set_app_settings";
    pub const GET_CHAT_LOCALS: &str = "get_chat_locals";
    pub const RETRY_SAVE: &str = "retry_save";
    pub const SET_DRAFT: &str = "set_draft";
    pub const ENQUEUE: &str = "enqueue";
    pub const EDIT_QUEUE_ENTRY: &str = "edit_queue_entry";
    pub const CANCEL_QUEUE_ENTRY: &str = "cancel_queue_entry";
    pub const RESUME_QUEUE: &str = "resume_queue";
    pub const RECHECK_QUEUE_STATE: &str = "recheck_queue_state";
    pub const SEND_QUEUE_ENTRY_NOW: &str = "send_queue_entry_now";
    pub const RECONCILE_SEND: &str = "reconcile_send";
    pub const SET_CHAT_PERMISSION: &str = "set_chat_permission";
    pub const SET_CHAT_CWD: &str = "set_chat_cwd";
    pub const ACKNOWLEDGE_FAILURE: &str = "acknowledge_failure";
    pub const SET_SELECTED_CHAT: &str = "set_selected_chat";
    pub const ADD_ATTACHMENT_FILE: &str = "add_attachment_file";
    /// 本文は生バイト（`tauri::ipc::Request` の Raw body）。チャットIDと名前はヘッダーで渡す。
    pub const ADD_ATTACHMENT_IMAGE_BYTES: &str = "add_attachment_image_bytes";
    pub const REMOVE_ATTACHMENT: &str = "remove_attachment";
    pub const OPEN_FILE: &str = "open_file";
    pub const SAVE_FILE_AS: &str = "save_file_as";
    pub const REQUEST_QUIT: &str = "request_quit";
    pub const QUIT_DECISION: &str = "quit_decision";
    pub const PREVIEW_FORCE_KILL: &str = "preview_force_kill";
    pub const FORCE_KILL: &str = "force_kill";
    pub const SET_ALWAYS_ON_TOP: &str = "set_always_on_top";
    pub const OPEN_MONITOR_WINDOW: &str = "open_monitor_window";
    pub const SET_MONITOR_WINDOW_SCOPE: &str = "set_monitor_window_scope";
    pub const SHOW_MAIN_WINDOW: &str = "show_main_window";
    pub const PREVIEW_DELETE: &str = "preview_delete";
    pub const DELETE_CHAT: &str = "delete_chat";
    pub const ARCHIVE_CHAT: &str = "archive_chat";
    pub const UNARCHIVE_CHAT: &str = "unarchive_chat";
    pub const EXPORT_MARKDOWN: &str = "export_markdown";
    pub const GET_USAGE: &str = "get_usage";
    pub const PICK_CODEX_EXECUTABLE: &str = "pick_codex_executable";
    pub const PICK_SAVE_FILE: &str = "pick_save_file";
}

// ───────────────────────────── エラー ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum IpcErrorCode {
    NotConnected,
    Unsupported,
    Rejected,
    /// 結果不明（失敗確定ではない）。UIは「確認中」と表示する。
    OutcomeUnknown,
    Protocol,
    Io,
    InvalidArgs,
    NotFound,
    /// ホストの規則で拒否（停止未確認中の削除・送信、受理不明中の再送など）。
    Blocked,
}

/// コマンドの失敗。Tauriコマンドは `Result<T, IpcError>` を返し、TSでは reject される。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
    /// `Blocked` の理由など、UIで出し分ける補助情報。
    pub blocked: Option<BlockedReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BlockedReason {
    StopUnconfirmed { record: LocalId },
    AcceptanceUnknown { attempt: LocalId },
    RunningElsewhere,
    CapabilityUnsupported { capability: String },
    RequestAlreadyResolved,
    /// 保存に必要な空きが足りない。操作を止めて案内する（自動削除しない、D02）。
    InsufficientSpace {
        #[cfg_attr(test, ts(type = "number"))]
        required: u64,
        #[cfg_attr(test, ts(type = "number"))]
        available: u64,
    },
    /// 保存できていない単位があるため、完了を示す操作を止めた。
    SaveFailed { scope: SaveScope },
    /// 作業中・停止未確認のため、作業フォルダは変更できない（完了・停止後に変更する、M44）。
    ChatBusy,
    /// 送信待ちがあり、新しいフォルダが対象になることの確認が要る（M44）。
    QueueRetargetUnconfirmed { waiting: u32 },
    /// 送信待ち以外の項目は編集・取消できない（送信中・受理不明・送信済み）。
    QueueEntryNotEditable,
    /// 削除保留中のため、このチャットへの新しい送信は止めている（M46）。
    DeletePending,
    /// フォルダは作業フォルダの指定であり、添付としてコピーしない。
    FolderIsWorkspace { path: String },
    /// 保存先に同名のファイルがある。UIで上書きを確認したら `overwriteConfirmed` で再実行する。
    TargetExists { path: String },
    /// 送信に使えない添付（コピー中・失敗・欠損）がある。
    AttachmentNotReady { attachment: LocalId },
    /// 選択中のモデルが画像入力に対応していない（表示できることと、モデルが読めることは別）。
    ModelLacksInput { input: AttachmentKind },
}

/// 保存先の選択ダイアログ（ファイルの書込みはしない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PickSaveFileArgs {
    pub default_name: String,
}

// ───────────────────────────── コマンド引数・戻り値 ─────────────────────────────

/// 監視・表示の全体像（renderer再読込み時の取り直し用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct HostSnapshot {
    /// この時点までに発行したイベントの最大seq。以後は seq+1 から適用する。
    #[cfg_attr(test, ts(type = "number"))]
    pub seq: u64,
    pub sources: Vec<SourceInfo>,
    pub chats: Vec<Chat>,
    pub agents: Vec<AgentView>,
    pub requests: Vec<PendingRequest>,
    pub stops: Vec<StopRecord>,
    /// チャット別のキュー（送信待ちの依頼・進行・保留理由）。
    pub queues: Vec<ChatQueue>,
    pub monitor_scope: MonitorScope,
    /// アプリ側の補足情報（ピン・下書き・モデル/権限・一覧の見え方）。再起動後も復元される。
    pub chat_locals: Vec<ChatLocalView>,
    /// 保存の状態（単位ごと）。失敗・未保存を保存済みと表示しないための根拠。
    pub save_status: Vec<SaveStatus>,
    pub settings: AppSettings,
    /// チャット別のモデル設定（再起動・renderer再読込み後の復元用）。
    pub model_settings: Vec<ChatModelEntry>,
    /// 起動時に読めなかった保存ファイルなどの警告（イベントは購読前に出るので、スナップショットで渡す）。
    pub startup_warnings: Vec<StartupWarning>,
    /// 添付の台帳（再起動・renderer再読込み後の復元用）。実体の有無は `state` で示す。
    pub attachments: Vec<AttachmentEntry>,
    /// 実在を確認した成果物。
    pub artifacts: Vec<ArtifactEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatModelEntry {
    pub chat: ChatKey,
    pub settings: ChatModelSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ConnectBackendArgs {
    pub backend: BackendKind,
    /// 実行ファイルの上書き（None＝設定値または既定）。
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ListChatsArgs {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub search: Option<String>,
    pub include_archived: bool,
}

pub type ListChatsResult = Page<ChatSummary>;

/// 会話を開く（保存履歴の読み取り。resumeしない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OpenChatArgs {
    pub chat: ChatKey,
}

pub type OpenChatResult = AgentHistory;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartChatArgs {
    pub backend: BackendKind,
    /// None＝一般チャット（ホストがアプリ管理の作業領域を割り当てる、M21）。
    pub cwd: Option<String>,
    pub model: Option<ModelChoice>,
    /// None＝初期値（WorkspaceWriteOnRequest）。
    pub permission: Option<PermissionPreset>,
    /// 最初の依頼文。あれば開始直後に送信する（チャット名の機械的切り出しにも使う）。
    pub first_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartChatResult {
    pub chat: Chat,
    pub first_send: Option<SendAttempt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum SendIntent {
    /// 新しいturnとして送る（実行中ならエラー。キュー登録は§3.7の別コマンドで扱う）。
    NewTurn,
    /// 実行中turnへの追加指示。
    Steer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SendMessageArgs {
    pub chat: ChatKey,
    pub text: String,
    pub attachments: Vec<LocalId>,
    pub intent: SendIntent,
}

/// 送信試行。`state` が `acceptanceUnknown` の間、UIは再送ボタンを出さない。
pub type SendMessageResult = SendAttempt;

/// 受理なしが確定した試行（`rejected` / `notFoundAfterReconcile`）だけ再送できる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RetrySendArgs {
    pub chat: ChatKey,
    pub attempt: LocalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RespondRequestArgs {
    pub request: RequestKey,
    pub answer: RequestAnswer,
}

pub type RespondRequestResult = RespondOutcome;

/// チャットの中断（親＋子孫の停止手順はホストが組み立てる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct InterruptChatArgs {
    pub chat: ChatKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct InterruptChatResult {
    /// 親への中断要求の受付結果（停止確認ではない）。
    pub ack: InterruptAck,
    /// 停止照合の記録。以後 `stopUpdated` イベントで更新される。
    pub record: StopRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ResumeChatArgs {
    pub chat: ChatKey,
}

pub type ResumeChatResult = ResumeOutcome;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ManageChatArgs {
    pub chat: ChatKey,
    pub op: ManageOp,
}

pub type ManageChatResult = ManageOutcome;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetPinnedArgs {
    pub chat: ChatKey,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ListModelsArgs {
    pub backend: BackendKind,
    pub include_hidden: bool,
}

pub type ListModelsResult = Vec<ModelInfo>;

/// チャット単位のモデル選択（既定値の変更は既存チャットに波及させない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetChatModelArgs {
    pub chat: ChatKey,
    pub choice: ModelChoice,
}

pub type SetChatModelResult = ChatModelSettings;

/// 右パネル・コンパクト監視窓の表示範囲（§3.5）。表示範囲の変更だけで送信先・中断・承認を変えない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MonitorScope {
    /// 選択中チャット（初期値）。完了済みも表示。
    SelectedChat { chat: Option<ChatKey> },
    /// 全チャット。`show_finished` で完了・停止済み・確認済み失敗を追加。
    AllChats { show_finished: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetMonitorScopeArgs {
    pub scope: MonitorScope,
}

/// 起動時の警告。`acknowledgeable` は、領域・ファイルに紐づく警告（パスやIDを含み、状況が変われば文が変わる）だけが true。
/// 保存できない状態の警告（空き不足・保存領域を開けない等）は false で、確認済みにせず毎回出す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartupWarning {
    pub message: String,
    pub acknowledgeable: bool,
}

impl StartupWarning {
    pub fn acknowledgeable(message: impl Into<String>) -> Self {
        StartupWarning { message: message.into(), acknowledgeable: true }
    }
    pub fn transient(message: impl Into<String>) -> Self {
        StartupWarning { message: message.into(), acknowledgeable: false }
    }
}

/// 起動時の保存データ警告の確認。`reset=false` は表示中の警告を確認済みにする、`true` は確認済みを解除する（次回起動から再表示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AcknowledgeWarningsArgs {
    pub reset: bool,
}

// ───────────────────────────── ホスト→UIイベント ─────────────────────────────

/// UIへの差分イベント。状態と鮮度は `AgentView` に並置して送り、UIは片方からもう片方を導かない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostEvent {
    SourceUpdated { source: SourceInfo },
    ChatUpdated { chat: Chat },
    ChatRemoved { chat: ChatKey },
    AgentUpdated { view: AgentView },
    TurnUpdated { turn: TurnKey, end: Option<TurnEnd> },
    ActivityUpdated { activity: Activity },
    /// 逐次本文。描画のみに使い、ホスト側の状態判定には使わない。
    ActivityDelta { item: ItemKey, delta: String },
    /// 承認・質問の到着・状態変化。到着は即時（2秒集約の対象外）。
    RequestUpdated { request: PendingRequest },
    SendUpdated { chat: ChatKey, attempt: SendAttempt },
    /// チャット1件分のキュー全体（項目・進行・保留理由）で置き換える。
    ChatQueueUpdated { queue: ChatQueue },
    StopUpdated { record: StopRecord },
    ModelSettingsUpdated { chat: ChatKey, settings: ChatModelSettings },
    /// チャット別の補足情報（ピン・下書き・モデル/権限・保存状態）の更新。
    ChatLocalUpdated { local: ChatLocalView },
    SaveStatusUpdated { status: SaveStatus },
    SettingsUpdated { settings: AppSettings },
    /// 添付の追加・コピーの進行・失敗・欠損の更新（1件分で置き換える）。
    AttachmentUpdated { entry: AttachmentEntry },
    /// 成果物の追加・実在確認の更新（1件分で置き換える）。
    ArtifactUpdated { entry: ArtifactEntry },
    /// 通知を開いた操作（トレイ・通知クリック）。該当チャットを表示するだけで、回答・再実行はしない。
    NavigateToChat { chat: ChatKey },
    /// 完全終了の進行（確認・停止照合・保存・保存失敗）。閉じる操作（トレイ格納）では出ない。
    QuitUpdated { phase: QuitPhase },
    /// sleepからの復帰を検出した。鮮度は要照合になり、送信は保留される。UIは表示中の履歴を取り直す。
    SystemResumed { at: UnixMillis },
    /// 未知イベント・版違い・取得不能項目の警告（M12）。
    Warning { source: Option<SourceId>, message: String, raw_label: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct HostEventEnvelope {
    /// ホスト全体で単調増加。UIは欠番を検出したら `get_snapshot` を呼ぶ。
    #[cfg_attr(test, ts(type = "number"))]
    pub seq: u64,
    pub at: UnixMillis,
    pub event: HostEvent,
}
