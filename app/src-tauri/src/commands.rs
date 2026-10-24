//! Tauriコマンド層（`backend::ipc` の契約の実装）。ユーザー操作の起点なので、ここでだけ `UserConfirmed` を発行する。
//! 引数はTS側 `invoke(name, { args })` に合わせて `args` 1個で受ける。業務判断は `Host` に置き、ここは薄く保つ。

use std::sync::Arc;

use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::attach;
use crate::backend::backend::{ManageOutcome, Page, ResumeOutcome, RespondOutcome, UserConfirmed, ChatSummary, AgentHistory};
use crate::backend::changes::*;
use crate::backend::ipc::*;
use crate::backend::local::{
    AddAttachmentFileArgs, AttachmentArgs, AttachmentEntry, OpenFileArgs, SaveFileAsArgs,
    AcknowledgeFailureArgs, AppSettings, ChatArgs, ChatLocalView, ChatQueue, DeleteOutcome, DeletePreview, ExportMarkdownArgs, GetUsageArgs, UsageReport, EditQueueEntryArgs, EnqueueArgs, ForceKillArgs, ForceKillPreview, QueueEntry, QueueEntryArgs, QuitDecisionArgs, QuitPhase,
    GetWorkFoldersArgs, CheckWorkFolderArgs, SetNextWorkFoldersArgs,
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
pub async fn list_projects(host: Hs<'_>, args: ListProjectsArgs) -> R<ProjectsOutcome> {
    host.list_projects(args).await
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
pub async fn set_next_work_folders(host: Hs<'_>, args: SetNextWorkFoldersArgs) -> R<SettingsImpact> {
    host.inner().clone().set_next_work_folders(args).await
}

#[tauri::command]
pub async fn get_work_folders(host: Hs<'_>, args: GetWorkFoldersArgs) -> R<WorkFoldersView> {
    host.inner().clone().get_work_folders(args).await
}

#[tauri::command]
pub async fn check_work_folder(host: Hs<'_>, args: CheckWorkFolderArgs) -> R<FolderCheck> {
    host.check_work_folder(args).await
}

#[tauri::command]
pub async fn set_chat_cwd(host: Hs<'_>, args: SetChatCwdArgs) -> R<SettingsImpact> {
    host.inner().clone().set_chat_cwd(args).await
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

/// 元のチャットの添付（台帳のID）を別のチャットへ新しいコピーとして付ける。元の台帳にあり使える状態のものだけ（任意のパスは受けない）。
#[tauri::command]
pub async fn carry_attachment(host: Hs<'_>, args: CarryAttachmentArgs) -> R<AttachmentEntry> {
    let h = host.inner().clone();
    match h.carry_attachments(&args.from, &args.to, std::slice::from_ref(&args.attachment)).await {
        Ok(mut v) if v.len() == 1 => Ok(v.remove(0)),
        Ok(_) => Err(IpcError { code: IpcErrorCode::Io, message: "添付を引き継げませんでした".into(), blocked: None }),
        Err(m) => Err(IpcError { code: IpcErrorCode::InvalidArgs, message: m, blocked: None }),
    }
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

/// 会話内のパスの実在・種類・絶対／相対パスの確認（読取りのメタデータだけ。内容は読まない）。
#[tauri::command]
pub async fn stat_paths(host: Hs<'_>, args: crate::pathref::StatPathsArgs) -> R<Vec<crate::pathref::PathInfo>> {
    Ok(host.inner().clone().stat_paths(args).await)
}

/// 会話内のパスを関連アプリで開く（ユーザー操作のときだけ）。作業フォルダ外・実行形式は確認が要る。
#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, host: Hs<'_>, args: crate::pathref::OpenPathArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.inner().clone().open_path_target(&args).await?;
    if args.target != crate::external_tools::OpenTarget::System {
        let settings = host.get_app_settings().external_tools;
        return crate::external_tools::launch(&settings, args.target, &path, args.line)
            .map_err(|m| IpcError { code: IpcErrorCode::InvalidArgs, message: m, blocked: None });
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })
}

/// 会話内のパスをエクスプローラーで表示する（ユーザー操作のときだけ）。
#[tauri::command]
pub async fn reveal_path(app: tauri::AppHandle, host: Hs<'_>, args: crate::pathref::RevealPathArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.inner().clone().reveal_path_target(&args).await?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("表示できませんでした: {e}"), blocked: None })
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

/// 監視窓のトグル（開いていれば閉じ、閉じていれば開く。開くときはフォーカスを奪わない）。操作後に開いているかを返す。
#[tauri::command]
pub async fn toggle_monitor_window(app: tauri::AppHandle, host: Hs<'_>) -> R<bool> {
    crate::win::window::toggle_monitor(&app, host.inner()).map_err(|e| io_err(format!("監視窓を切り替えられませんでした: {e}")))
}

/// 監視窓が開いている（表示中・最小化でない）か。
#[tauri::command]
pub async fn monitor_window_open(app: tauri::AppHandle) -> R<bool> {
    Ok(crate::win::window::monitor_is_open(&app))
}

/// 監視窓を閉じる（`monitor` ラベルの窓だけ。作業・収集・保存は続く）。
#[tauri::command]
pub async fn close_monitor_window(app: tauri::AppHandle) -> R<()> {
    crate::win::window::close_monitor(&app).map_err(|e| io_err(format!("監視窓を閉じられませんでした: {e}")))
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

/// 左一覧の「依頼整理の会話をまとめて削除」の1件分（確認画面を経たユーザー操作だけ）。ホストが対象かどうかを最終判定し、対象外は削除しない。
#[tauri::command]
pub async fn intake_delete_chat(host: Hs<'_>, args: ChatArgs) -> R<DeleteOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().intake_delete_chat(args, &confirmed).await
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

/// 作業フォルダの「参照…」（フォルダ選択）。選ぶだけで、検査（check_work_folder）も設定の変更もしない。キャンセルなら None。
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle) -> R<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title("作業フォルダを選択").blocking_pick_folder())
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

#[tauri::command]
pub async fn get_change_list(host: Hs<'_>, args: GetChangeListArgs) -> R<ChangeList> {
    host.inner().clone().get_change_list(args).await
}

/// 変更の控えの状況（区間ごとの状態。読取りのみ）。
#[tauri::command]
pub async fn get_baseline_status(host: Hs<'_>, args: ChatArgs) -> R<Vec<crate::backend::baseline::SegmentStatus>> {
    host.inner().clone().get_baseline_status(args).await
}

/// このチャットの控えを削除する（確認画面を経たユーザー操作だけ。作業中は削除しない）。
#[tauri::command]
pub async fn delete_baselines(host: Hs<'_>, args: ChatArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().delete_baselines(args, &confirmed).await
}

/// 所有者違いのリポジトリの選択を保存する（記録する／しない／AgentDockのgit操作にだけ許可。ユーザーのgit設定は変えない）。
#[tauri::command]
pub async fn set_repo_baseline(host: Hs<'_>, args: SetRepoBaselineArgs) -> R<crate::backend::local::BaselineSettings> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().set_repo_baseline(args, &confirmed)
}

#[tauri::command]
pub async fn get_file_diff(host: Hs<'_>, args: GetFileDiffArgs) -> R<UnifiedDiff> {
    host.inner().clone().get_file_diff(args).await
}

#[tauri::command]
pub async fn preview_revert(host: Hs<'_>, args: PreviewRevertArgs) -> R<RevertPlan> {
    host.inner().clone().preview_revert(args).await
}

#[tauri::command]
pub async fn set_purpose_note(host: Hs<'_>, args: SetPurposeNoteArgs) -> R<crate::backend::local::ConversationView> {
    host.inner().clone().set_purpose_note(args)
}

/// 引き継ぎの事前表示（読取りのみ）。
#[tauri::command]
pub async fn preview_handoff(host: Hs<'_>, args: ChatArgs) -> R<HandoffPreview> {
    host.inner().clone().preview_handoff(args).await
}

/// 以前の thread の履歴（読取りのみ。resumeしない）。
#[tauri::command]
pub async fn read_segment_history(host: Hs<'_>, args: ReadSegmentHistoryArgs) -> R<AgentHistory> {
    host.inner().clone().read_segment_history(args).await
}

#[tauri::command]
pub async fn set_work_mode(host: Hs<'_>, args: SetWorkModeArgs) -> R<SettingsImpact> {
    host.set_work_mode(args)
}

#[tauri::command]
pub async fn list_work_modes(host: Hs<'_>) -> R<Vec<crate::backend::parity::WorkModeInfo>> {
    host.list_work_modes().await
}

/// 応答した承認・質問・MCP許可の記録（読取りのみ。AgentDockが保存したものだけ）。
#[tauri::command]
pub async fn list_decision_records(host: Hs<'_>, args: ChatArgs) -> R<Vec<crate::backend::decision::DecisionRecord>> {
    host.inner().clone().list_decision_records(args).await
}

/// 目標の取得（読取りのみ。resumeしない）。
#[tauri::command]
pub async fn get_goal(host: Hs<'_>, args: ChatArgs) -> R<Known<crate::backend::parity::Goal>> {
    host.get_goal(args).await
}

#[tauri::command]
pub async fn set_goal(host: Hs<'_>, args: SetGoalArgs) -> R<OpAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().set_goal(args, &confirmed).await
}

#[tauri::command]
pub async fn clear_goal(host: Hs<'_>, args: ChatArgs) -> R<OpAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().clear_goal(args, &confirmed).await
}

#[tauri::command]
pub async fn get_backend_status(host: Hs<'_>) -> R<crate::backend::parity::BackendStatus> {
    host.get_backend_status().await
}

#[tauri::command]
pub async fn get_memory_status(host: Hs<'_>) -> R<crate::backend::parity::MemoryStatus> {
    host.get_memory_status().await
}

#[tauri::command]
pub async fn set_memory_mode(host: Hs<'_>, args: SetMemoryModeArgs) -> R<OpAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().set_memory_mode(args, &confirmed).await
}

/// memoriesのリセット。影響を表示して確認した後のユーザー操作だけが呼ぶ（`args.confirmed` が偽なら拒否）。
#[tauri::command]
pub async fn reset_memory(host: Hs<'_>, args: ResetMemoryArgs) -> R<ResetMemoryResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().reset_memory(args, &confirmed).await
}

/// レビュー対象の候補（読取りのみ。Gitのリポジトリでなければ理由つきで空）。
#[tauri::command]
pub async fn get_review_choices(host: Hs<'_>, args: ChatArgs) -> R<ReviewChoices> {
    host.inner().clone().get_review_choices(args).await
}

/// コードレビュー。確認画面の後のユーザー操作だけが呼ぶ。受付の結果だけを返す（結果は会話の記録の観測で示す）。
#[tauri::command]
pub async fn start_review(host: Hs<'_>, args: StartReviewArgs) -> R<ReviewOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().start_review(args, &confirmed).await
}

/// 会話の分岐。確認画面の後のユーザー操作だけが呼ぶ。
#[tauri::command]
pub async fn fork_chat(host: Hs<'_>, args: ForkChatArgs) -> R<ForkResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().fork_chat(args, &confirmed).await
}

/// 受理不明の分岐の照合（読取りのみ。再送しない）。
#[tauri::command]
pub async fn reconcile_fork(host: Hs<'_>, args: ReconcileForkArgs) -> R<ForkReconcile> {
    // 採用した分岐先へ「（編集）」「（再生成）」の名前を付けるため（ユーザーが「分岐を確認」を押した操作の一部）。
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().reconcile_fork(args, &confirmed).await
}

/// 編集して再送・再生成（M50）。ダイアログの確認後のユーザー操作だけが呼ぶ。分岐→下書き保存→（求められたときだけ）1回送る。
#[tauri::command]
pub async fn resend_as_fork(host: Hs<'_>, args: ResendAsForkArgs) -> R<ResendAsForkResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().resend_as_fork(args, &confirmed).await
}

/// 継続チャット: 「引き継いで送信」（DESIGN_CONT §6.3）。利用者の操作だけが呼ぶ。新しい thread を作り、資料つきで1回だけ送る。
/// 引き継ぎ元の thread は resume・送信・アーカイブ・削除しない。受理不明・拒否は再送しない。
#[tauri::command]
pub async fn handoff_and_send(host: Hs<'_>, args: HandoffAndSendArgs) -> R<HandoffResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().handoff_and_send(args, &confirmed).await
}

/// 状態不明の子孫の「履歴で再確認」（読取りのみ。resume・送信・停止をしない）。
#[tauri::command]
pub async fn recheck_unknown_agents(host: Hs<'_>, args: RecheckAgentsArgs) -> R<RecheckAgentsResult> {
    host.inner().clone().recheck_unknown_agents(args).await
}

/// 文脈の圧縮。確認画面の後のユーザー操作だけが呼ぶ（圧縮前の控えを保存してから送る）。
#[tauri::command]
pub async fn compact_chat(host: Hs<'_>, args: ChatArgs) -> R<CompactChatResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().compact_chat(args, &confirmed).await
}

/// 受理不明の操作（レビュー・圧縮）の照合（読取りのみ。再送しない）。
#[tauri::command]
pub async fn reconcile_op(host: Hs<'_>, args: ReconcileOpArgs) -> R<OpReconcile> {
    host.inner().clone().reconcile_op(args).await
}

#[tauri::command]
pub async fn list_compaction_snapshots(host: Hs<'_>, args: ChatArgs) -> R<Vec<UnixMillis>> {
    host.inner().clone().list_compaction_snapshots(args).await
}

#[tauri::command]
pub async fn read_compaction_snapshot(host: Hs<'_>, args: ReadCompactionArgs) -> R<CompactionSnapshot> {
    host.inner().clone().read_compaction_snapshot(args).await
}

/// 変更を戻す。確認画面の後のユーザー操作だけが呼ぶ（`UserConfirmed` はここでだけ発行する）。
#[tauri::command]
pub async fn revert_changes(host: Hs<'_>, args: RevertChangesArgs) -> R<RevertResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().revert_changes(args, &confirmed).await
}

/// フォルダのGit情報（読取りのみ。リポジトリでない・Gitなしは理由つき）。
#[tauri::command]
pub async fn git_info(host: Hs<'_>, args: crate::backend::worktree::GitInfoArgs) -> R<crate::backend::worktree::GitInfo> {
    host.inner().clone().git_info(args).await
}

/// worktreeの作成。確認画面の後のユーザー操作だけが呼ぶ（`state` が `ready` のときだけ作成成功）。
#[tauri::command]
pub async fn create_worktree(host: Hs<'_>, args: crate::backend::worktree::CreateWorktreeArgs) -> R<WorktreeRecord> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().create_worktree(args, &confirmed).await
}

/// worktree削除の確認内容（読取りのみ）。
#[tauri::command]
pub async fn preview_remove_worktree(host: Hs<'_>, args: crate::backend::worktree::WorktreeIdArgs) -> R<crate::backend::worktree::WorktreeRemovePreview> {
    host.inner().clone().preview_remove_worktree(args).await
}

/// worktreeの削除。確認画面の後のユーザー操作だけが呼ぶ（`force` は2段目の確認の後だけ）。
#[tauri::command]
pub async fn remove_worktree(host: Hs<'_>, args: crate::backend::worktree::RemoveWorktreeArgs) -> R<crate::backend::worktree::WorktreeRemoveOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().remove_worktree(args, &confirmed).await
}

// ───────────── 参考指定・Skills・指示ファイル・side相談（P3-4） ─────────────

/// 参考にできる過去のturnの一覧（読取りのみ。resumeしない）。
#[tauri::command]
pub async fn list_reference_turns(host: Hs<'_>, args: ChatArgs) -> R<ReferenceTurns> {
    host.inner().clone().list_reference_turns(args).await
}

/// 選んだturnの抜粋（読取りのみ。入力欄へ入れるのはUI。自動送信・要約はしない）。
#[tauri::command]
pub async fn build_reference(host: Hs<'_>, args: BuildReferenceArgs) -> R<ReferenceBlock> {
    host.inner().clone().build_reference(args).await
}

/// 作業フォルダのSkill一覧（読取りのみ）。
#[tauri::command]
pub async fn list_skills(host: Hs<'_>, args: ListSkillsArgs) -> R<crate::backend::parity::SkillList> {
    host.inner().clone().list_skills(args).await
}

/// Skill定義の冒頭の読取り（プレビュー。一覧にあるパスだけ）。
#[tauri::command]
pub async fn read_skill_preview(host: Hs<'_>, args: SkillPreviewArgs) -> R<SkillPreview> {
    host.inner().clone().read_skill_preview(args).await
}

/// Skillを入力欄の指定（チップ）にする（コピーしない。送信時に明示呼出しになる）。
#[tauri::command]
pub async fn add_skill_attachment(host: Hs<'_>, args: AddSkillArgs) -> R<AttachmentEntry> {
    host.inner().clone().add_skill_attachment(args).await
}

/// 指示ファイル（AGENTS.md等）の確認（読取りのみ）。
#[tauri::command]
pub async fn get_instruction_files(host: Hs<'_>, args: ChatArgs) -> R<InstructionFiles> {
    host.inner().clone().get_instruction_files(args).await
}

/// 指示ファイルを関連アプリで開く（ユーザー操作のときだけ。その会話の指示ファイルとして確認できたパスだけ）。
#[tauri::command]
pub async fn open_instruction_file(app: tauri::AppHandle, host: Hs<'_>, args: OpenInstructionFileArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.inner().clone().resolve_instruction_file(&args).await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })
}

/// AGENTS.md の雛形の作成。確認画面の後のユーザー操作だけが呼ぶ（新規作成のみ。既存があれば `alreadyExists` で何も書かない）。
#[tauri::command]
pub async fn create_instruction_template(host: Hs<'_>, args: ChatArgs) -> R<InstructionTemplateResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().create_instruction_template(args, &confirmed).await
}

// ───────────── MCP・Plugins（P3-5） ─────────────

/// ツールサーバー（MCP）の一覧（読取りのみ）。表示は「いま」の状態で、既存会話への反映ではない。
#[tauri::command]
pub async fn list_tool_servers(host: Hs<'_>) -> R<ToolServerList> {
    host.inner().clone().list_tool_servers().await
}

/// ツールサーバーの認可を始め、認可URLを返す（開くのはUIの明示クリックだけ。URLは記録しない）。
#[tauri::command]
pub async fn login_tool_server(host: Hs<'_>, args: ToolServerNameArgs) -> R<ToolServerLoginStart> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().login_tool_server(args, &confirmed).await
}

/// 設定の再読込みの要求（ボタンのときだけ。既存会話への反映は確認できない）。
#[tauri::command]
pub async fn reload_tool_servers(host: Hs<'_>) -> R<OpAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().reload_tool_servers(&confirmed).await
}

/// http:// か https:// の直後に、空白・`/`・`?`・`#` でない1文字以上のホストが続くものだけ開いてよい（先頭空白も不可）。
fn is_openable_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    rest.and_then(|r| r.chars().next()).is_some_and(|c| !c.is_whitespace() && !matches!(c, '/' | '?' | '#'))
}

#[cfg(test)]
mod open_url_tests {
    use super::is_openable_http_url;
    #[test]
    fn only_plain_http_urls_with_host() {
        for u in ["https://example.com/a", "http://localhost:3000", "HTTPS://Example.com"] { assert!(is_openable_http_url(u), "{u}"); }
        for u in ["https:///x", "http:a.com", " https://a.com", "javascript:alert(1)", "https://", "http://?x", "https:// a.com", "file:///C:/a"] { assert!(!is_openable_http_url(u), "{u}"); }
    }
}

/// 認可URLを既定のブラウザで開く（UIの明示クリックのときだけ。http・httpsのURLだけ）。
#[tauri::command]
pub async fn open_authorization_url(app: tauri::AppHandle, args: crate::backend::ipc::OpenUrlArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let url = args.url;
    if !is_openable_http_url(&url) {
        return Err(IpcError { code: IpcErrorCode::InvalidArgs, message: "http・https以外のURLは開きません".into(), blocked: None });
    }
    app.opener().open_url(url, None::<&str>).map_err(|_| IpcError { code: IpcErrorCode::Io, message: "ブラウザで開けませんでした".into(), blocked: None })
}

/// 拡張（Plugins）の一覧（読取りのみ）。
#[tauri::command]
pub async fn list_extensions(host: Hs<'_>) -> R<Vec<crate::backend::parity::ExtensionView>> {
    host.inner().clone().list_extensions().await
}

/// 管理操作（導入・削除・サーバーの追加・削除）。確認画面の後のユーザー操作だけが呼ぶ。
#[tauri::command]
pub async fn manage_extension(host: Hs<'_>, args: ManageExtensionArgs) -> R<ExtensionOpRecord> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().manage_extension(args, &confirmed).await
}

/// 管理操作の記録（新しい順。終了の記録がないものは結果未確認）。
#[tauri::command]
pub async fn list_extension_ops(host: Hs<'_>) -> R<Vec<ExtensionOpRecord>> {
    host.inner().clone().list_extension_ops().await
}

/// 目的メモの更新案の作成前の見積り（読取りのみ。何も作らず・送らない）。
#[tauri::command]
pub async fn preview_purpose_draft(host: Hs<'_>, args: crate::backend::draft::StartPurposeDraftArgs) -> R<crate::backend::draft::PurposeDraftEstimate> {
    host.inner().clone().preview_purpose_draft(args).await
}

/// 目的メモの更新案を作る。ユーザーが押したときの1回だけ（一時 thread を新しく開始する。自動で呼ばない・再送しない）。
#[tauri::command]
pub async fn start_purpose_draft(host: Hs<'_>, args: crate::backend::draft::StartPurposeDraftArgs) -> R<crate::backend::draft::PurposeDraftView> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().start_purpose_draft(args, &confirmed).await
}

/// 目的メモの更新案の最後の結果（なければ null）。
#[tauri::command]
pub async fn read_purpose_draft(host: Hs<'_>, args: ChatArgs) -> R<Option<crate::backend::draft::PurposeDraftView>> {
    host.inner().clone().read_purpose_draft(args).await
}

/// 更新案の作成を中止する（一時 thread の中断）。
#[tauri::command]
pub async fn cancel_purpose_draft(host: Hs<'_>, args: ChatArgs) -> R<crate::backend::draft::PurposeDraftView> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().cancel_purpose_draft(args, &confirmed).await
}

/// 更新案の結果を破棄する（目的メモは変わらない）。
#[tauri::command]
pub async fn discard_purpose_draft(host: Hs<'_>, args: ChatArgs) -> R<()> {
    host.inner().clone().discard_purpose_draft(args).await
}

/// side相談を開く（読取り専用の一時の分岐）。ユーザー操作だけが呼ぶ。
#[tauri::command]
pub async fn open_side(host: Hs<'_>, args: ChatArgs) -> R<OpenSideResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().open_side(args, &confirmed).await
}

/// sideへの送信（再送しない）。
#[tauri::command]
pub async fn send_side(host: Hs<'_>, args: SendSideArgs) -> R<OpAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().send_side(args, &confirmed).await
}

/// side相談を閉じる。実行中は `interrupt` を確認したうえでだけ閉じる。
#[tauri::command]
pub async fn close_side(host: Hs<'_>, args: CloseSideArgs) -> R<SideSessionMeta> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().close_side(args, &confirmed).await
}

/// side相談の記録（確定した発言。閉じた後は読み取り専用の記録）。
#[tauri::command]
pub async fn read_side(host: Hs<'_>, args: SideIdArgs) -> R<SideTranscript> {
    host.inner().clone().read_side(args).await
}

/// 選んだ発言を、主会話の入力欄へ入れる引用ブロックにする（自動送信しない）。
#[tauri::command]
pub async fn handoff_side(host: Hs<'_>, args: HandoffSideArgs) -> R<HandoffText> {
    host.inner().clone().handoff_side(args).await
}

/// 作業フォルダに対する委任の既定値（記憶した環境ID・現在のブランチ。読取りのみ）。
#[tauri::command]
pub async fn cloud_env_hint(host: Hs<'_>, args: crate::backend::cloud::CloudEnvArgs) -> R<crate::backend::cloud::CloudEnvHint> {
    host.inner().clone().cloud_env_hint(args).await
}

/// クラウドへ委任する。確認画面の後のユーザー操作だけが呼ぶ（送る前に記録を保存し、受理不明でも再送しない）。
#[tauri::command]
pub async fn submit_cloud_task(host: Hs<'_>, args: crate::backend::cloud::CloudSubmitArgs) -> R<CloudTaskRecord> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().submit_cloud_task(args, &confirmed).await
}

/// クラウド側のタスク一覧（読取り。開いたときとユーザーの「更新」だけ）。
#[tauri::command]
pub async fn list_cloud_tasks(host: Hs<'_>) -> R<crate::backend::parity::CloudTaskList> {
    host.inner().clone().list_cloud_tasks().await
}

/// タスク1件の状態（読取り。原文の先頭）。
#[tauri::command]
pub async fn cloud_task_status(host: Hs<'_>, args: crate::backend::cloud::CloudTaskIdArgs) -> R<crate::backend::parity::CloudCommandOutput> {
    host.inner().clone().cloud_task_status(args).await
}

/// タスクの差分（読取り。表示するだけでローカルは変えない）。
#[tauri::command]
pub async fn cloud_task_diff(host: Hs<'_>, args: crate::backend::cloud::CloudTaskIdArgs) -> R<crate::backend::parity::CloudCommandOutput> {
    host.inner().clone().cloud_task_diff(args).await
}

/// 差分の取込み。作業フォルダのファイルを変えるため、確認画面の後のユーザー操作だけが呼ぶ。
#[tauri::command]
pub async fn apply_cloud_task(host: Hs<'_>, args: crate::backend::cloud::CloudApplyArgs) -> R<crate::backend::cloud::CloudApplyResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().apply_cloud_task(args, &confirmed).await
}

// ───────────────────────────── 組織実行（P-ORG-7a） ─────────────────────────────
// 閲覧（org_view・org_roles_view・org_recover_candidates）は読取りだけで、何も送らない。
// `UserConfirmed` を作るのはこの層だけ。承認要求への応答・resume・中断はここにない。

/// 現在の組織実行の写し（読取りのみ）。
#[tauri::command]
pub async fn org_view(host: Hs<'_>) -> R<crate::backend::org::OrgSnapshotView> {
    Ok(host.org_view())
}

/// 役割定義の読込み結果（読取りのみ）。
#[tauri::command]
pub async fn org_roles_view(host: Hs<'_>) -> R<crate::backend::org::OrgRolesView> {
    Ok(host.org_roles_view())
}

/// グラフ定義（テンプレート）の読込み結果（読取りのみ）。
#[tauri::command]
pub async fn org_graphs_view(host: Hs<'_>) -> R<crate::backend::org::OrgGraphsView> {
    Ok(host.org_graphs_view())
}

/// グラフ定義を読み直す（動いているワークフローがあれば拒否。何も送らない）。
#[tauri::command]
pub async fn org_reload_graphs(host: Hs<'_>) -> R<crate::backend::org::OrgGraphsView> {
    host.org_reload_graphs_view()
}

/// グラフ定義ファイルを関連アプリで開く（利用者の操作のときだけ。グラフ定義フォルダ内のファイル名だけを受ける）。
#[tauri::command]
pub async fn org_open_graph_file(app: tauri::AppHandle, host: Hs<'_>, args: crate::backend::org::OrgOpenGraphFileArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.org_graph_file_path(&args)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })
}

/// 依頼整理チャットを作る（普通のチャット。組織実行はまだ始まらない）。
#[tauri::command]
pub async fn org_intake_begin(host: Hs<'_>, args: crate::backend::intake::IntakeBeginArgs) -> R<crate::backend::intake::IntakeBeginResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_intake_begin(args, &confirmed).await
}

/// 既存の自分のチャットを依頼整理にする（目的メモは変えない）。
#[tauri::command]
pub async fn org_intake_attach(host: Hs<'_>, args: crate::backend::intake::IntakeChatArgs) -> R<crate::backend::intake::IntakeView> {
    host.inner().clone().org_intake_attach(args).await
}

/// 依頼整理の写し（読取りのみ）。
#[tauri::command]
pub async fn org_intake_view(host: Hs<'_>, args: crate::backend::intake::IntakeChatArgs) -> R<crate::backend::intake::IntakeView> {
    host.inner().clone().org_intake_view(args).await
}

/// 「要約を作る」（押した回数だけ1回送る。再送しない）。
#[tauri::command]
pub async fn org_intake_summarize(host: Hs<'_>, args: crate::backend::intake::IntakeChatArgs) -> R<SendAttempt> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_intake_summarize(args, &confirmed).await
}

/// 「履歴から読み直す」（履歴の読取りだけ）。
#[tauri::command]
pub async fn org_intake_reload_summary(host: Hs<'_>, args: crate::backend::intake::IntakeChatArgs) -> R<crate::backend::intake::IntakeView> {
    host.inner().clone().org_intake_reload_summary(args).await
}

/// 要約・開始時の設定の保存（手で書く・編集）。
#[tauri::command]
pub async fn org_intake_save(host: Hs<'_>, args: crate::backend::intake::IntakeSaveArgs) -> R<crate::backend::intake::IntakeView> {
    host.inner().clone().org_intake_save(args).await
}

/// 依頼整理の確定版で組織実行を開始する（確認ダイアログの「開始」だけ）。
#[tauri::command]
pub async fn org_intake_start(host: Hs<'_>, args: crate::backend::intake::IntakeStartArgs) -> R<crate::backend::org::StartOrgResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_intake_start(args, confirmed).await
}

/// 依頼整理をやめる（チャットは消さない）。
#[tauri::command]
pub async fn org_intake_abandon(host: Hs<'_>, args: crate::backend::intake::IntakeChatArgs) -> R<crate::backend::intake::IntakeView> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_intake_abandon(args, &confirmed).await
}

/// 作業フォルダのGitの状態（読取りのみ）。
#[tauri::command]
pub async fn git_repo_status(host: Hs<'_>, args: crate::backend::git_ops::GitFolderArgs) -> R<crate::backend::git_ops::RepoStatusView> {
    host.git_repo_status(args).await
}

/// 作業フォルダの未コミットの変更の件数と代表パス（読取りのみ）。
#[tauri::command]
pub async fn git_dirty_summary(host: Hs<'_>, args: crate::backend::git_ops::GitFolderArgs) -> R<crate::backend::git_ops::DirtySummaryView> {
    host.git_dirty_summary(args).await
}

/// Gitの準備のプレビュー（読取りのみ。何も作らない）。
#[tauri::command]
pub async fn git_prep_preview(host: Hs<'_>, args: crate::backend::git_ops::GitPrepArgs) -> R<crate::backend::git_ops::RepoPrepPreview> {
    host.git_prep_preview(args).await
}

/// Gitの準備の実行（確認ダイアログの「Gitを準備する」だけ。`.git` フォルダだけを作る）。
#[tauri::command]
pub async fn git_prepare_repo(host: Hs<'_>, args: crate::backend::git_ops::GitPrepareArgs) -> R<crate::backend::git_ops::RepoPrepResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.git_prepare_repo(args, &confirmed).await
}

/// 前回の途中の準備（印のある `.git` でコミットなし）を取り除く（確認ダイアログの後だけ）。
#[tauri::command]
pub async fn git_prep_remove_interrupted(host: Hs<'_>, args: crate::backend::git_ops::GitFolderArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.git_prep_remove_interrupted(args, &confirmed).await
}

/// プッシュ前の確認（読取りのみ。ネットワークは使わない）。
#[tauri::command]
pub async fn git_push_preview(host: Hs<'_>, args: crate::backend::git_ops::PushPreviewArgs) -> R<crate::backend::git_ops::PushPreview> {
    host.git_push_preview(args).await
}

/// プッシュの実行（確認ダイアログの「プッシュする」だけ。確認時のコミットだけを送る。強制しない）。
#[tauri::command]
pub async fn git_push(host: Hs<'_>, args: crate::backend::git_ops::GitPushArgs) -> R<crate::backend::git_ops::PushResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.git_push(args, &confirmed).await
}

/// 結果が分からなかったプッシュの確認（`ls-remote`。読取りのみ）。
#[tauri::command]
pub async fn git_push_verify(host: Hs<'_>, args: crate::backend::git_ops::GitPushVerifyArgs) -> R<crate::backend::git_ops::PushVerify> {
    host.git_push_verify(args).await
}

/// 開始（利用者の「開始」の操作だけ。送信権を作る）。
#[tauri::command]
pub async fn org_start(host: Hs<'_>, args: crate::backend::org::StartOrgArgs) -> R<crate::backend::org::StartOrgResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_start_checked(args, confirmed).await
}

/// 再開（必要なチャットだけ resume し、新しい送信権を作る）。利用者の「再開」の操作だけ。
#[tauri::command]
pub async fn org_resume(host: Hs<'_>, args: crate::backend::org::OrgResumeArgs) -> R<crate::backend::org::OrgResumeResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_resume_checked(args, confirmed).await
}

/// 一時停止（送信権の失効だけ）。
#[tauri::command]
pub async fn org_pause(host: Hs<'_>, args: crate::backend::org::OrgWorkflowArgs) -> R<()> {
    host.org_pause_checked(args)
}

#[tauri::command]
pub async fn org_approve_plan(host: Hs<'_>, args: crate::backend::org::OrgWorkflowArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_approve_plan(args, &confirmed)
}

#[tauri::command]
pub async fn org_replan(host: Hs<'_>, args: crate::backend::org::OrgReplanArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_replan(args, &confirmed)
}

/// gate の判断（利用者の操作だけ。判断を待っている gate だけを受ける）。
#[tauri::command]
pub async fn org_gate_decide(host: Hs<'_>, args: crate::backend::org::OrgGateDecideArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_gate_decide(args, &confirmed)
}

/// 周回の上限で止まっている工程の「理由を書いてもう1周」（利用者の操作だけ）。
#[tauri::command]
pub async fn org_extra_round(host: Hs<'_>, args: crate::backend::org::OrgExtraRoundArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_extra_round(args, &confirmed)
}

/// 周回の上限で止まっている工程の「この結果で受け入れる」（利用者の操作だけ。何も送らない）。
#[tauri::command]
pub async fn org_accept_round(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_accept_round(args, &confirmed)
}

/// 進行タイムライン（読取りのみ。何も送らない）。
#[tauri::command]
pub async fn org_timeline(host: Hs<'_>, args: crate::backend::org::OrgTimelineArgs) -> R<crate::backend::org::OrgTimelineView> {
    host.org_timeline(args)
}

/// 割り込み指示（利用者の操作だけ。実行中の turn に届けられるときだけ turn/steer、そうでなければ次の送信に添える。新しい turn は始めない）。
#[tauri::command]
pub async fn org_interject(host: Hs<'_>, args: crate::backend::org::OrgInterjectArgs) -> R<crate::backend::org::OrgInterjectAck> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_interject(args, &confirmed)
}

/// 届いたか確認できない割り込みを、次の送信に添えるか破棄するか（利用者の操作だけ）。
#[tauri::command]
pub async fn org_interject_fallback(host: Hs<'_>, args: crate::backend::org::OrgInterjectFallbackArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_interject_fallback(args, &confirmed)
}

/// ワークフロー（またはタスク）への追加指示（利用者の操作だけ。次にエンジンが送る turn に添える）。
#[tauri::command]
pub async fn org_add_note(host: Hs<'_>, args: crate::backend::org::OrgAddNoteArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_add_note(args, &confirmed)
}

/// 追加指示の取り消し（利用者の操作だけ）。
#[tauri::command]
pub async fn org_withdraw_note(host: Hs<'_>, args: crate::backend::org::OrgWithdrawNoteArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_withdraw_note(args, &confirmed)
}

/// タスクのスキップ（確認を経た利用者の操作だけ）。
#[tauri::command]
pub async fn org_skip_task(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_skip_task(args, &confirmed)
}

/// 利用者の差し戻し（指摘つき。利用者の操作だけ）。
#[tauri::command]
pub async fn org_send_back(host: Hs<'_>, args: crate::backend::org::OrgSendBackArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_send_back(args, &confirmed)
}

/// 差し戻し上限で止まったタスクを、このまま受け入れる（利用者の操作だけ。何も送らない）。
#[tauri::command]
pub async fn org_accept_as_is(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_accept_as_is(args, &confirmed)
}

/// 保留／保留の解除（利用者の操作だけ）。
#[tauri::command]
pub async fn org_hold_task(host: Hs<'_>, args: crate::backend::org::OrgHoldArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_hold_task(args, &confirmed)
}

/// 成果物パネルの中身（読取りのみ。作業フォルダ・ブランチ・worktreeを変えない）。
#[tauri::command]
pub async fn org_artifacts(host: Hs<'_>, args: crate::backend::org_artifacts::OrgArtifactsArgs) -> R<crate::backend::org_artifacts::OrgArtifactsView> {
    host.inner().clone().org_artifacts(args).await
}

/// 成果物の1ファイルの差分（読取りのみ）。
#[tauri::command]
pub async fn org_artifact_diff(host: Hs<'_>, args: crate::backend::org_artifacts::OrgArtifactPathArgs) -> R<crate::backend::org_artifacts::OrgArtifactDiff> {
    host.inner().clone().org_artifact_diff(args).await
}

/// 成果物の1ファイルのプレビュー（読取りのみ）。
#[tauri::command]
pub async fn org_artifact_preview(host: Hs<'_>, args: crate::backend::org_artifacts::OrgArtifactPathArgs) -> R<crate::backend::org_artifacts::OrgArtifactPreview> {
    host.inner().clone().org_artifact_preview(args).await
}

/// 成果物を関連アプリで開く（利用者の操作のときだけ）。読取り専用のコピーを AgentDock の領域へ書き出して開く。実行形式は確認が要る。
#[tauri::command]
pub async fn org_artifact_open(app: tauri::AppHandle, host: Hs<'_>, args: crate::backend::org_artifacts::OrgArtifactOpenArgs) -> R<crate::backend::org_artifacts::OrgArtifactOpenResult> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.inner().clone().org_artifact_open_target(args).await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })?;
    Ok(crate::backend::org_artifacts::OrgArtifactOpenResult { path: path.to_string_lossy().into_owned() })
}

/// 取り込みの確認の材料（作業フォルダには何も書かない）。
#[tauri::command]
pub async fn org_import_preview(host: Hs<'_>, args: crate::backend::org_artifacts::OrgImportPreviewArgs) -> R<crate::backend::org_artifacts::OrgImportPreview> {
    host.inner().clone().org_import_preview(args).await
}

/// 取り込みの実行（確認ダイアログの「取り込む」だけ。確認から変わっていれば何も書かない）。
#[tauri::command]
pub async fn org_import(host: Hs<'_>, args: crate::backend::org_artifacts::OrgImportArgs) -> R<crate::backend::org_artifacts::OrgImportResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_import(args, &confirmed).await
}

/// 結果が確認できなかった取り込みの確認（読取りのみ。再実行しない）。
#[tauri::command]
pub async fn org_import_verify(host: Hs<'_>, args: crate::backend::org_artifacts::OrgImportRecordArgs) -> R<crate::backend::org_artifacts::OrgImportResult> {
    host.inner().clone().org_import_verify(args).await
}

/// 取り込み前に戻せるかの確認（読取りのみ）。
#[tauri::command]
pub async fn org_import_undo_preview(host: Hs<'_>, args: crate::backend::org_artifacts::OrgImportRecordArgs) -> R<crate::backend::org_artifacts::OrgUndoPreview> {
    host.inner().clone().org_import_undo_preview(args).await
}

/// 取り込み前に戻す（確認ダイアログの後だけ。送信済みなどでは何もしない）。
#[tauri::command]
pub async fn org_import_undo(host: Hs<'_>, args: crate::backend::org_artifacts::OrgUndoArgs) -> R<crate::backend::org_artifacts::OrgUndoResult> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_import_undo(args, &confirmed).await
}

/// 削除確認の内容（読取りのみ。何も消さない）。
#[tauri::command]
pub async fn org_preview_delete(host: Hs<'_>, args: crate::backend::org::OrgWorkflowArgs) -> R<crate::backend::org::OrgDeletePreview> {
    host.inner().clone().org_preview_delete(args).await
}

/// 終わったワークフローの削除（利用者が確認画面で選んだ後だけ。worktree・ブランチは消さない）。
#[tauri::command]
pub async fn org_delete_workflow(host: Hs<'_>, args: crate::backend::org::OrgDeleteArgs) -> R<crate::backend::org::OrgDeleteOutcome> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_delete_workflow(args, &confirmed).await
}

/// 終わったワークフローのアーカイブ（アプリ内だけの印。Codex へは送らない）。
#[tauri::command]
pub async fn org_archive_workflow(host: Hs<'_>, args: crate::backend::org::OrgWorkflowArgs) -> R<()> {
    host.inner().clone().org_archive_workflow(args).await
}

/// アーカイブの解除（印を消すだけ）。
#[tauri::command]
pub async fn org_unarchive_workflow(host: Hs<'_>, args: crate::backend::org::OrgWorkflowArgs) -> R<()> {
    host.inner().clone().org_unarchive_workflow(args).await
}

/// 取消し（利用者の操作だけが cancelled にする）。取り消したタスクのIDを返す。
#[tauri::command]
pub async fn org_cancel(host: Hs<'_>, args: crate::backend::org::OrgCancelArgs) -> R<Vec<String>> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_cancel(args, &confirmed)
}

#[tauri::command]
pub async fn org_accept_manual(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_accept_manual(args, &confirmed)
}

/// 止まったタスクのやり直し（送っていないことが確かなものだけ。新しい試行で、自動の再送ではない）。
#[tauri::command]
pub async fn org_retry_task(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_retry_task(args, &confirmed)
}

/// 止まったタスクを失敗として確定する（利用者の操作だけ）。
#[tauri::command]
pub async fn org_fail_task(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_fail_task(args, &confirmed)
}

/// 手動に切り替えたチャットの最新 turn の読取り（取込みの確認用。何も送らない）。
#[tauri::command]
pub async fn org_manual_result(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<crate::backend::org::OrgManualResultView> {
    host.inner().clone().org_manual_result(args).await
}

/// 手動に切り替えたチャットの、確認した turn の最終回答を結果として取り込む（利用者の操作だけ。チャットへは何も送らない）。
#[tauri::command]
pub async fn org_adopt_manual(host: Hs<'_>, args: crate::backend::org::OrgAdoptManualArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_adopt_manual(args, &confirmed).await
}

#[tauri::command]
pub async fn org_raise_turn_limit(host: Hs<'_>, args: crate::backend::org::OrgRaiseTurnLimitArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_raise_turn_limit(args, &confirmed)
}

/// 組織チャットを手動に切り替える（保存できたときだけ）。
#[tauri::command]
pub async fn org_release_chat(host: Hs<'_>, args: ChatArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_release_chat_checked(&args.chat, &confirmed).await
}

/// 作成結果が不明なタスクの結び付け先の候補（読取りのみ。自動では結び付けない）。
#[tauri::command]
pub async fn org_recover_candidates(host: Hs<'_>, args: crate::backend::org::OrgTaskArgs) -> R<Vec<crate::backend::org::OrgChatCandidate>> {
    host.inner().clone().org_recover_candidates(args).await
}

/// 利用者が選んだチャットをタスクに結び付ける（送信・resume はしない）。
#[tauri::command]
pub async fn org_choose_chat(host: Hs<'_>, args: crate::backend::org::OrgChooseChatArgs) -> R<()> {
    let confirmed = UserConfirmed::from_user_command();
    host.inner().clone().org_choose_chat(args, &confirmed).await
}

/// 停滞とみなす時間（分）の設定。
#[tauri::command]
pub async fn org_set_stall_minutes(host: Hs<'_>, args: crate::backend::org::OrgSetStallArgs) -> R<u32> {
    host.inner().clone().org_set_stall_minutes(args)
}

/// 役割定義を読み直す（動いているワークフローがあれば拒否）。
#[tauri::command]
pub async fn org_reload_roles(host: Hs<'_>) -> R<crate::backend::org::OrgRolesView> {
    host.org_reload_roles_view()
}

/// 権限を広げる役割を有効にする（確認画面の指紋が今の権限と一致するときだけ）。
#[tauri::command]
pub async fn org_enable_role(host: Hs<'_>, args: crate::backend::org::OrgEnableRoleArgs) -> R<crate::backend::org::OrgRolesView> {
    let confirmed = UserConfirmed::from_user_command();
    host.org_enable_role_checked(args, confirmed)
}

/// 役割定義ファイルを関連アプリで開く（利用者の操作のときだけ。役割フォルダ内のファイル名だけを受ける）。
#[tauri::command]
pub async fn org_open_role_file(app: tauri::AppHandle, host: Hs<'_>, args: crate::backend::org::OrgOpenRoleFileArgs) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = host.org_role_file_path(&args)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| IpcError { code: IpcErrorCode::Io, message: format!("開けませんでした: {e}"), blocked: None })
}

// Parent + backend-native subagent runs. Mutating actions originate only here.
#[tauri::command]
pub async fn subagent_start(host: Hs<'_>, args: crate::backend::subagents::SubagentStartArgs) -> R<crate::backend::subagents::SubagentRunView> {
    host.inner().clone().subagent_start(args, &UserConfirmed::from_user_command()).await
}
#[tauri::command]
pub async fn subagent_runs(host: Hs<'_>) -> R<Vec<crate::backend::subagents::SubagentRunView>> { host.inner().clone().subagent_runs().await }
#[tauri::command]
pub async fn subagent_recover(host: Hs<'_>, args: crate::backend::subagents::SubagentRecoverArgs) -> R<crate::backend::subagents::SubagentRunView> {
    host.inner().clone().subagent_recover(args, &UserConfirmed::from_user_command()).await
}
#[tauri::command]
pub async fn subagent_import_preview(host: Hs<'_>, args: crate::backend::subagents::SubagentRunArgs) -> R<crate::backend::subagents::SubagentImportPreview> {
    host.inner().clone().subagent_import_preview(args).await
}
#[tauri::command]
pub async fn subagent_import(host: Hs<'_>, args: crate::backend::subagents::SubagentImportArgs) -> R<crate::backend::org_artifacts::OrgImportOutcome> {
    host.inner().clone().subagent_import(args, &UserConfirmed::from_user_command()).await
}
#[tauri::command]
pub async fn subagent_verify_import(host: Hs<'_>, args: crate::backend::subagents::SubagentRunArgs) -> R<Vec<crate::backend::subagents::SubagentImportRecord>> {
    host.inner().clone().subagent_verify_import(args).await
}
