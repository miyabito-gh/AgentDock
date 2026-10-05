pub mod backend;
pub mod codex;
pub mod commands;
pub mod host;

use std::sync::Arc;

use tauri::{Emitter, Manager, RunEvent};

use backend::ipc::HOST_EVENT_CHANNEL;
use host::Host;

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
            let host = Arc::new(Host::new(dir));
            let handle = app.handle().clone();
            host.set_emitter(Arc::new(move |env| {
                let _ = handle.emit(HOST_EVENT_CHANNEL, env);
            }));
            host.start_event_pump();
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
