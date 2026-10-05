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
        .on_window_event(|window, event| {
            // 通常画面のフォーカス（通知の抑制判定用）。監視窓は含めない。
            if let tauri::WindowEvent::Focused(focused) = event {
                if window.label() == "main" {
                    if let Some(host) = window.app_handle().try_state::<Arc<Host>>() {
                        host.set_main_focused(*focused);
                    }
                }
            }
        })
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
            // OS通知。クリックは通常画面を表示して該当チャットを開くだけ（回答・再実行はしない）。
            {
                let weak = Arc::downgrade(&host);
                let handle = app.handle().clone();
                host.attach_notifier(Arc::new(move |n| {
                    let weak = weak.clone();
                    let handle = handle.clone();
                    // 表示が遅くてもイベント処理を止めないよう、別スレッドで出す。
                    tauri::async_runtime::spawn_blocking(move || {
                        win::toast::show(&n, move |chat| {
                            if let Some(w) = handle.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                            if let Some(host) = weak.upgrade() {
                                host.navigate_to_chat(chat);
                            }
                        });
                    });
                }));
            }
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
            commands::acknowledge_failure,
            commands::set_selected_chat,
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
