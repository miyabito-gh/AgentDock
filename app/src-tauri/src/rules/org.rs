//! 組織実行（P-ORG-4c）の純粋ロジック: sandbox健全性チェックの判定と、config.toml の trust 追記の分類。
//!
//! どちらも読むだけ。config.toml は本文を受け取って比べるだけで、書かない・保存しない・ログに出さない。
//! 既存の `judge_config`／`ConfigCompare`（CLI管理操作用）には触れない。

use org_engine::{ChatRef, TurnRef};

use crate::backend::backend::{BackendError, FailureClass};
use crate::backend::local::QueueHold;
use crate::backend::model::{ActivityKind, AgentKey, AgentState, BackendKind, ChatKey, ExternalId, Known, MessagePhase, TurnEnd, TurnKey, TurnRecord};
use crate::backend::parity::SandboxCheck;
use crate::rules::queue::{GateDecision, GateInput};
use crate::store::records::OrgBinding;

// ───────────────────────────── 参照の符号化（エンジンへ渡す不透明な値） ─────────────────────────────

/// ホストの `ChatKey` を、エンジンが比べて保存するだけの文字列にする（`codex:<id>`）。`chat_key_of_ref` で戻せる。
pub fn chat_ref_of(chat: &ChatKey) -> ChatRef {
    match chat.backend {
        BackendKind::Codex => ChatRef::new(format!("codex:{}", chat.id.0)),
    }
}

/// `chat_ref_of` の逆。知らない形式なら None（推定しない）。
pub fn chat_key_of_ref(r: &ChatRef) -> Option<ChatKey> {
    let id = r.0.strip_prefix("codex:").filter(|id| !id.is_empty())?;
    Some(ChatKey { backend: BackendKind::Codex, id: ExternalId(id.to_string()) })
}

/// turn の中立ID。チャットは別に渡すので、turn ID そのものにする。
pub fn turn_ref_of(turn: &ExternalId) -> TurnRef {
    TurnRef::new(turn.0.clone())
}

/// ルートの `TurnKey` に戻す（チャットのルートのturnだけを扱う）。
pub fn turn_key_of_ref(chat: &ChatKey, turn: &TurnRef) -> TurnKey {
    TurnKey { agent: AgentKey { backend: chat.backend, id: chat.id.clone() }, turn_id: ExternalId(turn.0.clone()) }
}

// ───────────────────────────── 組織チャットの保護 ─────────────────────────────

/// 利用者の操作の種類（保護の判定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgOp {
    /// 利用者の手動送信・再送。
    UserSend,
    /// キューへの積み込み・編集・再開・今すぐ送る。
    Queue,
    /// モデル・権限・作業フォルダ・計画／実行・目標などの設定変更、レビュー・圧縮・再送の分岐。
    Change,
    /// アーカイブ・解除・削除。ワークフロー単位でだけ行う。
    Manage,
    /// 開く・読む。
    Open,
    /// 中断（停止は利用者がいつでもできる）。
    Stop,
    /// 承認・質問への回答（利用者が既存の承認カードで答える）。
    Answer,
    /// 元を変えない分岐・別threadの相談。
    Branch,
    /// ピン・名前変更。
    Label,
}

impl OrgOp {
    fn allowed_on_owned_chat(self) -> bool {
        matches!(self, OrgOp::Open | OrgOp::Stop | OrgOp::Answer | OrgOp::Branch | OrgOp::Label)
    }
}

pub const ORG_OWNED_MESSAGE: &str = "組織実行中のチャットです。介入はワークフロー画面の「手動に切り替える」から行ってください";

/// 組織が所有し、まだ手動に切り替えていないチャットに対する利用者の操作を判定する。
/// 所有は `chat.json.org` の記録だけで決める（推定しない）。解放済みなら通常のチャットとして扱う。
pub fn org_guard(binding: Option<&OrgBinding>, op: OrgOp) -> Result<(), &'static str> {
    match binding {
        Some(b) if b.released_at.is_none() && !op.allowed_on_owned_chat() => Err(ORG_OWNED_MESSAGE),
        _ => Ok(()),
    }
}

/// 手動の結果の取り込み中の送信を断る文言。
pub const ORG_ADOPTING_MESSAGE: &str = "結果の取り込み中です。終わるまで送信できません。";

/// 取り込み中のタスクのチャットか。解放済み（手動に切り替え済み）のチャットでも、取り込みの間は通常の送信を断る。
/// `tasks` は（タスクのチャット, 取り込み中か）の一覧。
pub fn is_adopting(chat: &ChatKey, tasks: impl IntoIterator<Item = (Option<ChatKey>, bool)>) -> bool {
    tasks.into_iter().any(|(c, adopting)| adopting && c.as_ref() == Some(chat))
}

// ───────────────────────────── 最終回答の取り出し ─────────────────────────────

/// 履歴からそのturnの最終回答を取り出す。turn が見つからない・終端が完了と確認できない・履歴が部分取得・本文が読めないときは None
/// （推定しない）。最終回答の位置づけ（Final）があればその最後の発言、なければ途中経過（Commentary）でない最後の発言。
pub fn final_message_of(turns: &[TurnRecord], turn: &TurnKey) -> Option<String> {
    let rec = turns.iter().find(|t| &t.key == turn)?;
    if !rec.complete || rec.end != Some(TurnEnd::Completed) {
        return None;
    }
    let messages = || rec.entries.iter().filter(|e| e.kind == ActivityKind::AgentMessage);
    let pick = messages().filter(|e| e.message_phase == MessagePhase::Final).last().or_else(|| messages().filter(|e| e.message_phase != MessagePhase::Commentary).last())?;
    pick.text.value().cloned().filter(|t| !t.trim().is_empty())
}

/// 履歴からそのturnの終端を読む（再起動・wake後の照合。読取りだけ）。turn が見つからない・終端が確認できない（進行中・不明）は None
/// （推定しない。None は「終わっていない」ではなく「確認できていない」）。
pub fn turn_end_of(turns: &[TurnRecord], turn: &TurnKey) -> Option<TurnEnd> {
    turns.iter().find(|t| &t.key == turn)?.end
}

/// 履歴からそのturnの失敗の区分を読む（読取りだけ）。turn が見つからないときは未取得。
pub fn turn_failure_of(turns: &[TurnRecord], turn: &TurnKey) -> Known<FailureClass> {
    turns.iter().find(|t| &t.key == turn).map_or(Known::NotFetched, |t| t.failure.clone())
}

/// 履歴で確認した終端をエンジンの入力にする。失敗は、区分が読めたときだけ利用上限か否かを決め、
/// 区分が欠けている・読めていないときは「区分不明」にする（利用上限でも実行失敗でも推測で決めない）。
pub fn turn_end_kind_of(end: TurnEnd, failure: &Known<FailureClass>) -> org_engine::TurnEndKind {
    match end {
        TurnEnd::Completed => org_engine::TurnEndKind::Completed,
        TurnEnd::Interrupted => org_engine::TurnEndKind::Interrupted,
        TurnEnd::Failed => match failure {
            Known::Value { value: FailureClass::UsageLimit, .. } => org_engine::TurnEndKind::Failed { usage_limit: true },
            Known::Value { value: FailureClass::Other, .. } => org_engine::TurnEndKind::Failed { usage_limit: false },
            _ => org_engine::TurnEndKind::FailedUnclassified,
        },
    }
}

// ───────────────────────────── 子孫の作業中判定（キューの判断の読取り） ─────────────────────────────

/// 組織の完了判定に渡す子孫の様子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescendantReport {
    pub working: u32,
    /// 状態を確認できていない子孫（不明のまま数える。完了・失敗にしない）。
    pub unknown: u32,
    /// 結論を出してよいか。false のときは完了と判断しない。
    pub scan_complete: bool,
}

/// 失敗・中断が確定している子孫は、作業中でも不明でもないので判定から外す（親の完了を止めない）。
pub fn drop_terminal_failures(input: &mut GateInput) {
    input.descendants.retain(|f| {
        let failed_end = matches!(f.latest_end, Some((TurnEnd::Failed | TurnEnd::Interrupted, _)));
        let failed_state = matches!(f.state, AgentState::Failed | AgentState::Interrupted);
        !(failed_end || failed_state)
    });
}

/// キューの判断（`evaluate_gate`）の結果を、子孫の様子に読み替える。送信の可否ではなく読取りだけに使う。
pub fn descendants_from_gate(decision: &GateDecision, root: &AgentKey) -> DescendantReport {
    let open = |working: u32, unknown: u32| DescendantReport { working, unknown, scan_complete: true };
    let undecided = |unknown: u32| DescendantReport { working: 0, unknown, scan_complete: false };
    match decision {
        GateDecision::Send { unconfirmed_descendants, .. } => open(0, *unconfirmed_descendants),
        GateDecision::Hold(QueueHold::DescendantsActive { targets }) => open(targets.len() as u32, 0),
        GateDecision::Hold(QueueHold::ParentWorking) => open(1, 0),
        GateDecision::Hold(QueueHold::NotLive { targets }) => {
            if targets.iter().any(|t| &t.agent == root) {
                undecided(0)
            } else {
                open(0, targets.len() as u32)
            }
        }
        GateDecision::Hold(QueueHold::StateUnknown { targets, .. }) => undecided(targets.len() as u32),
        // 接続・停止未確認・受理不明などは、子孫について結論を出さない。
        GateDecision::Hold(_) | GateDecision::Stop(_) | GateDecision::Nothing | GateDecision::Paused => undecided(0),
    }
}

// ───────────────────────────── sandbox健全性チェックの判定 ─────────────────────────────

/// 健全性チェックの結果。`Unhealthy` はワークフローを `Halted{SandboxUnhealthy}`（要対応）で止める根拠。
/// 自動再試行・自動回避はしない（利用者の「再開」でだけ出し直す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxVerdict {
    Healthy,
    /// 失敗・応答なし・非対応。成功とは扱わない。`reason` は診断用（標準エラー本文は含めない）。
    /// `sandbox_failed` が真なら sandbox が実際に起動できなかった（準備未完了・確認コマンドの失敗）。
    /// 偽なら確認自体を実行できなかった（要求の拒否・応答なし・非対応）で、sandbox の不調とは断定しない。
    Unhealthy { reason: String, sandbox_failed: bool },
}

/// 確認自体を実行できなかったときの案内文の前半（理由と後半は `sandbox_notice` が付ける）。
pub const SANDBOX_CHECK_NOT_RUN_PREFIX: &str = "確認を実行できませんでした: ";
pub const SANDBOX_CHECK_NOT_RUN_SUFFIX: &str = "。これはAgentDockの確認処理の問題の可能性があります。組織実行は始めていません。";

/// 判定結果からワークフロー画面に出す案内文を作る（`Healthy` は空）。
pub fn sandbox_notice(verdict: &SandboxVerdict) -> String {
    match verdict {
        SandboxVerdict::Healthy => String::new(),
        SandboxVerdict::Unhealthy { reason, sandbox_failed: true } => format!("{SANDBOX_UNHEALTHY_NOTICE}\n（{reason}）"),
        SandboxVerdict::Unhealthy { reason, sandbox_failed: false } => {
            format!("{SANDBOX_CHECK_NOT_RUN_PREFIX}{reason}{SANDBOX_CHECK_NOT_RUN_SUFFIX}")
        }
    }
}

/// 失敗時の案内文（DESIGN_ORG_P4 §5.2。ワークフロー画面の帯に出す）。
pub const SANDBOX_UNHEALTHY_NOTICE: &str = "Codex の sandbox を起動できないため、組織実行を始めていません。\n原因の候補: Codex のプラグイン（node_repl／computer-use）と sandbox の更新が競合している（os error 32）。詳しくは %USERPROFILE%\\.codex\\.sandbox\\ のログを確認してください。\n回避: 設定 →「Codex」→「sandbox の競合を回避する」をオンにし、AgentDock を再起動してから「再開」。";

/// `ParityOps::sandbox_check` の結果を判定する。応答なし（`OutcomeUnknown`）・拒否・非対応はすべて `Unhealthy`。
pub fn judge_sandbox(result: &Result<SandboxCheck, BackendError>) -> SandboxVerdict {
    let check = match result {
        Ok(c) => c,
        Err(BackendError::Unsupported { .. }) => return not_run("このバックエンドでは sandbox の確認ができません".to_string()),
        Err(BackendError::OutcomeUnknown { .. }) => return not_run("確認コマンドの結果が得られませんでした（応答なし）".to_string()),
        Err(e) => return not_run(e.to_string()),
    };
    match &check.readiness {
        Known::Value { value, .. } if value == "ready" => {}
        Known::Value { value, .. } => return failed(format!("sandbox の準備状態が ready ではありません（{value}）")),
        _ => return not_run("sandbox の準備状態を読み取れませんでした".to_string()),
    }
    match &check.exit_code {
        Known::Value { value: 0, .. } => SandboxVerdict::Healthy,
        Known::Value { value, .. } => failed(format!("確認コマンドが終了コード {value} で失敗しました")),
        _ => not_run("確認コマンドの終了コードを読み取れませんでした".to_string()),
    }
}

/// sandbox が実際に起動できなかった。
fn failed(reason: String) -> SandboxVerdict {
    SandboxVerdict::Unhealthy { reason, sandbox_failed: true }
}

/// 確認を実行できなかった（sandbox の不調とは断定しない。始めない点は同じ）。
fn not_run(reason: String) -> SandboxVerdict {
    SandboxVerdict::Unhealthy { reason, sandbox_failed: false }
}

// ───────────────────────────── trust 追記の分類（J2-A） ─────────────────────────────

/// config.toml の前後の比較結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustCompare {
    Unchanged,
    /// 追加された行が `[projects.'<path>']` 見出しと `trust_level = "trusted"` だけ（削除・変更なし）。
    CodexTrustAppended { projects: Vec<String> },
    /// それ以外の変化がある（Codex本体・他の操作・利用者の編集の可能性。AgentDock は書いていない）。値は返さない。
    Other { changed_lines: usize },
    /// 前後どちらかを読めなかった。一致とは扱わない。
    Unverified { reason: String },
}

/// 前後の config.toml 本文を比べ、「Codex が trust を追記しただけ」かを分類する。
/// 行の多重集合の差で判定する（空行は無視、前後の空白は無視）。見出しは `[projects."…"]`・`[projects.'…']` の両方を受ける。
/// 追記と認めるのは、見出しと `trust_level = "trusted"` が同数で1件以上あり、ほかの追加・削除がないときだけ。
pub fn classify_trust_append(before: Option<&str>, after: Option<&str>) -> TrustCompare {
    let (Some(before), Some(after)) = (before, after) else {
        return TrustCompare::Unverified { reason: "config.toml を読み取れなかったため、前後を比べられません".to_string() };
    };
    let mut removed = lines_of(before);
    let mut added = Vec::new();
    for line in lines_of(after) {
        match removed.iter().position(|l| *l == line) {
            Some(i) => {
                removed.swap_remove(i);
            }
            None => added.push(line),
        }
    }
    if added.is_empty() && removed.is_empty() {
        return TrustCompare::Unchanged;
    }
    let other = |n: usize| TrustCompare::Other { changed_lines: n };
    if !removed.is_empty() {
        return other(added.len() + removed.len());
    }
    let mut projects = Vec::new();
    let mut trust_lines = 0usize;
    for line in &added {
        if let Some(p) = project_heading(line) {
            projects.push(p);
        } else if is_trusted_line(line) {
            trust_lines += 1;
        } else {
            return other(added.len());
        }
    }
    if projects.is_empty() || projects.len() != trust_lines {
        return other(added.len());
    }
    TrustCompare::CodexTrustAppended { projects }
}

/// 空行を除き、前後の空白を落とした行。
fn lines_of(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// `[projects."…"]`／`[projects.'…']` の見出しなら、中のパスを返す（基本文字列の `\\` は `\` に戻す）。
fn project_heading(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    let rest = inner.strip_prefix("projects.")?.trim();
    if let Some(p) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Some(p.replace("\\\\", "\\"));
    }
    rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')).map(str::to_string)
}

/// `trust_level = "trusted"`（空白・引用符の種類は問わない）。コメント付きは別の行として扱う（一致しない）。
fn is_trusted_line(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "trust_level=\"trusted\"" || compact == "trust_level='trusted'"
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "model = \"gpt-5\"\n\n[projects.'C:\\work\\a']\ntrust_level = \"trusted\"\n";

    fn check(readiness: Known<String>, exit: Known<i32>) -> SandboxCheck {
        SandboxCheck { readiness, exit_code: exit, stderr_head: Known::NotFetched }
    }

    #[test]
    fn trust_appended_only() {
        let after = format!("{BASE}\n[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::CodexTrustAppended { projects: vec!["C:\\work\\b".to_string()] });
        // 基本文字列の見出し（\\ は \ に戻る）、同一内容は Unchanged。
        let after = format!("{BASE}[projects.\"C:\\\\work\\\\c\"]\ntrust_level = \"trusted\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::CodexTrustAppended { projects: vec!["C:\\work\\c".to_string()] });
        assert_eq!(classify_trust_append(Some(BASE), Some(BASE)), TrustCompare::Unchanged);
    }

    #[test]
    fn heading_without_trust_value_is_other() {
        let after = format!("{BASE}[projects.'C:\\work\\b']\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { changed_lines: 1 });
        // 値だけの追加も追記とは認めない。
        let after = format!("{BASE}trust_level = \"trusted\"\n");
        assert!(matches!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { .. }));
    }

    #[test]
    fn other_key_changes_mixed_in_are_other() {
        let after = format!("{BASE}[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\napproval_policy = \"never\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { changed_lines: 3 });
        // untrusted は追記と認めない。
        let after = format!("{BASE}[projects.'C:\\work\\b']\ntrust_level = \"untrusted\"\n");
        assert!(matches!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { .. }));
    }

    #[test]
    fn any_removal_is_other_even_with_an_append() {
        let after = "[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\n";
        match classify_trust_append(Some(BASE), Some(after)) {
            TrustCompare::Other { changed_lines } => assert!(changed_lines >= 3),
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn unreadable_side_is_unverified_not_unchanged() {
        assert!(matches!(classify_trust_append(None, Some(BASE)), TrustCompare::Unverified { .. }));
        assert!(matches!(classify_trust_append(Some(BASE), None), TrustCompare::Unverified { .. }));
        assert!(matches!(classify_trust_append(None, None), TrustCompare::Unverified { .. }));
    }

    #[test]
    fn whitespace_and_blank_lines_are_ignored() {
        let after = "model = \"gpt-5\"\n\n\n  [projects.'C:\\work\\a']  \n  trust_level = \"trusted\"  \n";
        assert_eq!(classify_trust_append(Some(BASE), Some(after)), TrustCompare::Unchanged);
    }

    #[test]
    fn sandbox_healthy_only_when_ready_and_exit_zero() {
        let ok = check(Known::direct("ready".to_string()), Known::direct(0));
        assert_eq!(judge_sandbox(&Ok(ok)), SandboxVerdict::Healthy);
    }

    #[test]
    fn sandbox_failures_are_unhealthy_and_never_healthy() {
        let not_ready = check(Known::direct("updateRequired".to_string()), Known::NotFetched);
        assert!(matches!(judge_sandbox(&Ok(not_ready)), SandboxVerdict::Unhealthy { .. }));
        let bad_exit = check(Known::direct("ready".to_string()), Known::direct(1));
        assert!(matches!(judge_sandbox(&Ok(bad_exit)), SandboxVerdict::Unhealthy { .. }));
        // 終了コードを読めない（0 で代用しない）。
        let missing = check(Known::direct("ready".to_string()), Known::Missing);
        assert!(matches!(judge_sandbox(&Ok(missing)), SandboxVerdict::Unhealthy { .. }));
        // 起動失敗は競合案内、要求の拒否は別文言（どちらも始めない）。
        let started_bad = judge_sandbox(&Ok(check(Known::direct("ready".to_string()), Known::direct(1))));
        assert!(sandbox_notice(&started_bad).starts_with(SANDBOX_UNHEALTHY_NOTICE));
        let rejected = judge_sandbox(&Err(BackendError::Rejected { code: None, message: "custom outputBytesCap is not supported".into() }));
        assert!(matches!(rejected, SandboxVerdict::Unhealthy { sandbox_failed: false, .. }));
        let text = sandbox_notice(&rejected);
        assert!(text.starts_with(SANDBOX_CHECK_NOT_RUN_PREFIX) && text.contains("outputBytesCap") && !text.contains("os error 32"));
        let unreadable = check(Known::Missing, Known::NotFetched);
        assert!(matches!(judge_sandbox(&Ok(unreadable)), SandboxVerdict::Unhealthy { .. }));
        for e in [
            BackendError::OutcomeUnknown { message: "timeout".into() },
            BackendError::Unsupported { capability: "sandboxCheck".into() },
            BackendError::Rejected { code: None, message: "x".into() },
            BackendError::NotConnected,
        ] {
            assert!(matches!(judge_sandbox(&Err(e)), SandboxVerdict::Unhealthy { .. }));
        }
    }

    fn binding(released: bool) -> OrgBinding {
        OrgBinding {
            workflow_id: "w1".into(),
            task_id: "t1".into(),
            role_id: "worker".into(),
            released_at: released.then_some(crate::backend::model::UnixMillis(5)),
        }
    }

    #[test]
    fn owned_chat_refuses_user_sends_and_changes_but_not_stop_answer_or_open() {
        let b = binding(false);
        for op in [OrgOp::UserSend, OrgOp::Queue, OrgOp::Change, OrgOp::Manage] {
            assert!(org_guard(Some(&b), op).is_err(), "{op:?}");
        }
        for op in [OrgOp::Open, OrgOp::Stop, OrgOp::Answer, OrgOp::Branch, OrgOp::Label] {
            assert!(org_guard(Some(&b), op).is_ok(), "{op:?}");
        }
    }

    #[test]
    fn released_or_unowned_chat_is_an_ordinary_chat() {
        for op in [OrgOp::UserSend, OrgOp::Queue, OrgOp::Change, OrgOp::Manage] {
            assert!(org_guard(Some(&binding(true)), op).is_ok());
            assert!(org_guard(None, op).is_ok());
        }
    }

    #[test]
    fn chat_ref_round_trips_and_unknown_forms_are_refused() {
        let k = ChatKey { backend: BackendKind::Codex, id: ExternalId("th-1".into()) };
        let r = chat_ref_of(&k);
        assert_eq!(r.0, "codex:th-1");
        assert_eq!(chat_key_of_ref(&r), Some(k.clone()));
        assert_eq!(chat_key_of_ref(&ChatRef::new("other:x")), None);
        assert_eq!(chat_key_of_ref(&ChatRef::new("codex:")), None);
        let t = turn_key_of_ref(&k, &turn_ref_of(&ExternalId("t9".into())));
        assert_eq!((t.agent.id.0.as_str(), t.turn_id.0.as_str()), ("th-1", "t9"));
    }

    fn entry(text: Known<String>, phase: MessagePhase) -> crate::backend::model::TranscriptEntry {
        crate::backend::model::TranscriptEntry {
            key: crate::backend::model::ItemKey { agent: root(), turn_id: Some(ExternalId("t1".into())), item_id: ExternalId("i".into()) },
            kind: ActivityKind::AgentMessage,
            text,
            phase: crate::backend::model::ActivityPhase::Completed,
            message_phase: phase,
            attachments: Vec::new(),
        }
    }

    fn record(end: Option<TurnEnd>, complete: bool, entries: Vec<crate::backend::model::TranscriptEntry>) -> TurnRecord {
        TurnRecord {
            key: TurnKey { agent: root(), turn_id: ExternalId("t1".into()) },
            end,
            started_at: Known::NotFetched,
            completed_at: Known::NotFetched,
            entries,
            complete,
            failure: Known::NotFetched,
        }
    }

    #[test]
    fn final_message_is_taken_only_from_a_confirmed_complete_turn() {
        let want = TurnKey { agent: root(), turn_id: ExternalId("t1".into()) };
        let msgs = vec![entry(Known::direct("progress".to_string()), MessagePhase::Commentary), entry(Known::direct("RESULT".to_string()), MessagePhase::Final)];
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Completed), true, msgs.clone())], &want).as_deref(), Some("RESULT"));
        // Not confirmed complete, partial history, another turn, or an unreadable body are all "not obtained".
        assert_eq!(final_message_of(&[record(None, true, msgs.clone())], &want), None);
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Failed), true, msgs.clone())], &want), None);
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Completed), false, msgs)], &want), None);
        assert_eq!(final_message_of(&[], &want), None);
        let unreadable = vec![entry(Known::NotFetched, MessagePhase::Final)];
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Completed), true, unreadable)], &want), None);
        // Without a Final marker, the last non-commentary message is used; commentary alone is never taken.
        let unmarked = vec![entry(Known::direct("A".to_string()), MessagePhase::Unknown)];
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Completed), true, unmarked)], &want).as_deref(), Some("A"));
        let only_commentary = vec![entry(Known::direct("c".to_string()), MessagePhase::Commentary)];
        assert_eq!(final_message_of(&[record(Some(TurnEnd::Completed), true, only_commentary)], &want), None);
    }

    #[test]
    fn turn_end_is_read_only_when_the_history_confirms_it() {
        let want = TurnKey { agent: root(), turn_id: ExternalId("t1".into()) };
        let other = TurnKey { agent: root(), turn_id: ExternalId("t2".into()) };
        assert_eq!(turn_end_of(&[record(Some(TurnEnd::Interrupted), true, Vec::new())], &want), Some(TurnEnd::Interrupted));
        // still running / unknown / another turn / no turns: not confirmed (None is never read as "not ended")
        assert_eq!(turn_end_of(&[record(None, true, Vec::new())], &want), None);
        assert_eq!(turn_end_of(&[record(Some(TurnEnd::Completed), true, Vec::new())], &other), None);
        assert_eq!(turn_end_of(&[], &want), None);
    }

    #[test]
    fn adoption_blocks_only_the_chat_of_the_adopting_task() {
        let a = ChatKey { backend: BackendKind::Codex, id: ExternalId("a".into()) };
        let b = ChatKey { backend: BackendKind::Codex, id: ExternalId("b".into()) };
        let tasks = vec![(Some(a.clone()), true), (Some(b.clone()), false), (None, true)];
        assert!(is_adopting(&a, tasks.clone()));
        assert!(!is_adopting(&b, tasks.clone()));
        assert!(!is_adopting(&a, vec![(Some(a.clone()), false)]));
        assert!(!is_adopting(&a, Vec::new()));
    }

    #[test]
    fn a_failed_turn_is_classified_only_from_a_read_class() {
        use org_engine::TurnEndKind as K;
        assert_eq!(turn_end_kind_of(TurnEnd::Failed, &Known::direct(FailureClass::UsageLimit)), K::Failed { usage_limit: true });
        assert_eq!(turn_end_kind_of(TurnEnd::Failed, &Known::direct(FailureClass::Other)), K::Failed { usage_limit: false });
        for unknown in [Known::Missing, Known::NotFetched, Known::Unsupported] {
            assert_eq!(turn_end_kind_of(TurnEnd::Failed, &unknown), K::FailedUnclassified);
        }
        assert_eq!(turn_end_kind_of(TurnEnd::Completed, &Known::Missing), K::Completed);
        assert_eq!(turn_end_kind_of(TurnEnd::Interrupted, &Known::direct(FailureClass::UsageLimit)), K::Interrupted);
        // the class is read from the history record of that turn
        let want = TurnKey { agent: root(), turn_id: ExternalId("t1".into()) };
        let mut rec = record(Some(TurnEnd::Failed), true, Vec::new());
        rec.failure = Known::direct(FailureClass::UsageLimit);
        assert_eq!(turn_failure_of(&[rec], &want), Known::direct(FailureClass::UsageLimit));
        assert_eq!(turn_failure_of(&[], &want), Known::NotFetched);
    }

    fn root() -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId("r".into()) }
    }

    fn target(id: &str, state: AgentState) -> crate::backend::local::HoldTarget {
        crate::backend::local::HoldTarget {
            agent: AgentKey { backend: BackendKind::Codex, id: ExternalId(id.into()) },
            state,
            freshness: crate::backend::model::Freshness::Live,
        }
    }

    #[test]
    fn descendant_report_never_concludes_from_an_undecided_gate() {
        let r = root();
        let send = GateDecision::Send { entry: crate::backend::model::LocalId("e".into()), unconfirmed_descendants: 2 };
        assert_eq!(descendants_from_gate(&send, &r), DescendantReport { working: 0, unknown: 2, scan_complete: true });
        let active = GateDecision::Hold(QueueHold::DescendantsActive { targets: vec![target("c", AgentState::Running)] });
        assert_eq!(descendants_from_gate(&active, &r), DescendantReport { working: 1, unknown: 0, scan_complete: true });
        let unknown = GateDecision::Hold(QueueHold::StateUnknown { targets: vec![target("c", AgentState::Unknown)], descendant_scan_incomplete: true });
        assert!(!descendants_from_gate(&unknown, &r).scan_complete);
        let parent_not_live = GateDecision::Hold(QueueHold::NotLive { targets: vec![target("r", AgentState::Idle)] });
        assert!(!descendants_from_gate(&parent_not_live, &r).scan_complete);
        let child_not_live = GateDecision::Hold(QueueHold::NotLive { targets: vec![target("c", AgentState::Idle)] });
        assert_eq!(descendants_from_gate(&child_not_live, &r), DescendantReport { working: 0, unknown: 1, scan_complete: true });
        assert!(!descendants_from_gate(&GateDecision::Hold(QueueHold::Disconnected), &r).scan_complete);
        assert!(!descendants_from_gate(&GateDecision::Nothing, &r).scan_complete);
    }
}
