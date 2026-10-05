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

/// 登録の保持。Drop で登録解除。
pub struct PowerWatcher {
    _private: (),
}

impl PowerWatcher {
    /// コールバックはOSのスレッドから呼ばれる。中では `mpsc` へ送るだけにする。
    pub fn start(on_event: Box<dyn Fn(PowerEvent) + Send + Sync + 'static>) -> std::io::Result<Self> {
        let _ = on_event;
        todo!("P6")
    }
}
