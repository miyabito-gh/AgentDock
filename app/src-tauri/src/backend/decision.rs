//! 応答した承認・質問・MCP許可のローカル記録（項目51、`app/DESIGN_APPROVAL_HISTORY.md`）。
//!
//! Codexの履歴（thread/read）にはこの種の項目がないため、ユーザーが押した結果（事実）だけを、AgentDockの
//! チャット別領域（`decisions.jsonl`、追記）へ残す。応答の代行・再送には使わない。内容の要約は短く切る（長さの上限は `rules::decision`）。

use serde::{Deserialize, Serialize};

use super::model::*;

pub const DECISION_SCHEMA_VERSION: u32 = 1;

/// 要求の種類（表示用の短い区分。Codex固有の名前や原文は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum DecisionKind {
    Command,
    FileChange,
    Permissions,
    Question,
    /// MCP等の外部ツールからの許可・入力の確認。
    ToolPermission,
    Other,
}

/// 選んだ応答。`ResolvedElsewhere` と `Unconfirmed` はユーザーが選んだものではなく、そのときの状態の記録。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DecisionOutcome {
    Allowed { scope: DecisionScope },
    Declined,
    Cancelled,
    /// 質問に答えた（答えの本文は保存しない）。
    Answered,
    /// 選択肢の意味を分類できなかった応答（選んだ選択肢の表示名だけ残す）。
    Chosen { label: String },
    /// 応答を送ったが、受理を確認できない（再送しない）。
    Unconfirmed,
    /// 別の場所（Codexの別画面・turn終了など）で解決された。AgentDockの応答ではない。
    ResolvedElsewhere,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DecisionRecord {
    pub id: LocalId,
    pub chat: ChatKey,
    /// 要求元のエージェント（子・孫の名前の表示に使う）。
    pub agent: AgentKey,
    /// 記録時点の表示名の写し（取得できたときだけ。画面は現在の名前を優先する）。
    pub agent_name: Option<String>,
    pub turn: Option<ExternalId>,
    pub kind: DecisionKind,
    /// 内容の短い要約（コマンド・パス・URL・質問文は長さを切る。秘密入力は含めない）。
    pub summary: String,
    pub outcome: DecisionOutcome,
    pub requested_at: UnixMillis,
    /// 応答（または解決を観測）した時刻。
    pub at: UnixMillis,
}

/// `decisions.jsonl` の1行。新しい版の行は読まない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionEntry {
    pub schema_version: u32,
    pub record: DecisionRecord,
}

/// 記録の保存結果（UIが「保存できていません」を出すため）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DecisionSave {
    Saved,
    Failed { message: String },
}
