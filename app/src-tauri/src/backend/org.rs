//! 組織実行（DESIGN_ORG・P-ORG-7a）のUI⇔ホストのIPC型。Codex固有の型は持たない。
//!
//! - 状態は2軸（業務状態・確認状態）を別の値で運び、どちらも `label`（文字）と `icon`（名前）を持つ。色だけで区別させない。
//! - 取得できない値は `None`／空配列にして、0や空文字で代用しない。
//! - 引数の型は、選択・閲覧だけでは何も送らない（実行するコマンドだけが送信・再開・承認の入口になる）。

use serde::{Deserialize, Serialize};

use super::model::{ChatKey, Known, OrgWorktreeAccess, TurnEnd, UnixMillis};

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
    /// 差し戻しの辺の周回が上限に達し、利用者の判断（もう1周・取消し）を待っている。
    RoundLimit,
    /// 検証で不合格が続き、差し戻しの上限に達した（利用者が「もう一度直させる」「このまま受け入れる」「失敗として確定」を選ぶ）。
    ReworkLimit,
    /// Reviewer は合格としたが、実行した確認のコマンドが失敗した。利用者が「もう一度直させる」「このまま受け入れる」「失敗として確定」を選ぶ。
    CheckFailed,
    /// 利用上限で turn が失敗した。利用者が「新しい試行で再送」「失敗として確定」などを選ぶ。
    UsageLimit,
    /// turn が失敗した（利用上限以外、または区分不明）。利用者が選ぶ。
    TurnFailed,
    ExecutionFailed,
    InvalidArtifact,
    LimitReached,
}

/// 計画のタスクの種類（調査は、ファイルを変えず結果を報告に書く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgTaskKind {
    #[default]
    Change,
    Investigate,
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
    /// 開始時のグラフ定義の写しを使えない（「再開」では解除できない）。
    DefinitionUnavailable,
    /// 利用上限で turn が失敗したため全体を止めている（「再開」で解除。送るのは利用者が選んだタスクだけ）。
    UsageLimit,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgRetryKind {
    /// 送っていないことが確かなので、最初からやり直す。
    Unsent,
    /// 前の turn が結果なしで終わったので、同じチャットへ新しい試行を送る。
    NewAttempt,
}

/// 再送の見込み（利用者の操作1回で1回だけ送る。自動では送らない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRetryView {
    pub kind: OrgRetryKind,
    /// 前の turn の終わり方（例「利用上限」「失敗」「中断」）。`Unsent` のときは空。
    pub reason_label: String,
    /// これまでの再送回数と、その上限。
    pub used: u32,
    pub max: u32,
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
    /// 途中の出力（スロット）を保存できなかった（以後は何も送らない）。
    SlotWriteFailed,
    /// 定義（グラフ・役割）についての案内（開始時の定義との違い・写しが使えない・スロットを読めない）。
    DefinitionNote,
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
    /// 種類（調査か、変更か）。
    #[serde(default)]
    pub kind: OrgTaskKind,
    pub title: String,
    pub deps: Vec<String>,
    /// 終わっていない依存（「T2,T3待ち」の元）。
    pub waiting_on: Vec<String>,
    pub status: OrgStatusView,
    pub confirm: OrgConfirmView,
    /// 試行（1 + 差し戻し回数）と、その上限。
    pub attempts: u32,
    pub max_attempts: u32,
    /// 利用者が押した再送（新しい試行）の回数。差し戻し（上の試行）とは別の数。
    #[serde(default)]
    pub retries_used: u32,
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
    /// 新しい試行の再送もできるときも真（`retry.is_some()` と同値）。
    pub can_retry: bool,
    /// 再送の見込み（できないときは None。理由は `retry_refusal`）。
    #[serde(default)]
    pub retry: Option<OrgRetryView>,
    /// 再送できない理由（利用上限・失敗・中断で止まっているときだけ。ボタンの title に出す）。
    #[serde(default)]
    pub retry_refusal: Option<String>,
    /// 「失敗として確定」を出せるか（止まっていて確認状態が確定）。
    #[serde(default)]
    pub can_fail: bool,
    /// 「結果を取り込む」を出せるか（手動に切り替え済みで確認状態が確定、チャットあり）。
    #[serde(default)]
    pub can_adopt_manual: bool,
    /// 「止まっています（要対応）」の理由の1行（メモリ上の通知から。再起動で失われたときは、その旨）。要対応でなければ None。
    pub attention_note: Option<String>,
    /// 今の業務状態に入った時刻（記録から辿れないときは None。経過時間は出さない）。
    #[serde(default)]
    pub since: Option<UnixMillis>,
    /// 割り込み指示の経路（送れないときは理由）。過去の記録では常に `Refused`。
    pub interject: OrgInterjectRouteView,
    /// スキップできるときの確認の材料。
    #[serde(default)]
    pub skip: Option<OrgSkipView>,
    /// スキップできないときの理由（計画の作業・検証タスクだけが対象。それ以外は None）。
    #[serde(default)]
    pub skip_refusal: Option<String>,
    /// 差し戻せるか（検証待ち、または差し戻し上限で止まっている作業タスク）。
    #[serde(default)]
    pub can_send_back: bool,
    /// 差し戻しの指摘が必須か（差し戻し上限で止まっているときは任意）。
    #[serde(default)]
    pub send_back_needs_note: bool,
    /// 差し戻せない理由（差し戻しの対象のタスクでだけ）。
    #[serde(default)]
    pub send_back_refusal: Option<String>,
    /// 「このまま受け入れる」を出せるか（差し戻し上限で止まっている作業タスク）。
    #[serde(default)]
    pub can_accept_as_is: bool,
    /// 保留にできるか（まだ送っていない計画の作業・検証タスク）。
    #[serde(default)]
    pub can_hold: bool,
    /// 保留中か（スケジューラは送らない）。
    #[serde(default)]
    pub held: bool,
    /// このタスクにまだ届いていない追加指示の件数。
    #[serde(default)]
    pub notes_pending: u32,
    /// 止まっている理由の説明（止まっていない・終わったタスクは None）。表示用の導出で、状態は変えない。
    #[serde(default)]
    pub stop: Option<OrgStopView>,
}

/// 止まり方の分類（「止まっています」カードの見出しの元）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgStopKind {
    ReworkLimit,
    CheckFailed,
    NothingWritten,
    NeedsAttention,
    UsageLimit,
    TurnFailed,
    Manual,
    ManualStop,
    RoundLimit,
    Limit,
    AwaitingApproval,
    /// 送ったか確認できていません（受理不明）。
    SendUnknown,
    /// 切断・再起動などの後で、状態を読み直す前。
    NeedsReconcile,
    /// 中断を要求したが、停止は未確認。
    InterruptRequested,
    StopUnconfirmed,
    /// 先行タスクが止まっている／失敗したため待っている。
    DependencyBlocked,
    /// 成果物（調査報告を含む）を読めず失敗した。
    InvalidArtifact,
}

/// 今できる操作の識別子（ボタンの出し分けの元。有効かどうかは既存の can_* と一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgStopAction {
    SendBack,
    AcceptAsIs,
    Fail,
    RetryNew,
    RetryUnsent,
    Cancel,
    AdoptManual,
    AcceptManual,
    ChooseChat,
    OpenChat,
    History,
}

/// 手がかり1件。`known` が偽のときの `text` は「記録されていません」系の文（0や空で埋めない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgStopEvidence {
    pub label: String,
    /// 切り詰め済みの行（最大3行・各120字）。
    pub lines: Vec<String>,
    pub known: bool,
    /// 切り詰めた（全文はチャットか経過で見る）。
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgStopView {
    pub kind: OrgStopKind,
    /// 見出し（1行。利用者の語）。
    pub headline: String,
    /// 事実の短い行。
    pub facts: Vec<String>,
    pub evidence: Vec<OrgStopEvidence>,
    pub actions: Vec<OrgStopAction>,
    /// 次の一手（1〜2文）。
    pub hint: String,
    /// 確認状態が確定でないための表示（「止まった」ではなく事実どおりに書く）。
    pub unconfirmed: bool,
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
    /// タスクごとの「新しい試行での再送」の上限（0〜10）。
    pub max_retries: u32,
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
    /// グラフで動くワークフローの流れ（ノード・辺・周回）。従来の計画経路・過去の記録では None。
    #[serde(default)]
    pub graph: Option<OrgGraphView>,
    /// 保存データから読んだ過去のワークフロー（読み取り専用。操作は受け付けない）。
    #[serde(default)]
    pub readonly: bool,
    /// アーカイブした時刻（アプリ内だけの操作。左一覧から隠す）。
    #[serde(default)]
    pub archived_at: Option<UnixMillis>,
    /// 削除が途中のとき、その進み（済／失敗）。続きは「削除を続ける」から。
    #[serde(default)]
    pub deletion: Option<OrgDeletionView>,
    /// 追加指示（古い順。取り消したものも含む）。
    #[serde(default)]
    pub notes: Vec<OrgNoteView>,
    /// 割り込み指示（決着していないものと最新の20件）。
    #[serde(default)]
    pub interjects: Vec<OrgInterjectView>,
    /// 計画の作成後に追加した追加指示の件数（計画には反映されていない。承認カードに出す）。
    #[serde(default)]
    pub notes_after_plan: u32,
    /// 成果物の印（レビュー待ち・取り込み済みの件数。エンジン状態と台帳だけから導く。Gitの事実は成果物パネルで確認する）。
    #[serde(default)]
    pub artifact_hint: Option<super::org_artifacts::OrgArtifactHint>,
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
    /// 保存済みの過去のワークフロー（新しい順。最近の一部だけ。`workflow` と同じものは含めない。読み取り専用）。
    #[serde(default)]
    pub history: Vec<OrgWorkflowView>,
    /// アーカイブしたワークフロー（新しい順に最大50件。読み取り専用）。
    #[serde(default)]
    pub archived: Vec<OrgWorkflowView>,
    /// 履歴の別枠（削除途中）に収まらず一覧に出さなかった、削除途中のワークフローの件数。
    #[serde(default)]
    pub deletion_overflow: u32,
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

// ───────────────────────────── グラフ（流れ）の表示 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgNodeKind {
    Agent,
    Gate,
    Router,
    PlannedTasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgEdgeKind {
    /// 先へ進む辺。
    Forward,
    /// 前の工程へ戻る辺（周回。回数に上限がある）。
    Feedback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgOnExhausted {
    /// 上限に達したら戻り先を失敗にする。
    Fail,
    /// 上限に達したら利用者に聞く（もう1周・取消し）。
    Ask,
}

/// 判定（スロットの値のうち、辺の選択に使う項目）。値を読めなかったときは「未取得」で、空や既定値で代用しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgVerdict {
    /// まだ出力がない。
    Pending,
    /// 出力はあるが、値を読めなかった。
    NotObtained,
    Value { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGateChoiceView {
    pub id: String,
    pub label: String,
    /// 理由の入力が必須か。
    pub needs_note: bool,
}

/// gate が見せるスロットの内容（長いものは切る）。`text=None` は未取得（まだ出力がない、または読めなかった）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGateShowView {
    pub slot: String,
    pub text: Option<String>,
    pub truncated: bool,
}

/// gate の工程。`awaiting=true` のときだけ、利用者の判断を受け付ける。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGateView {
    pub prompt: String,
    pub choices: Vec<OrgGateChoiceView>,
    pub shows: Vec<OrgGateShowView>,
    pub awaiting: bool,
}

/// 周回が上限に達して止まっている戻り先の工程。`awaiting=true` のときだけ「もう1周」を受け付ける。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRoundLimitView {
    /// 上限に達した辺（`from->to`）。
    pub edge: String,
    pub used: u32,
    pub allowed: u32,
    pub awaiting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphNodeView {
    pub id: String,
    pub label: String,
    pub kind: OrgNodeKind,
    pub role_id: Option<String>,
    /// この工程のタスク（まだ動いていない工程は None）。
    pub task: Option<String>,
    /// None＝まだ動いていない（未実行）。
    pub status: Option<OrgStatusView>,
    pub confirm: Option<OrgConfirmView>,
    /// 周回の中の工程の「第N周」と「最大M周」（周回の外・まだ動いていない工程は None）。
    pub round: Option<u32>,
    pub max_round: Option<u32>,
    /// 最後の判定（辺の選択に使う項目がない工程は None）。
    pub last_verdict: Option<OrgVerdict>,
    pub gate: Option<OrgGateView>,
    pub round_limit: Option<OrgRoundLimitView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphEdgeView {
    pub from: String,
    pub to: String,
    pub kind: OrgEdgeKind,
    /// 辺の条件の文言（条件がなければ「常に」）。
    pub label: String,
    /// 通った回数（先へ進む辺は 0 か 1）。
    pub used: u32,
    /// 差し戻しの辺の許容回数（定義の最大＋利用者が足した分）。先へ進む辺は None。
    pub allowed: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRoundEntryView {
    /// その辺の判定元スロットの何回目の出力か（1始まり）。
    pub index: u32,
    pub verdict: OrgVerdict,
}

/// 差し戻しの辺ごとの周回。履歴は古い順（読めた範囲の最近の分）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgRoundView {
    pub edge: String,
    pub from: String,
    pub to: String,
    pub used: u32,
    /// 定義の最大。
    pub max_rounds: u32,
    /// 利用者が足した回数。
    pub extra: u32,
    pub on_exhausted: OrgOnExhausted,
    pub history: Vec<OrgRoundEntryView>,
}

/// グラフで動くワークフローの流れ（読み取り専用）。ノードは先へ進む順に並ぶ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphView {
    pub graph_id: String,
    pub name: String,
    pub nodes: Vec<OrgGraphNodeView>,
    pub edges: Vec<OrgGraphEdgeView>,
    pub rounds: Vec<OrgRoundView>,
}

// ───────────────────────────── グラフ定義（テンプレート） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgWorkspaceKind {
    Git,
    Folder,
}

/// 開始前の見積り（定義から計算した上限。実際の使用量ではない）。トークンは「1 turn あたりの土台」の目安からの概算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgEstimateView {
    /// 最大の活動数（周回を含む）。
    pub activations: u32,
    /// 通常の turn 数（1活動1 turn）。
    pub turns_typical: u32,
    /// 最大の turn 数（形式修正込み）。
    pub turns_max: u32,
    /// 最大の turn 数 × 1 turn あたりの土台（目安）。
    pub tokens_max: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphTemplateView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub workspace: OrgWorkspaceKind,
    /// 使う役割のID。
    pub roles: Vec<String>,
    /// 定義ファイルの名前（開く操作の指定に使う）。
    pub file: String,
    /// 開始に選べるか。選べないときは `reasons` に理由。
    pub selectable: bool,
    pub reasons: Vec<String>,
    pub estimate: OrgEstimateView,
    /// 定義が提案する turn 上限（なければ None）。
    pub suggested_max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphProblemView {
    pub file: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGraphsView {
    /// まだ読み込んでいなければ false（テンプレートは空。読み込んだ空とは違う）。
    pub loaded: bool,
    pub templates: Vec<OrgGraphTemplateView>,
    /// 使えない定義ファイル（読めない・検証に通らない）。理由つき。
    pub problems: Vec<OrgGraphProblemView>,
    pub created: Vec<String>,
    /// turn 上限の既定と、指定できる最大。
    pub default_max_turns: u32,
    pub max_turns_limit: u32,
    /// 1 turn あたりの土台のトークン数（目安）。
    pub tokens_per_turn_hint: u32,
}

// ───────────────────────────── 介入・進行タイムライン（DESIGN_ORG_R2） ─────────────────────────────

/// 割り込み指示の経路（入力欄の上の文言の元）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgInterjectRoute {
    /// 実行中の turn に今すぐ届ける（`turn/steer`）。
    Steer,
    /// 次にそのタスクへ送るとき（差し戻し・再送など）に添える。
    NextTurn,
    /// 今は送れない（理由は `label`）。
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInterjectRouteView {
    pub route: OrgInterjectRoute,
    /// 画面に出す文（経路の説明、または送れない理由）。
    pub label: String,
}

/// スキップの確認の材料（一緒に飛ばすタスクと、検証なしで完了にするタスク）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgSkipView {
    /// 同時に飛ばすタスク（検証役）。
    pub also: Vec<String>,
    /// 検証なしで完了にするタスク（そのタスクの唯一の検証役を飛ばすとき）。
    pub completes_unreviewed: Option<String>,
}

/// 追加指示1件。`delivered_to` は届いたタスク、`pending_for` はまだ届いていない（終わっていない）タスク。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgNoteView {
    pub id: String,
    pub text: String,
    /// 宛先: 全体（これから送るすべて）／タスク。
    pub task: Option<String>,
    #[cfg_attr(test, ts(type = "number"))]
    pub seq: u64,
    pub delivered_to: Vec<String>,
    pub pending_for: Vec<String>,
    pub withdrawn: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgInterjectOutcomeKind {
    /// まだ決着していない（送信中）。
    Open,
    Delivered,
    NotDelivered,
    /// 届いたか確認できない。「次の送信に添える」「破棄」を選べる。
    Unknown,
    ReconciledDelivered,
    /// 照合で見つからなかった。「次の送信に添える」「破棄」を選べる。
    ReconciledNotFound,
    FallbackNoted,
    Discarded,
}

/// 割り込み指示1件の写し。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInterjectView {
    pub id: String,
    pub task: String,
    /// 本文の先頭（長いものは切る）。
    pub text_head: String,
    pub outcome: OrgInterjectOutcomeKind,
    /// 結果の文（届きました／届きませんでした（理由）／確認できません…）。
    pub label: String,
    /// 「次の送信に添える」「破棄」を選べるか。
    pub can_decide: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgActorKind {
    User,
    Role,
    Engine,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgActor {
    pub kind: OrgActorKind,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgTone {
    Info,
    Good,
    Warn,
    Bad,
    Wait,
}

/// 進行タイムラインの1行（記録を人間向けに整えたもの。会話の内容は入れない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTimelineEntry {
    /// 記録の通し番号（行の識別。まとめた行は最初の番号）。
    #[cfg_attr(test, ts(type = "number"))]
    pub seq: u64,
    #[cfg_attr(test, ts(type = "number"))]
    pub seq_to: u64,
    pub at: UnixMillis,
    pub task: Option<String>,
    pub actor: OrgActor,
    pub tone: OrgTone,
    pub text: String,
    /// まとめた記録の件数（1なら単独）。
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTimelineView {
    /// 古い順。`after_seq` より後に変わった行だけ（なければ全体の新しい側の `limit` 件）。
    pub entries: Vec<OrgTimelineEntry>,
    #[cfg_attr(test, ts(type = "number"))]
    pub last_seq: u64,
    /// 古い行が保持の上限で落ちている、または件数の上限で切った。
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgTimelineArgs {
    pub workflow_id: String,
    #[serde(default)]
    #[cfg_attr(test, ts(type = "number | null"))]
    pub after_seq: Option<u64>,
    pub limit: u32,
}

/// 割り込み指示。実行するコマンド（利用者の操作）。`fallback` は「届かなければ次の送信に添える」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInterjectArgs {
    pub workflow_id: String,
    pub task: String,
    pub text: String,
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInterjectAck {
    /// 経路は送る前にホストが判定した結果。`NextTurn` のときは、すぐには届かない（次の送信に添える）。
    pub route: OrgInterjectRoute,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInterjectFallbackArgs {
    pub workflow_id: String,
    pub id: String,
    /// true=次の送信に添える／false=破棄。
    pub add: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgAddNoteArgs {
    pub workflow_id: String,
    pub text: String,
    /// 宛先のタスク（None=これからエンジンが送るすべて）。
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgWithdrawNoteArgs {
    pub workflow_id: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgSendBackArgs {
    pub workflow_id: String,
    pub task: String,
    /// 利用者の指摘。検証待ちのタスクでは必須、差し戻し上限で止まっているタスクでは任意。
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgHoldArgs {
    pub workflow_id: String,
    pub task: String,
    pub held: bool,
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
    /// テンプレート（グラフ定義）のID。None は従来の計画経路（`standard` と同じ）。
    #[serde(default)]
    pub graph_id: Option<String>,
    /// turn 上限（None は既定の30。1〜200）。
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// 新しい試行での再送の上限（None は既定の3。0〜10）。
    #[serde(default)]
    pub max_retries: Option<u32>,
}

/// 差し戻して再計画する。理由は Planner に利用者の要望として渡る（任意）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgReplanArgs {
    pub workflow_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// gate の判断。選択肢の定義が理由を必須にしているときは、理由が要る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgGateDecideArgs {
    pub workflow_id: String,
    pub task: String,
    pub choice: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// 周回上限に達した工程の「理由を書いてもう1周」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgExtraRoundArgs {
    pub workflow_id: String,
    /// 上限に達して止まっている戻り先の工程のタスク。
    pub task: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgOpenGraphFileArgs {
    /// `OrgGraphTemplateView.file`／`OrgGraphProblemView.file`（フォルダ内のファイル名。パス区切りは受けない）。
    pub file: String,
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

/// 再開。上限で止まったタスクのうち、利用者がチェックしたものだけを新しい試行として送る（自動では送らない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgResumeArgs {
    pub workflow_id: String,
    #[serde(default)]
    pub retry_tasks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgManualEnd {
    Completed,
    Running,
    Failed,
    Interrupted,
    /// 履歴で終端を確認できない。
    Unknown,
}

/// 手動に切り替えたチャットの最新 turn の読取り（取込みの確認用。読取りだけで何も送らない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgManualResultView {
    /// 最新の turn（履歴に turn がなければ None）。
    pub turn_id: Option<String>,
    pub end: OrgManualEnd,
    /// 全件読めたか。
    pub complete: bool,
    /// 最終回答の先頭400字（読めないときは None）。
    pub final_head: Option<String>,
    /// 取り込めない理由（取り込めるときは None）。
    pub problem: Option<String>,
    /// このタスクの結果の形式の説明（クリップボードへコピーする用。送信はしない）。取得できなければ None。
    pub format_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgAdoptManualArgs {
    pub workflow_id: String,
    pub task: String,
    /// 確認画面で見せた turn。履歴の最新と違えば拒否（確認し直し）。
    pub turn_id: String,
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
    /// 開始は成功したが、依頼整理側の記録（開始済みの印）を保存できなかったときの警告。ワークフローは動いている。
    #[serde(default)]
    pub intake_warning: Option<String>,
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

// ───────────────────────────── 削除・アーカイブ（DESIGN_ORG_DELETE） ─────────────────────────────

/// 削除・アーカイブの判定。拒否のときは理由の一覧（利用者向けの文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgManageVerdict {
    Allowed,
    Blocked { reasons: Vec<String> },
}

/// 削除の進み（途中で止まったとき）。`done`・`failed` は段の名前（利用者向けの文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgDeletionView {
    pub requested_at: UnixMillis,
    pub done: Vec<String>,
    pub failed: Vec<String>,
}

/// 削除で消える（または解放される）未解放の役割チャット1件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgDeleteChatRow {
    pub chat: ChatKey,
    /// 記録の所有情報から分からないときは空。
    pub task_id: String,
    pub role_id: String,
    pub name: Option<String>,
    /// このチャットが削除できない理由（あれば）。
    pub blocked: Option<String>,
}

/// 残る所有worktree1件（自動では消さない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgKeptWorktreeRow {
    pub path: String,
    pub branch: String,
    pub access: OrgWorktreeAccess,
    /// フォルダがまだあるか。
    pub exists: bool,
    /// 未コミット・無視ファイルの件数（読めなければ未取得）。
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub dirty: Known<u32>,
    /// ブランチにあって現在のHEADにない（未取込みの）コミット数（読めなければ未取得）。
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub unmerged_commits: Known<u32>,
}

/// 削除確認の内容（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgDeletePreview {
    pub workflow_id: String,
    pub title: Option<String>,
    pub verdict: OrgManageVerdict,
    /// 消える（または「残す」を選ぶと解放される）未解放の役割チャット。
    pub chats: Vec<OrgDeleteChatRow>,
    /// 手動に切り替え済みで残るチャット。
    pub kept_released: Vec<ChatKey>,
    /// 残る所有worktree。
    pub worktrees: Vec<OrgKeptWorktreeRow>,
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub record_bytes: Known<u64>,
    /// 途中の削除があれば、その進み。
    pub deletion: Option<OrgDeletionView>,
    /// このワークフローを開始した依頼整理の会話（紐づけを確認できたときだけ。None のときは `intake_unlinked` を見る）。
    #[serde(default)]
    pub intake: Option<OrgDeleteIntakeRow>,
    /// 依頼整理の会話との紐づけを確認できない（記録がない）。この場合は依頼整理の会話を削除の対象にしない。
    #[serde(default)]
    pub intake_unlinked: bool,
}

/// ワークフローの削除と一緒に消せる依頼整理の会話1件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgDeleteIntakeRow {
    pub chat: ChatKey,
    pub name: Option<String>,
    /// 依頼整理の状態の文（開始済み・やめたもの等）。
    pub state: String,
    /// すでにアプリの記録にない（削除済み）。
    pub gone: bool,
    /// 削除できない理由（あれば）。この理由はワークフローの削除を止めない。
    pub blocked: Option<String>,
}

/// 依頼整理の会話の削除の結果（ワークフロー削除と一緒に選んだときだけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgIntakeDeleteResult {
    pub deleted: bool,
    /// 削除できなかった理由（`deleted` が false のとき）。
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgDeleteArgs {
    pub workflow_id: String,
    /// false なら役割チャットを消さず、手動に切り替えて通常チャットとして残す。
    pub delete_chats: bool,
    /// 確認画面で見せたチャット。実行時の集合と違えば拒否（確認し直し）。
    pub expect_chats: Vec<ChatKey>,
    /// 一緒に削除する依頼整理の会話（確認画面で見せたもの）。None なら削除しない。ホストが紐づけを確かめ、違えば拒否する。
    #[serde(default)]
    pub intake_chat: Option<ChatKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgDeleteOutcome {
    /// 依頼整理の会話は別の結果（`intake`）。会話を消せなくても、ワークフローは削除済み。
    Deleted {
        #[serde(default)]
        intake: Option<OrgIntakeDeleteResult>,
    },
    /// 途中で止まった。完了と偽らず、残りは同じ操作で続ける。
    Partial {
        done: Vec<String>,
        failed: Vec<String>,
        #[serde(default)]
        intake: Option<OrgIntakeDeleteResult>,
    },
}
