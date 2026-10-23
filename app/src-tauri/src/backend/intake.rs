//! 組織実行の依頼整理（DESIGN_ORG_INTAKE §1・§4）のデータ型とIPC型。Codex固有の型は持たない。
//!
//! - 依頼整理は利用者の普通のチャットで行う（`org` 所有記録なし）。要約・状態は `chat.json` の `conversation.intake` に持つ。
//! - 取得できない値は `None`／空配列にして、0や空文字で代用しない。

use serde::{Deserialize, Serialize};

use super::model::{ChatKey, LocalId, UnixMillis};

/// 依頼要約。文字列と配列だけ（バックエンドの構造化出力の形と同じ）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeSummary {
    pub goal: String,
    pub deliverable: String,
    pub inputs: Vec<IntakeInput>,
    pub constraints: Vec<String>,
    pub done_criteria: Vec<String>,
    pub open_questions: Vec<String>,
}

/// 入力ファイル1件（作業フォルダからの相対パス＋メモ）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeInput {
    pub path: String,
    pub note: String,
}

/// 開始時の設定（要約カードの下段）。None は既定。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeStartPrefs {
    /// 作業フォルダ（Planner の作業フォルダ）。
    pub folder: String,
    #[serde(default)]
    pub graph_id: Option<String>,
    #[serde(default)]
    pub max_writers: Option<u32>,
    #[serde(default)]
    pub max_total: Option<u32>,
    #[serde(default)]
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum IntakeState {
    /// 会話中（要約はまだない、または失敗して読み取れなかった）。
    Drafting,
    /// 要約の turn を送った。結果の確認待ち。
    Summarizing { client_message_id: String, sent_at: UnixMillis },
    /// 要約がある（編集できる）。
    Ready,
    /// この版でワークフローを開始した。
    Started { workflow_id: String, rev: u32, at: UnixMillis },
    /// 依頼整理をやめた（チャットは消さない）。
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeStarted {
    pub workflow_id: String,
    pub rev: u32,
    pub at: UnixMillis,
}

/// `chat.json` の `conversation.intake`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntakeRecord {
    pub id: LocalId,
    pub state: IntakeState,
    #[serde(default)]
    pub summary: Option<IntakeSummary>,
    /// 要約の版（編集・要約の成功のたびに1増える）。開始は、この値と同じ版でだけ受ける。
    #[serde(default)]
    pub rev: u32,
    pub prefs: IntakeStartPrefs,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started: Vec<IntakeStarted>,
    /// 「依頼が決まっているので書く」で作った（会話せず、要約を手で書く）。
    #[serde(default)]
    pub write_only: bool,
}

// ───────────────────────────── 画面向け ─────────────────────────────

/// 入力ファイルの検査結果（作業フォルダに対して）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum InputCheck {
    /// 作業フォルダ内でコミット済みで変更がない。
    Committed,
    /// 作業フォルダ内だが未コミット・変更あり（Worker の作業場所には最新が入らない）。
    Uncommitted,
    /// 作業フォルダの外。
    Outside,
    /// 存在しない。
    Missing,
    /// 検査できなかった（Git を読めない等）。
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct InputCheckView {
    pub path: String,
    pub check: InputCheck,
    /// 表示文。
    pub label: String,
}

/// 開始できない理由（goal 空・完了条件なし・16KB超）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeBlockerView {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeView {
    pub chat: ChatKey,
    pub state: IntakeState,
    pub summary: Option<IntakeSummary>,
    pub rev: u32,
    pub prefs: IntakeStartPrefs,
    pub last_error: Option<String>,
    pub started: Vec<IntakeStarted>,
    pub input_checks: Vec<InputCheckView>,
    pub blockers: Vec<IntakeBlockerView>,
    /// Planner へ送る依頼文（開始確認ダイアログのプレビューと、開始時に送る文は1文字も違わない）。作れないときは None。
    pub composed_preview: Option<String>,
    /// バックエンドが構造化出力に対応するか（偽なら「要約を作る」を出さない）。
    pub structured_output: bool,
    /// 要約の結果を確認できていない（切断・受理不明など）。「履歴から読み直す」「もう一度要約を作る」だけを出す。
    pub summary_unconfirmed: bool,
    /// 会話せず要約を手で書く入口で作った。画面は要約を直接開き、相談用の案内を出さない。
    pub write_only: bool,
    /// 依頼整理の間に子エージェントの作成を観測した（相談専用のはずなので警告する。自動では止めない）。
    pub subagent_warning: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum IntakeBeginMode {
    /// 会話してから要約を作る。
    Converse,
    /// 依頼が決まっているので、要約カードを空で開く。
    Write,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeBeginArgs {
    pub folder: String,
    #[serde(default)]
    pub model: Option<super::model::ModelChoice>,
    pub mode: IntakeBeginMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeBeginResult {
    pub chat: super::model::Chat,
    pub intake: IntakeView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeChatArgs {
    pub chat: ChatKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeSaveArgs {
    pub chat: ChatKey,
    pub base_rev: u32,
    pub summary: IntakeSummary,
    pub prefs: IntakeStartPrefs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct IntakeStartArgs {
    pub chat: ChatKey,
    pub rev: u32,
    /// 確認ダイアログで見せた依頼文。現在の内容から組み立てた文と一字一句同じときだけ開始する。
    pub composed_request: String,
}

/// `org_start` に付ける由来（依頼整理の確定版）。`org_intake_start` だけが使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntakeOrigin {
    pub chat: ChatKey,
    pub intake_id: LocalId,
    pub rev: u32,
    pub summary: IntakeSummary,
    pub composed_request: String,
}

/// `org\<wf>\intake.json`（開始時の要約の写し）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntakeCopy {
    pub schema_version: u32,
    pub chat: ChatKey,
    pub intake_id: LocalId,
    pub rev: u32,
    pub summary: IntakeSummary,
    pub composed_request: String,
    /// 開始（ファイルを書いた）時刻。
    pub started_at: UnixMillis,
}
