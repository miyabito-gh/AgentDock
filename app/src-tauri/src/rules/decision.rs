//! 応答記録の組立て（項目51）。純粋ロジック。時刻・IDは引数で渡す。
//!
//! - 記録するのは「ユーザーが押した結果」と、応答後の受理未確認・別の場所での解決だけ。期限切れ（切断）・取消は記録しない。
//! - 要約は短く切る（コマンド全文・長いパス・URLを残さない）。質問の答えの本文と秘密入力は保存しない。

use crate::backend::decision::{DecisionKind, DecisionOutcome, DecisionRecord};
use crate::backend::model::*;

/// 要約の上限文字数。
pub const SUMMARY_MAX_CHARS: usize = 80;
/// 選択肢の表示名の上限文字数。
const LABEL_MAX_CHARS: usize = 40;

/// 空白（改行を含む）を1つにまとめ、`max` 文字を超えたら末尾を「…」にする。
pub fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut s: String = flat.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

fn kind_of(k: &RequestKind) -> DecisionKind {
    match k {
        RequestKind::CommandApproval => DecisionKind::Command,
        RequestKind::FileChangeApproval => DecisionKind::FileChange,
        RequestKind::PermissionsApproval => DecisionKind::Permissions,
        RequestKind::UserInput => DecisionKind::Question,
        RequestKind::ToolElicitation => DecisionKind::ToolPermission,
        RequestKind::Other { .. } => DecisionKind::Other,
    }
}

/// 内容の短い要約。取得できていない項目は推測せず、要求の概要文（`summary`）に戻る。
pub fn summary_of(r: &PendingRequest) -> String {
    let d = &r.detail;
    let text = match &r.kind {
        RequestKind::CommandApproval => d.command.value().cloned().unwrap_or_else(|| d.summary.clone()),
        RequestKind::FileChangeApproval => match d.files.value() {
            Some(f) if !f.is_empty() => {
                let first = f[0].clone();
                if f.len() > 1 { format!("{first} ほか{}件", f.len() - 1) } else { first }
            }
            _ => d.summary.clone(),
        },
        RequestKind::PermissionsApproval => d.extra_permissions.value().cloned().unwrap_or_else(|| d.summary.clone()),
        RequestKind::UserInput => match d.questions.first() {
            Some(q) if q.secret => "質問（秘密入力のため内容は記録しません）".to_string(),
            Some(q) => q.text.clone(),
            None => d.summary.clone(),
        },
        RequestKind::ToolElicitation => match d.server.value() {
            Some(s) => format!("{s}: {}", d.summary),
            None => d.summary.clone(),
        },
        RequestKind::Other { .. } => d.summary.clone(),
    };
    clip(&text, SUMMARY_MAX_CHARS)
}

/// 要求の新しい状態から記録を作る。記録の対象でない状態（保留・期限切れ・取消）は `None`。
/// `agent_name` は記録時点の表示名（取得できたときだけ）。
pub fn build_record(r: &PendingRequest, new_state: &RequestState, id: LocalId, agent_name: Option<String>) -> Option<DecisionRecord> {
    let (outcome, at) = match new_state {
        RequestState::Answered { option_id, at } => (answered_outcome(r, option_id.as_deref()), *at),
        RequestState::AnswerUnconfirmed { at } => (DecisionOutcome::Unconfirmed, *at),
        RequestState::ResolvedElsewhere { at } => (DecisionOutcome::ResolvedElsewhere, *at),
        RequestState::Pending | RequestState::Cancelled { .. } | RequestState::Expired { .. } | RequestState::Unanswerable { .. } => return None,
    };
    Some(DecisionRecord {
        id,
        chat: r.chat.clone(),
        agent: r.agent.clone(),
        agent_name,
        turn: r.turn.clone(),
        kind: kind_of(&r.kind),
        summary: summary_of(r),
        outcome,
        requested_at: r.received_at,
        at,
    })
}

fn answered_outcome(r: &PendingRequest, option_id: Option<&str>) -> DecisionOutcome {
    if r.kind == RequestKind::UserInput {
        return DecisionOutcome::Answered;
    }
    let Some(opt) = option_id.and_then(|id| r.options.iter().find(|o| o.id == id)) else {
        // 選択肢を特定できない応答は、意味を決めつけない。
        return DecisionOutcome::Chosen { label: "（選択肢を特定できません）".into() };
    };
    match opt.effect {
        DecisionEffect::Allow => DecisionOutcome::Allowed { scope: opt.scope },
        DecisionEffect::Deny => DecisionOutcome::Declined,
        DecisionEffect::Cancel => DecisionOutcome::Cancelled,
        DecisionEffect::Other => DecisionOutcome::Chosen { label: clip(&opt.label, LABEL_MAX_CHARS) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ek(s: &str) -> ExternalId {
        ExternalId(s.into())
    }

    fn opt(id: &str, effect: DecisionEffect, scope: DecisionScope) -> DecisionOption {
        DecisionOption { id: id.into(), label: id.into(), effect, scope, description: None }
    }

    fn req(kind: RequestKind, command: Option<&str>) -> PendingRequest {
        PendingRequest {
            key: RequestKey { backend: BackendKind::Codex, source: SourceId("s".into()), request_id: ek("r1") },
            chat: ChatKey { backend: BackendKind::Codex, id: ek("c") },
            agent: AgentKey { backend: BackendKind::Codex, id: ek("child") },
            turn: Some(ek("t1")),
            item: None,
            kind,
            detail: RequestDetail {
                summary: "概要".into(),
                reason: Known::NotFetched,
                command: command.map(|c| Known::direct(c.to_string())).unwrap_or(Known::NotFetched),
                cwd: Known::NotFetched,
                files: Known::NotFetched,
                extra_permissions: Known::NotFetched,
                url: Known::NotFetched,
                server: Known::NotFetched,
                questions: vec![],
            },
            options: vec![
                opt("once", DecisionEffect::Allow, DecisionScope::Once),
                opt("no", DecisionEffect::Deny, DecisionScope::Once),
                opt("stop", DecisionEffect::Cancel, DecisionScope::Once),
            ],
            state: RequestState::Pending,
            received_at: UnixMillis(10),
        }
    }

    fn rec(r: &PendingRequest, s: RequestState) -> Option<DecisionRecord> {
        build_record(r, &s, LocalId("d1".into()), Some("Kant".into()))
    }

    #[test]
    fn clip_flattens_whitespace_and_cuts_by_chars() {
        assert_eq!(clip("a\n  b\tc", 10), "a b c");
        let long = "あ".repeat(100);
        let c = clip(&long, 80);
        assert_eq!(c.chars().count(), 80);
        assert!(c.ends_with('…'));
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn allow_deny_cancel_map_from_the_option_effect_and_keep_the_scope() {
        let r = req(RequestKind::CommandApproval, Some("git status"));
        let a = rec(&r, RequestState::Answered { option_id: Some("once".into()), at: UnixMillis(20) }).unwrap();
        assert_eq!(a.outcome, DecisionOutcome::Allowed { scope: DecisionScope::Once });
        assert_eq!((a.kind, a.summary.as_str(), a.at, a.requested_at), (DecisionKind::Command, "git status", UnixMillis(20), UnixMillis(10)));
        assert_eq!(a.agent_name.as_deref(), Some("Kant"));
        let d = rec(&r, RequestState::Answered { option_id: Some("no".into()), at: UnixMillis(20) }).unwrap();
        assert_eq!(d.outcome, DecisionOutcome::Declined);
        let c = rec(&r, RequestState::Answered { option_id: Some("stop".into()), at: UnixMillis(20) }).unwrap();
        assert_eq!(c.outcome, DecisionOutcome::Cancelled);
    }

    #[test]
    fn unknown_option_is_not_guessed() {
        let r = req(RequestKind::CommandApproval, Some("x"));
        let a = rec(&r, RequestState::Answered { option_id: Some("zzz".into()), at: UnixMillis(20) }).unwrap();
        assert!(matches!(a.outcome, DecisionOutcome::Chosen { .. }));
    }

    #[test]
    fn question_answers_are_not_stored_and_secret_questions_hide_the_text() {
        let mut r = req(RequestKind::UserInput, None);
        r.detail.questions = vec![Question { id: "q".into(), header: None, text: "パスワードは?".into(), options: vec![], allows_free_text: true, secret: true }];
        let a = rec(&r, RequestState::Answered { option_id: None, at: UnixMillis(20) }).unwrap();
        assert_eq!(a.outcome, DecisionOutcome::Answered);
        assert!(!a.summary.contains("パスワード"));
    }

    #[test]
    fn long_command_is_clipped() {
        let long = format!("curl https://example.com/{}", "x".repeat(300));
        let r = req(RequestKind::CommandApproval, Some(&long));
        assert_eq!(summary_of(&r).chars().count(), SUMMARY_MAX_CHARS);
    }

    #[test]
    fn unconfirmed_and_resolved_elsewhere_are_recorded_but_pending_expired_cancelled_are_not() {
        let r = req(RequestKind::CommandApproval, Some("x"));
        assert_eq!(rec(&r, RequestState::AnswerUnconfirmed { at: UnixMillis(3) }).unwrap().outcome, DecisionOutcome::Unconfirmed);
        assert_eq!(rec(&r, RequestState::ResolvedElsewhere { at: UnixMillis(3) }).unwrap().outcome, DecisionOutcome::ResolvedElsewhere);
        assert!(rec(&r, RequestState::Pending).is_none());
        assert!(rec(&r, RequestState::Expired { at: UnixMillis(3) }).is_none());
        assert!(rec(&r, RequestState::Cancelled { at: UnixMillis(3) }).is_none());
    }

    #[test]
    fn missing_detail_falls_back_to_the_request_summary() {
        let r = req(RequestKind::CommandApproval, None);
        assert_eq!(summary_of(&r), "概要");
    }
}
