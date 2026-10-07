//! クラウド委任（#13、P3-6）のIPC型。CLI補助（`codex cloud`）で行い、App Serverに経路はない。
//!
//! - 委任は依頼文・環境ID・ブランチを入力して明示確認した後だけ。送る前に `CloudTaskRecord`（`Submitting`）を保存する。
//! - 状態語は原文のまま表示し、AgentDockの状態（§4.1）へ写像しない。鮮度は取得時刻で示す（定期取得しない）。
//! - 取込み（apply）は作業フォルダを変える操作。作業中のチャットがないことを確認し、明示確認の後だけ。結果は終了コードと `git status` の差で示す。

use serde::{Deserialize, Serialize};

use super::model::*;
use super::parity::CloudRunStatus;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudEnvArgs {
    pub folder: String,
}

/// 作業フォルダに対する委任の既定値（記憶した環境ID・現在のブランチ）。取得できなければ None（空文字で代用しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudEnvHint {
    /// 記憶の単位（リポジトリのルート。Gitでなければ None）。
    pub repo_key: Option<String>,
    pub env_id: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudSubmitArgs {
    pub prompt: String,
    pub env_id: String,
    pub branch: Option<String>,
    /// 作業フォルダ（環境IDの記憶・取込み先の既定に使う）。
    pub folder: Option<String>,
    pub origin_chat: Option<ChatKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudTaskIdArgs {
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudApplyArgs {
    pub task_id: String,
    /// 取込み先の作業フォルダ（Gitのリポジトリ内であること）。
    pub folder: String,
}

/// 取込みの結果。成功・失敗を断定せず、終了コードと `git status` の前後の差を示す（Gitで読めなければ未取得）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudApplyResult {
    pub status: CloudRunStatus,
    /// 出力の先頭（原文）。
    pub output_head: String,
    /// 取込みの前後で、変更状態が変わった（新しく現れた）パス。
    pub changed_paths: Known<Vec<String>>,
    pub folder: String,
}
