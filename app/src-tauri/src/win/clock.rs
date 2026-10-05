//! ローカル時刻の文字列（P7。Markdownエクスポートの日時表示用）。

/// ローカル時刻 `YYYY-MM-DD HH:MM:SS`。
#[cfg(windows)]
pub fn local_time_text() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: 出力は有効な SYSTEMTIME。
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(not(windows))]
pub fn local_time_text() -> String {
    String::new()
}
