//! 観測 → エンジンへの入力の変換（DESIGN_ORG_P4 §3.6）。純粋な変換で、待たない・送信しない・承認に答えない。
//!
//! `OrgObserver` は、組織が所有するチャット（`chat.json.org` があり未解放のもの）の対応表を持つ。
//! ホストの監視経路（イベント処理・送信結果・中断）は、結果の `DriverMsg` を司令task（`driver.rs`）のチャネルへ流すだけ。
//! 承認・質問は件数を数えて知らせるだけで、回答には関わらない。

use std::collections::{HashMap, HashSet};

use org_engine::{AttemptRef, OrgInput, ReconcileCause, SettleOutcome, TaskId, TurnEndKind, WorkflowId};

use super::driver::{DriverMsg, UsageSample};
use crate::backend::backend::{BackendEvent, FailureClass, GapScope};
use crate::backend::model::{AgentKey, ChatKey, ConnectionState, ExternalId, LocalId, SendAttempt, SendState, TurnEnd, TurnKey};
use crate::rules::org::{chat_ref_of, turn_ref_of};

#[derive(Default)]
pub struct OrgObserver {
    bound: HashMap<ChatKey, (WorkflowId, TaskId)>,
    /// 受理不明を返した試行。のちに受理が確定したら「照合で受理」として知らせる。
    unknown_attempts: HashSet<LocalId>,
    /// 利用上限で失敗したturn（`TurnEnded` より前に届く分類を覚えておく）。
    usage_limited: HashSet<TurnKey>,
}

impl OrgObserver {
    pub fn bind(&mut self, chat: ChatKey, workflow: WorkflowId, task: TaskId) {
        self.bound.insert(chat, (workflow, task));
    }

    pub fn unbind(&mut self, chat: &ChatKey) {
        self.bound.remove(chat);
    }

    pub fn clear(&mut self) {
        self.bound.clear();
        self.unknown_attempts.clear();
        self.usage_limited.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }

    pub fn contains(&self, chat: &ChatKey) -> bool {
        self.bound.contains_key(chat)
    }

    pub fn task_of(&self, chat: &ChatKey) -> Option<&TaskId> {
        self.bound.get(chat).map(|(_, t)| t)
    }

    /// 送信状態の更新を、エンジンへの `SendSettled` にする。`Sending`（進行中）は知らせない。
    /// 照合（読取りのみ）の結果も同じ口で届く。
    pub fn on_send(&mut self, chat: &ChatKey, attempt: &SendAttempt) -> Vec<DriverMsg> {
        let Some(task) = self.task_of(chat).cloned() else { return Vec::new() };
        let aref = AttemptRef::new(attempt.attempt_id.0.clone());
        let was_unknown = self.unknown_attempts.contains(&attempt.attempt_id);
        let outcome = match &attempt.state {
            SendState::Sending => return Vec::new(),
            SendState::Accepted { turn } => {
                let t = turn_ref_of(&turn.turn_id);
                self.unknown_attempts.remove(&attempt.attempt_id);
                if was_unknown {
                    SettleOutcome::ReconciledAccepted { turn: t }
                } else {
                    SettleOutcome::Accepted { turn: t }
                }
            }
            SendState::Rejected { .. } => {
                self.unknown_attempts.remove(&attempt.attempt_id);
                SettleOutcome::Rejected
            }
            SendState::AcceptanceUnknown { .. } => {
                self.unknown_attempts.insert(attempt.attempt_id.clone());
                SettleOutcome::Unknown
            }
            SendState::NotFoundAfterReconcile => {
                self.unknown_attempts.remove(&attempt.attempt_id);
                SettleOutcome::ReconciledNotFound
            }
        };
        vec![DriverMsg::Input(OrgInput::SendSettled { task, attempt: aref, outcome })]
    }

    /// 利用者の中断の操作を知らせる（中断そのものはホストの既存の経路が行う）。
    pub fn on_interrupt(&self, chat: &ChatKey) -> Vec<DriverMsg> {
        if self.contains(chat) {
            vec![DriverMsg::Input(OrgInput::InterruptRequested { chat: chat_ref_of(chat) })]
        } else {
            Vec::new()
        }
    }

    /// バックエンドのイベントを入力にする。`open_requests` は、そのチャットで回答待ちの承認・質問の件数を現在の状態から数える。
    /// `chat_of_agent` はエージェント（子孫を含む）が属するチャット。
    pub fn on_event(
        &mut self,
        event: &BackendEvent,
        open_requests: &dyn Fn(&ChatKey) -> u32,
        chat_of_agent: &dyn Fn(&AgentKey) -> Option<ChatKey>,
    ) -> Vec<DriverMsg> {
        match event {
            BackendEvent::TurnFailureClass { turn, class: FailureClass::UsageLimit } => {
                if self.is_root_of_bound(&turn.agent) {
                    self.usage_limited.insert(turn.clone());
                }
                Vec::new()
            }
            BackendEvent::TurnEnded { turn, end, .. } => {
                let limited = self.usage_limited.remove(turn);
                let Some(chat) = self.root_chat(&turn.agent) else { return Vec::new() };
                let end = match end {
                    TurnEnd::Completed => TurnEndKind::Completed,
                    TurnEnd::Failed => TurnEndKind::Failed { usage_limit: limited },
                    TurnEnd::Interrupted => TurnEndKind::Interrupted,
                };
                vec![DriverMsg::Input(OrgInput::TurnEnded { chat: chat_ref_of(&chat), turn: turn_ref_of(&turn.turn_id), end })]
            }
            BackendEvent::RequestOpened { request } => self.pending_count(&request.chat, open_requests),
            BackendEvent::RequestResolved { .. } => {
                // 解決された要求がどのチャットのものかはイベントに無いので、組織のチャットすべての件数を数え直す。
                let chats: Vec<ChatKey> = self.bound.keys().cloned().collect();
                chats.iter().flat_map(|c| self.pending_count(c, open_requests)).collect()
            }
            BackendEvent::Connection { state: ConnectionState::Disconnected { .. } } => {
                vec![DriverMsg::Input(OrgInput::Freshness { cause: ReconcileCause::Disconnect })]
            }
            BackendEvent::Gap { scope, .. } => match scope {
                GapScope::Source => vec![DriverMsg::Input(OrgInput::Freshness { cause: ReconcileCause::Disconnect })],
                GapScope::Agent { agent } if self.is_root_of_bound(agent) => {
                    vec![DriverMsg::Input(OrgInput::Freshness { cause: ReconcileCause::Disconnect })]
                }
                GapScope::Agent { .. } => Vec::new(),
            },
            BackendEvent::TokenUsage { agent, turn, last_total, thread_total, context_window } => {
                let Some(chat) = self.root_chat(agent) else { return Vec::new() };
                vec![DriverMsg::Usage(UsageSample {
                    chat: chat_ref_of(&chat),
                    turn: turn.as_ref().map(turn_ref_of),
                    last_total_tokens: *last_total,
                    thread_total_tokens: *thread_total,
                    model_context_window: *context_window,
                })]
            }
            BackendEvent::SandboxFailureObserved { agent } => {
                let chat = self.root_chat(agent).or_else(|| chat_of_agent(agent).filter(|c| self.contains(c)));
                match chat {
                    Some(c) => vec![DriverMsg::Input(OrgInput::SandboxFailure { chat: chat_ref_of(&c) })],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn pending_count(&self, chat: &ChatKey, open_requests: &dyn Fn(&ChatKey) -> u32) -> Vec<DriverMsg> {
        if !self.contains(chat) {
            return Vec::new();
        }
        vec![DriverMsg::Input(OrgInput::RequestPending { chat: chat_ref_of(chat), open: open_requests(chat) })]
    }

    fn root_chat(&self, agent: &AgentKey) -> Option<ChatKey> {
        let chat = ChatKey { backend: agent.backend, id: ExternalId(agent.id.0.clone()) };
        self.contains(&chat).then_some(chat)
    }

    fn is_root_of_bound(&self, agent: &AgentKey) -> bool {
        self.root_chat(agent).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{BackendKind, Evidence, EvidenceSource, UnixMillis};

    fn chat(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn agent(id: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn turn(a: &str, t: &str) -> TurnKey {
        TurnKey { agent: agent(a), turn_id: ExternalId(t.into()) }
    }
    fn ev() -> Evidence {
        Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(0) }
    }
    fn observer() -> OrgObserver {
        let mut o = OrgObserver::default();
        o.bind(chat("c1"), WorkflowId::new("w"), TaskId::new("t1"));
        o
    }
    fn attempt(state: SendState) -> SendAttempt {
        SendAttempt { attempt_id: LocalId("att-1".into()), client_message_id: "cm".into(), at: UnixMillis(0), state }
    }
    fn input(m: &DriverMsg) -> &OrgInput {
        match m {
            DriverMsg::Input(i) => i,
            _ => panic!("not an input"),
        }
    }
    fn events(o: &mut OrgObserver, e: BackendEvent) -> Vec<DriverMsg> {
        o.on_event(&e, &|_| 0, &|_| None)
    }

    #[test]
    fn send_progress_is_not_reported_and_unrelated_chats_are_ignored() {
        let mut o = observer();
        assert!(o.on_send(&chat("c1"), &attempt(SendState::Sending)).is_empty());
        assert!(o.on_send(&chat("other"), &attempt(SendState::Rejected { message: "x".into() })).is_empty());
    }

    #[test]
    fn unknown_then_accepted_is_reported_as_a_reconciliation() {
        let mut o = observer();
        let m = o.on_send(&chat("c1"), &attempt(SendState::AcceptanceUnknown { since: UnixMillis(1) }));
        assert!(matches!(input(&m[0]), OrgInput::SendSettled { outcome: SettleOutcome::Unknown, .. }));
        let m = o.on_send(&chat("c1"), &attempt(SendState::Accepted { turn: turn("c1", "t9") }));
        assert!(matches!(input(&m[0]), OrgInput::SendSettled { outcome: SettleOutcome::ReconciledAccepted { turn }, .. } if turn.0 == "t9"));
        // 直接の受理は照合ではない。
        let mut o = observer();
        let m = o.on_send(&chat("c1"), &attempt(SendState::Accepted { turn: turn("c1", "t9") }));
        assert!(matches!(input(&m[0]), OrgInput::SendSettled { outcome: SettleOutcome::Accepted { .. }, .. }));
        let m = o.on_send(&chat("c1"), &attempt(SendState::NotFoundAfterReconcile));
        assert!(matches!(input(&m[0]), OrgInput::SendSettled { outcome: SettleOutcome::ReconciledNotFound, .. }));
    }

    #[test]
    fn usage_limit_class_arrives_before_the_turn_end_and_is_consumed_once() {
        let mut o = observer();
        assert!(events(&mut o, BackendEvent::TurnFailureClass { turn: turn("c1", "t1"), class: FailureClass::UsageLimit }).is_empty());
        let end = |o: &mut OrgObserver| events(o, BackendEvent::TurnEnded { turn: turn("c1", "t1"), end: TurnEnd::Failed, error: None, evidence: ev() });
        assert!(matches!(input(&end(&mut o)[0]), OrgInput::TurnEnded { end: TurnEndKind::Failed { usage_limit: true }, .. }));
        assert!(matches!(input(&end(&mut o)[0]), OrgInput::TurnEnded { end: TurnEndKind::Failed { usage_limit: false }, .. }));
    }

    #[test]
    fn only_the_root_turn_of_a_bound_chat_ends_a_task() {
        let mut o = observer();
        let child = events(&mut o, BackendEvent::TurnEnded { turn: turn("child", "t1"), end: TurnEnd::Completed, error: None, evidence: ev() });
        assert!(child.is_empty());
        let other = events(&mut o, BackendEvent::TurnEnded { turn: turn("zzz", "t1"), end: TurnEnd::Completed, error: None, evidence: ev() });
        assert!(other.is_empty());
        let root = events(&mut o, BackendEvent::TurnEnded { turn: turn("c1", "t1"), end: TurnEnd::Interrupted, error: None, evidence: ev() });
        assert!(matches!(input(&root[0]), OrgInput::TurnEnded { end: TurnEndKind::Interrupted, .. }));
    }

    #[test]
    fn disconnect_and_gaps_mean_freshness_never_failure() {
        let mut o = observer();
        let m = events(&mut o, BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } });
        assert!(matches!(input(&m[0]), OrgInput::Freshness { cause: ReconcileCause::Disconnect }));
        assert!(!events(&mut o, BackendEvent::Gap { scope: GapScope::Source, reason: "x".into() }).is_empty());
        assert!(!events(&mut o, BackendEvent::Gap { scope: GapScope::Agent { agent: agent("c1") }, reason: "x".into() }).is_empty());
        assert!(events(&mut o, BackendEvent::Gap { scope: GapScope::Agent { agent: agent("child") }, reason: "x".into() }).is_empty());
        assert!(events(&mut o, BackendEvent::Connection { state: ConnectionState::Connected }).is_empty());
    }

    #[test]
    fn request_events_only_report_counts() {
        let mut o = observer();
        let m = o.on_event(&BackendEvent::RequestResolved { request: crate::backend::model::RequestKey { backend: BackendKind::Codex, source: crate::backend::model::SourceId("s".into()), request_id: ExternalId("r".into()) }, evidence: ev() }, &|_| 2, &|_| None);
        assert!(matches!(input(&m[0]), OrgInput::RequestPending { open: 2, .. }));
    }

    #[test]
    fn sandbox_failure_of_a_descendant_maps_to_the_owning_chat() {
        let mut o = observer();
        let chat_of = |a: &AgentKey| (a.id.0 == "child").then(|| chat("c1"));
        let m = o.on_event(&BackendEvent::SandboxFailureObserved { agent: agent("child") }, &|_| 0, &chat_of);
        assert!(matches!(input(&m[0]), OrgInput::SandboxFailure { chat } if chat.0 == "codex:c1"));
        assert!(o.on_event(&BackendEvent::SandboxFailureObserved { agent: agent("nobody") }, &|_| 0, &|_| None).is_empty());
    }

    #[test]
    fn interrupt_is_reported_only_for_bound_chats() {
        let o = observer();
        assert_eq!(o.on_interrupt(&chat("c1")).len(), 1);
        assert!(o.on_interrupt(&chat("x")).is_empty());
    }
}
