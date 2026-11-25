//! Antigravity（Google の `agy` CLI、Gemini 系モデル）の `AiBackend` 実装。
//!
//! 方式は1ターン=1プロセス（`agy -p ... --output-format stream-json`）。ACP は使わない。
//! 会話の正本は AgentDock の記録（`<app_data_dir>/antigravity/chats.json`）で、agy 側の会話データは読まない・消さない。
//!
//! - [`cmd`]: 起動引数・実行ファイル解決・`agy models` / `--version` の解析（純粋）。
//! - [`stream`]: 出力行の解析と、1ターン分のイベント組立て（純粋）。
//! - [`store`]: チャット記録の保存（原子的書込み・壊れたファイルの退避）。
//! - [`view`]: 記録 → 共通データモデルの変換（純粋）。
//! - [`backend`]: `AiBackend` 実装（プロセスの起動・読取り task・中断）。

pub mod backend;
pub mod cmd;
pub mod store;
pub mod stream;
pub mod view;

pub use backend::{antigravity_capabilities, AntigravityBackend};
