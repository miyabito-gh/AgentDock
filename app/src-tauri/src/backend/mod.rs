//! AIバックエンド境界（T1設計）。
//!
//! - [`model`]: バックエンド共通の正規化データモデル（要件§4・§5）。Codex固有型を含めない。
//! - [`backend`]: AIバックエンドtrait（要件§2.3・§3.1〜3.6・§3.9）。Codex App Serverはその実装の一つ（T3）。
//! - [`ipc`]: UI⇔ホストのTauri IPCペイロード（`app/src/ipc/types.ts`と1対1で対応させる）。
//!
//! 設計メモは `app/DESIGN_T1.md`。

pub mod backend;
pub mod ipc;
/// 段階②のアプリ側データとIPC追加分（`app/DESIGN_P2.md`）。
pub mod local;
pub mod model;
