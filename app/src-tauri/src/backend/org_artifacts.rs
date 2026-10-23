//! 組織実行の成果物パネルと「取り込む」（DESIGN_ORG_R3 §3）のIPC型。Codex固有の型は持たない。
//!
//! - 差分・行数は Git から読んだ値で、取れないものは `Known` の欠け（「未確認」）で運ぶ。0や空で代用しない。
//! - 取り込み・戻すの結果は `Unknown` を持ち、読み戻しで確かめられたときだけ `Applied`／`Undone` にする。

use serde::{Deserialize, Serialize};

use super::changes::ChangeKind;
use super::model::{Known, UnixMillis};
use super::org::OrgTaskPhase;

// ───────────────────────────── 状態 ─────────────────────────────

/// 成果物（書込みタスクのコミット）の状態。保存しない（Git・台帳・エンジンの状態から毎回導く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgArtifactStatus {
    Working,
    AiReviewing,
    ReadyForReview,
    NotPassed,
    Superseded,
    Imported,
    IncludedElsewhere,
    NoChanges,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgPreviewKind {
    Markdown,
    Text,
    Table,
    Image,
    None,
}

/// 一覧の行の印（エンジン状態＋台帳だけから導く軽い値。Gitの事実は成果物パネルで確認する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactHint {
    pub ready: u32,
    pub imported: u32,
}

// ───────────────────────────── 成果物パネル ─────────────────────────────

/// 成果物の1ファイル。`path` が元の場所、移動・名前変更のときは `move_to` が新しい場所（`ChangedFile` と同じ形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactFile {
    pub path: String,
    pub kind: ChangeKind,
    pub move_to: Option<String>,
    pub additions: Known<u32>,
    pub deletions: Known<u32>,
    pub binary: bool,
    pub preview: OrgPreviewKind,
}

/// ファイル数と行数の合計。`unknown` は行数を数えられなかったファイルの数（0でなければ行数は一部だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactTotals {
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
    pub binary: u32,
    pub unknown: u32,
}

/// Worker の報告（自己申告。判定には使わない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgWorkerReportView {
    pub summary: String,
    pub changed_files: Vec<String>,
    pub tests_ran: bool,
    pub tests_passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgReviewVerdict {
    Pass,
    Rework,
    /// 計画の作り直しを求めた（グラフの検証工程）。
    Replan,
    /// 判定を取得できなかった（推定しない）。
    Unknown,
}

/// 検証役が実行した確認のコマンドの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgCheckOutcome {
    Passed,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgCheckView {
    pub command: String,
    pub outcome: OrgCheckOutcome,
    pub summary: String,
}

/// 検証役の判定1回分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgReviewView {
    pub reviewer_task: String,
    pub verdict: OrgReviewVerdict,
    pub findings: Vec<String>,
    /// 実行した確認のコマンド。None = 記録なし（この版より前の出力、または取得できなかった）。空の配列は「確認を実行しなかった」。
    pub checks: Option<Vec<OrgCheckView>>,
    /// 成功した確認が1件もないときの理由（検証役の申告）。
    pub no_check_reason: Option<String>,
    /// 検証役は合格としたが、確認のコマンドが失敗したため、合格を保留した判定。
    pub held_back: bool,
    /// 何回目の判定か（1から）。
    pub round: u32,
    /// 判定の対象になったコミット（分かるときだけ）。
    pub commit: Option<String>,
    /// 履歴の最終回答から読んだ（保存された出力ではない）。
    pub from_history: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportTargetView {
    pub folder: String,
    pub branch: Known<String>,
    pub head: Known<String>,
    /// 作業フォルダの未コミットの変更の件数。
    pub dirty: Known<u32>,
    /// 取り込みを止める理由（空なら前提は満たしている）。
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgImportState {
    Applied,
    NotApplied,
    Partial,
    Unknown,
    Undone,
}

/// 台帳（imports.jsonl）から畳んだ取り込み記録。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportRecordView {
    pub id: String,
    pub at: UnixMillis,
    pub folder: String,
    pub branch: String,
    pub before: String,
    pub result: String,
    pub tasks: Vec<String>,
    pub state: OrgImportState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactItem {
    pub task: String,
    pub title: String,
    pub role: String,
    pub branch: String,
    pub base: Option<String>,
    pub commit: Option<String>,
    pub status: OrgArtifactStatus,
    pub status_label: String,
    /// 状態への注記（台帳にあるが今のブランチに含まれない、など）。
    pub status_note: Option<String>,
    pub files: Known<Vec<OrgArtifactFile>>,
    pub totals: Known<OrgArtifactTotals>,
    pub report: Known<OrgWorkerReportView>,
    pub reviews: Known<Vec<OrgReviewView>>,
    /// 検証・申告についての注意（テスト未実行の合格、申告と判定の食い違い）。
    pub review_notes: Vec<String>,
    /// Worker の申告と差分の違い（申告にあって差分にない／差分にあって申告にないパス）。
    pub report_mismatch: Vec<String>,
    /// コミットされていない変更の件数（成果に含まれない）。
    pub uncommitted: Known<u32>,
    pub importable: bool,
    pub import_refusal: Option<String>,
    pub imports: Vec<OrgImportRecordView>,
}

/// 調査の結論1件（根拠と参照つき）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInvestigationFindingView {
    pub point: String,
    pub basis: String,
    pub refs: Vec<String>,
}

/// 調査タスクの報告（ファイルを変えない調査の成果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInvestigationView {
    pub summary: String,
    pub findings: Vec<OrgInvestigationFindingView>,
    pub references: Vec<String>,
    pub open_questions: Vec<String>,
}

/// 成果物パネルの「調査報告」の1件。ファイルがないので、取り込み・差分の対象ではない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgInvestigationItem {
    pub task: String,
    pub title: String,
    pub phase: OrgTaskPhase,
    pub status_label: String,
    /// 保存した報告 → 履歴の最終回答の順に読む。読めなければ「未取得」（推定しない）。
    pub report: Known<OrgInvestigationView>,
    /// 報告の試行（保存先の版）。
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactsView {
    pub workflow_id: String,
    pub target: OrgImportTargetView,
    pub items: Vec<OrgArtifactItem>,
    /// 調査タスクの報告（取り込み対象の `items` とは別）。
    pub investigations: Vec<OrgInvestigationItem>,
    pub fetched: Known<UnixMillis>,
    pub git_supports_import: Known<bool>,
    /// 取り込み記録のうち、まだ決着していない（確認できない・戻せる）もの。
    pub records: Vec<OrgImportRecordView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactsArgs {
    pub workflow_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactPathArgs {
    pub workflow_id: String,
    pub task: String,
    /// `OrgArtifactFile.path`（移動のときは元の場所）。
    pub path: String,
    /// 移動・名前変更の先（`OrgArtifactFile.move_to`）。
    pub move_to: Option<String>,
}

/// 「開く…」（書き出した読取り専用のコピーを関連アプリで開く）。実行形式は確認のうえ `risk_confirmed` で再実行する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactOpenArgs {
    pub workflow_id: String,
    pub task: String,
    pub path: String,
    pub move_to: Option<String>,
    #[serde(default)]
    pub risk_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactDiff {
    pub path: String,
    pub text: Known<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactPreview {
    pub kind: OrgPreviewKind,
    pub text: Option<String>,
    pub data_url: Option<String>,
    pub truncated: bool,
    #[cfg_attr(test, ts(type = "number"))]
    pub bytes: u64,
    /// プレビューできない理由（あれば「開く…」を案内する）。
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgArtifactOpenResult {
    pub path: String,
}

// ───────────────────────────── 取り込む ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportPreviewArgs {
    pub workflow_id: String,
    pub tasks: Vec<String>,
}

/// 取り込み候補の1件（確認画面の「取り込む成果」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportCandidate {
    pub task: String,
    pub title: String,
    pub branch: String,
    pub commit: String,
    pub status: OrgArtifactStatus,
}

/// 取り込みの確認の材料（作業フォルダには何も書いていない）。`blockers` があれば取り込めない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportPreview {
    pub token: String,
    pub folder: String,
    pub branch: String,
    pub head: String,
    pub candidates: Vec<OrgImportCandidate>,
    /// 取り込みで変わるファイル（今のコミットと結果の差）。
    pub files: Vec<OrgArtifactFile>,
    pub totals: OrgArtifactTotals,
    pub conflicts: Vec<String>,
    pub dirty_overlap: Vec<String>,
    pub locked_files: Vec<String>,
    /// 重ならずにそのまま残る未コミットの変更の件数。
    pub dirty_other: u32,
    /// 合格していない成果のタスク（同意チェックが要る）。
    pub not_passed: Vec<String>,
    /// 取り込み前に知らせておく注意（テスト未実行の合格など。タスクIDつき）。
    pub cautions: Vec<String>,
    pub blockers: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportArgs {
    pub workflow_id: String,
    pub token: String,
    pub accept_not_passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgImportOutcome {
    Applied { head: String },
    NotApplied { reason: String },
    /// HEADは前のままだが、対象ファイルの一部が書き換わった（ロック等）。
    Partial { files: Vec<String> },
    Unknown { head_now: Known<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportResult {
    pub record_id: String,
    pub outcome: OrgImportOutcome,
    /// 取り込んだファイル数（`Applied` のときだけ）。
    pub files: Option<u32>,
    pub folder: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgImportRecordArgs {
    pub workflow_id: String,
    pub record_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OrgUndoMode {
    Reset,
    RestoreFiles,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgUndoPreview {
    pub allowed: bool,
    pub refusal: Option<String>,
    pub mode: OrgUndoMode,
    pub files: Vec<String>,
    /// 戻す先（取り込み前のコミット）と、今のコミット。
    pub before: String,
    pub head: Known<String>,
    /// 画面が `org_import_undo` に渡す「確認したときのHEAD」。
    pub expected_head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgUndoArgs {
    pub workflow_id: String,
    pub record_id: String,
    pub expected_head: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OrgUndoOutcome {
    Undone { head: String },
    NotUndone { reason: String },
    Unknown { head_now: Known<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OrgUndoResult {
    pub outcome: OrgUndoOutcome,
}
