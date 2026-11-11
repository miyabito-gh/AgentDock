//! 返信・推敲のグローバルショートカット（`app/DESIGN_COMPOSE.md`）。既定は Ctrl+Alt+R（設定で変更できる）。
//!
//! - 押したら、前面ウィンドウの選択文を取り込み（`win::capture`）、パネルを開く。既定のモードは前回使った方。
//! - 登録に失敗したら（他のアプリが使用中・書式が不正など）理由を持つ。設定画面が `status` で読んで表示する。
//! - 作成中にもう一度押したときは、取り込み直さず、パネルを前面に出すだけ（作成中の結果を消さない）。
//! - ペットを隠していても、パネルのためだけにペット窓を出す（`win::pet::show_for_compose`）。パネルを閉じると窓も閉じる。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use super::{capture, pet};
use crate::backend::local::{ComposeOrigin, ComposeState};
use crate::host::Host;

/// 登録の状態（画面側の型 `ComposeShortcutStatus` と同じ形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    /// 登録できているキー（なければ None）。
    pub registered: Option<String>,
    /// 登録できなかった理由（平易な日本語）。
    pub error: Option<String>,
}

#[derive(Default)]
pub struct ComposeShortcut {
    status: Mutex<Option<ShortcutStatus>>,
    /// 取り込み中（連打で重ねない）。
    capturing: AtomicBool,
}

impl ComposeShortcut {
    pub fn status(&self) -> ShortcutStatus {
        self.status.lock().unwrap().clone().unwrap_or(ShortcutStatus { registered: None, error: None })
    }
}

/// 入力されたキーの表記を検査して整える（空＝登録しない）。書式が不正なら理由。
pub fn parse(text: &str) -> Result<Option<Shortcut>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let sc: Shortcut = t.parse().map_err(|e| format!("「{t}」は使えないキーの指定です（{e}）"))?;
    // Ctrl・Alt・Win のどれかを含まないキー単体は、ふだんの入力を奪うので断る（Shift だけも同じ）。
    let lower = t.to_ascii_lowercase();
    let has_modifier = ["ctrl", "control", "alt", "option", "super", "win", "cmd", "command", "meta"].iter().any(|m| lower.split('+').any(|p| p.trim() == *m));
    if !has_modifier {
        return Err("Ctrl・Alt・Win のどれかと組み合わせてください（キー単体や Shift だけでは登録できません）".into());
    }
    Ok(Some(sc))
}

/// 設定のキーで登録し直す（前の登録は外す）。失敗しても状態に理由を残すだけで、アプリは続ける。
pub fn apply(app: &AppHandle, shortcut: &str) -> ShortcutStatus {
    let gs = app.global_shortcut();
    let status = match parse(shortcut) {
        Err(e) => ShortcutStatus { registered: None, error: Some(e) },
        Ok(None) => {
            let _ = gs.unregister_all();
            ShortcutStatus { registered: None, error: None }
        }
        Ok(Some(sc)) => {
            let _ = gs.unregister_all();
            match gs.on_shortcut(sc, |app, _sc, event| {
                if event.state == ShortcutState::Pressed {
                    on_pressed(app);
                }
            }) {
                Ok(()) => ShortcutStatus { registered: Some(shortcut.trim().to_string()), error: None },
                Err(e) => ShortcutStatus { registered: None, error: Some(register_error_text(shortcut.trim(), &e.to_string())) },
            }
        }
    };
    if let Some(s) = app.try_state::<Arc<ComposeShortcut>>() {
        *s.status.lock().unwrap() = Some(status.clone());
    }
    if let Some(e) = &status.error {
        crate::diag::log("compose", &format!("shortcut not registered: {e}"));
    }
    status
}

fn register_error_text(key: &str, detail: &str) -> String {
    format!("「{key}」を登録できませんでした。ほかのアプリが使っているキーかもしれません。別のキーにしてください（{detail}）")
}

/// ショートカットが押されたとき。取り込みは別スレッド（Win32の待ち・クリップボード）で行い、イベントの処理を止めない。
fn on_pressed(app: &AppHandle) {
    let Some(host) = app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return };
    let Some(state) = app.try_state::<Arc<ComposeShortcut>>().map(|s| s.inner().clone()) else { return };
    if state.capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run(&app, &host).await;
        state.capturing.store(false, Ordering::SeqCst);
    });
}

async fn run(app: &AppHandle, host: &Arc<Host>) {
    // 作成中なら取り込み直さず、パネルを前面に出すだけ。
    let view = host.compose_view();
    if view.open && view.state == ComposeState::Generating {
        let _ = pet::show_for_compose(app, host);
        return;
    }
    let handle = app.clone();
    let captured = match tauri::async_runtime::spawn_blocking(move || capture::capture(&handle)).await {
        Ok(c) => c,
        // 取り込みの処理が落ちた。クリップボードを触れた保証はないので、空で開き、貼り付けてもらう。
        Err(_) => capture::Captured { source: String::new(), origin: ComposeOrigin::None, notice: Some("文章を取り込めませんでした。貼り付けてください".into()) },
    };
    crate::commands::compose_open_from_shortcut(host, captured.source, captured.origin, captured.notice).await;
    if let Err(e) = pet::show_for_compose(app, host) {
        crate::diag::log("compose", &format!("show panel failed: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_not_registered_and_a_modifier_is_required() {
        assert_eq!(parse("  ").unwrap(), None);
        assert!(parse("Ctrl+Alt+R").unwrap().is_some());
        assert!(parse("CTRL+SHIFT+F9").unwrap().is_some());
        for bad in ["R", "F9", "Shift+R", "Ctrl+", "Ctrl+Alt+あ", "Hyper+R"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_registration_failure_names_the_key_and_the_cause() {
        let t = register_error_text("Ctrl+Alt+R", "already registered");
        assert!(t.contains("Ctrl+Alt+R") && t.contains("ほかのアプリ") && t.contains("already registered"));
    }
}
