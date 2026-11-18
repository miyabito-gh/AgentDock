//! 受付係（`app/DESIGN_ROUTER.md` §9）のIPC型。Codex固有の型は持たない。
//!
//! - 振り分けの記録・ルール帳の正本は `store::records`（`RouterRecord`）と `rules::router`（`RulesFile`）。ここはUIへ出す写し。
//! - 取得できない値は `None`／空にして、0や空文字で代用しない。

use serde::{Deserialize, Serialize};

use super::ipc::SendIntent;
use super::model::{ChatKey, LocalId, SendAttempt};
use crate::rules::router::{Rule, RuleOp, RulesChange};
use crate::store::records::{DispatchProposal, DispatchState, DispatchTarget};

/// 受付係の画面の写し（読取りだけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RouterView {
    pub chat: ChatKey,
    /// ルール帳（`rules.json`）を読めず、退避して空から始めたときの警告。黙って消さない。
    pub rules_warning: Option<String>,
    pub rules_revision: u32,
    /// 振り分けカード（新しい順）。
    pub dispatches: Vec<DispatchView>,
    /// 受付係の返答ごとの解析結果（新しい順）。`turn` が返答（turn）のID。
    pub turns: Vec<RouterTurnView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RouterTurnView {
    pub turn: String,
    /// 解析できた返答文。None なら原文を通常の本文として見せる（「振り分け案を読み取れませんでした」）。
    pub reply: Option<String>,
    pub parse_error: Option<String>,
    /// 検査で落とした振り分け案の理由。
    pub route_rejects: Vec<String>,
    pub notices: Vec<RuleNoticeView>,
}

/// 返答の下の1行（「ルールを更新：…［取り消す］」または「ルールは更新していません（理由）」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RuleNoticeView {
    /// 書いた（取り消せる）なら、その変更が作ったルール帳の版（`router_rule_undo` に渡す）。書かなかったときは None。
    pub revision: Option<u32>,
    pub text: String,
    /// 取り消し済み。
    pub undone: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DispatchView {
    pub id: LocalId,
    /// どの返答（turn）から出た案か。
    pub source_turn: String,
    pub route_index: u32,
    pub target: DispatchTarget,
    pub suggested: DispatchTarget,
    pub state: DispatchState,
    pub reported: bool,
    /// 送り先チャットの今の名前（新規で未作成なら None）。
    pub target_title: Option<String>,
    pub suggested_title: Option<String>,
    /// 送り先が今実行中か（実行中なら追加指示として送る旨をカードに出す）。
    pub target_running: bool,
    /// 送り先が使えなくなっている理由（アーカイブ・組織実行・引き継ぎ済みなど）。あれば［送る］を出さない。
    pub target_problem: Option<String>,
    /// 受付係の案の中身。古い記録にはない。
    pub proposal: Option<DispatchProposal>,
}

/// カードの［送る］。`text` は編集後の本文（既定は案の本文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RouterDispatchArgs {
    pub dispatch_id: LocalId,
    /// ［別の会話へ］で送り先を変えたとき。None なら案の送り先。送り先が新規の案には必須（新規は `router_dispatch_created`）。
    #[serde(default)]
    pub target_override: Option<ChatKey>,
    pub text: String,
    /// None＝自動（送り先が実行中なら追加指示、そうでなければ新しい turn）。
    #[serde(default)]
    pub intent: Option<SendIntent>,
}

/// 新規の案を、UIが既存の新規チャット作成の経路で作った後に呼ぶ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RouterDispatchCreatedArgs {
    pub dispatch_id: LocalId,
    /// 作ったチャット。
    pub chat: ChatKey,
    /// `start_chat` の戻り値の `firstSend`（最初の依頼を付けて作ったとき）。None＝送っていない（案は「提案」のまま）。
    /// 状態が確定していない（送信中）・受理不明のときは「受理を確認できていない」として記録する。確定後にもう一度呼ぶと更新できる。
    #[serde(default)]
    pub first_send: Option<SendAttempt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RouterDispatchIdArgs {
    pub dispatch_id: LocalId,
}

/// ルール帳の画面の写し。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RulesView {
    pub revision: u32,
    pub warning: Option<String>,
    pub rules: Vec<RuleView>,
    /// 新しい順。
    pub history: Vec<RuleChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    pub rule: Rule,
    /// チャット指定のルールの、送り先の今の名前。解決できなければ None（「送り先なし」）。
    pub target_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RuleChangeView {
    pub change: RulesChange,
    /// 「ルールを更新：…」の要約。
    pub summary: String,
}

/// ルールの手での編集の送り先の指定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RuleTargetInput {
    Chat { chat: ChatKey },
    NewIn { folder: String },
}

/// 手での編集（出所は手動）。上限・禁止語は会話からの変更と同じに守る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RuleEditArgs {
    pub op: RuleOp,
    /// update・remove の対象。
    #[serde(default)]
    pub rule_id: Option<String>,
    /// add・update の本文（remove では無視）。
    #[serde(default)]
    pub text: String,
    /// add: None＝条件だけのルール。update: None＝送り先は変えない。
    #[serde(default)]
    pub target: Option<RuleTargetInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RuleUndoArgs {
    /// 取り消す変更が作ったルール帳の版。
    pub revision: u32,
    /// その後に同じルールを変えた変更があるとき、確認のうえ true で再実行する。
    #[serde(default)]
    pub force: bool,
}
