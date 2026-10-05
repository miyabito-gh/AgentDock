//! OS通知（Windowsトースト、P5）。`tauri-winrt-notification` を直接使う（クリックを受けるため）。
//!
//! - AUMID は `com.agentdock.app`（インストール時に登録される識別子）。インストールしていない開発実行では表示されないことがあり、受入の証拠にしない。
//! - クリックは該当チャットを開く通知（`on_open`）を呼ぶだけ。回答・再実行・キュー再開はしない。
//! - 本文は呼出し側（`rules::notify::render`）が作った文字列をそのまま出す。

use tauri_winrt_notification::{Sound, Toast};

use crate::backend::model::ChatKey;
use crate::rules::notify::OsNotification;

pub const APP_ID: &str = "com.agentdock.app";

pub fn show(n: &OsNotification, on_open: impl Fn(ChatKey) + Send + 'static) {
    let chat = n.chat.clone();
    let toast = Toast::new(APP_ID)
        .title(&n.title)
        .text1(&n.body)
        .sound(if n.sound { Some(Sound::Default) } else { None })
        .on_activated(move |_| {
            on_open(chat.clone());
            Ok(())
        });
    if let Err(e) = toast.show() {
        crate::diag::log("notify", &format!("toast show failed: {e}"));
    }
}
