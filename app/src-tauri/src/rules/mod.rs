//! 段階②の純粋ロジック（時刻・状態を引数で受け取り、I/Oをしない）。単体テストはここにだけ書く。
//!
//! - [`queue`]: 完了後キューの自動送信判断（§3.7）
//! - [`notify`]: 通知の2秒集約・重複防止・抑制（§3.11、D03）
//! - [`manage`]: 削除保留・アーカイブ反映・完全終了・wake（§3.4・§3.6・§3.11）
//! - [`window`]: 窓位置の画面外補正（M37）
//! - [`export`]: Markdownエクスポートの組立て（M41）

pub mod export;
pub mod manage;
pub mod notify;
pub mod queue;
pub mod window;
