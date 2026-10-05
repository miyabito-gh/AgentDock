//! Codex App Server用のstdio JSON-RPCクライアント（T2）。意味解釈は持たない（変換はT3）。
//!
//! - [`id`]: JSON-RPC IDの可逆符号化（`n:5` / `s:abc`）。
//! - [`rpc`]: 入出力に依存しない要求/応答対応・通知配信・サーバー要求・切断処理。
//! - [`process`]: codex.exeの起動（`app-server`、stdio）・版確認・initialize握手。

pub mod id;
pub mod process;
pub mod rpc;

pub use process::{check_version, CodexProcess, LaunchError, TARGET_VERSION};
pub use rpc::{RpcClient, RpcEvent};
