//! Gitの準備（`git init`＋初回コミット）とアプリからのプッシュ（DESIGN_ORG_INTAKE §2・§3）のIPC型。
//!
//! - 状態を変える操作（準備・プッシュ）は確認ダイアログの内容に紐づく値（`previewToken`・`expectedSha`）を持つ。
//! - 結果が分からないものは `Unknown` で運び、成功に変換しない。URL の資格情報は載せない。

use serde::{Deserialize, Serialize};

// ───────────────────────────── リポジトリの状態 ─────────────────────────────

/// 作業フォルダのGitの状態（読取りのみで判定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RepoStatusView {
    /// リポジトリのルートで、コミットがある。
    Ready { root: String, branch: String },
    /// 上位のリポジトリの一部で、コミットがある（そのまま使う。準備しない）。
    Nested { root: String },
    /// Gitのリポジトリではない。
    NotRepo,
    /// リポジトリだがコミットがない。
    NoCommit { root: String },
    /// 前回のAgentDockの準備が途中で止まっている（印のある `.git` でコミットなし）。
    PrepInterrupted { root: String },
    /// Gitが所有者の違いで拒否した。`repo` はGitが示したリポジトリの場所（示されなければ作業フォルダ。許可の対象に使う）。
    OwnerMismatch { repo: String },
    /// 確認できなかった（Gitを起動できない・時間切れなど）。
    Unavailable { message: String },
}

pub type RepoStatus = RepoStatusView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitFolderArgs {
    pub folder: String,
}

/// 作業フォルダの未コミットの変更（読取りのみ）。組織実行の開始前の注意に使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DirtySummaryView {
    /// 未コミットの変更（追跡中の変更・未追跡）のファイル数。
    pub total: u32,
    /// 代表のパス（先頭の数件。リポジトリのルートからの相対）。
    pub sample: Vec<String>,
}

// ───────────────────────────── Gitの準備 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum PrepKind {
    /// `.git` を新しく作る。
    NotRepo,
    /// 利用者の既存リポジトリ（コミットなし）。`.git` は消さない。
    NoCommit,
    /// 前回の途中の準備の続き（コミットだけ）。
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PrepFileView {
    pub path: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub size: u64,
}

/// コミットから外す候補（巨大ファイル・一時ファイル）。追記するときは利用者に示した内容だけを書く。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PrepExcludeView {
    pub path: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub size: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PrepAuthorView {
    pub name: String,
    pub email: String,
    /// 利用者のGit設定がなく、AgentDock名義で作る。
    pub is_agentdock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RepoPrepPreview {
    pub folder: String,
    pub kind: PrepKind,
    /// 最初のコミットに入るファイル（除外を追記する指定なら、除外後）。
    pub file_count: u32,
    #[cfg_attr(test, ts(type = "number"))]
    pub total_bytes: u64,
    /// 大きいファイル上位10件。
    pub largest: Vec<PrepFileView>,
    /// 外す候補。`applyExcludes` を真にして準備すると、`excludeLines` を `.git\info\exclude` へ追記して外す。
    pub suggested_excludes: Vec<PrepExcludeView>,
    pub exclude_lines: Vec<String>,
    pub apply_excludes: bool,
    /// 準備できない理由（断る）。ある間は実行できない。
    pub refusal: Option<String>,
    /// 注意だけ（同期フォルダなど）。
    pub notes: Vec<String>,
    pub author: PrepAuthorView,
    /// ファイル一覧の指紋。実行時に再計算して一致しなければ実行しない。
    pub preview_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitPrepArgs {
    pub folder: String,
    /// 除外の候補を `.git\info\exclude` へ追記する（既定は偽。追記のみ・既存は上書きしない）。
    #[serde(default)]
    pub apply_excludes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitPrepareArgs {
    pub folder: String,
    pub preview_token: String,
    #[serde(default)]
    pub apply_excludes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PrepOutcome {
    /// 初回コミットを確認できた。
    Prepared { commit: String },
    /// 明確に失敗し、AgentDockが作った `.git` だけを取り除いた。
    RolledBack { reason: String },
    /// 失敗した（`leftover` は残っている途中の `.git` の場所。既存のリポジトリや取り除けなかったとき）。
    Failed { reason: String, leftover: Option<String> },
    /// 結果を確認できない（時間切れなど）。実際の状態を読み直して示す。
    Unknown { status: RepoStatusView },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RepoPrepResult {
    pub outcome: PrepOutcome,
}

// ───────────────────────────── プッシュ ─────────────────────────────

/// プッシュできない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum PushBlocker {
    NoRemote,
    Detached,
    /// `agentdock/` で始まるブランチ（AgentDockの作業用）。
    InternalBranch,
    NothingToPush,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PushBlockerView {
    pub kind: PushBlocker,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PushCommitView {
    pub sha: String,
    pub short: String,
    pub author: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PushPreviewArgs {
    pub folder: String,
    /// リモートが複数で追跡先がないときの選択（None は `origin`、なければ先頭）。
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PushPreview {
    pub folder: String,
    pub remotes: Vec<String>,
    pub remote: Option<String>,
    /// 送り先のURL（資格情報は伏せる）。
    pub url: Option<String>,
    pub branch: Option<String>,
    /// 追跡先（`origin/main` の形）。なければ None。
    pub upstream: Option<String>,
    /// 確認時のHEAD。実行はこのコミットだけを送る。
    pub head_sha: Option<String>,
    pub commits: Vec<PushCommitView>,
    pub commit_total: u32,
    pub truncated: bool,
    /// 未コミットの変更の件数（送られない）。
    pub dirty_count: u32,
    pub blocker: Option<PushBlockerView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitPushArgs {
    pub folder: String,
    pub remote: String,
    pub branch: String,
    pub expected_sha: String,
    #[serde(default)]
    pub set_upstream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GitPushVerifyArgs {
    pub folder: String,
    pub remote: String,
    pub branch: String,
    pub expected_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PushRejectReason {
    /// リモートが先に進んでいる（pull が必要）。
    NonFastForward,
    /// プッシュ前のフックが断った。
    HookDeclined,
    Other { text: String },
}

/// プッシュの結果。`Unknown` は送れたか確認できない（成功として扱わない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PushResult {
    /// `upstream_note` は、追跡先の設定を頼んだのに設定できなかったときの理由（プッシュ自体は成功している）。
    Pushed { sha: String, upstream_note: Option<String> },
    Rejected { reason: PushRejectReason },
    AuthFailed,
    NetworkError,
    Unknown { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PushVerify {
    /// リモートのブランチが送ったコミットを指している。
    Confirmed,
    /// リモートにブランチがない、または別のコミットを指している。
    NotThere,
    Unavailable { message: String },
}
