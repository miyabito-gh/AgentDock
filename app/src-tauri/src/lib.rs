pub mod attach;
pub mod backend;
pub mod codex;
pub mod commands;
pub mod diag;
pub mod exec;
pub mod gitops;
pub mod host;
pub mod rules;
pub mod store;
pub mod win;

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent};

use backend::ipc::HOST_EVENT_CHANNEL;
use host::Host;
use store::Store;

/// トレイのアイコンとメニュー。左クリックで通常画面を表示。完全終了は「AgentDockを終了」だけ（窓を閉じても終了しない）。
fn setup_tray(app: &tauri::App, host: Arc<Host>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "AgentDockを開く", true, None::<&str>)?;
    let monitor = MenuItem::with_id(app, "monitor", "監視窓を開く", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "AgentDockを終了", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &monitor, &sep, &quit])?;
    let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("AgentDock")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => win::window::show_main(app),
            "monitor" => {
                if let Err(e) = win::window::open_monitor(app, &host) {
                    diag::log("tray", &format!("open monitor failed: {e}"));
                }
            }
            "quit" => {
                // 作業中なら確認を出すので、通常画面を表示する。作業がなければそのまま保存して終了する。
                // メニューのイベントはtokioランタイム外のスレッドで届くので、Tauriのランタイムで実行する。
                let (host, app) = (host.clone(), app.clone());
                tauri::async_runtime::spawn(async move {
                    if matches!(host.request_quit(), backend::local::QuitPhase::Confirming { .. }) {
                        win::window::show_main(&app);
                    }
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                win::window::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        // 添付：ファイル選択・保存先の指定（dialog）、関連アプリで開く（opener、Rust側から明示操作でだけ）、選択範囲の貼り付け（clipboard）。
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // 2つ目の起動は、既存の通常画面を表示して終わる（重複して Codex を起動しない）。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| win::window::show_main(app)))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .on_window_event(|window, event| match event {
            // 通常画面のフォーカス（通知の抑制判定用）。監視窓は含めない。
            tauri::WindowEvent::Focused(focused) => {
                if window.label() == win::window::MAIN_LABEL {
                    if let Some(host) = window.app_handle().try_state::<Arc<Host>>() {
                        host.set_main_focused(*focused);
                    }
                }
            }
            // 通常画面を閉じる＝トレイへ格納（確認なし。作業・収集・保存は続く）。監視窓は閉じるとその窓だけ破棄される。
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if window.label() == win::window::MAIN_LABEL {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => win::window::note_bounds(window),
            _ => {}
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
                            win::window::show_main(&handle);
                            if let Some(host) = weak.upgrade() {
                                host.navigate_to_chat(chat);
                            }
                        });
                    });
                }));
            }
            // 完全終了（停止と保存を確認した後だけ呼ばれる）。
            {
                let handle = app.handle().clone();
                host.attach_exit(Arc::new(move || handle.exit(0)));
            }
            // setupはtokioランタイム外のスレッドで走るため、Tauriのランタイムに入ってからtokio::spawnする。
            tauri::async_runtime::block_on(async {
                host.start_event_pump();
                // sleep/wake の検出（復帰では通知・再送・resumeをしない。鮮度を要照合にして読み直すだけ）。
                host.start_power_watch();
            });
            app.manage(host.clone());
            // 通常画面の位置を復元して表示する。自動起動（--autostart）なら通常画面は開かず、トレイ格納で起動する。
            let start_hidden = std::env::args().any(|a| a == "--autostart");
            win::window::restore_main(app.handle(), &host, start_hidden);
            setup_tray(app, host).map_err(|e| format!("tray: {e}"))?;
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
            commands::acknowledge_warnings,
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
            commands::recheck_queue_state,
            commands::send_queue_entry_now,
            commands::reconcile_send,
            commands::set_chat_permission,
            commands::set_chat_cwd,
            commands::acknowledge_failure,
            commands::set_selected_chat,
            commands::add_attachment_file,
            commands::add_attachment_image_bytes,
            commands::remove_attachment,
            commands::open_file,
            commands::save_file_as,
            commands::read_file_preview,
            commands::request_quit,
            commands::quit_decision,
            commands::preview_force_kill,
            commands::force_kill,
            commands::set_always_on_top,
            commands::open_monitor_window,
            commands::set_monitor_window_scope,
            commands::show_main_window,
            commands::touch_chat_used,
            commands::preview_delete,
            commands::delete_chat,
            commands::archive_chat,
            commands::unarchive_chat,
            commands::export_markdown,
            commands::get_usage,
            commands::get_change_list,
            commands::get_baseline_status,
            commands::delete_baselines,
            commands::get_file_diff,
            commands::preview_revert,
            commands::revert_changes,
            commands::set_work_mode,
            commands::list_work_modes,
            commands::get_goal,
            commands::set_goal,
            commands::clear_goal,
            commands::get_backend_status,
            commands::get_memory_status,
            commands::set_memory_mode,
            commands::reset_memory,
            commands::get_review_choices,
            commands::start_review,
            commands::fork_chat,
            commands::reconcile_fork,
            commands::resend_as_fork,
            commands::recheck_unknown_agents,
            commands::compact_chat,
            commands::reconcile_op,
            commands::list_compaction_snapshots,
            commands::read_compaction_snapshot,
            commands::git_info,
            commands::create_worktree,
            commands::preview_remove_worktree,
            commands::remove_worktree,
            commands::list_reference_turns,
            commands::build_reference,
            commands::list_skills,
            commands::add_skill_attachment,
            commands::get_instruction_files,
            commands::create_instruction_template,
            commands::open_instruction_file,
            commands::list_tool_servers,
            commands::login_tool_server,
            commands::reload_tool_servers,
            commands::open_authorization_url,
            commands::list_extensions,
            commands::manage_extension,
            commands::list_extension_ops,
            commands::cloud_env_hint,
            commands::submit_cloud_task,
            commands::list_cloud_tasks,
            commands::cloud_task_status,
            commands::cloud_task_diff,
            commands::apply_cloud_task,
            commands::open_side,
            commands::send_side,
            commands::close_side,
            commands::read_side,
            commands::handoff_side,
            commands::pick_codex_executable,
            commands::pick_save_file,
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
