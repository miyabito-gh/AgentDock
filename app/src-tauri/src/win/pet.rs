//! ペット窓（DESIGN_PET）。透明・枠なし・最前面・タスクバー非表示の2つ目の窓 `pet`。
//!
//! - 大きさは「1コマ（192×208）×倍率」に吹き出しの領域を足したもの（TS側 `pet/layout.ts` と同じ数値）。位置は `windows.json` に保存し、起動時に画面外なら戻す。
//! - 透明部分のクリックを下の窓へ通す: Windowsでは窓単位の切替（`set_ignore_cursor_events`）しかないので、短い間隔で
//!   カーソル位置を見て、画面側が通知した「不透明な領域」の上にいるときだけ入力を受ける（無効化中は窓がマウスイベントを受けないため、前面側では判定できない）。
//! - 隠す＝窓を閉じる（描画・監視スレッドも止まる）。表示＝作り直す。アプリの終了時は他の窓と一緒に閉じる。
//! - 吹き出しは、ペットが表示中のときだけ OS通知の代わりに送る（`emit_bubble`）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

use super::window::{work_areas, PET_LABEL};
use crate::backend::local::{PetCursor, PetRect, PetSettings, WindowBounds, WindowKind};
use crate::backend::model::ChatKey;
use crate::host::Host;
use crate::rules::notify::{NotificationKind, OsNotification};
use crate::rules::window::fit_to_monitors;

/// 吹き出しの表示（payload: [`PetBubble`]）。UI側の名前は `ipc/client.ts` の `PET_BUBBLE_EVENT` と同じ。
pub const PET_BUBBLE_EVENT: &str = "agentdock://pet-bubble";

const FRAME_W: f64 = 192.0;
const FRAME_H: f64 = 208.0;
const BUBBLE_W: f64 = 240.0;
const BUBBLE_H: f64 = 96.0;
/// カード（簡易チャット・回答・メニュー）を開いているときの上側の領域。TS側 `PetApp.tsx` の `CARD_ZONE_*` と同じ数値。
const CARD_W: f64 = 316.0;
const CARD_H: f64 = 420.0;
/// 返信・推敲のパネルを開いているときの上側の領域。TS側 `PetApp.tsx` の `COMPOSE_ZONE_H` と同じ数値。
const COMPOSE_W: f64 = 440.0;
const COMPOSE_H: f64 = 560.0;
const POLL_MS: u64 = 33;

/// 窓の大きさ（論理px）。幅は1コマと吹き出し（カード）の広いほう、高さは1コマ＋吹き出し（カード）。
pub fn window_size(scale_percent: u32, card: bool) -> (f64, f64) {
    window_size_for(scale_percent, card, false)
}

/// `compose`（返信・推敲のパネル）は幅広・縦長で、`card` より優先する。
pub fn window_size_for(scale_percent: u32, card: bool, compose: bool) -> (f64, f64) {
    let s = scale_percent as f64 / 100.0;
    let (w, h) = if compose {
        (COMPOSE_W, COMPOSE_H)
    } else if card {
        (CARD_W, CARD_H)
    } else {
        (BUBBLE_W, BUBBLE_H)
    };
    ((FRAME_W * s).max(w), FRAME_H * s + h)
}

/// 画面側から受け取った不透明領域と、監視スレッドの世代。
#[derive(Default)]
pub struct PetRuntime {
    regions: Mutex<Vec<PetRect>>,
    generation: AtomicU64,
    /// カードを開いている（窓を広げている）か。
    card: AtomicBool,
    /// 返信・推敲のパネルを開いているか（このとき `card` も true）。
    compose: AtomicBool,
    /// 通知の一時停止の終わり（UNIXミリ秒）。保存しない（アプリを閉じれば解除）。
    paused_until: Mutex<Option<i64>>,
}

impl PetRuntime {
    pub fn set_regions(&self, regions: Vec<PetRect>) {
        *self.regions.lock().unwrap() = regions;
    }
}

fn hit(regions: &[PetRect], x: f64, y: f64) -> bool {
    regions.iter().any(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
}

/// ペット窓が開いている（表示中）か。
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(PET_LABEL).is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// カーソルの、ペット窓の左上からの位置（CSS px）。窓がない・取得できないときは None。
pub fn cursor(app: &AppHandle) -> Option<PetCursor> {
    let w = app.get_webview_window(PET_LABEL)?;
    let c = w.cursor_position().ok()?;
    let p = w.outer_position().ok()?;
    let f = w.scale_factor().ok()?;
    Some(PetCursor { x: (c.x - p.x as f64) / f, y: (c.y - p.y as f64) / f })
}

/// 位置を画面内へ収める（大きさは変えない）。
fn fit_position(app: &AppHandle, x: i32, y: i32, size: PhysicalSize<u32>) -> PhysicalPosition<i32> {
    let b = fit_to_monitors(WindowBounds { x, y, width: size.width, height: size.height, maximized: false }, &work_areas(app), (size.width, size.height));
    PhysicalPosition::new(b.x, b.y)
}

fn spawn_hit_watcher(app: &AppHandle, generation: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut ignoring: Option<bool> = None;
        loop {
            std::thread::sleep(Duration::from_millis(POLL_MS));
            let Some(rt) = app.try_state::<Arc<PetRuntime>>() else { return };
            if rt.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let Some(w) = app.get_webview_window(PET_LABEL) else { return };
            // 取得できないときは、クリックを受けない側（通す）に倒す。
            let want_ignore = match cursor(&app) {
                Some(c) => !hit(&rt.regions.lock().unwrap(), c.x, c.y),
                None => true,
            };
            if ignoring != Some(want_ignore) && w.set_ignore_cursor_events(want_ignore).is_ok() {
                ignoring = Some(want_ignore);
            }
        }
    });
}

/// ペット窓を開く（なければ作る）。フォーカスは奪わない。位置は保存値（画面外なら補正）。
pub fn open_pet(app: &AppHandle, host: &Arc<Host>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(PET_LABEL) {
        return w.show().map_err(|e| e.to_string());
    }
    let (lw, lh) = window_size(host.get_app_settings().pet.scale_percent, false);
    let saved = host.saved_bounds(WindowKind::Pet);
    let w = WebviewWindowBuilder::new(app, PET_LABEL, WebviewUrl::App("index.html?view=pet".into()))
        .title("AgentDock ペット")
        .inner_size(lw, lh)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .build()
        .map_err(|e| e.to_string())?;
    // 領域が届くまでは、クリックを通す。
    let _ = w.set_ignore_cursor_events(true);
    if let (Some(b), Ok(size)) = (saved, w.inner_size()) {
        let p = fit_position(app, b.x, b.y, size);
        let _ = w.set_position(p);
    }
    let Some(rt) = app.try_state::<Arc<PetRuntime>>() else { return Err("pet runtime missing".into()) };
    let generation = rt.generation.fetch_add(1, Ordering::SeqCst) + 1;
    rt.set_regions(Vec::new());
    rt.card.store(false, Ordering::SeqCst);
    rt.compose.store(false, Ordering::SeqCst);
    spawn_hit_watcher(app, generation);
    Ok(())
}

pub fn close_pet(app: &AppHandle) -> Result<(), String> {
    if let Some(rt) = app.try_state::<Arc<PetRuntime>>() {
        rt.generation.fetch_add(1, Ordering::SeqCst);
    }
    if let Some(w) = app.get_webview_window(PET_LABEL) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 倍率に合わせて窓の大きさを変える。足元（下端の中央）を動かさない。
fn resize(app: &AppHandle, scale_percent: u32) {
    let rt = app.try_state::<Arc<PetRuntime>>();
    let card = rt.as_ref().is_some_and(|rt| rt.card.load(Ordering::SeqCst));
    let compose = rt.as_ref().is_some_and(|rt| rt.compose.load(Ordering::SeqCst));
    let Some(w) = app.get_webview_window(PET_LABEL) else { return };
    let (Ok(old), Ok(pos)) = (w.inner_size(), w.outer_position()) else { return };
    let (lw, lh) = window_size_for(scale_percent, card, compose);
    if w.set_size(LogicalSize::new(lw, lh)).is_err() {
        return;
    }
    let Ok(new) = w.inner_size() else { return };
    let x = pos.x + (old.width as i32 - new.width as i32) / 2;
    let y = pos.y + old.height as i32 - new.height as i32;
    let _ = w.set_position(fit_position(app, x, y, new));
}

/// カードの開閉に合わせて窓の大きさを変える（足元固定）。
pub fn set_card(app: &AppHandle, host: &Arc<Host>, open: bool, compose: bool) {
    if let Some(rt) = app.try_state::<Arc<PetRuntime>>() {
        rt.card.store(open, Ordering::SeqCst);
        rt.compose.store(open && compose, Ordering::SeqCst);
    }
    resize(app, host.get_app_settings().pet.scale_percent);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 通知の一時停止（`minutes` が None なら再開）。終わりの時刻（UNIXミリ秒）を返す。
pub fn pause_notifications(app: &AppHandle, minutes: Option<u32>) -> Option<i64> {
    let until = minutes.map(|m| now_ms() + m as i64 * 60_000);
    if let Some(rt) = app.try_state::<Arc<PetRuntime>>() {
        *rt.paused_until.lock().unwrap() = until;
    }
    until
}

/// 一時停止中なら終わりの時刻。期限が過ぎていれば解除して None。
pub fn paused_until(app: &AppHandle) -> Option<i64> {
    let rt = app.try_state::<Arc<PetRuntime>>()?;
    let mut g = rt.paused_until.lock().unwrap();
    if g.is_some_and(|t| t <= now_ms()) {
        *g = None;
    }
    *g
}

/// 一時停止中に出さない通知か。承認・質問待ち（回答が必要なもの）は止めない。
pub fn suppressed_by_pause(app: &AppHandle, n: &OsNotification) -> bool {
    !matches!(n.kind, NotificationKind::AwaitingAnswer | NotificationKind::OrgAttention) && paused_until(app).is_some()
}

/// 保存したペット設定を窓へ反映する（表示・非表示・大きさ）。
pub fn apply_settings(app: &AppHandle, host: &Arc<Host>, pet: &PetSettings) -> Result<(), String> {
    if pet.visible {
        open_pet(app, host)?;
        resize(app, pet.scale_percent);
        Ok(())
    } else {
        close_pet(app)
    }
}

/// 返信・推敲のパネルを出すために、ペット窓を前面に出す。ペットを隠している（窓がない）ときは、パネルのためだけに窓を作る
/// （設定の「表示」は変えない。UIは設定が非表示ならペットの絵を出さず、パネルを閉じるとホストが窓を閉じる）。
pub fn show_for_compose(app: &AppHandle, host: &Arc<Host>) -> Result<(), String> {
    open_pet(app, host)?;
    if let Some(w) = app.get_webview_window(PET_LABEL) {
        let _ = w.set_focus();
    }
    Ok(())
}

/// 表示と非表示を切り替える。操作後に表示中かを返す。
pub fn toggle_pet(app: &AppHandle, host: &Arc<Host>) -> Result<bool, String> {
    let mut pet = host.get_app_settings().pet;
    pet.visible = !is_open(app);
    let pet = host.set_pet_settings(pet).map_err(|e| e.message)?;
    apply_settings(app, host, &pet)?;
    Ok(pet.visible)
}

/// 起動時に、保存した設定が表示ならペット窓を開く。
pub fn restore_pet(app: &AppHandle, host: &Arc<Host>) {
    let pet = host.get_app_settings().pet;
    if pet.visible {
        if let Err(e) = open_pet(app, host) {
            crate::diag::log("pet", &format!("open failed: {e}"));
        }
    }
}

/// 吹き出し1件（画面側の型 `PetBubbleMessage` と同じ形）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PetBubble {
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    pub chat: Option<ChatKey>,
}

pub fn bubble_of(n: &OsNotification) -> PetBubble {
    let kind = match n.kind {
        NotificationKind::AwaitingAnswer => "awaitingAnswer",
        NotificationKind::Completed => "completed",
        NotificationKind::Failed => "failed",
        NotificationKind::OrgAttention => "orgAttention",
        NotificationKind::OrgCompleted => "orgCompleted",
        NotificationKind::OrgFailed => "orgFailed",
    };
    PetBubble { kind, title: n.title.trim_start_matches("AgentDock: ").to_string(), body: n.body.clone(), chat: n.chat.clone() }
}

/// ペットが表示中なら吹き出しとして送り、true を返す（OS通知は出さない）。隠れているときは false（呼び出し側がOS通知を出す）。
pub fn emit_bubble(app: &AppHandle, n: &OsNotification) -> bool {
    // 返信・推敲のパネルのためだけに窓が開いている間（設定は非表示）は、吹き出しを出さず OS 通知に任せる。
    let shown = app.try_state::<Arc<Host>>().is_none_or(|h| h.get_app_settings().pet.visible);
    if !is_open(app) || !shown {
        return false;
    }
    app.emit_to(PET_LABEL, PET_BUBBLE_EVENT, bubble_of(n)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_fits_the_frame_and_the_bubble() {
        assert_eq!(window_size(100, false), (240.0, 304.0));
        assert_eq!(window_size(150, false), (288.0, 408.0));
        assert_eq!(window_size(50, false), (240.0, 200.0));
        assert_eq!(window_size(100, true), (316.0, 628.0));
        assert_eq!(window_size(150, true), (316.0, 732.0));
        // 返信・推敲のパネルは幅広・縦長（足元の1コマ分は同じ）
        assert_eq!(window_size_for(100, true, true), (440.0, 768.0));
        assert_eq!(window_size_for(100, false, true), (440.0, 768.0));
        assert_eq!(window_size_for(150, true, false), window_size(150, true));
    }

    #[test]
    fn hit_test_uses_only_the_reported_regions() {
        let r = [PetRect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 }];
        assert!(hit(&r, 10.0, 20.0));
        assert!(hit(&r, 109.9, 69.9));
        assert!(!hit(&r, 110.0, 30.0));
        assert!(!hit(&r, 50.0, 70.0));
        assert!(!hit(&[], 50.0, 30.0));
    }
}
