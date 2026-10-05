pub mod attach;
pub mod backend;
pub mod codex;
pub mod commands;
pub mod diag;
pub mod host;
pub mod rules;
pub mod store;
pub mod win;

use std::sync::Arc;

use tauri::{Emitter, Manager, RunEvent};

use backend::ipc::HOST_EVENT_CHANNEL;
use host::Host;
use store::Store;

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            // 専用領域は %LOCALAPPDATA%\com.agentdock.app（診断ログも diag\ へ）。段階①でRoamingに作った領域は移動しない。
            let dir = app.path().app_local_data_dir().map_err(|e| format!("app local data dir: {e}"))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
            diag::init(&dir);
            // 保存層を開いて復元する（ピン・モデル設定・下書き・設定。復元だけで送信・再開はしない）。
            let host = Arc::new(match Store::open(dir.clone()) {
                Ok(store) => Host::with_store(dir, Arc::new(store)),
                Err(e) => {
                    diag::log("store", &format!("open failed: {e}"));
                    let host = Host::new(dir);
                    host.add_startup_warning(format!("保存領域を開けませんでした（{e}）。この起動中の変更は保存されません。"));
                    host
                }
            });
            let handle = app.handle().clone();
            host.set_emitter(Arc::new(move |env| {
                let _ = handle.emit(HOST_EVENT_CHANNEL, env);
            }));
            // setupはtokioランタイム外のスレッドで走るため、Tauriのランタイムに入ってからtokio::spawnする。
            tauri::async_runtime::block_on(async { host.start_event_pump() });
            app.manage(host);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::connect_backend,
            commands::list_chats,
            commands::open_chat,
            commands::start_chat,
            commands::send_message,
            commands::retry_send,
            commands::respond_request,
            commands::interrupt_chat,
            commands::resume_chat,
            commands::manage_chat,
            commands::set_pinned,
            commands::list_models,
            commands::set_chat_model,
            commands::set_monitor_scope,
            commands::open_diag_dir,
            commands::get_app_settings,
            commands::set_app_settings,
            commands::get_chat_locals,
            commands::retry_save,
            commands::set_draft,
            commands::enqueue,
            commands::edit_queue_entry,
            commands::cancel_queue_entry,
            commands::resume_queue,
            commands::reconcile_send,
            commands::set_chat_permission,
            commands::set_chat_cwd,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // 接続を閉じる（実行中作業の停止確認ではない）。
            if let Some(host) = handle.try_state::<Arc<Host>>() {
                let host = host.inner().clone();
                tauri::async_runtime::block_on(async move { host.shutdown().await });
            }
        }
    });
}
