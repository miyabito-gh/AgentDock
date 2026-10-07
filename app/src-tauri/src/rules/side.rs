//! side相談の純粋ロジック（段階③ P3-4、`app/DESIGN_P3.md` §1 #10）。I/Oをしない。
//!
//! - sideは一時的な分岐（ephemeral）。一覧に出さず、キューの自動送信の条件（親・子孫の状態）に入れない。
//!   ただし停止対象には入れる（完全終了・強制終了の確認、主会話の削除の停止照合）。
//! - 引渡しは、選んだ発言を引用ブロックにして文字列で返すだけ。入力欄への挿入と送信はUIとユーザーが行う。
//! - 再起動・切断で一時の分岐は消える。「開いている」記録は「終了（再開不可）」に変える（状態を完了扱いにはしない）。

use std::collections::HashMap;

use super::compose::fenced;
use crate::backend::model::*;

/// 主会話の停止の範囲（主会話の会話と、主会話に属するsideの会話）。停止対象の列挙は、この範囲で行う。
/// `sides` は side の会話 → 主会話。
pub fn stop_scope(main: &ChatKey, sides: &HashMap<ChatKey, ChatKey>) -> Vec<ChatKey> {
    let mut out = vec![main.clone()];
    let mut attached: Vec<&ChatKey> = sides.iter().filter(|(_, m)| *m == main).map(|(t, _)| t).collect();
    attached.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    out.extend(attached.into_iter().cloned());
    out
}

/// 履歴の最後の終端したturn（side相談の分岐点）。進行中・終端未確認のturnは分岐点にしない。
pub fn last_terminal_turn(turns: &[TurnRecord]) -> Option<ExternalId> {
    turns.iter().rev().find(|t| t.end.is_some()).map(|t| t.key.turn_id.clone())
}

/// 起動時: 開いたままの記録を「終了（再開不可）」にする。変更があれば true。
pub fn end_open_after_restart(sessions: &mut [SideSessionMeta]) -> bool {
    let mut changed = false;
    for s in sessions {
        if s.state == SideState::Open {
            s.state = SideState::Ended { reason: "アプリの再起動で終了しました（一時的な相談は再開できません）".into() };
            changed = true;
        }
    }
    changed
}

/// 送れる状態か。理由つきで断る。実行中の送信は追加指示にしない（新しいturnを始める側の入口だけ）。
pub fn check_sendable(state: &SideState, running: bool) -> Result<(), &'static str> {
    if let SideState::Ended { .. } = state {
        return Err("この相談は終了しています（一時的な相談は再開できません）");
    }
    if running {
        return Err("実行中です。完了を待つか、中断してから送ってください");
    }
    Ok(())
}

/// エージェント発言の確定本文。逐次本文を受け取れていればそれ（全文）、なければ短い要約（短縮されている可能性あり）。
/// どちらもなければ None（空の発言を作らない）。戻り値の bool は「短縮されている可能性」。
pub fn finalize_agent_text(buffer: Option<&str>, summary: &Known<String>) -> Option<(String, bool)> {
    if let Some(b) = buffer.filter(|b| !b.is_empty()) {
        return Some((b.to_string(), false));
    }
    summary.value().filter(|s| !s.is_empty()).map(|s| (s.clone(), true))
}

fn role_label(r: SideRole) -> &'static str {
    match r {
        SideRole::User => "自分（sideへの質問）",
        SideRole::Agent => "sideのエージェント",
    }
}

/// 選んだ発言（添字。渡された順）を主会話へ渡す引用ブロックにする。範囲外の添字・空の選択は断る。原文のまま・要約しない。
pub fn handoff_text(main_title: &str, entries: &[SideEntry], picks: &[u32]) -> Result<String, String> {
    if picks.is_empty() {
        return Err("渡す発言を選んでください".into());
    }
    let mut parts: Vec<String> = Vec::new();
    let mut truncated = false;
    for &i in picks {
        let Some(e) = entries.get(i as usize) else { return Err("選んだ発言が見つかりません".into()) };
        truncated |= e.truncated;
        parts.push(format!("{}:\n{}", role_label(e.role), e.text));
    }
    let title = main_title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut head = format!("> side相談から引き渡し（会話「{title}」とは別の一時的な相談の発言。原文のままで、要約・加工はしていません）。");
    if truncated {
        head.push_str("\n> 注意: 短縮されている可能性のある発言を含みます。");
    }
    Ok(format!("{head}\n{}\n", fenced(&parts.join("\n\n"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }

    fn turn(id: &str, end: Option<TurnEnd>) -> TurnRecord {
        TurnRecord {
            key: TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("m".into()) }, turn_id: ExternalId(id.into()) },
            end,
            started_at: Known::NotFetched,
            completed_at: Known::NotFetched,
            entries: Vec::new(),
            complete: true,
        }
    }

    fn side(id: &str, state: SideState) -> SideSessionMeta {
        SideSessionMeta { id: LocalId(id.into()), main: ck("m"), thread: ck(&format!("t-{id}")), state, opened_at: None }
    }

    fn e(role: SideRole, text: &str, truncated: bool) -> SideEntry {
        SideEntry { role, text: text.into(), at: UnixMillis(0), truncated }
    }

    #[test]
    fn stop_scope_includes_only_the_main_chat_and_its_own_sides() {
        let sides: HashMap<ChatKey, ChatKey> = [(ck("s1"), ck("m")), (ck("s2"), ck("other")), (ck("s3"), ck("m"))].into_iter().collect();
        assert_eq!(stop_scope(&ck("m"), &sides), vec![ck("m"), ck("s1"), ck("s3")]);
        assert_eq!(stop_scope(&ck("other"), &sides), vec![ck("other"), ck("s2")]);
        assert_eq!(stop_scope(&ck("none"), &sides), vec![ck("none")]);
        assert_eq!(stop_scope(&ck("m"), &HashMap::new()), vec![ck("m")]);
    }

    #[test]
    fn fork_point_is_the_last_terminal_turn_never_a_running_one() {
        let turns = vec![turn("t1", Some(TurnEnd::Completed)), turn("t2", Some(TurnEnd::Interrupted)), turn("t3", None)];
        assert_eq!(last_terminal_turn(&turns), Some(ExternalId("t2".into())));
        assert_eq!(last_terminal_turn(&[turn("t3", None)]), None);
        assert_eq!(last_terminal_turn(&[]), None);
    }

    #[test]
    fn open_sessions_end_after_restart_without_touching_ended_ones() {
        let mut v = vec![side("a", SideState::Open), side("b", SideState::Ended { reason: "closed".into() })];
        assert!(end_open_after_restart(&mut v));
        assert!(matches!(&v[0].state, SideState::Ended { reason } if reason.contains("再起動")));
        assert_eq!(v[1].state, SideState::Ended { reason: "closed".into() });
        assert!(!end_open_after_restart(&mut v), "nothing left to change");
    }

    #[test]
    fn sending_is_refused_when_ended_or_running() {
        assert!(check_sendable(&SideState::Open, false).is_ok());
        assert!(check_sendable(&SideState::Open, true).is_err());
        assert!(check_sendable(&SideState::Ended { reason: "x".into() }, false).is_err());
    }

    #[test]
    fn agent_text_prefers_the_streamed_full_text_and_flags_summary_only() {
        let sum = Known::direct("short…".to_string());
        assert_eq!(finalize_agent_text(Some("full text"), &sum), Some(("full text".into(), false)));
        assert_eq!(finalize_agent_text(None, &sum), Some(("short…".into(), true)));
        assert_eq!(finalize_agent_text(Some(""), &sum), Some(("short…".into(), true)));
        assert_eq!(finalize_agent_text(None, &Known::Missing), None, "no empty speech is recorded");
    }

    #[test]
    fn handoff_quotes_selected_entries_verbatim_and_survives_fences() {
        let entries = vec![e(SideRole::User, "質問", false), e(SideRole::Agent, "答え:\n```\ncode\n```", true), e(SideRole::Agent, "UNSELECTED", false)];
        let t = handoff_text("主会話", &entries, &[0, 1]).unwrap();
        assert!(t.contains("自分（sideへの質問）:\n質問"));
        assert!(t.contains("答え:\n```\ncode\n```"), "original text is kept");
        assert!(!t.contains("UNSELECTED"), "unselected entries are not included");
        assert!(t.contains("短縮されている可能性"), "truncated entries are flagged");
        assert!(t.lines().any(|l| l == "````text"), "the outer fence is longer than the inner one");
        assert!(handoff_text("m", &entries, &[0]).unwrap().find("短縮").is_none());
    }

    #[test]
    fn handoff_refuses_empty_or_out_of_range_selection() {
        let entries = vec![e(SideRole::User, "q", false)];
        assert!(handoff_text("m", &entries, &[]).is_err());
        assert!(handoff_text("m", &entries, &[1]).is_err());
    }
}
