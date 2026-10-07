//! Codex App Server用のstdio JSON-RPCクライアント（T2）。意味解釈は持たない（変換はT3）。
//!
//! - [`id`]: JSON-RPC IDの可逆符号化（`n:5` / `s:abc`）。
//! - [`rpc`]: 入出力に依存しない要求/応答対応・通知配信・サーバー要求・切断処理。
//! - [`wire`]・[`convert`]・[`tree`]・[`events`]: Codex型→共通モデルの純粋な変換（T3）。
//! - [`adapter`]: `AiBackend` のCodex実装（T3）。
//! - [`process`]: codex.exeの起動（`app-server`、stdio）・版確認・initialize握手。

pub mod adapter;
pub mod cli;
pub mod convert;
pub mod events;
pub mod id;
pub mod outbox;
pub mod parity;
pub mod parity_table;
pub mod process;
pub mod rpc;
pub mod tree;
pub mod wire;

pub use process::{check_version, CodexProcess, LaunchError, TARGET_VERSION};
pub use rpc::{RpcClient, RpcEvent};
pub use adapter::CodexBackend;
