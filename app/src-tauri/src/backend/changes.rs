//! 変更ファイルの一覧・差分・「変更を戻す」の中立型（段階③ P3-1、Git基準の控えへ置換: P3B、`app/DESIGN_P3B.md`）。
//!
//! - 出所は「控え基準」（turnの開始時・終了時にAgentDockがGitで控えた内容の差）と「Git」（HEAD比較）の2つだけ。バックエンドの報告は使わない。
//! - 戻す操作は、選んだ区間の基準 B の内容へファイルを書き戻す（判定は `rules::baseline`、控えの型は `baseline`）。推定で戻さない。
//! - Codex固有の型・メソッド名を含めない。

use serde::{Deserialize, Serialize};

use super::model::*;

/// 変更の種類。移動は `move_to` を持つ `Modified` で表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
}

// ───────────────────────────── 一覧・差分（IPC） ─────────────────────────────

/// 一覧の対象（控え基準）。`Git` 出所では無視する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChangeScope {
    Turn { turn: ExternalId },
    Chat,
}

/// 出所。混ぜずに表示する。
/// `Baseline`＝turnの開始時・終了時にAgentDockがGitで控えた内容の差、`Git`＝Git上の現在の差分（HEAD比較）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ChangeSource {
    Baseline,
    Git,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ListStatus {
    Ready,
    /// 取得できなかった（空と区別する）。
    NotFetched { message: String },
    /// この経路では使えない（Gitなし・リポジトリでない等）。
    NotSupported { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub kind: ChangeKind,
    pub move_to: Option<String>,
    pub source: ChangeSource,
    /// 追加・削除の行数。数えられないもの（未追跡・バイナリ）は `NotFetched`（0で代用しない）。
    pub additions: Known<u32>,
    pub deletions: Known<u32>,
    /// そのファイルを最後に変えた区間のturn（Gitの差分では None）。
    pub turn: Option<ExternalId>,
    /// 「戻す」で戻し済み（以後の判定に使わない）。
    pub reverted: bool,
    /// Gitがバイナリ扱いにした（行数・本文は出せない。`additions`/`deletions` は0ではなく `Unsupported`）。
    #[serde(default)]
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChangeList {
    pub scope: ChangeScope,
    pub source: ChangeSource,
    pub status: ListStatus,
    pub files: Vec<ChangedFile>,
    /// 表示上の注記（控えの範囲・途中の状態を含む等）。
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GetChangeListArgs {
    pub chat: ChatKey,
    /// 控え基準での範囲（`source` が `Git` のときは無視する）。
    pub scope: ChangeScope,
    pub source: ChangeSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GetFileDiffArgs {
    pub chat: ChatKey,
    pub path: String,
    pub source: ChangeSource,
    pub turn: Option<ExternalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct UnifiedDiff {
    pub path: String,
    pub source: ChangeSource,
    pub status: ListStatus,
    pub text: String,
}

// ───────────────────────────── 変更を戻す（IPC） ─────────────────────────────

/// 戻せない理由の分類（UIの出し分け用。文面は `message`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum RevertBlockCode {
    // ── Git基準の控え（P3B。DESIGN_P3B §4.3） ──
    // 止める（強制不可）
    /// rebase・merge・cherry-pick 等の途中。
    GitBusy,
    NotARepository,
    GitUnavailable,
    /// 範囲に控えのある区間がない（控え導入前のturn・控えの失敗）。
    NoBaseline,
    /// 作業フォルダの外のファイル。
    OutsideWorkFolder,
    /// 控えていないパス（リンク・サブモジュール・64MiB超・合計上限・読めない）。
    NotSnapshotted,
    /// turnの後にGitのHEAD・ブランチが変わった。
    HeadMoved,
    /// すでに控えの内容へ戻してある。
    AlreadyReverted,
    // 要確認（強制可。2段目の確認でファイル単位に選ぶ）
    /// turnの終了時の控えがない（切断・再起動など）。
    EndUnknown,
    /// 区間の間に別の変更がある。
    ChangedBetween,
    /// turnの後に別の変更がある。
    ChangedAfter,
    /// 同じリポジトリで同時に作業していた別の会話も変更した可能性がある。
    ConcurrentChange,
    /// 作業フォルダの外、または解決後の位置を確認できないパス。
    PathOutside,
    /// 書き戻す内容を取り出せなかった。
    ReadFailed,
}

/// 要確認の理由1件（`code` は要確認の4種のどれか）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OverrideReason {
    pub code: RevertBlockCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RevertVerdict {
    Revertible { summary: String },
    /// 帰属を判別できないため止めた。理由を表示したうえで、ユーザーが2段目の確認で選んだときだけ戻せる。
    NeedsOverride { summary: String, reasons: Vec<OverrideReason> },
    Blocked { code: RevertBlockCode, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevertItem {
    pub path: String,
    /// 最初に戻す変更の種類。
    pub kind: Option<ChangeKind>,
    /// 戻すと一緒に戻る、より後のturn（同じファイルへの後続の変更）。選んだturnだけを戻せないことの表示。
    pub includes_turns: Vec<ExternalId>,
    pub verdict: RevertVerdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevertPlan {
    pub id: LocalId,
    pub chat: ChatKey,
    pub items: Vec<RevertItem>,
    pub created_at: UnixMillis,
    /// この時刻を過ぎた計画では実行できない（作り直す）。
    pub expires_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PreviewRevertArgs {
    pub chat: ChatKey,
    /// そのturn以降の変更を戻す（None＝このチャットの控えのある変更すべて＝最初の区間から）。
    pub turn: Option<ExternalId>,
    /// 対象のファイル（None＝該当するすべて）。
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevertChangesArgs {
    pub chat: ChatKey,
    pub plan_id: LocalId,
    /// 計画のうち実行するファイル（戻せると判定したものだけ有効）。
    pub paths: Vec<String>,
    /// 2段目の確認で選んだ要確認のファイル（`NeedsOverride` で、理由が計画時と同じものだけ有効）。既定は空。
    #[serde(default)]
    pub forced: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevertFailure {
    pub path: String,
    pub reason: String,
}

/// 戻した結果。部分成功を完了と表示しない（`failed` が空でないときは一部のみ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RevertResult {
    pub reverted: Vec<String>,
    /// `reverted` のうち、2段目の確認で強制して戻したファイル。
    #[serde(default)]
    pub forced: Vec<String>,
    pub failed: Vec<RevertFailure>,
    /// 戻す前の控えの場所。
    pub backup_dir: Option<String>,
}
