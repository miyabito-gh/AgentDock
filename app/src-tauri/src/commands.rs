//! Tauriコマンド層（`backend::ipc` の契約の実装）。ユーザー操作の起点なので、ここでだけ `UserConfirmed` を発行する。
//! 引数はTS側 `invoke(name, { args })` に合わせて `args` 1個で受ける。業務判断は `Host` に置き、ここは薄く保つ。

use std::sync::Arc;

use tauri::State;

use crate::backend::backend::{ManageOutcome, Page, ResumeOutcome, RespondOutcome, UserConfirmed, ChatSummary, AgentHistory};
use crate::backend::ipc::*;
use crate::backend::local::{AcknowledgeFailureArgs, AppSettings, ChatLocalView, RetrySaveArgs, SaveStatus, SetAppSettingsArgs, SetDraftArgs, SetSelectedChatArgs};
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
