//! 保存した記録（[`StoredChat`]）→ 共通データモデル（Chat・Agent・AgentStatus・TurnRecord 等）の純粋な変換。
//!
//! 規則: agy の型を UI・保存形式に出さない。結果不明（`TurnStatus::Unknown`）は終端にせず、未取得・欠落は
//! `Known` で表す（0・空文字・Done で代用しない）。

use super::store::{StoredChat, StoredTurn, TurnStatus};
use super::stream::denied_note;
use crate::backend::backend::{AgentHistory, ChatSummary};
use crate::backend::model::*;

const PREVIEW_MAX_CHARS: usize = 80;

pub fn chat_key(id: &str) -> ChatKey {
    ChatKey { backend: BackendKind::Antigravity, id: ExternalId(id.to_string()) }
}

pub fn agent_key(id: &str) -> AgentKey {
    AgentKey { backend: BackendKind::Antigravity, id: ExternalId(id.to_string()) }
}

pub fn turn_key(chat_id: &str, turn_id: &str) -> TurnKey {
    TurnKey { agent: agent_key(chat_id), turn_id: ExternalId(turn_id.to_string()) }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

/// 最初の依頼の先頭（一覧の仮表示。確認済みの名前ではない）。
pub fn preview(c: &StoredChat) -> Known<String> {
    match c.turns.first().map(|t| t.user_text.trim()).filter(|t| !t.is_empty()) {
        Some(t) => Known::direct(truncate(t, PREVIEW_MAX_CHARS)),
        None => Known::Missing,
    }
}

pub fn to_chat(c: &StoredChat) -> Chat {
    Chat {
        key: chat_key(&c.id),
        kind: c.kind,
        cwd: Known::direct(c.cwd.clone()),
        name: match c.name.as_deref().filter(|n| !n.is_empty()) {
            Some(n) => Known::direct(n.to_string()),
            None => Known::Missing,
        },
        preview: preview(c),
        pinned: false,
        archived: Known::direct(c.archived),
        origin: ChatOrigin::AppManaged,
        draft: None,
        created_at: Known::direct(UnixMillis(c.created_at)),
        last_used_at: c.last_used_at.map(UnixMillis),
        // 発話前のチャット（記録だけがある）。
        no_history: c.turns.is_empty(),
        project_id: None,
    }
}

pub fn to_agent(c: &StoredChat) -> Agent {
    Agent {
        key: agent_key(&c.id),
        chat: chat_key(&c.id),
        parent: ParentLink::Root,
        // AgentDock が作った会話で、分岐は非対応（分岐元なしと導出できる）。
        forked_from: Known::Value { value: None, basis: Basis::Derived },
        display_name: match c.name.as_deref().filter(|n| !n.is_empty()) {
            Some(n) => Known::direct(n.to_string()),
            None => Known::Missing,
        },
        role: Known::NotFetched,
        assignment: Known::NotFetched,
        agent_path: Known::NotFetched,
        model: Known::NotFetched,
        effort: Known::NotFetched,
        spawn_spec: Known::NotFetched,
        latest_turn: c.turns.last().map(|t| ExternalId(t.turn_id.clone())),
    }
}

fn evidence(source: EvidenceSource, label: &str, now: UnixMillis) -> Evidence {
    Evidence { source, raw_label: Some(label.to_string()), source_time: None, observed_at: now }
}

/// チャットの状態。実行中の turn があれば Running、結果不明の turn が最後なら Unknown、他は Idle
/// （プロセスは turn ごとに終わるので、常駐の「読込み済み」はない）。
pub fn agent_status(c: &StoredChat, source: EvidenceSource, label: &str, now: UnixMillis) -> AgentStatus {
    let ev = evidence(source, label, now);
    let (state, raw, turn) = match c.turns.last() {
        Some(t) if t.status == TurnStatus::Running => (AgentState::Running, "active", Some(ExternalId(t.turn_id.clone()))),
        Some(t) if t.status == TurnStatus::Unknown => (AgentState::Unknown, "turn:outcomeUnknown", Some(ExternalId(t.turn_id.clone()))),
        _ => (AgentState::Idle, "idle", None),
    };
    AgentStatus { state, raw: RawState { label: raw.to_string() }, scope: StateScope::Agent, turn, wait: None, evidence: ev }
}

fn turn_end(s: TurnStatus) -> Option<TurnEnd> {
    match s {
        TurnStatus::Completed => Some(TurnEnd::Completed),
        TurnStatus::Failed => Some(TurnEnd::Failed),
        TurnStatus::Interrupted => Some(TurnEnd::Interrupted),
        TurnStatus::Running | TurnStatus::Unknown => None,
    }
}

pub fn to_record(chat_id: &str, t: &StoredTurn) -> TurnRecord {
    let agent = agent_key(chat_id);
    let end = turn_end(t.status);
    let entry = |item: &str, kind: ActivityKind, text: String, phase: ActivityPhase| TranscriptEntry {
        key: ItemKey { agent: agent.clone(), turn_id: Some(ExternalId(t.turn_id.clone())), item_id: ExternalId(format!("{}-{item}", t.turn_id)) },
        kind,
        text: Known::direct(text),
        phase,
        message_phase: MessagePhase::Unknown,
        attachments: Vec::new(),
    };
    let mut entries = vec![entry("user", ActivityKind::UserMessage, t.user_text.clone(), ActivityPhase::Completed)];
    if !t.response_text.is_empty() {
        let phase = match t.status {
            TurnStatus::Running => ActivityPhase::InProgress,
            TurnStatus::Unknown => ActivityPhase::Unconfirmed,
            _ => ActivityPhase::Completed,
        };
        entries.push(entry("response", ActivityKind::AgentMessage, t.response_text.clone(), phase));
    }
    // 成功を偽らないため、自動拒否した操作・失敗の理由も履歴に残す。
    if !t.denied_actions.is_empty() {
        entries.push(entry(
            "denied",
            ActivityKind::Other { raw: "antigravity.denied_actions".into() },
            format!("承認が必要な操作を自動拒否しました: {}", denied_note(&t.denied_actions)),
            ActivityPhase::Completed,
        ));
    }
    if let Some(e) = t.error.as_deref().filter(|e| !e.is_empty()) {
        entries.push(entry("error", ActivityKind::Other { raw: "antigravity.error".into() }, e.to_string(), ActivityPhase::Completed));
    }
    TurnRecord {
        key: turn_key(chat_id, &t.turn_id),
        end,
        started_at: Known::direct(UnixMillis(t.started_at)),
        completed_at: match (end, t.ended_at) {
            (Some(_), Some(at)) => Known::direct(UnixMillis(at)),
            (Some(_), None) => Known::Missing,
            // 実行中は未取得。結果不明は終わりを観測していない（欠落）。
            (None, _) if t.status == TurnStatus::Running => Known::NotFetched,
            (None, _) => Known::Missing,
        },
        entries,
        complete: end.is_some(),
        // 失敗の区分（利用上限等）は agy が示さないので、欠落。
        failure: if t.status == TurnStatus::Failed { Known::Missing } else { Known::NotFetched },
    }
}

pub fn to_history(c: &StoredChat, include_turns: bool, source: EvidenceSource, label: &str, now: UnixMillis) -> AgentHistory {
    AgentHistory {
        agent: to_agent(c),
        chat: Some(to_chat(c)),
        status: agent_status(c, source, label, now),
        turns: if include_turns { c.turns.iter().map(|t| to_record(&c.id, t)).collect() } else { Vec::new() },
        spawns: Vec::new(),
        spawn_supplements: Vec::new(),
    }
}

pub fn to_summary(c: &StoredChat, now: UnixMillis) -> ChatSummary {
    ChatSummary { chat: to_chat(c), root: to_agent(c), status: agent_status(c, EvidenceSource::StatusQuery, "antigravity.store", now), preview: preview(c) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::antigravity::store::DeniedAction;
    use crate::backend::backend::PermissionPreset;

    fn chat() -> StoredChat {
        StoredChat {
            id: "agy-1".into(),
            conversation_id: None,
            cwd: "C:\\w".into(),
            extra_folders: vec![],
            model: Some("gemini-x".into()),
            permission: PermissionPreset::ReadOnly,
            kind: ChatKind::Development,
            name: None,
            created_at: 5,
            last_used_at: None,
            archived: false,
            turns: vec![],
            ephemeral: false,
        }
    }

    fn turn(id: &str, status: TurnStatus) -> StoredTurn {
        StoredTurn {
            turn_id: id.into(),
            client_message_id: "cm".into(),
            user_text: "依頼".into(),
            response_text: "返答".into(),
            status,
            started_at: 7,
            ended_at: Some(9),
            denied_actions: vec![],
            error: None,
            usage: None,
            process: None,
        }
    }

    #[test]
    fn empty_chat_has_no_history_and_missing_name() {
        let c = chat();
        let v = to_chat(&c);
        assert!(v.no_history);
        assert_eq!((v.name.clone(), v.preview.clone()), (Known::Missing, Known::Missing));
        assert_eq!(v.key.backend, BackendKind::Antigravity);
        assert_eq!(agent_status(&c, EvidenceSource::StatusQuery, "x", UnixMillis(0)).state, AgentState::Idle);
    }

    #[test]
    fn unknown_outcome_is_not_a_terminal_and_not_complete() {
        let r = to_record("agy-1", &turn("agy-1-t1", TurnStatus::Unknown));
        assert_eq!(r.end, None);
        assert!(!r.complete);
        assert_eq!(r.completed_at, Known::Missing);
        assert_eq!(r.entries[1].phase, ActivityPhase::Unconfirmed);
        let mut c = chat();
        c.turns.push(turn("agy-1-t1", TurnStatus::Unknown));
        assert_eq!(agent_status(&c, EvidenceSource::HistoryRead, "x", UnixMillis(0)).state, AgentState::Unknown);
    }

    #[test]
    fn terminal_turns_map_to_records_and_failure_class_is_not_invented() {
        let ok = to_record("agy-1", &turn("t", TurnStatus::Completed));
        assert_eq!((ok.end, ok.complete, ok.failure.clone()), (Some(TurnEnd::Completed), true, Known::NotFetched));
        assert_eq!(ok.completed_at, Known::direct(UnixMillis(9)));
        let bad = to_record("agy-1", &turn("t", TurnStatus::Failed));
        assert_eq!((bad.end, bad.failure), (Some(TurnEnd::Failed), Known::Missing));
        assert_eq!(to_record("agy-1", &turn("t", TurnStatus::Interrupted)).end, Some(TurnEnd::Interrupted));
        let running = to_record("agy-1", &turn("t", TurnStatus::Running));
        assert_eq!((running.end, running.complete, running.completed_at), (None, false, Known::NotFetched));
    }

    #[test]
    fn transcript_keeps_user_response_denials_and_errors() {
        let mut t = turn("t", TurnStatus::Completed);
        t.denied_actions = vec![DeniedAction { action: Some("command".into()), display_name: Some("Bash".into()) }];
        t.error = Some("boom".into());
        let r = to_record("agy-1", &t);
        let texts: Vec<_> = r.entries.iter().map(|e| e.text.value().cloned().unwrap()).collect();
        assert_eq!(texts, ["依頼", "返答", "承認が必要な操作を自動拒否しました: Bash (command)", "boom"]);
        assert_eq!(r.entries[0].kind, ActivityKind::UserMessage);
        assert_eq!(r.entries[1].kind, ActivityKind::AgentMessage);
        // 返答が空ならエージェント発言の項目を作らない。
        t.response_text.clear();
        assert_eq!(to_record("agy-1", &t).entries.len(), 3);
    }

    #[test]
    fn running_turn_shows_running_status_with_its_turn_id() {
        let mut c = chat();
        c.turns.push(turn("agy-1-t1", TurnStatus::Running));
        let s = agent_status(&c, EvidenceSource::StatusQuery, "x", UnixMillis(0));
        assert_eq!((s.state, s.turn), (AgentState::Running, Some(ExternalId("agy-1-t1".into()))));
        assert_eq!(to_agent(&c).latest_turn, Some(ExternalId("agy-1-t1".into())));
        assert_eq!(to_chat(&c).preview, Known::direct("依頼".to_string()));
    }
}
