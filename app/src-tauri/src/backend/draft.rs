//! 目的メモの下書き支援（`app/DESIGN_CONT_DRAFT.md`）のデータ型とIPC型。Codex固有の型・語は持たない。
//!
//! 下書きは「提案の一覧」で、保存はしない。目的メモの変更は既存の `set_purpose_note`（版の照合つき）だけ。

use serde::{Deserialize, Serialize};

use super::model::*;

/// 提案の種類（目的メモの節に対応する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum DraftKind {
    Decision,
    Constraint,
    OpenIssue,
    Reference,
}

/// 提案の操作。`Replace`・`Remove` は現在のメモの1行を対象にする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum DraftAction {
    Add,
    Replace,
    Remove,
}

/// 提案に付く機械検査の印。`NoEvidence` 以外は「要確認」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum ProposalIssue {
    /// 出典番号が台帳（または入力）に存在しない。
    CitationMissing,
    /// 番号は存在するが、原文を渡しておらず台帳も読めない（目次だけ）。
    CitationUnverifiable,
    /// 出典の記載が矛盾する（出典なしで根拠なしでない／出典ありで根拠なし）。
    CitationInconsistent,
    /// モデルが根拠なしと申告した（要確認ではない）。
    NoEvidence,
    /// 置き換え・削除の対象の行が、現在のメモにちょうど1つ見つからない。
    ReplaceTargetMissing,
    /// 形式崩れ（本文の空・150字超、種類・操作が不正）。
    Malformed,
}

impl ProposalIssue {
    /// 機械的な問題（要確認）か。根拠なしの申告は含めない。
    pub fn needs_review(self) -> bool {
        !matches!(self, ProposalIssue::NoEvidence)
    }
}

/// 番号の体系。`Global` は台帳の通し番号、`HeadOnly` は今の thread だけの連番（`[今#n]`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum DraftMode {
    Global,
    HeadOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DraftProposal {
    pub index: u32,
    /// 不正な値だったときは None（形式崩れの印が付く）。
    pub kind: Option<DraftKind>,
    pub action: Option<DraftAction>,
    pub text: String,
    /// 置き換え・削除の対象（現在のメモの該当行）。追加では空。
    pub replaces: String,
    pub sources: Vec<u32>,
    pub no_evidence: bool,
    pub note: String,
    pub issues: Vec<ProposalIssue>,
    /// 採用チェックの既定。印のない追加・置き換えだけ true（削除は常に false）。
    pub default_pick: bool,
}

/// 出典チップで開く往復の抜粋（ホストが入力から返す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RoundGlance {
    pub number: u32,
    pub time: Option<String>,
    pub user_head: String,
    pub ai_head: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DraftState {
    /// 材料を集めている（まだ何も送っていない）。
    Preparing,
    /// 送信を受理して作成中。
    Running { started_at: UnixMillis },
    /// 結果を読めた（確認待ち）。`review_count` は要確認の件数、`dropped` は上限超過で捨てた件数。
    Ready { proposals: Vec<DraftProposal>, omitted: String, dropped: u32, review_count: u32 },
    /// 結果を読めなかった（項目にせず本文をそのまま示す）。
    Unreadable { raw: String, reason: String },
    /// 作成を始められなかった（拒否）。
    Rejected { message: String },
    /// 作成を始めたか確認できない（再送しない）。結果が届けばここに出る。
    AcceptanceUnknown,
    /// 接続が切れた。結果は不明（完了・失敗ではない）。
    Lost,
    /// 中止された。
    Interrupted,
    /// バックエンドが失敗を報告した。
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PurposeDraftView {
    pub id: LocalId,
    pub chat: ChatKey,
    /// 押した時点の目的メモの版（未設定は null）。保存はこの版を期待版にする。
    pub base_revision: Option<u32>,
    pub mode: DraftMode,
    /// 一時 thread へ送った本文の文字数。
    pub input_chars: u32,
    pub rounds_included: u32,
    pub rounds_available: u32,
    pub toc_rounds: u32,
    /// 実行中の往復を含めていない（作業中に押した）。
    pub excluded_running: bool,
    /// 使用トークン数（通知が届いたときだけ。推定しない）。
    pub usage_tokens: Option<u64>,
    pub state: DraftState,
    /// 提案が出典に挙げた往復の抜粋。
    pub glances: Vec<RoundGlance>,
}

/// 作成前の見積り（読取りのみ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct PurposeDraftEstimate {
    /// 作れない理由（空なら作れる）。
    pub blockers: Vec<String>,
    /// 以前の thread を読めず、「今の thread だけで作る」を選べる。
    pub head_only_available: bool,
    pub rounds_included: u32,
    pub rounds_available: u32,
    pub toc_rounds: u32,
    /// 送る本文の文字数の見込み。
    pub input_chars: u32,
    pub purpose_revision: Option<u32>,
    pub excluded_running: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct StartPurposeDraftArgs {
    pub chat: ChatKey,
    /// 画面が見ていた目的メモの版（違えば作らない）。
    pub expected_revision: Option<u32>,
    /// 以前の thread を読めないとき、利用者が「今の thread だけで作る」を押した。
    pub head_only: bool,
}
