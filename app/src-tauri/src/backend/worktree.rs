//! Git worktree（#14、P3-7）のIPC型。AgentDock管理の操作で、バックエンドには依存しない。
//!
//! - `GitInfo`: フォルダがGitのリポジトリか、現在のブランチ・変更件数、worktreeの一覧（読取りのみ）。
//! - 作成は `WorktreeRecord`（`model`）を `Creating` で保存してからgitを実行する。削除はAgentDockが作った記録があるものだけ。
//! - 取得できなかった値は `Known::NotFetched`・`None`（0・空文字で代用しない）。

use serde::{Deserialize, Serialize};

use super::changes::ListStatus;
use super::model::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitInfoArgs {
    pub folder: String,
}

/// worktreeの出所。`Main` はリポジトリ本体の作業ツリー、`AgentDock` は台帳に記録のあるもの、`External` はそれ以外
/// （作業フォルダとして選べるが、AgentDockは削除しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorktreeOrigin {
    Main,
    AgentDock { record: LocalId },
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    pub path: String,
    /// ブランチ名。detached・bare なら None。
    pub branch: Option<String>,
    pub head: Option<String>,
    pub origin: WorktreeOrigin,
    pub detached: bool,
    pub locked: bool,
    /// Gitが「実体が見つからない」としている。
    pub prunable: bool,
}

/// `status` が `Ready` のときだけ他の項目に意味がある。`NotSupported` はGitなし・リポジトリでない（理由つき）、`NotFetched` は取得できなかった。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub folder: String,
    pub status: ListStatus,
    pub root: Option<String>,
    /// 現在のブランチ名（detachedなら `HEAD`）。
    pub branch: Option<String>,
    /// 現在のコミット。コミットがまだない・取得できなければ None。
    pub head: Option<String>,
    /// 未コミット・未追跡の変更の件数。
    pub dirty_count: Known<u32>,
    pub worktrees: Vec<WorktreeEntry>,
}

/// worktreeの作成。`branch`・`base` を省略すると、ブランチは `agentdock/<チャット名を安全化>-<短いID>`、基点は現在のコミット（HEAD）。
/// 元の作業ツリーの未コミット変更は持ち込まれない（新しいコミットからの作業ツリー）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeArgs {
    pub repo_root: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    /// ブランチ名の元にするチャット名（なければ `chat`）。
    pub chat_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WorktreeIdArgs {
    pub id: LocalId,
}

/// 削除できない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum RemoveBlockReason {
    OutsideRoot,
    InUse,
    MainWorktree,
    NotListed,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorktreeRemoveVerdict {
    /// 通常の削除（`--force` なし）。
    Allowed,
    /// 未コミットの変更がある。「変更を破棄して削除」は2段目の確認の後だけ。
    NeedsDiscard { dirty: u32 },
    /// Gitの一覧にもディスクにもない。台帳の記録だけを外す。
    AlreadyGone,
    Blocked { reason: RemoveBlockReason },
}

/// 削除確認に出す内容（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRemovePreview {
    pub record: WorktreeRecord,
    pub verdict: WorktreeRemoveVerdict,
    /// このworktreeを使っているチャット。
    pub using_chats: Vec<ChatKey>,
    /// 未コミットの変更の件数（読めなければ未取得）。
    pub dirty: Known<u32>,
    /// 判定の補足（Gitの出力など。読めなかった理由）。
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RemoveWorktreeArgs {
    pub id: LocalId,
    /// 未コミットの変更を破棄して削除する（2段目の確認の後だけ）。
    pub force: bool,
    /// マージ済みならブランチも削除する（`git branch -d`。マージされていなければ残る）。
    pub delete_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BranchOutcome {
    /// 削除を選んでいない（ブランチは残る）。
    Kept,
    Deleted,
    /// 削除を選んだが削除されなかった（マージされていない等）。ブランチは残る。
    NotDeleted { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorktreeRemoveOutcome {
    Removed { branch: BranchOutcome },
    /// すでに実体がなかったので、台帳の記録だけを外した（ブランチは残る）。
    RecordDropped,
}
