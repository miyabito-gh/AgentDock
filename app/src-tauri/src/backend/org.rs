//! 組織実行（DESIGN_ORG・P-ORG-7a）のUI⇔ホストのIPC型。Codex固有の型は持たない。
//!
//! - 状態は2軸（業務状態・確認状態）を別の値で運び、どちらも `label`（文字）と `icon`（名前）を持つ。色だけで区別させない。
//! - 取得できない値は `None`／空配列にして、0や空文字で代用しない。
//! - 引数の型は、選択・閲覧だけでは何も送らない（実行するコマンドだけが送信・再開・承認の入口になる）。

use serde::{Deserialize, Serialize};

use super::model::{ChatKey, TurnEnd};

// ───────────────────────────── 状態（2軸） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgTaskPhase {
    Pending,
    Ready,
    Running,
    Blocked,
    Reviewing,
    Rework,
    Completed,
    Failed,
    Cancelled,
}

/// 止まった・失敗した理由の種類（`Blocked`／`Failed` のときだけ付く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgReason {
    AwaitingApproval,
    Manual,
    ManualStop,
    Limit,
    NeedsAttention,
    ExecutionFailed,
    InvalidArtifact,
    LimitReached,
}

/// 業務状態（軸1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgStatusView {
    pub phase: OrgTaskPhase,
    pub reason: Option<OrgReason>,
    /// 表示文（例「実行中」「止まっています（要対応）」）。
    pub label: String,
    /// アイコン名。語彙: hourglass／circle／play／pause／eye／rotate／check／x／ban。
    pub icon: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgConfirmCode {
    Confirmed,
    SendUnknown,
    NeedsReconcile,
    InterruptRequested,
    StopUnconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgReconcileCause {
    Disconnect,
    Wake,
    Restart,
}

/// 確認状態（軸2）。`Confirmed` 以外のタスクは止まって見える（自動では進めない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgConfirmView {
    pub state: OrgConfirmCode,
    pub cause: Option<OrgReconcileCause>,
    pub label: String,
    /// アイコン名。語彙: shield-check／help-circle／refresh／stop-circle／alert-triangle。
    pub icon: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgHaltKind {
    Restart,
    Shutdown,
    User,
    NeedsResume,
    SandboxUnhealthy,
    AwaitingPlanApproval,
}

/// ワークフロー全体の停止（新規ディスパッチが止まっている理由）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgHaltView {
    pub kind: OrgHaltKind,
    pub label: String,
    pub icon: String,
    /// 「再開」で解除できるか（計画の承認待ちは承認でだけ解除される）。
    pub resumable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgEffect {
    UserTreeWrite,
    PushMergeTag,
    OutsideWorkdir,
    NetworkOrInstall,
    SecretsOrAuth,
    DeletionPrimary,
}

// ───────────────────────────── 照合・trust・通知 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgWorktreeKind {
    NotOwned,
    Missing,
    Clean,
    Uncommitted,
    Unreadable,
}

/// 作業場所の照合結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgWorktreeView {
    pub kind: OrgWorktreeKind,
    /// `Uncommitted` のときの変更件数。
    pub entries: Option<u32>,
    pub label: String,
}

/// 再起動・wake後の照合（読取りだけ）の直近の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgReconcileView {
    pub task: String,
    /// 履歴を読めたか。
    pub turn_read: bool,
    /// 履歴で終端を確認できたときだけ。None は「確認できていない」。
    pub turn_end: Option<TurnEnd>,
    pub worktree: OrgWorktreeView,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgTrustKind {
    CodexTrustAppended,
    Other,
    Unverified,
}

/// config.toml の前後照合の表示用（AgentDock は書いていない。自動で戻さない・送信を止めない）。`Unchanged` のときは出さない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTrustView {
    pub kind: OrgTrustKind,
    /// 情報（追記だけ）か注意（想定外・未確認）か。
    pub severity: OrgSeverity,
    pub text: String,
    /// `CodexTrustAppended` のとき、追記された作業フォルダ。
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgSeverity {
    Info,
    Attention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgNoticeKind {
    PlanNeedsApproval,
    PlanAutoApproved,
    TaskCompleted,
    TaskFailed,
    TaskBlocked,
    TurnLimitReached,
    UsageLimit,
    SandboxUnhealthy,
    DescendantsUnconfirmed,
    CancelUnconfirmed,
    WorkflowCompleted,
    Refused,
    Config,
    SendRefused,
    PersistFailed,
    Reconciled,
    Stalled,
}

/// 直近の通知の1件（新しいものが後ろ）。本文は定型文で、会話内容は入れない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgNoticeView {
    #[cfg_attr(test, ts(type = "number"))]
    pub serial: u64,
    pub kind: OrgNoticeKind,
    pub severity: OrgSeverity,
    pub task: Option<String>,
    pub text: String,
}

/// 計画を自動承認できなかった理由の1件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgConfirmReasonView {
    pub task: Option<String>,
    pub text: String,
}

/// 計画承認カードの材料。理由は通知（メモリ上）から取るので、再起動後は `reasons_known=false`（承認待ちの事実だけが分かる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgPlanApprovalView {
    pub reasons: Vec<OrgConfirmReasonView>,
    pub reasons_known: bool,
}

// ───────────────────────────── 画面の写し ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTaskView {
    pub id: String,
    pub role: String,
    pub title: String,
    pub deps: Vec<String>,
    /// 終わっていない依存（「T2,T3待ち」の元）。
    pub waiting_on: Vec<String>,
    pub status: OrgStatusView,
    pub confirm: OrgConfirmView,
    /// 試行（1 + 差し戻し回数）と、その上限。
    pub attempts: u32,
    pub max_attempts: u32,
    /// 役割チャット（なければ None。選んで開くのは閲覧だけ）。
    pub chat: Option<ChatKey>,
    pub commit: Option<String>,
    pub effects: Vec<OrgEffect>,
    /// 実行中のまま新しい出力がない、と通知した（その後の活動は反映しない。通知だけで、自動では止めない）。
    pub stall_notified: bool,
    pub reconcile: Option<OrgReconcileView>,
    /// 取り消せるか（終わっていないタスク）。
    pub can_cancel: bool,
    /// 「手動で完了にする」を出せるか（手動に切り替え済みで確認状態が確定）。
    pub can_accept_manual: bool,
    /// 作成結果が不明で、利用者のチャット選択が要るか。
    pub needs_chat_choice: bool,
    /// 「やり直す」を出せるか（送っていないことが確かで、初回のturnが受理されていない）。受理不明のものは出さない。
    pub can_retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgLimitsView {
    pub max_writers: u32,
    pub max_total: u32,
    pub max_rework: u32,
    pub max_auto_replans: u32,
    pub max_turns: u32,
    pub max_tasks: u32,
    pub stagnation_minutes: u32,
}

/// 使用量。turn数は数えた値。トークンは取れた行の合計で、取れなかった行は件数で示す（0で代用しない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgUsageView {
    pub turns_used: u32,
    pub turns_remaining: u32,
    pub auto_replans_used: u32,
    /// 取れた行が1件もなければ None。
    #[cfg_attr(test, ts(type = "number | null"))]
    pub known_tokens: Option<u64>,
    pub rows: u32,
    pub rows_without_tokens: u32,
}

/// 組織実行1件の画面向けの写し。選んだだけでは何も送らない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgWorkflowView {
    pub workflow_id: String,
    pub title: Option<String>,
    pub request: String,
    pub repo: String,
    pub halt: Option<OrgHaltView>,
    pub limits: OrgLimitsView,
    pub usage: OrgUsageView,
    pub tasks: Vec<OrgTaskView>,
    pub plan_approval: Option<OrgPlanApprovalView>,
    /// `halt=SandboxUnhealthy` のときの案内文。
    pub sandbox_notice: Option<String>,
    pub trust: Option<OrgTrustView>,
    pub notices: Vec<OrgNoticeView>,
    /// 新しい送信権があるか（なければ「再開」まで何も送らない）。
    pub dispatching: bool,
    /// 全タスクが終わっているか（完了帯の表示用）。
    pub finished: bool,
}

/// 現在の組織実行（なければ `workflow=None`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgSnapshotView {
    pub workflow: Option<OrgWorkflowView>,
    /// 停滞とみなす時間（分）。
    pub stall_minutes: u32,
}

// ───────────────────────────── 役割定義 ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRoleView {
    pub id: String,
    pub display_name: String,
    /// そのファイル名（フォルダ内。開くときの指定に使う）。
    pub file: String,
    pub enabled: bool,
    /// 権限を広げている理由（空なら基準の範囲内）。
    pub widened: Vec<String>,
    /// 今の権限に対する確認が済んでいるか。
    pub confirmed: bool,
    /// 確認画面に出した権限の指紋。有効化はこの値が今の権限と一致するときだけ受ける。
    pub fingerprint: String,
    pub sandbox: String,
    pub approval: String,
    pub workplace: String,
    pub allow_network: bool,
    pub extra_writable_dirs: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_concurrent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRoleProblemView {
    pub file: String,
    pub reasons: Vec<String>,
    /// 読めないファイルを退避した先。
    pub aside: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRolesView {
    /// まだ読み込んでいなければ false（役割は空。読み込んだ空とは違う）。
    pub loaded: bool,
    pub roles: Vec<OrgRoleView>,
    pub problems: Vec<OrgRoleProblemView>,
    pub created: Vec<String>,
}

// ───────────────────────────── 引数 ─────────────────────────────

/// 開始。実行するコマンド（利用者の「開始」の操作）。並列数は未指定なら既定（同時書込み2・全体3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartOrgArgs {
    pub request: String,
    pub repo: String,
    /// 1〜4（範囲外は拒否）。
    pub max_writers: Option<u32>,
    pub max_total: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgWorkflowArgs {
    /// 画面が見ていたワークフロー。動いているものと違えば何もせず拒否する（古い画面からの操作を防ぐ）。
    pub workflow_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTaskArgs {
    pub workflow_id: String,
    pub task: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgCancelScope {
    /// 1タスク。
    Task { task: String },
    /// 終わっていないタスクすべて（計画承認カードの「取り消す」）。
    Workflow,
}

/// 取消し（利用者の操作だけが `cancelled` にできる）。Codex側の停止は確認できたときだけ確定とする（未確認の案内が通知で出る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgCancelArgs {
    pub workflow_id: String,
    pub scope: OrgCancelScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRaiseTurnLimitArgs {
    pub workflow_id: String,
    /// 新しいturn上限（今の値より大きいこと）。
    pub to: u32,
}

/// 作成結果が不明なタスクの、結び付け先の候補。**自動では結び付けない**（利用者が選ぶ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgChatCandidate {
    pub chat: ChatKey,
    pub name: Option<String>,
    /// 選べるか。AgentDockが作った組織の記録のない（他のタスクに結び付いていない）チャットだけ。
    pub selectable: bool,
    /// 選べない理由。
    pub why_not: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgChooseChatArgs {
    pub workflow_id: String,
    pub task: String,
    pub chat: ChatKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgEnableRoleArgs {
    pub role_id: String,
    /// 確認画面に出した `OrgRoleView.fingerprint`。今の権限と違えば有効にしない（確認後にファイルが変わった場合）。
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgOpenRoleFileArgs {
    /// `OrgRoleView.file`／`OrgRoleProblemView.file`（フォルダ内のファイル名。パス区切りは受けない）。
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgSetStallArgs {
    /// 1〜1440分。
    pub minutes: u32,
}

/// 開始の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartOrgResult {
    pub workflow_id: String,
}

/// 再開の結果。再開できなかったチャットは live に戻していない（その分は送らずに止まる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgResumeResult {
    pub resumed: Vec<ChatKey>,
    pub failed: Vec<OrgResumeFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgResumeFailure {
    pub chat: ChatKey,
    pub message: String,
}
