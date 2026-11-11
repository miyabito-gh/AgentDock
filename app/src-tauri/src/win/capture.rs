//! 選択した文章の取り込み（`app/DESIGN_COMPOSE.md`「文章の取り込み」）。グローバルショートカットを押した時点の前面ウィンドウへ
//! `SendInput` で Ctrl+C を送り、クリップボードの変化を最大0.3秒待って文字を読む。読んだらクリップボードを元へ戻す。
//!
//! - 戻せるのは文字だけ。元が文字以外（画像・ファイルなど）のときは戻さず、その旨を `notice` で伝える。
//! - 選択がなければ（クリップボードが変わらなければ）、元のクリップボードの文字を使う（出どころは「クリップボードから」）。
//! - ショートカットの修飾キー（Ctrl+Alt）が押されたままだと Ctrl+C にならない。解放を最大0.35秒待ち、まだ押されていれば
//!   一時的に解放の入力を送ってから Ctrl+C を送る（Alt が押されたまま離されたときのメニュー起動は、無害なキーを挟んで防ぐ）。
//! - 前面が AgentDock 自身の窓（パネルなど）のときは Ctrl+C を送らない（パネル内の選択で取り込み直さない）。
//! - 取り込んだ文章は診断ログに書かない。

use tauri::AppHandle;

use crate::backend::local::ComposeOrigin;

/// 元のクリップボードの中身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Original {
    Text(String),
    Empty,
    /// 文字以外（画像・ファイルなど）。
    Other,
}

/// 取り込みのあとにクリップボードをどうするか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// 何もしない（変わっていない）。
    Nothing,
    Text(String),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub source: String,
    pub origin: ComposeOrigin,
    pub notice: Option<String>,
    pub restore: Restore,
}

/// 取り込み結果の取り扱いの決定（純粋）。`changed` はクリップボードが変わったか、`copied` はそのとき読めた文字。
pub fn plan(original: &Original, changed: bool, copied: Option<&str>) -> Plan {
    let restore = match (changed, original) {
        (false, _) => Restore::Nothing,
        (true, Original::Text(t)) => Restore::Text(t.clone()),
        (true, Original::Empty) => Restore::Clear,
        (true, Original::Other) => Restore::Nothing,
    };
    let lost = changed && *original == Original::Other;
    let notice = lost.then(|| "元のクリップボードは文字以外だったため、戻せませんでした".to_string());
    if let Some(t) = copied.filter(|t| changed && !t.trim().is_empty()) {
        return Plan { source: t.to_string(), origin: ComposeOrigin::Selection, notice, restore };
    }
    // 選択がなかった（または文字として読めなかった）。元のクリップボードの文字を使う。
    match original {
        Original::Text(t) if !t.trim().is_empty() => Plan { source: t.clone(), origin: ComposeOrigin::Clipboard, notice, restore },
        _ => Plan { source: String::new(), origin: ComposeOrigin::None, notice, restore },
    }
}

/// 取り込んだ結果。
#[derive(Clone, PartialEq, Eq)]
pub struct Captured {
    pub source: String,
    pub origin: ComposeOrigin,
    pub notice: Option<String>,
}

impl std::fmt::Debug for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Captured { <redacted> }")
    }
}

/// 前面ウィンドウの選択文を取り込む（ブロックする。別スレッドで呼ぶ）。
pub fn capture(app: &AppHandle) -> Captured {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let original = read_original(app);
    let (changed, copied) = if sys::foreground_is_own() { (false, None) } else { try_copy(app) };
    let p = plan(&original, changed, copied.as_deref());
    match &p.restore {
        Restore::Nothing => {}
        Restore::Text(t) => {
            let _ = app.clipboard().write_text(t.clone());
        }
        Restore::Clear => {
            let _ = app.clipboard().clear();
        }
    }
    Captured { source: p.source, origin: p.origin, notice: p.notice }
}

fn read_original(app: &AppHandle) -> Original {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    match sys::clipboard_kind() {
        sys::Kind::Empty => Original::Empty,
        sys::Kind::Other => Original::Other,
        sys::Kind::Text => match app.clipboard().read_text() {
            Ok(t) => Original::Text(t),
            // 文字のはずが読めなかった。戻せない可能性があるので「文字以外」と同じ扱いにする。
            Err(_) => Original::Other,
        },
    }
}

const COPY_WAIT_MS: u64 = 300;
const POLL_MS: u64 = 10;

/// Ctrl+C を送り、クリップボードの変化を待って文字を読む。変わらなければ（選択なし・送れなかった）`(false, None)`。
fn try_copy(app: &AppHandle) -> (bool, Option<String>) {
    use std::time::{Duration, Instant};
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let before = sys::clipboard_seq();
    sys::send_copy();
    let deadline = Instant::now() + Duration::from_millis(COPY_WAIT_MS);
    while sys::clipboard_seq() == before {
        if Instant::now() >= deadline {
            return (false, None);
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
    // 書き込み中でロックされていることがあるので、数回やり直して読む。
    for _ in 0..10 {
        if let Ok(t) = app.clipboard().read_text() {
            return (true, Some(t));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    (true, None)
}

#[cfg(windows)]
mod sys {
    use std::time::{Duration, Instant};

    use windows::Win32::System::DataExchange::{CountClipboardFormats, GetClipboardSequenceNumber, IsClipboardFormatAvailable};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL,
        VK_RMENU, VK_RSHIFT, VK_RWIN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    /// 文字（UTF-16）のクリップボード形式。
    const CF_UNICODETEXT: u32 = 13;
    /// 割り当てのないキー。Alt の押下と解放のあいだに挟んで、メニューの起動を防ぐ。
    const VK_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
    const MODIFIER_WAIT_MS: u64 = 350;

    pub enum Kind {
        Empty,
        Text,
        Other,
    }

    pub fn clipboard_seq() -> u32 {
        // SAFETY: 引数なしの読み取り専用のAPI。
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn clipboard_kind() -> Kind {
        // SAFETY: クリップボードを開かずに形式の有無だけを調べるAPI。
        unsafe {
            if CountClipboardFormats() == 0 {
                Kind::Empty
            } else if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
                Kind::Text
            } else {
                Kind::Other
            }
        }
    }

    /// 前面のウィンドウが、このプロセス（AgentDock）のものか。
    pub fn foreground_is_own() -> bool {
        // SAFETY: 前面ウィンドウの取得とそのプロセスIDの読み取りだけ。
        unsafe {
            let h = GetForegroundWindow();
            if h.0.is_null() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            pid == std::process::id()
        }
    }

    const MODIFIERS: [VIRTUAL_KEY; 8] = [VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LSHIFT, VK_RSHIFT, VK_LWIN, VK_RWIN];

    fn is_down(vk: VIRTUAL_KEY) -> bool {
        // SAFETY: キーの物理状態の読み取りだけ。
        unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
    }

    fn held() -> Vec<VIRTUAL_KEY> {
        MODIFIERS.iter().copied().filter(|k| is_down(*k)).collect()
    }

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn send(inputs: &[INPUT]) {
        if inputs.is_empty() {
            return;
        }
        // SAFETY: 組み立てた入力の配列とその要素の大きさを渡す。送れなかった場合（権限の高いウィンドウなど）は何も起きない。
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// 前面ウィンドウへ Ctrl+C を送る。修飾キーが押されたままなら、解放を待ち、まだなら一時的に解放の入力を送る。
    pub fn send_copy() {
        if is_down(VK_LMENU) || is_down(VK_RMENU) {
            send(&[key(VK_MASK, false), key(VK_MASK, true)]);
        }
        let deadline = Instant::now() + Duration::from_millis(MODIFIER_WAIT_MS);
        while !held().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut inputs: Vec<INPUT> = held().into_iter().map(|k| key(k, true)).collect();
        inputs.extend([key(VK_CONTROL, false), key(VK_C, false), key(VK_C, true), key(VK_CONTROL, true)]);
        send(&inputs);
    }
}

#[cfg(not(windows))]
mod sys {
    pub enum Kind {
        Empty,
        Text,
        Other,
    }
    pub fn clipboard_seq() -> u32 {
        0
    }
    pub fn clipboard_kind() -> Kind {
        Kind::Text
    }
    pub fn foreground_is_own() -> bool {
        false
    }
    pub fn send_copy() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Original {
        Original::Text(t.into())
    }

    #[test]
    fn a_selection_is_used_and_the_original_text_is_put_back() {
        let p = plan(&text("元の文字"), true, Some("選んだ文"));
        assert_eq!((p.source.as_str(), p.origin, p.restore, p.notice), ("選んだ文", ComposeOrigin::Selection, Restore::Text("元の文字".into()), None));
    }

    #[test]
    fn an_empty_original_is_cleared_again_after_a_selection() {
        let p = plan(&Original::Empty, true, Some("選んだ文"));
        assert_eq!((p.origin, p.restore), (ComposeOrigin::Selection, Restore::Clear));
    }

    #[test]
    fn a_non_text_original_cannot_be_restored_and_says_so() {
        let p = plan(&Original::Other, true, Some("選んだ文"));
        assert_eq!((p.origin, p.restore), (ComposeOrigin::Selection, Restore::Nothing));
        assert!(p.notice.unwrap().contains("文字以外"));
    }

    #[test]
    fn without_a_selection_the_clipboard_text_is_used_and_nothing_is_touched() {
        let p = plan(&text("クリップボードの文"), false, None);
        assert_eq!((p.source.as_str(), p.origin, p.restore, p.notice), ("クリップボードの文", ComposeOrigin::Clipboard, Restore::Nothing, None));
        // 文字以外・空のクリップボードで選択もなければ、空（パネルで貼り付けてもらう）。注意書きも出さない（変えていない）
        for o in [Original::Empty, Original::Other] {
            let p = plan(&o, false, None);
            assert_eq!((p.source.as_str(), p.origin, p.restore, p.notice), ("", ComposeOrigin::None, Restore::Nothing, None));
        }
    }

    #[test]
    fn a_changed_but_blank_clipboard_falls_back_to_the_original_text_and_restores_it() {
        let p = plan(&text("元の文字"), true, Some("  \n"));
        assert_eq!((p.source.as_str(), p.origin, p.restore), ("元の文字", ComposeOrigin::Clipboard, Restore::Text("元の文字".into())));
        // 読めなかった（None）ときも同じ
        let p = plan(&text("元の文字"), true, None);
        assert_eq!((p.origin, p.restore), (ComposeOrigin::Clipboard, Restore::Text("元の文字".into())));
    }

    #[test]
    fn captured_debug_never_shows_the_text() {
        let c = Captured { source: "秘密".into(), origin: ComposeOrigin::Selection, notice: None };
        assert!(!format!("{c:?}").contains("秘密"));
    }
}
