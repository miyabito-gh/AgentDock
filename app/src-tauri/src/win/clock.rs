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

/// ローカル時刻の UTC からの差（分。東が正）。今の値を使うので、夏時間のある地域では過去の時刻が1時間ずれうる（表示用）。
/// 2回の読取りの間に分が変わっても 15 分単位に丸めて誤差を消す。
#[cfg(windows)]
pub fn local_utc_offset_min() -> i32 {
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // SAFETY: 出力は有効な SYSTEMTIME。
    let (l, u) = unsafe {
        let mut l = std::mem::zeroed();
        let mut u = std::mem::zeroed();
        GetLocalTime(&mut l);
        GetSystemTime(&mut u);
        (l, u)
    };
    let mut diff = (l.wHour as i32 * 60 + l.wMinute as i32) - (u.wHour as i32 * 60 + u.wMinute as i32);
    if diff > 14 * 60 {
        diff -= 1440;
    } else if diff < -12 * 60 {
        diff += 1440;
    }
    (diff + 7).div_euclid(15) * 15
}

#[cfg(not(windows))]
pub fn local_utc_offset_min() -> i32 {
    0
}
