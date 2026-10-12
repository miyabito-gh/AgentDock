//! 窓の操作（P6、§3.11・M37）: 通常画面の表示、監視窓、位置・サイズの復元と記録。
//!
//! - 位置の画面外補正は `rules::window::fit_to_monitors`（`tauri-plugin-window-state` は使わない）。保存は `windows.json`（ホスト側）。
//! - 位置は外枠の左上、サイズは内側（クライアント領域）。どちらも物理ピクセル。
//! - 最前面はフォーカスを移さない（`set_always_on_top` はアクティブ化しない）。

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window};

use crate::backend::local::{WindowBounds, WindowKind};
use crate::host::Host;
use crate::rules::window::{fit_to_monitors, WorkArea};

pub const MAIN_LABEL: &str = "main";
pub const MONITOR_LABEL: &str = "monitor";
/// 監視窓の開閉通知（payload: bool）。UI側の名前は `ipc/client.ts` の `MONITOR_WINDOW_EVENT` と同じ。
pub const MONITOR_WINDOW_EVENT: &str = "agentdock://monitor-window";
const MAIN_MIN: (u32, u32) = (960, 600);
const MONITOR_MIN: (u32, u32) = (300, 260);

pub fn kind_of(label: &str) -> Option<WindowKind> {
    match label {
        MAIN_LABEL => Some(WindowKind::Main),
        MONITOR_LABEL => Some(WindowKind::Monitor),
        _ => None,
    }
}

pub fn label_of(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Main => MAIN_LABEL,
        WindowKind::Monitor => MONITOR_LABEL,
    }
}

fn min_of(kind: WindowKind) -> (u32, u32) {
    match kind {
        WindowKind::Main => MAIN_MIN,
        WindowKind::Monitor => MONITOR_MIN,
    }
}

/// 各モニターの作業領域（物理ピクセル）。主モニターを先頭にする。
fn work_areas(app: &AppHandle) -> Vec<WorkArea> {
    let primary = app.primary_monitor().ok().flatten();
    let mut monitors = app.available_monitors().unwrap_or_default();
    if let Some(p) = &primary {
        monitors.sort_by_key(|m| !(m.position() == p.position() && m.size() == p.size()));
    }
    monitors
        .iter()
        .map(|m| {
            let r = m.work_area();
            WorkArea { x: r.position.x, y: r.position.y, width: r.size.width, height: r.size.height }
        })
        .collect()
}

/// 保存した位置・サイズを窓へ適用する（画面外なら見える位置へ補正）。
fn apply_bounds(app: &AppHandle, window: &WebviewWindow, kind: WindowKind, saved: WindowBounds) {
    let b = fit_to_monitors(saved, &work_areas(app), min_of(kind));
    let _ = window.set_size(PhysicalSize::new(b.width, b.height));
    let _ = window.set_position(PhysicalPosition::new(b.x, b.y));
    if b.maximized {
        let _ = window.maximize();
    }
}

/// 通常画面を表示して前面に出す（トレイ・通知クリック・2つ目の起動・監視窓の「通常画面で開く」）。
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 起動時に通常画面の位置・最前面を復元する。`start_hidden`（自動起動）ならトレイ格納で起動し、通常画面は開かない。
pub fn restore_main(app: &AppHandle, host: &Arc<Host>, start_hidden: bool) {
    let Some(w) = app.get_webview_window(MAIN_LABEL) else { return };
    if let Some(saved) = host.saved_bounds(WindowKind::Main) {
        apply_bounds(app, &w, WindowKind::Main, saved);
    }
    if host.get_app_settings().main_window.always_on_top {
        let _ = w.set_always_on_top(true);
    }
    if !start_hidden {
        let _ = w.show();
    }
}

/// 監視窓を開く（なければ作る）。フォーカスは奪わない。閉じても通常画面と作業には影響しない。
pub fn open_monitor(app: &AppHandle, host: &Arc<Host>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(MONITOR_LABEL) {
        let _ = w.unminimize();
        w.show().map_err(|e| e.to_string())?;
        emit_monitor_state(app, true);
        return Ok(());
    }
    let prefs = host.get_app_settings().monitor_window;
    let w = WebviewWindowBuilder::new(app, MONITOR_LABEL, WebviewUrl::App("index.html?view=monitor".into()))
        .title("AgentDock 監視窓")
        .inner_size(380.0, 560.0)
        .min_inner_size(300.0, 260.0)
        .focused(false)
        .always_on_top(prefs.always_on_top)
        .build()
        .map_err(|e| e.to_string())?;
    if let Some(saved) = host.saved_bounds(WindowKind::Monitor) {
        apply_bounds(app, &w, WindowKind::Monitor, saved);
    }
    emit_monitor_state(app, true);
    Ok(())
}

/// 監視窓のトグルで何をするか。表示中（最小化でない）なら閉じ、それ以外（なし・非表示・最小化）は開く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorToggle {
    Open,
    Close,
}

fn toggle_action(exists: bool, visible: bool, minimized: bool) -> MonitorToggle {
    if exists && visible && !minimized { MonitorToggle::Close } else { MonitorToggle::Open }
}

/// 監視窓が表示中（最小化でない）か。取得できない状態は開いているとみなさない。
pub fn monitor_is_open(app: &AppHandle) -> bool {
    match app.get_webview_window(MONITOR_LABEL) {
        Some(w) => toggle_action(true, w.is_visible().unwrap_or(false), w.is_minimized().unwrap_or(false)) == MonitorToggle::Close,
        None => false,
    }
}

/// 開閉をUIへ知らせる（`MONITOR_WINDOW_EVENT`、payloadは開いているかのbool）。
pub fn emit_monitor_state(app: &AppHandle, open: bool) {
    let _ = app.emit(MONITOR_WINDOW_EVENT, open);
}

/// 監視窓を閉じる（`monitor` ラベルの窓だけ）。なければ何もしない。
pub fn close_monitor(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(MONITOR_LABEL) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 開いていれば閉じ、閉じていれば開く（判定はここで1回）。操作後に開いているかを返す。
pub fn toggle_monitor(app: &AppHandle, host: &Arc<Host>) -> Result<bool, String> {
    let w = app.get_webview_window(MONITOR_LABEL);
    let (visible, minimized) = match &w {
        Some(w) => (w.is_visible().unwrap_or(false), w.is_minimized().unwrap_or(false)),
        None => (false, false),
    };
    match toggle_action(w.is_some(), visible, minimized) {
        MonitorToggle::Close => {
            close_monitor(app)?;
            Ok(false)
        }
        MonitorToggle::Open => {
            open_monitor(app, host)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_closes_only_a_visible_non_minimized_window() {
        assert_eq!(toggle_action(true, true, false), MonitorToggle::Close);
        assert_eq!(toggle_action(true, true, true), MonitorToggle::Open);
        assert_eq!(toggle_action(true, false, false), MonitorToggle::Open);
        assert_eq!(toggle_action(false, false, false), MonitorToggle::Open);
    }
}

/// 窓の現在の位置・サイズ。最小化中は記録しない（位置が無効）。最大化中は通常時の位置・サイズを保ったまま最大化の印だけ付ける。
fn current_bounds(window: &Window, prev: Option<WindowBounds>) -> Option<WindowBounds> {
    if window.is_minimized().unwrap_or(false) {
        return None;
    }
    if window.is_maximized().unwrap_or(false) {
        return prev.map(|p| WindowBounds { maximized: true, ..p });
    }
    let pos = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    if size.width == 0 || size.height == 0 {
        return None;
    }
    Some(WindowBounds { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: false })
}

/// 移動・リサイズのたびに呼ぶ。保存はホストが500msまとめて行う。
pub fn note_bounds(window: &Window) {
    let Some(kind) = kind_of(window.label()) else { return };
    let Some(host) = window.app_handle().try_state::<Arc<Host>>() else { return };
    if let Some(b) = current_bounds(window, host.saved_bounds(kind)) {
        host.set_window_bounds(kind, b);
    }
}
