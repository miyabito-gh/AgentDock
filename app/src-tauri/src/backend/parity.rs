//! VS Code拡張との同等性の操作のtrait（段階③、`app/DESIGN_P3.md` §2.2）。
//!
//! [`AiBackend`] は変えず、別traitにする。全メソッドの既定実装は `BackendError::Unsupported`（成功を装わない）。
//! 実装しないバックエンドは自動的に非対応になる。状態を変える操作は [`UserConfirmed`] を要求し、監視・自動処理からは呼べない。
//! 受理不明（`OpAck::Unknown`・`BackendError::OutcomeUnknown`）は再送しない。操作ごとの読取り専用の照合だけを行う（DESIGN_P3 §0.2）。
//!
//! 型は中立名（Codexの `thread`・`collaborationMode`・`serviceTier`・`plugin`・`mcp` を使わない）。
//! ここの型は各タスク（P3-1〜P3-6）が使い始めるときに必要な分だけ広げる。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::backend::{AiBackend, BackendError, BackendResult, UserConfirmed};
use super::model::*;

/// 能力が非対応（または未実装）であることを表すエラー。
pub fn unsupported<T>(op: ParityOp) -> BackendResult<T> {
    Err(BackendError::Unsupported { capability: op.name() })
}

// ───────────────────────────── 操作の入出力（中立型） ─────────────────────────────

/// 会話の分岐（side相談の一時分岐もこれで表す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkParams {
    /// この turn まで（None＝最新の終端turnまで）。
    pub through_turn: Option<ExternalId>,
    /// 保存されない一時の分岐（side相談）。
    pub ephemeral: bool,
    /// 読取り専用の権限で開く。
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkOutcome {
    pub ack: OpAck,
    pub chat: Option<ChatKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ReviewTarget {
    UncommittedChanges,
    BaseBranch { branch: String },
    Commit { sha: String, title: Option<String> },
    Custom { instructions: String },
}

/// 目標（Goal）の状態。未知の値は `Unknown`＋原文（中立名に写像できないものを捨てない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    Unknown { raw: String },
}

/// 目標。トークン数・時間は表示だけで、料金や残量に換算しない（§3.10）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    #[cfg_attr(test, ts(as = "Known<f64>"))]
    pub token_budget: Known<u64>,
    #[cfg_attr(test, ts(as = "Known<f64>"))]
    pub tokens_used: Known<u64>,
    #[cfg_attr(test, ts(as = "Known<f64>"))]
    pub time_used_secs: Known<u64>,
    /// 更新時刻。バックエンドが単位を示していない間は取得扱い（推測で換算しない）。
    pub updated_at: Known<UnixMillis>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GoalUpdate {
    pub objective: Option<String>,
    pub status: Option<GoalStatus>,
    #[cfg_attr(test, ts(as = "Option<f64>"))]
    pub token_budget: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WorkModeInfo {
    pub mode: WorkMode,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: Known<String>,
    pub scope: Known<String>,
    pub enabled: Known<bool>,
    /// Skill定義ファイルのパス（明示指定でそのまま渡す）。
    pub path: String,
    pub errors: Vec<String>,
}

/// 作業フォルダのSkillの一覧。読み込めなかったSkill定義の問題は、Skill単位ではなく一覧全体の `errors`（`パス: 内容`）に出す。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SkillList {
    pub skills: Vec<SkillInfo>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ToolServerConnection {
    NotStarted,
    Starting,
    Connected,
    AuthRequired,
    Failed,
    Cancelled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ToolServerAuth {
    NotLoggedIn,
    Bearer,
    OAuth,
    Unsupported,
    Unknown,
}

/// MCP相当のサーバー1件の状態（項目ごとに `Known`／列挙で持ち、一覧に出ることを会話への反映と扱わない、§4.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ToolServerView {
    pub name: String,
    pub connection: ToolServerConnection,
    pub auth: ToolServerAuth,
    pub tool_count: Known<u32>,
    pub tools_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ExtensionView {
    /// 管理操作に渡す識別子（`名前@提供元` など）。
    pub id: String,
    pub name: String,
    pub installed: Known<bool>,
    pub enabled_in_config: Known<bool>,
    /// 取得手段がない項目は未取得のまま（一覧に出ることを既存会話への反映と扱わない）。
    pub cache_present: Known<bool>,
    pub advertised_in_chat: Known<bool>,
}

/// 拡張・ツールサーバーの管理操作（CLI補助。ユーザーの明示確認の後だけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ExtensionOp {
    Install { id: String },
    Remove { id: String },
    AddToolServer { name: String, command: Vec<String> },
    RemoveToolServer { name: String },
}

/// 設定の前後照合の結果（自動rollbackはしない）。キーは設定の項目名だけで、値は含めない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConfigCompare {
    /// 前後で設定に変化がない。
    Unchanged,
    /// 変化はすべて、操作の対象の項目の下にある。
    ChangedAsExpected,
    /// 操作の対象外の項目が変わっている。
    Unexpected { keys: Vec<String> },
    /// 照合できなかった（設定を読めなかった等）。一致とは扱わない。
    Unverified { reason: String },
}

/// CLIの実行結果の区分。タイムアウト・異常終了は「結果未確認」（成功・失敗のどちらにも変換しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ExtensionOpStatus {
    ExitedZero,
    ExitedNonZero,
    ResultUnconfirmed { reason: String },
    /// 起動できなかった（実行していない）。
    NotRun { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ExtensionOpResult {
    pub status: ExtensionOpStatus,
    pub exit_code: Known<i32>,
    pub config_compare: ConfigCompare,
    /// 設定ファイルの前後のハッシュ（変更の有無の照合用。内容は記録しない）。
    pub config_hash_before: Known<String>,
    pub config_hash_after: Known<String>,
    /// 想定形式なら要約、そうでなければ原文の先頭（`summary_is_raw` が真）。
    pub output_summary: String,
    pub summary_is_raw: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSubmit {
    pub prompt: String,
    pub env_id: String,
    pub branch: Option<String>,
}

/// 委任の送信結果（バックエンドが観測した事実。再送しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CloudSubmitOutcome {
    /// 出力からタスクIDを取れた（不透明な文字列）。
    Submitted { task_id: String },
    /// 送られなかったと分かる（起動できない・引数不正・非0終了・サブコマンドなし）。
    Rejected { message: String },
    /// 送られたか分からない（時間切れ・異常終了・IDを取れない）。再送せず、一覧で照合する。
    Unknown { message: String },
}

/// クラウド側のタスク1件。状態語は原文のまま表示し、AgentDockの状態（§4.1）へ写像しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudTaskInfo {
    pub task_id: String,
    pub state_text: Known<String>,
    pub title: Known<String>,
    pub updated_text: Known<String>,
}

/// 一覧の取得結果。想定形式なら `tasks`、そうでなければ要約せず原文の先頭（`raw`）。鮮度は取得時刻で示す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudTaskList {
    pub observed_at: UnixMillis,
    pub tasks: Vec<CloudTaskInfo>,
    pub raw: Option<String>,
}

/// CLIの終了の観測。成功・失敗を推測しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CloudRunStatus {
    ExitedZero,
    ExitedNonZero { code: i32 },
    Unconfirmed { reason: String },
}

/// status・diff・apply の出力。diff は全文（上限あり）、それ以外は原文の先頭。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct CloudCommandOutput {
    pub status: CloudRunStatus,
    pub text: String,
    pub observed_at: UnixMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum LimitWindowRole {
    Primary,
    Secondary,
}

/// 利用上限の枠1つ分。使用率はバックエンドが示した値のまま（残量・料金への換算や、回復の推測をしない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindowView {
    pub role: LimitWindowRole,
    pub used_percent: f64,
    pub window_minutes: Known<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RateLimitView {
    pub name: Known<String>,
    pub windows: Vec<RateLimitWindowView>,
}

/// 累計の使用量（表示だけ。換算しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    #[cfg_attr(test, ts(as = "Known<f64>"))]
    pub lifetime_tokens: Known<u64>,
    #[cfg_attr(test, ts(as = "Known<f64>"))]
    pub peak_daily_tokens: Known<u64>,
}

/// 接続・アカウント・設定の読取り表示（トークン類は含めない。取れない項目は `Known` のまま）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct BackendStatus {
    pub version: Known<String>,
    pub executable: Known<String>,
    pub experimental_enabled: Known<bool>,
    pub account_kind: Known<String>,
    pub plan: Known<String>,
    /// 利用上限（取れなければ未取得。使用率を残量に換算しない）。
    pub rate_limits: Known<Vec<RateLimitView>>,
    pub usage: Known<UsageSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    pub summary: Known<String>,
}

// ───────────────────────────── trait ─────────────────────────────

#[async_trait]
pub trait ParityOps: AiBackend {
    // ── 分岐・side相談（P3-2・P3-4） ──
    async fn fork_chat(&self, _chat: ChatKey, _params: ForkParams, _confirmed: &UserConfirmed) -> BackendResult<ForkOutcome> {
        unsupported(ParityOp::Fork)
    }

    // ── レビュー・圧縮（P3-2） ──
    async fn start_review(&self, _chat: ChatKey, _target: ReviewTarget, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CodeReview)
    }
    async fn compact(&self, _chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Compact)
    }

    // ── Goal（P3-3） ──
    /// 読取り。表示のためにresumeしない。
    async fn get_goal(&self, _chat: ChatKey) -> BackendResult<Known<Goal>> {
        unsupported(ParityOp::Goal)
    }
    async fn set_goal(&self, _chat: ChatKey, _update: GoalUpdate, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Goal)
    }
    async fn clear_goal(&self, _chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Goal)
    }

    // ── 計画／実行の切替（P3-3） ──
    async fn list_work_modes(&self) -> BackendResult<Vec<WorkModeInfo>> {
        unsupported(ParityOp::WorkMode)
    }

    // ── Skills・指示ファイル（P3-4） ──
    async fn list_skills(&self, _cwd: String, _force_reload: bool) -> BackendResult<SkillList> {
        unsupported(ParityOp::Skills)
    }
    /// 読み込まれた指示ファイルのパス（読取り）。
    async fn instruction_sources(&self, _chat: ChatKey) -> BackendResult<Known<Vec<String>>> {
        unsupported(ParityOp::InstructionFiles)
    }

    // ── ツールサーバー・拡張（P3-5） ──
    async fn list_tool_servers(&self) -> BackendResult<Vec<ToolServerView>> {
        unsupported(ParityOp::ToolServers)
    }
    /// 認可URLを返す（開くのはUIの明示クリック）。
    async fn login_tool_server(&self, _name: String, _confirmed: &UserConfirmed) -> BackendResult<String> {
        unsupported(ParityOp::ToolServers)
    }
    async fn reload_tool_servers(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::ToolServers)
    }
    async fn list_extensions(&self) -> BackendResult<Vec<ExtensionView>> {
        unsupported(ParityOp::Extensions)
    }
    async fn manage_extension(&self, _op: ExtensionOp, _confirmed: &UserConfirmed) -> BackendResult<ExtensionOpResult> {
        unsupported(ParityOp::Extensions)
    }

    // ── クラウド委任（P3-6） ──
    /// 委任の送信（再送しない）。呼び出し側が送る前に記録を保存する。
    async fn cloud_submit(&self, _request: CloudSubmit, _confirmed: &UserConfirmed) -> BackendResult<CloudSubmitOutcome> {
        unsupported(ParityOp::CloudDelegation)
    }
    /// 読取り。ユーザーの操作（パネルを開く・更新）でだけ呼ぶ（定期取得しない）。
    async fn cloud_list(&self) -> BackendResult<CloudTaskList> {
        unsupported(ParityOp::CloudDelegation)
    }
    /// 読取り。出力は原文（先頭）で、状態語をAgentDockの状態へ写像しない。
    async fn cloud_status(&self, _task_id: String) -> BackendResult<CloudCommandOutput> {
        unsupported(ParityOp::CloudDelegation)
    }
    /// 読取り。差分は表示するだけ（ローカルを変えない）。
    async fn cloud_diff(&self, _task_id: String) -> BackendResult<CloudCommandOutput> {
        unsupported(ParityOp::CloudDelegation)
    }
    /// ローカルの作業フォルダを変える操作。呼び出し側が作業中のチャットがないことと確認を済ませている。
    async fn cloud_apply(&self, _task_id: String, _cwd: String, _confirmed: &UserConfirmed) -> BackendResult<CloudCommandOutput> {
        unsupported(ParityOp::CloudDelegation)
    }

    // ── 状態・速度・memories（P3-3） ──
    async fn backend_status(&self) -> BackendResult<BackendStatus> {
        unsupported(ParityOp::BackendStatus)
    }
    async fn memory_status(&self) -> BackendResult<MemoryStatus> {
        unsupported(ParityOp::Memory)
    }
    async fn set_memory_mode(&self, _chat: ChatKey, _mode: String, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Memory)
    }
    async fn reset_memory(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Memory)
    }
}
