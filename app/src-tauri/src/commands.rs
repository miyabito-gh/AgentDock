//! Tauriコマンド層（`backend::ipc` の契約の実装）。ユーザー操作の起点なので、ここでだけ `UserConfirmed` を発行する。
//! 引数はTS側 `invoke(name, { args })` に合わせて `args` 1個で受ける。業務判断は `Host` に置き、ここは薄く保つ。

use std::sync::Arc;

use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::attach;
use crate::backend::backend::{ManageOutcome, Page, ResumeOutcome, RespondOutcome, UserConfirmed, ChatSummary, AgentHistory};
use crate::backend::ipc::*;
use crate::backend::local::{
    AddAttachmentFileArgs, AttachmentArgs, AttachmentEntry, OpenFileArgs, SaveFileAsArgs,
    AcknowledgeFailureArgs, AppSettings, ChatArgs, ChatLocalView, ChatQueue, DeleteOutcome, DeletePreview, ExportMarkdownArgs, GetUsageArgs, UsageReport, EditQueueEntryArgs, EnqueueArgs, ForceKillArgs, ForceKillPreview, QueueEntry, QueueEntryArgs, QuitDecisionArgs, QuitPhase,
    ReconcileSendArgs, RetrySaveArgs, SaveStatus, SetAlwaysOnTopArgs, SetAppSettingsArgs, SetChatCwdArgs, SetChatPermissionArgs, SetDraftArgs, SetMonitorWindowScopeArgs, SetSelectedChatArgs,
    SettingsImpact, ShowMainWindowArgs,
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
    host.inner().clone().manage_chat(args, confirmed).await
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
pub async fn acknowledge_warnings(host: Hs<'_>, args: AcknowledgeWarningsArgs) -> R<AppSettings> {
    host.inner().clone().acknowledge_warnings(args)
}

#[tauri::command]
pub async fn get_app_settings(host: Hs<'_>) -> R<AppSettings> {
    Ok(host.get_app_settings())
}

#[tauri::command]
pub async fn set_app_settings(app: tauri::AppHandle, host: Hs<'_>, args: SetAppSettingsArgs) -> R<AppSettings> {
    // Windowsログイン時の自動起動（初期値オフ。オンなら通常画面を開かずトレイ格納で起動する）。変更できなければ設定も保存しない。
    if args.settings.autostart != host.get_app_settings().autostart {
        let launch = app.autolaunch();
        let res = if args.settings.autostart { launch.enable() } else { launch.disable() };
        // 無効化は「もともと登録がない」場合に失敗することがある。結果として登録がなければ成功とみなす。
        if let Err(e) = res {
            if args.settings.autostart || launch.is_enabled().unwrap_or(true) {
                return Err(io_err(format!("自動起動の設定を変更できませんでした: {e}")));
            }
        }
    }
    host.inner().clone().set_app_settings(args)
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

/// 「状態を再確認」。保留中の子孫の履歴を読み直す（読み取りのみ。resumeしない）。
#[tauri::command]
pub async fn recheck_queue_state(host: Hs<'_>, args: ChatArgs) -> R<ChatQueue> {
    host.inner().clone().recheck_queue_state(args).await
}

/// 「確認して今すぐ送る」。UIの確認ダイアログで承認された操作なので、ここで証票を作る。先頭の1件だけを送る。
#[tauri::command]
pub async fn send_queue_entry_now(host: Hs<'_>, args: QueueEntryArgs) -> R<ChatQueue> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().send_queue_entry_now(args, &confirmed).await
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
    attach::check_image_size(bytes.len()).map_err(|m| bad(&m))?;
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
    host.check_open_risk(&args.target, &path, args.risk_confirmed)?;
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

// ───────────────────────────── 窓・常駐・終了（P6） ─────────────────────────────

fn io_err(message: String) -> IpcError {
    IpcError { code: IpcErrorCode::Io, message, blocked: None }
}

/// 完全終了の要求。作業中なら確認が出る（`quitUpdated`）。閉じる操作（トレイ格納）は別で、確認しない。
#[tauri::command]
pub async fn request_quit(host: Hs<'_>) -> R<QuitPhase> {
    Ok(host.inner().clone().request_quit())
}

#[tauri::command]
pub async fn quit_decision(host: Hs<'_>, args: QuitDecisionArgs) -> R<QuitPhase> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().quit_decision(args, &confirmed).await
}

#[tauri::command]
pub async fn preview_force_kill(host: Hs<'_>, args: ForceKillArgs) -> R<ForceKillPreview> {
    host.preview_force_kill(args)
}

/// 強制終了。確認画面を経たユーザー操作だけが呼ぶ。
#[tauri::command]
pub async fn force_kill(host: Hs<'_>, args: ForceKillArgs) -> R<Vec<StopRecord>> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().force_kill(args, &confirmed).await
}

/// 最前面のオン・オフ（窓ごと）。フォーカスは移さない。設定は保存される。
#[tauri::command]
pub async fn set_always_on_top(app: tauri::AppHandle, host: Hs<'_>, args: SetAlwaysOnTopArgs) -> R<()> {
    if let Some(w) = app.get_webview_window(crate::win::window::label_of(args.window)) {
        w.set_always_on_top(args.on).map_err(|e| io_err(format!("最前面を切り替えられませんでした: {e}")))?;
    }
    host.inner().clone().set_always_on_top_pref(args.window, args.on)
}

#[tauri::command]
pub async fn open_monitor_window(app: tauri::AppHandle, host: Hs<'_>) -> R<()> {
    crate::win::window::open_monitor(&app, host.inner()).map_err(|e| io_err(format!("監視窓を開けませんでした: {e}")))
}

#[tauri::command]
pub async fn set_monitor_window_scope(host: Hs<'_>, args: SetMonitorWindowScopeArgs) -> R<()> {
    host.inner().clone().set_monitor_window_scope(args.scope)
}

/// 通常画面を前面に出す。`chat` があればそのチャットを開く（表示だけで、回答・再実行はしない）。
#[tauri::command]
pub async fn show_main_window(app: tauri::AppHandle, host: Hs<'_>, args: ShowMainWindowArgs) -> R<()> {
    crate::win::window::show_main(&app);
    if let Some(chat) = args.chat {
        host.navigate_to_chat(chat);
    }
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

// ───────────────────────────── 削除・アーカイブ・エクスポート・使用量（P7） ─────────────────────────────

/// ユーザーがチャットを開いた操作で、最近利用時刻を更新する（一覧の並び用。背景の読み込みでは呼ばない）。
#[tauri::command]
pub async fn touch_chat_used(host: Hs<'_>, args: ChatArgs) -> R<()> {
    host.inner().clone().touch_chat_used(&args.chat);
    Ok(())
}

/// 削除確認に出す内容（実行しない）。
#[tauri::command]
pub async fn preview_delete(host: Hs<'_>, args: ChatArgs) -> R<DeletePreview> {
    host.inner().clone().preview_delete(args).await
}

/// 削除（確認画面を経たユーザー操作だけ）。停止を確認できなければ保留、部分失敗は完了と偽らない。
#[tauri::command]
pub async fn delete_chat(host: Hs<'_>, args: ChatArgs) -> R<DeleteOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().delete_chat(args, &confirmed).await
}

/// アーカイブ（アプリの一覧からは即座に隠す。Codexへの反映は作業終了・停止確認の後）。
#[tauri::command]
pub async fn archive_chat(host: Hs<'_>, args: ChatArgs) -> R<ChatLocalView> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().archive_chat(args, confirmed).await
}

#[tauri::command]
pub async fn unarchive_chat(host: Hs<'_>, args: ChatArgs) -> R<ChatLocalView> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().unarchive_chat(args, &confirmed).await
}

/// Markdownエクスポート。保存先は `pick_save_file` で選んだパス（上書きはOSの確認ダイアログで済み）。
#[tauri::command]
pub async fn export_markdown(host: Hs<'_>, args: ExportMarkdownArgs) -> R<()> {
    host.inner().clone().export_markdown(args).await
}

#[tauri::command]
pub async fn get_usage(host: Hs<'_>, args: GetUsageArgs) -> R<UsageReport> {
    host.inner().clone().get_usage(args).await
}

fn picked_path(p: Option<tauri_plugin_dialog::FilePath>) -> R<Option<String>> {
    match p {
        None => Ok(None),
        Some(fp) => fp.into_path().map(|p| Some(p.to_string_lossy().into_owned())).map_err(|e| io_err(format!("選択したパスを取得できませんでした: {e}"))),
    }
}

/// codex.exe の「参照…」。選ぶだけで、設定は変えない。キャンセルなら None。
#[tauri::command]
pub async fn pick_codex_executable(app: tauri::AppHandle) -> R<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("codex.exe を選択").add_filter("実行ファイル", &["exe"]).blocking_pick_file()
    })
    .await
    .map_err(|e| io_err(format!("ダイアログが異常終了しました: {e}")))?;
    picked_path(picked)
}

/// 保存先の選択（同名ファイルがあればOSが上書きを確認する）。キャンセルなら None。
#[tauri::command]
pub async fn pick_save_file(app: tauri::AppHandle, args: PickSaveFileArgs) -> R<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("保存先を選択").set_file_name(args.default_name).add_filter("Markdown", &["md"]).blocking_save_file()
    })
    .await
    .map_err(|e| io_err(format!("ダイアログが異常終了しました: {e}")))?;
    picked_path(picked)
}
