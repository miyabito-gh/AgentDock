//! Tauriコマンド層（`backend::ipc` の契約の実装）。ユーザー操作の起点なので、ここでだけ `UserConfirmed` を発行する。
//! 引数はTS側 `invoke(name, { args })` に合わせて `args` 1個で受ける。業務判断は `Host` に置き、ここは薄く保つ。

use std::sync::Arc;

use tauri::State;

use crate::backend::backend::{ManageOutcome, Page, ResumeOutcome, RespondOutcome, UserConfirmed, ChatSummary, AgentHistory};
use crate::backend::ipc::*;
use crate::backend::local::{
    AcknowledgeFailureArgs, AddAttachmentFileArgs, AppSettings, AttachmentArgs, AttachmentEntry, OpenFileArgs, SaveFileAsArgs, ChatArgs, ChatLocalView, ChatQueue, EditQueueEntryArgs, EnqueueArgs, QueueEntry, QueueEntryArgs, ReconcileSendArgs, RetrySaveArgs,
    SaveStatus, SetAppSettingsArgs, SetChatCwdArgs, SetChatPermissionArgs, SetDraftArgs, SetSelectedChatArgs, SettingsImpact,
};
use crate::backend::model::*;
use crate::host::Host;

type Hs<'a> = State<'a, Arc<Host>>;
type R<T> = Result<T, IpcError>;

#[tauri::command]
pub async fn get_snapshot(host: Hs<'_>) -> R<HostSnapshot> {
    Ok(host.snapshot())
}

#[tauri::command]
pub async fn connect_backend(host: Hs<'_>, args: ConnectBackendArgs) -> R<SourceInfo> {
    host.inner().clone().connect(args).await
}

#[tauri::command]
pub async fn list_chats(host: Hs<'_>, args: ListChatsArgs) -> R<Page<ChatSummary>> {
    host.list_chats(args).await
}

#[tauri::command]
pub async fn open_chat(host: Hs<'_>, args: OpenChatArgs) -> R<AgentHistory> {
    host.inner().clone().open_chat(args).await
}

#[tauri::command]
pub async fn start_chat(host: Hs<'_>, args: StartChatArgs) -> R<StartChatResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().start_chat(args, &confirmed).await
}

#[tauri::command]
pub async fn send_message(host: Hs<'_>, args: SendMessageArgs) -> R<SendAttempt> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().send_message(args, &confirmed).await
}

#[tauri::command]
pub async fn retry_send(host: Hs<'_>, args: RetrySendArgs) -> R<SendAttempt> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().retry_send(args, confirmed).await
}

#[tauri::command]
pub async fn respond_request(host: Hs<'_>, args: RespondRequestArgs) -> R<RespondOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.respond_request(args, &confirmed).await
}

#[tauri::command]
pub async fn interrupt_chat(host: Hs<'_>, args: InterruptChatArgs) -> R<InterruptChatResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().interrupt_chat(args, &confirmed).await
}

#[tauri::command]
pub async fn resume_chat(host: Hs<'_>, args: ResumeChatArgs) -> R<ResumeOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().resume_chat(args, &confirmed).await
}

#[tauri::command]
pub async fn manage_chat(host: Hs<'_>, args: ManageChatArgs) -> R<ManageOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.manage_chat(args, &confirmed).await
}

#[tauri::command]
pub async fn set_pinned(host: Hs<'_>, args: SetPinnedArgs) -> R<Chat> {
    host.set_pinned(args)
}

#[tauri::command]
pub async fn list_models(host: Hs<'_>, args: ListModelsArgs) -> R<Vec<ModelInfo>> {
    host.list_models(args).await
}

#[tauri::command]
pub async fn set_chat_model(host: Hs<'_>, args: SetChatModelArgs) -> R<ChatModelSettings> {
    host.set_chat_model(args)
}

#[tauri::command]
pub async fn set_monitor_scope(host: Hs<'_>, args: SetMonitorScopeArgs) -> R<()> {
    host.set_monitor_scope(args);
    Ok(())
}

#[tauri::command]
pub async fn get_app_settings(host: Hs<'_>) -> R<AppSettings> {
    Ok(host.get_app_settings())
}

#[tauri::command]
pub async fn set_app_settings(host: Hs<'_>, args: SetAppSettingsArgs) -> R<AppSettings> {
    host.set_app_settings(args)
}

#[tauri::command]
pub async fn get_chat_locals(host: Hs<'_>) -> R<Vec<ChatLocalView>> {
    Ok(host.get_chat_locals())
}

#[tauri::command]
pub async fn retry_save(host: Hs<'_>, args: RetrySaveArgs) -> R<SaveStatus> {
    host.retry_save(args).await
}

#[tauri::command]
pub async fn set_draft(host: Hs<'_>, args: SetDraftArgs) -> R<()> {
    host.set_draft(args)
}

#[tauri::command]
pub async fn enqueue(host: Hs<'_>, args: EnqueueArgs) -> R<QueueEntry> {
    host.inner().clone().enqueue(args)
}

#[tauri::command]
pub async fn edit_queue_entry(host: Hs<'_>, args: EditQueueEntryArgs) -> R<QueueEntry> {
    host.inner().clone().edit_queue_entry(args)
}

#[tauri::command]
pub async fn cancel_queue_entry(host: Hs<'_>, args: QueueEntryArgs) -> R<()> {
    host.inner().clone().cancel_queue_entry(args)
}

/// 「キューを再開」。ユーザー操作なのでここで証票を作る（親がliveでなければ再開も行う）。
#[tauri::command]
pub async fn resume_queue(host: Hs<'_>, args: ChatArgs) -> R<ChatQueue> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().resume_queue(args, &confirmed).await
}

/// 「履歴と照合」。読み取りのみで、再送しない。
#[tauri::command]
pub async fn reconcile_send(host: Hs<'_>, args: ReconcileSendArgs) -> R<SendAttempt> {
    host.inner().clone().reconcile_send(args).await
}

#[tauri::command]
pub async fn set_chat_permission(host: Hs<'_>, args: SetChatPermissionArgs) -> R<SettingsImpact> {
    host.inner().clone().set_chat_permission(args)
}

#[tauri::command]
pub async fn set_chat_cwd(host: Hs<'_>, args: SetChatCwdArgs) -> R<SettingsImpact> {
    host.inner().clone().set_chat_cwd(args)
}

/// 「確認済み」（印を外すだけ。再実行・成功化・キュー再開はしない）。
#[tauri::command]
pub async fn acknowledge_failure(host: Hs<'_>, args: AcknowledgeFailureArgs) -> R<()> {
    host.inner().clone().acknowledge_failure(args)
}

/// 選択中のチャット（通知の抑制判定だけに使う）。
#[tauri::command]
pub async fn set_selected_chat(host: Hs<'_>, args: SetSelectedChatArgs) -> R<()> {
    host.set_selected_chat(args.chat);
    Ok(())
}

/// ファイル選択・ドロップで得たパスを添付する（チャット領域へコピーする。元ファイルは変更しない）。
#[tauri::command]
pub async fn add_attachment_file(host: Hs<'_>, args: AddAttachmentFileArgs) -> R<AttachmentEntry> {
    host.inner().clone().add_attachment_file(args).await
}

/// クリップボード画像。本文は生バイト（PNG）、チャットと名前はヘッダー（`x-chat-backend`・`x-chat-id`・`x-file-name`）で受ける。
#[tauri::command]
pub async fn add_attachment_image_bytes(host: Hs<'_>, request: tauri::ipc::Request<'_>) -> R<AttachmentEntry> {
    let bad = |m: &str| IpcError { code: IpcErrorCode::InvalidArgs, message: m.to_string(), blocked: None };
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else { return Err(bad("画像のデータを受け取れませんでした")) };
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let backend: BackendKind = header("x-chat-backend")
        .and_then(|b| serde_json::from_value(serde_json::Value::String(b)).ok())
        .ok_or_else(|| bad("チャットの種別を指定してください"))?;
    let id = header("x-chat-id").filter(|i| !i.is_empty()).ok_or_else(|| bad("チャットを指定してください"))?;
    let name = header("x-file-name").filter(|n| !n.is_empty()).unwrap_or_else(|| "clipboard.png".to_string());
    host.inner().clone().add_attachment_image_bytes(ChatKey { backend, id: ExternalId(id) }, name, bytes.clone()).await
}

/// 送信前の添付を取り外す（使っていないコピーだけ消す）。
#[tauri::command]
pub async fn remove_attachment(host: Hs<'_>, args: AttachmentArgs) -> R<()> {
    host.inner().clone().remove_attachment(args).await
}

/// 関連アプリで開く（ユーザー操作のときだけ。生成完了では呼ばない）。実在を確認し、なければ欠損にして止める。
#[tauri::command]
pub async fn open_file(app: tauri::AppHandle, host: Hs<'_>, args: OpenFileArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.inner().clone().resolve_file(&args.target).await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })
}

/// 名前を付けて保存。同名があれば `TargetExists` で止まる（UIで確認したら `overwriteConfirmed` で再実行）。
#[tauri::command]
pub async fn save_file_as(host: Hs<'_>, args: SaveFileAsArgs) -> R<()> {
    host.inner().clone().save_file_as(args).await
}

/// 画像のアプリ内プレビュー用バイト列（画像だけ。大きいものは返さない）。
#[tauri::command]
pub async fn read_file_preview(host: Hs<'_>, args: OpenFileArgs) -> R<tauri::ipc::Response> {
    let bytes = host.inner().clone().read_image_preview(&args.target).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 診断ログのフォルダをエクスプローラーで開く（ユーザー操作）。戻り値はログファイルの場所。
#[tauri::command]
pub async fn open_diag_dir() -> R<String> {
    let path = crate::diag::log_path().ok_or_else(|| IpcError { code: IpcErrorCode::NotFound, message: "診断ログを作成できていません".into(), blocked: None })?;
    if let Some(dir) = path.parent() {
        std::process::Command::new("explorer")
            .arg(dir)
            .spawn()
            .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("エクスプローラーを起動できません: {e}"), blocked: None })?;
    }
    Ok(path.to_string_lossy().into_owned())
}
