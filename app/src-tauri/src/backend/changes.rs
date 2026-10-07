//! 変更ファイルの一覧・差分・「変更を戻す」の中立型（段階③ P3-1、`app/DESIGN_P3.md` §1 #1・#2）。
//!
//! - バックエンドが報告した変更（[`FileChange`]）は `BackendEvent` で流れ、ホストが観測記録（[`ChangeRecord`]）として
//!   `chats\<dirId>\changes.jsonl` に追記する。記録は「観測した事実」だけで、観測していない変更は含まない。
//! - 戻す操作は、記録した差分の逆適用と、観測直後の内容のハッシュ照合だけで行う（推定で戻さない）。
//! - Codex固有の型・メソッド名を含めない。

use serde::{Deserialize, Serialize};

use super::model::*;

fn one() -> u32 {
    1
}

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

/// バックエンドが報告したファイル変更1件（`BackendEvent` の中身。保存・UIには `ChangeRecord` を使う）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// 変更前のパス（追加なら対象のパス）。
    pub path: String,
    pub kind: ChangeKind,
    /// 移動先（移動を伴う変更のとき）。
    pub move_to: Option<String>,
    /// 報告された差分。更新は統一diff。追加・削除はバックエンドによっては本文そのもの（`rules::diff` で扱う）。
    pub diff: String,
}

/// 観測直後のファイルの状態（戻す前の照合の根拠）。読めなかったものは `Unknown`（推定しない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PostState {
    Unknown,
    Absent,
    Hash { sha256: String },
}

/// 観測した変更1件の記録（`changes.jsonl`）。`path` は観測時に作業フォルダ基準で絶対パスにしたもの
/// （作業フォルダが分からず相対のままの記録は、戻す対象にできない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRecord {
    #[serde(default = "one")]
    pub schema_version: u32,
    pub chat: ChatKey,
    pub agent: AgentKey,
    pub turn: Option<ExternalId>,
    pub item: ExternalId,
    pub path: String,
    pub kind: ChangeKind,
    pub move_to: Option<String>,
    pub diff: String,
    pub post: PostState,
    pub observed_at: UnixMillis,
}

impl ChangeRecord {
    /// 変更後にファイルがある（あった）場所。
    pub fn post_path(&self) -> &str {
        self.move_to.as_deref().unwrap_or(&self.path)
    }
}

/// `changes.jsonl` の1行。末尾の欠けた行は読込み時に無視する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChangeLine {
    Observed { record: ChangeRecord },
    /// 「戻す」で元に戻したファイル。以前の記録はこの時点で消化済み（以後の判定に使わない）。
    /// `records` は戻した記録の識別子。旧形式の行（`None`）は、時刻とパスだけで消化を判定する（従来どおり）。
    Reverted { at: UnixMillis, chat: ChatKey, paths: Vec<String>, backup_dir: String, #[serde(default)] records: Option<Vec<RecordId>> },
}

/// 観測した記録1件の識別子（チャット・項目・観測時のパス）。「戻し」が消化した記録を特定する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordId {
    pub chat: ChatKey,
    pub item: ExternalId,
    pub path: String,
}

// ───────────────────────────── 一覧・差分（IPC） ─────────────────────────────

/// 一覧の対象。`WorkingTree` はGit上の現在の差分（読取りのみ）、他はバックエンドが報告した変更。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChangeScope {
    Turn { turn: ExternalId },
    Chat,
    WorkingTree,
}

/// 出所。混ぜずに表示する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ChangeSource {
    BackendReported,
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
    /// 報告されたturn（Gitの差分では None）。
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
    /// 表示上の注記（観測の範囲・コマンドによる変更は含まれない場合がある等）。
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GetChangeListArgs {
    pub chat: ChatKey,
    pub scope: ChangeScope,
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
    /// AgentDockが観測した記録がない（再起動中・外部・コマンドによる変更）。
    NotObserved,
    /// 別の会話の、より新しい変更記録がある。
    OtherChatLater,
    /// 現在の内容が観測直後と一致しない（別の変更の可能性）。
    HashMismatch,
    /// 観測直後の内容を確認できていない。
    HashUnknown,
    /// 記録ではファイルがあるはずだが、現在は無い。
    MissingNow,
    /// 差分の文脈が現在の内容と一致しない。
    ContextMismatch,
    NotText,
    /// 記録した差分を読み取れない。
    DiffUnreadable,
    Unsupported,
    /// 戻し先に別のファイルがある（上書きしない）。
    TargetExists,
    PathUnresolved,
    /// 作業フォルダの外（`..`・リンク越え・別ドライブを含む）、または作業フォルダを確認できない。
    PathOutside,
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RevertVerdict {
    Revertible { summary: String },
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
    /// そのturnの変更から戻す（None＝このチャットの観測した変更すべて）。
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub failed: Vec<RevertFailure>,
    /// 戻す前の控えの場所。
    pub backup_dir: Option<String>,
}
