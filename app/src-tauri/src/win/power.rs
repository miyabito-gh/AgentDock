//! sleep/wake の検出（P6、§3.11）。
//!
//! - 主: `PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK)` で PBT_APMSUSPEND / PBT_APMRESUMEAUTOMATIC を受ける。
//! - 予備: 5秒刻みのタイマーで壁時計の飛びを見る（`rules::manage::looks_like_resume`）。
//! - 復帰時はホストが live の鮮度を要照合にし（状態は変えない）、送信を保留し、履歴と対象集合を照合してから live に戻す。
//!   復帰だけでは通知・再送・resumeをしない。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspending,
    Resumed,
}

type Callback = Box<dyn Fn(PowerEvent) + Send + Sync + 'static>;

/// `PBT_APMSUSPEND` / `PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND`。
const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
/// `DEVICE_NOTIFY_CALLBACK`。
#[cfg(windows)]
const DEVICE_NOTIFY_CALLBACK: u32 = 2;

/// 通知の種類からイベントへ。関係のない種類（電源設定の変更など）は None。
/// RESUMEAUTOMATIC はユーザー不在の復帰でも来る。RESUMESUSPEND はユーザー操作の復帰。両方来ることがあるので、ホスト側は重ねて呼ばれても害のない処理にする。
pub fn event_of(kind: u32) -> Option<PowerEvent> {
    match kind {
        PBT_APMSUSPEND => Some(PowerEvent::Suspending),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(PowerEvent::Resumed),
        _ => None,
    }
}

/// 登録の保持。Drop で登録解除。ポインタは整数で持つ（`Send`/`Sync` のため。解放は Drop だけ）。
pub struct PowerWatcher {
    context: usize,
    params: usize,
    registration: usize,
}

#[cfg(windows)]
unsafe extern "system" fn on_power(context: *const core::ffi::c_void, kind: u32, _setting: *const core::ffi::c_void) -> u32 {
    if let Some(ev) = event_of(kind) {
        // 本体は Watcher が生きている間だけ有効。パニックをOSのスレッドへ伝えない。
        let cb = &*(context as *const Callback);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(ev)));
    }
    0
}

impl PowerWatcher {
    /// コールバックはOSのスレッドから呼ばれる。中では `mpsc` へ送るだけにする。
    #[cfg(windows)]
    pub fn start(on_event: Box<dyn Fn(PowerEvent) + Send + Sync + 'static>) -> std::io::Result<Self> {
        use windows_sys::Win32::System::Power::{PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS};
        let context: *mut Callback = Box::into_raw(Box::new(on_event));
        // 構造体は登録の間OSが参照する。解除まで保持し、Dropで解放する。
        let params = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: context as *mut core::ffi::c_void }));
        let mut registration: *mut core::ffi::c_void = std::ptr::null_mut();
        let rc = unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as _, &mut registration as *mut _ as _) };
        if rc != 0 {
            unsafe {
                drop(Box::from_raw(params));
                drop(Box::from_raw(context));
            }
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        Ok(PowerWatcher { context: context as usize, params: params as usize, registration: registration as usize })
    }

    #[cfg(not(windows))]
    pub fn start(on_event: Box<dyn Fn(PowerEvent) + Send + Sync + 'static>) -> std::io::Result<Self> {
        let _ = on_event;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "power notification is Windows-only"))
    }
}

#[cfg(windows)]
impl Drop for PowerWatcher {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerUnregisterSuspendResumeNotification};
        unsafe {
            // 解除してから解放する（解除後はコールバックが呼ばれない）。
            PowerUnregisterSuspendResumeNotification(self.registration as _);
            drop(Box::from_raw(self.params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS));
            drop(Box::from_raw(self.context as *mut Callback));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_suspend_and_resume_kinds() {
        assert_eq!(event_of(PBT_APMSUSPEND), Some(PowerEvent::Suspending));
        assert_eq!(event_of(PBT_APMRESUMEAUTOMATIC), Some(PowerEvent::Resumed));
        assert_eq!(event_of(PBT_APMRESUMESUSPEND), Some(PowerEvent::Resumed));
        assert_eq!(event_of(0x8013), None);
    }
}
