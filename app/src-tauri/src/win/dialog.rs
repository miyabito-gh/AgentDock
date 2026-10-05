//! ファイル選択・保存ダイアログ（Win32 共通ダイアログ）と、ローカル時刻の文字列（P7）。
//!
//! - codex.exe の「参照…」とMarkdownエクスポートの保存先選択に使う。選ぶだけで、ファイルの読み書きはしない。
//! - 保存ダイアログは同名ファイルの上書きをOSの確認で尋ねる（`OFN_OVERWRITEPROMPT`）。
//! - 呼出しはUIスレッドを塞がないよう、呼出し側で `spawn_blocking` に載せる。

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    const MAX_PATH_CHARS: usize = 32_768;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// `説明\0パターン\0…\0\0` の形のフィルタ。
    fn filter_buf(filters: &[(&str, &str)]) -> Vec<u16> {
        let mut v: Vec<u16> = Vec::new();
        for (name, pattern) in filters {
            v.extend(std::ffi::OsStr::new(name).encode_wide());
            v.push(0);
            v.extend(std::ffi::OsStr::new(pattern).encode_wide());
            v.push(0);
        }
        v.push(0);
        v
    }

    /// 選んだパス。キャンセルなら None。`save_name` が Some なら保存ダイアログ（既定のファイル名）。
    pub fn pick_path(owner: isize, title: &str, filters: &[(&str, &str)], save_name: Option<&str>, default_ext: Option<&str>) -> Result<Option<String>, String> {
        let mut file: Vec<u16> = vec![0; MAX_PATH_CHARS];
        if let Some(name) = save_name {
            for (i, c) in std::ffi::OsStr::new(name).encode_wide().take(MAX_PATH_CHARS - 1).enumerate() {
                file[i] = c;
            }
        }
        let filter = filter_buf(filters);
        let title_w = wide(title);
        let ext_w = default_ext.map(wide);
        // SAFETY: 構造体は0初期化して必要な項目だけ設定する。参照するバッファ（file・filter・title・ext）は呼出しの間生きている。
        let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner as _;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = MAX_PATH_CHARS as u32;
        ofn.lpstrTitle = title_w.as_ptr();
        if let Some(e) = &ext_w {
            ofn.lpstrDefExt = e.as_ptr();
        }
        let ok = if save_name.is_some() {
            ofn.Flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
            // SAFETY: `ofn` は有効で、上のバッファを指す。
            unsafe { GetSaveFileNameW(&mut ofn) }
        } else {
            ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
            // SAFETY: 同上。
            unsafe { GetOpenFileNameW(&mut ofn) }
        };
        if ok == 0 {
            // SAFETY: 引数なしの取得関数。
            let code = unsafe { CommDlgExtendedError() };
            return if code == 0 { Ok(None) } else { Err(format!("ファイル選択ダイアログを開けませんでした（エラー {code}）")) };
        }
        let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Ok(Some(std::ffi::OsString::from_wide(&file[..len]).to_string_lossy().into_owned()))
    }

    /// ローカル時刻 `YYYY-MM-DD HH:MM:SS`。
    pub fn local_time_text() -> String {
        // SAFETY: 出力は有効な SYSTEMTIME。
        let t = unsafe {
            let mut t = std::mem::zeroed();
            GetLocalTime(&mut t);
            t
        };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
}

#[cfg(windows)]
pub use imp::{local_time_text, pick_path};

#[cfg(not(windows))]
pub fn pick_path(_owner: isize, _title: &str, _filters: &[(&str, &str)], _save_name: Option<&str>, _default_ext: Option<&str>) -> Result<Option<String>, String> {
    Err("ファイル選択ダイアログはWindowsでだけ使えます".into())
}

#[cfg(not(windows))]
pub fn local_time_text() -> String {
    String::new()
}
