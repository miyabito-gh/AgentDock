//! Windows固有の処理（P6）。依存は `windows-sys`（Job Object・電源通知・空き容量）。
//!
//! - [`job`]: AgentDockが起動したApp ServerをJob Objectで包み、強制終了で子孫プロセスごと終了する（合意 2026-10-06）。
//! - [`power`]: sleep/wake の検出（`PowerRegisterSuspendResumeNotification`、コールバック型。窓ハンドル不要）。
//! - [`window`]: 通常画面・監視窓の表示、位置・サイズの復元と記録。

pub mod clock;
pub mod job;
pub mod power;
pub mod toast;
pub mod window;
