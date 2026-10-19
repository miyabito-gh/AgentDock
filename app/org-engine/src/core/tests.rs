use super::*;
use crate::classify_after_restart;
use serde_json::json;
use std::collections::{HashSet, VecDeque};

const REPORT: &str = r#"{"summary":"done","changedFiles":["a.rs"],"tests":{"ran":true,"passed":true}}"#;
const PASS: &str = r#"{"verdict":"pass","findings":[]}"#;
const REWORK: &str = r#"{"verdict":"rework","findings":["fix the edge case"]}"#;

fn tid(s: &str) -> TaskId {
    TaskId::new(s)
}

/// Plan JSON: (id, role, deps, effects).
fn plan_json(tasks: &[(&str, &str, &[&str], &[&str])]) -> String {
    let v: Vec<_> = tasks
        .iter()
        .map(|(id, role, deps, effects)| json!({"id": id, "role": role, "deps": deps, "effects": effects, "title": format!("title {id}"), "instructions": format!("do {id}")}))
        .collect();
    json!({ "tasks": v }).to_string()
}

/// A fake host: answers actions automatically (unless told otherwise) and records everything.
struct Sim {
    core: OrgCore,
    log: Vec<OrgAction>,
    unknown_sends: HashSet<TaskId>,
    notlive_sends: HashSet<TaskId>,
    hold_settle: bool,
    sandbox_ok: bool,
    auto_commit: bool,
    refresh_fail: bool,
    worktree_fail: Option<String>,
}

impl Sim {
    fn new() -> Self {
        let mut s = Self::ungranted();
        s.feed(OrgInput::Grant { granted: true });
        s
    }

    fn ungranted() -> Self {
        Self {
            core: OrgCore::new(1000),
            log: Vec::new(),
            unknown_sends: HashSet::new(),
            notlive_sends: HashSet::new(),
            hold_settle: false,
            sandbox_ok: true,
            auto_commit: true,
            refresh_fail: false,
            worktree_fail: None,
        }
    }

    fn from_core(core: OrgCore) -> Self {
        let mut s = Self::ungranted();
        s.core = core;
        s
    }

    fn feed(&mut self, input: OrgInput) -> Vec<OrgAction> {
        let mut all = Vec::new();
        let mut q = VecDeque::from([input]);
        while let Some(i) = q.pop_front() {
            let actions = self.core.handle(i);
            for a in &actions {
                if let Some(r) = self.reply(a) {
                    q.push_back(r);
                }
            }
            all.extend(actions);
        }
        self.log.extend(all.clone());
        all
    }

    fn reply(&self, a: &OrgAction) -> Option<OrgInput> {
        match a {
            OrgAction::CreateRoleChat { task, .. } => Some(OrgInput::ChatCreated { task: task.clone(), result: CreateResult::Created(ChatRef::new(format!("chat-{task}"))) }),
            OrgAction::Workspace(WorkspaceAction::CreateWorktree { task, .. }) if self.worktree_fail.is_some() => Some(OrgInput::WorkspaceResult {
                task: task.clone(),
                result: WorkspaceOutcome::Failed { detail: self.worktree_fail.clone().unwrap() },
            }),
            OrgAction::Workspace(WorkspaceAction::CreateWorktree { task, .. }) => {
                Some(OrgInput::WorkspaceResult { task: task.clone(), result: WorkspaceOutcome::WorktreeCreated { path: format!("wt/{task}") } })
            }
            OrgAction::Workspace(WorkspaceAction::Commit { task, .. }) if self.auto_commit => {
                Some(OrgInput::WorkspaceResult { task: task.clone(), result: WorkspaceOutcome::Committed { commit: format!("c-{task}") } })
            }
            OrgAction::Workspace(WorkspaceAction::RefreshDetached { task, cwd, .. }) => Some(OrgInput::WorkspaceResult {
                task: task.clone(),
                result: if self.refresh_fail { WorkspaceOutcome::Failed { detail: "x".into() } } else { WorkspaceOutcome::WorktreeCreated { path: cwd.clone() } },
            }),
            OrgAction::CheckSandbox { .. } => Some(OrgInput::SandboxChecked { ok: self.sandbox_ok, detail: "d".into() }),
            OrgAction::Send { task, attempt, .. } => {
                if self.notlive_sends.contains(task) {
                    Some(OrgInput::NotLive { task: task.clone() })
                } else if self.hold_settle {
                    None
                } else if self.unknown_sends.contains(task) {
                    Some(OrgInput::SendSettled { task: task.clone(), attempt: attempt.clone(), outcome: SettleOutcome::Unknown })
                } else {
                    Some(OrgInput::SendSettled { task: task.clone(), attempt: attempt.clone(), outcome: SettleOutcome::Accepted { turn: TurnRef::new(format!("turn-{attempt}")) } })
                }
            }
            _ => None,
        }
    }

    fn st(&self) -> &WorkflowState {
        self.core.state().unwrap()
    }

    fn t(&self, id: &str) -> &Task {
        self.st().task(&tid(id)).unwrap()
    }

    fn start(&mut self, limits: Limits) {
        self.feed(OrgInput::Start { workflow: WorkflowId::new("w1"), request: "make it".into(), repo: "C:/repo".into(), limits });
    }

    /// The task's current turn ends (completed) and its final message is `text`.
    fn complete_turn(&mut self, id: &str, text: &str) -> Vec<OrgAction> {
        let (chat, turn) = {
            let t = self.t(id);
            (t.chat().unwrap().clone(), t.accepted_turn().unwrap().clone())
        };
        self.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete: true });
        self.feed(OrgInput::TurnEnded { chat, turn: turn.clone(), end: TurnEndKind::Completed });
        self.feed(OrgInput::FinalMessage { task: tid(id), turn, text: Some(text.to_string()) })
    }

    /// Start the workflow and run the planner to a plan.
    fn with_plan(&mut self, plan: &str) {
        self.start(Limits::default());
        self.complete_turn("planner-1", plan);
    }

    fn sends(&self, id: &str) -> usize {
        self.log.iter().filter(|a| matches!(a, OrgAction::Send { task, .. } if task.0 == id)).count()
    }

    fn notices(&self) -> Vec<&OrgNotice> {
        self.log.iter().filter_map(|a| if let OrgAction::Notify(n) = a { Some(n) } else { None }).collect()
    }
}

fn has_send(actions: &[OrgAction]) -> bool {
    actions.iter().any(|a| matches!(a, OrgAction::Send { .. }))
}

#[test]
fn send_is_always_preceded_by_dispatched_persist() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("r", "reviewer", &["a"], &[])]));
    sim.complete_turn("a", REPORT);
    sim.complete_turn("r", PASS);
    let mut checked = 0;
    for (i, a) in sim.log.iter().enumerate() {
        if let OrgAction::Send { task, attempt, client_message_id, .. } = a {
            let before = &sim.log[..i];
            let dispatched = before.iter().any(|p| {
                matches!(p, OrgAction::Persist(ev) if matches!(&ev.kind,
                    OrgEventKind::Dispatched { task: t, attempt: at, client_message_id: c, .. } if t == task && at == attempt && c == client_message_id))
            });
            assert!(dispatched, "Send #{i} for {task} has no earlier Persist(Dispatched)");
            assert!(before.iter().any(|p| matches!(p, OrgAction::Persist(ev) if matches!(ev.kind, OrgEventKind::TurnSpent { .. }))));
            checked += 1;
        }
    }
    assert!(checked >= 3, "planner, worker and reviewer were sent ({checked})");
}

#[test]
fn no_action_without_grant_and_none_after_ungrant() {
    let mut sim = Sim::ungranted();
    sim.start(Limits::default());
    assert!(!sim.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. } | OrgAction::Send { .. })));
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Ready);
    sim.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(sim.log.iter().all(|a| !matches!(a, OrgAction::CreateRoleChat { .. })));
    sim.feed(OrgInput::Grant { granted: true });
    let acts = sim.feed(OrgInput::Tick { now_ms: 3000 });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })) || sim.sends("planner-1") == 1);
    sim.feed(OrgInput::Grant { granted: false });
    sim.complete_turn("planner-1", &plan_json(&[("a", "worker", &[], &[])]));
    // plan adopted (events only) but nothing is started without the grant
    assert_eq!(sim.t("a").status(), TaskStatus::Ready);
    assert!(sim.t("a").chat().is_none());
}

#[test]
fn unknown_send_is_never_resent_and_blocks_only_dependents() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("a"));
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &[], &[]), ("c", "worker", &["a"], &[])]));
    assert_eq!(sim.t("a").confirm(), ConfirmState::SendUnknown);
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    assert_eq!(sim.sends("a"), 1);

    // the independent task moves on to completion
    sim.complete_turn("b", REPORT);
    assert_eq!(sim.t("b").status(), TaskStatus::Completed);

    let before = sim.log.len();
    for i in 0..5 {
        let acts = sim.feed(OrgInput::Tick { now_ms: 5000 + i });
        assert!(!has_send(&acts));
    }
    sim.feed(OrgInput::UserResume);
    sim.feed(OrgInput::Grant { granted: true });
    assert_eq!(sim.sends("a"), 1, "a is never sent again");
    assert!(sim.log[before..].iter().all(|a| !matches!(a, OrgAction::Send { .. } | OrgAction::CreateRoleChat { .. })));
    assert_eq!(sim.t("c").status(), TaskStatus::Pending, "the dependent does not advance");
    assert_eq!(sim.sends("c"), 0);
}

#[test]
fn reconciled_not_found_blocks_for_attention_not_cancel_or_fail() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("a"));
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let attempt = sim.t("a").last_attempt().unwrap().clone();
    sim.feed(OrgInput::SendSettled { task: tid("a"), attempt, outcome: SettleOutcome::ReconciledNotFound });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
    assert_eq!(sim.sends("a"), 1);
    for i in 0..3 {
        assert!(!has_send(&sim.feed(OrgInput::Tick { now_ms: 9000 + i })));
    }
}

#[test]
fn reconciled_accepted_resolves_unknown_and_continues() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("a"));
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let attempt = sim.t("a").last_attempt().unwrap().clone();
    sim.feed(OrgInput::SendSettled { task: tid("a"), attempt, outcome: SettleOutcome::ReconciledAccepted { turn: TurnRef::new("tr") } });
    assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
    sim.complete_turn("a", REPORT);
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn freshness_marks_running_for_reconcile_and_never_fails() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &[], &[])]));
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Wake });
    for id in ["a", "b"] {
        assert_eq!(sim.t(id).status(), TaskStatus::Running);
        assert_eq!(sim.t(id).confirm(), ConfirmState::NeedsReconcile(ReconcileCause::Wake));
    }
    assert_eq!(sim.st().halt(), Some(HaltReason::NeedsResume));
    assert!(!has_send(&sim.feed(OrgInput::Tick { now_ms: 7000 })));
    // reconciliation (read only) brings the end back; the artifact is still validated
    let acts = sim.complete_turn("a", REPORT);
    assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
    // the commit is a write: it waits for the user's resume
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::Workspace(_))));
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    let acts = sim.feed(OrgInput::UserResume);
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Workspace(WorkspaceAction::Commit { .. }))));
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
}

#[test]
fn late_turn_end_changes_nothing() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let chat = sim.t("a").chat().unwrap().clone();
    let acts = sim.feed(OrgInput::TurnEnded { chat, turn: TurnRef::new("old-turn"), end: TurnEndKind::Failed { usage_limit: false } });
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
    assert!(sim.t("a").observed().is_none());
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Persist(ev) if matches!(ev.kind, OrgEventKind::TurnObserved { .. }))));
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::ReadFinalMessage { .. })));
}

#[test]
fn turn_end_before_acceptance_is_applied_after_it() {
    let mut sim = Sim::new();
    sim.hold_settle = true;
    sim.start(Limits::default());
    let t = sim.t("planner-1");
    assert_eq!(t.confirm(), ConfirmState::SendUnknown);
    let chat = t.chat().unwrap().clone();
    let attempt = t.last_attempt().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete: true });
    let acts = sim.feed(OrgInput::TurnEnded { chat, turn: TurnRef::new("tt"), end: TurnEndKind::Completed });
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::ReadFinalMessage { .. })));
    let acts = sim.feed(OrgInput::SendSettled { task: tid("planner-1"), attempt, outcome: SettleOutcome::Accepted { turn: TurnRef::new("tt") } });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::ReadFinalMessage { .. })));
}

#[test]
fn parse_failure_gets_one_format_fix_then_invalid_artifact() {
    let mut sim = Sim::new();
    sim.start(Limits::default());
    sim.complete_turn("planner-1", "this is not json");
    assert_eq!(sim.sends("planner-1"), 2, "planner turn plus one format fix");
    let kinds: Vec<TurnKind> = sim
        .log
        .iter()
        .filter_map(|a| match a {
            OrgAction::Persist(ev) => match &ev.kind {
                OrgEventKind::Dispatched { kind, .. } => Some(*kind),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(kinds, vec![TurnKind::Plan, TurnKind::FormatFix]);
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Running);
    sim.complete_turn("planner-1", "{\"tasks\": \"still wrong\"}");
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Failed(FailReason::InvalidArtifact));
    assert_eq!(sim.sends("planner-1"), 2, "no second format fix");
    assert_eq!(sim.st().counters().turns_used, 2, "the format fix counts as a turn");
}

#[test]
fn user_manual_stops_all_sends_for_the_task() {
    let mut sim = Sim::ungranted();
    sim.start(Limits::default());
    sim.feed(OrgInput::UserManual { task: tid("planner-1") });
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Blocked(BlockedReason::Manual));
    sim.feed(OrgInput::Grant { granted: true });
    for i in 0..3 {
        let acts = sim.feed(OrgInput::Tick { now_ms: 2000 + i });
        assert!(!has_send(&acts));
        assert!(!acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })));
    }
    sim.feed(OrgInput::UserResume);
    assert!(sim.log.iter().all(|a| !matches!(a, OrgAction::Send { .. })));
    // the user may declare the manual task done
    sim.feed(OrgInput::UserAcceptManual { task: tid("planner-1") });
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Completed);
}

#[test]
fn not_live_halts_for_resume_and_does_not_send() {
    let mut sim = Sim::new();
    sim.notlive_sends.insert(tid("planner-1"));
    sim.start(Limits::default());
    assert_eq!(sim.st().halt(), Some(HaltReason::NeedsResume));
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert_eq!(sim.t("planner-1").confirm(), ConfirmState::Confirmed);
    assert_eq!(sim.sends("planner-1"), 1);
    for i in 0..3 {
        assert!(!has_send(&sim.feed(OrgInput::Tick { now_ms: 3000 + i })));
    }
    // the user resumes; the host made the chat live; the task is sent once more (nothing was sent before)
    sim.notlive_sends.clear();
    sim.feed(OrgInput::UserResume);
    assert_eq!(sim.st().halt(), None);
    assert_eq!(sim.sends("planner-1"), 2);
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Running);
}

#[test]
fn chat_that_is_not_live_is_not_sent_to() {
    let mut sim = Sim::new();
    sim.start(Limits::default());
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Disconnect });
    let before = sim.sends("planner-1");
    sim.feed(OrgInput::UserResume);
    sim.feed(OrgInput::Tick { now_ms: 1 });
    assert_eq!(sim.sends("planner-1"), before);
    assert!(!sim.log.iter().rev().take(5).any(|a| matches!(a, OrgAction::Send { .. })));
}

#[test]
fn chat_creation_unknown_blocks_and_is_not_retried() {
    let mut core = OrgCore::new(1);
    core.handle(OrgInput::Grant { granted: true });
    let acts = core.handle(OrgInput::Start { workflow: WorkflowId::new("w1"), request: "r".into(), repo: "p".into(), limits: Limits::default() });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })));
    // the chat request is recorded before the attempt
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Persist(ev) if matches!(ev.kind, OrgEventKind::ChatRequested { .. }))));
    core.handle(OrgInput::ChatCreated { task: tid("planner-1"), result: CreateResult::Unknown });
    let t = core.state().unwrap().task(&tid("planner-1")).unwrap();
    assert_eq!(t.confirm(), ConfirmState::SendUnknown);
    assert_eq!(t.status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert!(t.chat().is_none());
    for i in 0..3 {
        let acts = core.handle(OrgInput::Tick { now_ms: 10 + i });
        assert!(!acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. } | OrgAction::Send { .. })));
    }
}

#[test]
fn restored_engine_does_not_recreate_a_chat_whose_creation_was_lost() {
    let mut sim = Sim::ungranted();
    sim.start(Limits::default());
    sim.feed(OrgInput::Grant { granted: true });
    // Simulate a crash right after the chat request was recorded: build the state by hand.
    let mut s = WorkflowState::new(WorkflowId::new("w1"), Limits::default());
    s.record(OrgEventKind::TaskAdded { task: PlanTask { id: tid("planner-1"), role: RoleId::new("planner"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() } }, 0).unwrap();
    s.record(OrgEventKind::Requested { request: "r".into(), repo: "p".into() }, 0).unwrap();
    s.record(OrgEventKind::StatusChanged { task: tid("planner-1"), to: TaskStatus::Ready, cause: Cause::Engine }, 0).unwrap();
    s.record(OrgEventKind::ChatRequested { task: tid("planner-1"), cwd: "p".into() }, 0).unwrap();
    let mut restored = Sim::from_core(OrgCore::restored(s, 5));
    restored.feed(OrgInput::Grant { granted: true });
    restored.feed(OrgInput::UserResume);
    restored.feed(OrgInput::Tick { now_ms: 6 });
    assert!(!restored.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. } | OrgAction::Send { .. })));
    assert_eq!(restored.t("planner-1").confirm(), ConfirmState::SendUnknown);
    assert_eq!(restored.t("planner-1").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
}

#[test]
fn recovered_chat_binding_lets_a_lost_creation_continue_without_creating_again() {
    let mut s = WorkflowState::new(WorkflowId::new("w1"), Limits::default());
    s.record(OrgEventKind::TaskAdded { task: PlanTask { id: tid("planner-1"), role: RoleId::new("planner"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() } }, 0).unwrap();
    s.record(OrgEventKind::Requested { request: "r".into(), repo: "p".into() }, 0).unwrap();
    s.record(OrgEventKind::StatusChanged { task: tid("planner-1"), to: TaskStatus::Ready, cause: Cause::Engine }, 0).unwrap();
    s.record(OrgEventKind::ChatRequested { task: tid("planner-1"), cwd: "p".into() }, 0).unwrap();
    let mut sim = Sim::from_core(OrgCore::restored(s, 5));
    // A task that never requested a chat cannot be bound.
    sim.feed(OrgInput::ChatRecovered { task: tid("nope"), chat: ChatRef::new("c") });
    sim.feed(OrgInput::ChatRecovered { task: tid("planner-1"), chat: ChatRef::new("codex:th") });
    assert_eq!(sim.t("planner-1").chat(), Some(&ChatRef::new("codex:th")));
    // A second record for the same task does not move it.
    sim.feed(OrgInput::ChatRecovered { task: tid("planner-1"), chat: ChatRef::new("codex:other") });
    assert_eq!(sim.t("planner-1").chat(), Some(&ChatRef::new("codex:th")));
    sim.feed(OrgInput::Grant { granted: true });
    sim.feed(OrgInput::Live { chat: ChatRef::new("codex:th"), live: true });
    sim.feed(OrgInput::UserResume);
    assert!(!sim.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })));
    assert_eq!(sim.sends("planner-1"), 1);
}

#[test]
fn sandbox_failure_while_running_needs_the_user_and_halts_new_launches() {
    let mut sim = Sim::new();
    sim.start(Limits::default());
    let chat = sim.t("planner-1").chat().unwrap().clone();
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Running);
    sim.feed(OrgInput::SandboxFailure { chat });
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert_eq!(sim.st().halt(), Some(HaltReason::SandboxUnhealthy));
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::SandboxUnhealthy { .. })));
    assert!(!matches!(sim.t("planner-1").status(), TaskStatus::Failed(_) | TaskStatus::Cancelled));
}

#[test]
fn plan_with_declared_effect_waits_for_approval() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"])]));
    assert_eq!(sim.st().halt(), Some(HaltReason::AwaitingPlanApproval));
    assert_eq!(sim.t("planner-1").status(), TaskStatus::Completed);
    assert_eq!(sim.sends("a"), 0);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::PlanNeedsApproval { .. })));
    // "resume" does not approve a plan
    sim.feed(OrgInput::UserResume);
    assert_eq!(sim.st().halt(), Some(HaltReason::AwaitingPlanApproval));
    sim.feed(OrgInput::UserApprovePlan);
    assert_eq!(sim.st().halt(), None);
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn replan_cancels_the_proposal_and_asks_again() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["pushMergeTag"])]));
    sim.feed(OrgInput::UserReplan { note: None });
    assert_eq!(sim.t("a").status(), TaskStatus::Cancelled);
    assert_eq!(sim.st().halt(), None);
    assert_eq!(sim.sends("planner-2"), 1);
    sim.complete_turn("planner-2", &plan_json(&[("b", "worker", &[], &[])]));
    // a re-plan is always put to the user
    assert_eq!(sim.st().halt(), Some(HaltReason::AwaitingPlanApproval));
}

#[test]
fn replan_reason_reaches_the_new_planner_as_a_user_note() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["pushMergeTag"])]));
    sim.feed(OrgInput::UserReplan { note: Some("  split it into smaller steps  ".into()) });
    let note = sim.st().notes().iter().find(|n| matches!(&n.scope, NoteScope::Node { node } if node.0 == "planner-2")).unwrap();
    assert_eq!(note.text, "split it into smaller steps");
    let text = sim.log.iter().find_map(|a| if let OrgAction::Send { task, text, .. } = a { (task.0 == "planner-2").then(|| text.clone()) } else { None }).unwrap();
    assert!(text.contains("split it into smaller steps") && text.contains("-----BEGIN USER NOTE-----"));
    // a blank reason adds nothing
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["pushMergeTag"])]));
    sim.feed(OrgInput::UserReplan { note: Some("   ".into()) });
    assert!(sim.st().notes().is_empty());
}

#[test]
fn worker_review_pass_completes_both_and_the_workflow() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    assert_eq!(sim.t("r").status(), TaskStatus::Pending);
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("w").status(), TaskStatus::Reviewing);
    assert_eq!(sim.t("w").commit(), Some("c-w"));
    // the reviewer works on a worktree detached at the commit
    assert!(sim.log.iter().any(|a| matches!(a,
        OrgAction::Workspace(WorkspaceAction::CreateWorktree { task, base: WorktreeBase::DetachedAt(c), .. }) if task.0 == "r" && c == "c-w")));
    assert!(sim.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { task, spec } if task.0 == "r" && spec.sandbox == SandboxKind::ReadOnly)));
    assert!(sim.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { task, spec } if task.0 == "w" && spec.sandbox == SandboxKind::WorkspaceWrite)));
    assert_eq!(sim.t("r").status(), TaskStatus::Running);
    sim.complete_turn("r", PASS);
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
    assert_eq!(sim.t("r").status(), TaskStatus::Completed);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
}

#[test]
fn rework_is_limited_to_two_sendbacks() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    assert_eq!(sim.t("w").rework_count(), 1);
    assert_eq!(sim.t("r").status(), TaskStatus::Rework);
    assert_eq!(sim.t("w").status(), TaskStatus::Running, "sent back and dispatched again");
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("r").status(), TaskStatus::Running, "review round 2");
    sim.complete_turn("r", REWORK);
    assert_eq!(sim.t("w").rework_count(), 2);
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    assert_eq!(sim.t("w").status(), TaskStatus::Blocked(BlockedReason::ReworkLimit));
    assert_eq!(sim.t("w").last_findings(), ["fix the edge case"]);
    assert_eq!(sim.t("r").status(), TaskStatus::Rework);
    assert_eq!(sim.sends("w"), 3, "attempts = 1 + 2 rework");
    // the rework prompt carries the findings as quoted material
    let rework_text = sim
        .log
        .iter()
        .filter_map(|a| match a {
            OrgAction::Send { task, text, .. } if task.0 == "w" => Some(text.clone()),
            _ => None,
        })
        .nth(1)
        .unwrap();
    assert!(rework_text.contains("fix the edge case") && rework_text.contains("MATERIAL"));
}

#[test]
fn completion_waits_for_working_descendants() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[])]));
    let (chat, turn) = (sim.t("w").chat().unwrap().clone(), sim.t("w").accepted_turn().unwrap().clone());
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 1, unknown: 0, scan_complete: true });
    sim.feed(OrgInput::TurnEnded { chat: chat.clone(), turn: turn.clone(), end: TurnEndKind::Completed });
    let acts = sim.feed(OrgInput::FinalMessage { task: tid("w"), turn, text: Some(REPORT.into()) });
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::Workspace(_))));
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    // scan not complete is not "no children"
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete: false });
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    // open approvals/questions also hold it
    sim.feed(OrgInput::RequestPending { chat: chat.clone(), open: 1 });
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 2, scan_complete: true });
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    sim.feed(OrgInput::RequestPending { chat, open: 0 });
    // unknown children are left unknown (noted), the task proceeds
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::DescendantsUnconfirmed { count: 2, .. })));
}

#[test]
fn unreadable_final_message_needs_reconcile() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[])]));
    let (chat, turn) = (sim.t("w").chat().unwrap().clone(), sim.t("w").accepted_turn().unwrap().clone());
    sim.feed(OrgInput::TurnEnded { chat, turn: turn.clone(), end: TurnEndKind::Completed });
    sim.feed(OrgInput::FinalMessage { task: tid("w"), turn, text: None });
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    assert!(matches!(sim.t("w").confirm(), ConfirmState::NeedsReconcile(_)));
}

#[test]
fn failures_usage_limit_and_interrupts_are_not_cancellations() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &[], &[])]));
    let ca = sim.t("a").chat().unwrap().clone();
    let ta = sim.t("a").accepted_turn().unwrap().clone();
    sim.feed(OrgInput::TurnEnded { chat: ca, turn: ta, end: TurnEndKind::Failed { usage_limit: true } });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::UsageLimit));
    assert_eq!(sim.st().halt(), Some(HaltReason::UsageLimit));
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::UsageLimit { .. })));

    let cb = sim.t("b").chat().unwrap().clone();
    let tb = sim.t("b").accepted_turn().unwrap().clone();
    sim.feed(OrgInput::InterruptRequested { chat: cb.clone() });
    assert_eq!(sim.t("b").confirm(), ConfirmState::InterruptRequested);
    sim.feed(OrgInput::TurnEnded { chat: cb, turn: tb, end: TurnEndKind::Interrupted });
    assert_eq!(sim.t("b").status(), TaskStatus::Blocked(BlockedReason::ManualStop));
    assert_eq!(sim.t("b").confirm(), ConfirmState::Confirmed);

    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let ca = sim.t("a").chat().unwrap().clone();
    let ta = sim.t("a").accepted_turn().unwrap().clone();
    sim.feed(OrgInput::TurnEnded { chat: ca, turn: ta, end: TurnEndKind::Failed { usage_limit: false } });
    // a failed turn is not terminal by itself: the user decides
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::TurnFailed));
    assert_eq!(sim.st().halt(), None);
}

#[test]
fn sandbox_failure_halts_before_the_first_turn_and_resume_rechecks() {
    let mut sim = Sim::new();
    sim.sandbox_ok = false;
    sim.start(Limits::default());
    assert_eq!(sim.st().halt(), Some(HaltReason::SandboxUnhealthy));
    assert_eq!(sim.sends("planner-1"), 0);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::SandboxUnhealthy { .. })));
    let checks = sim.log.iter().filter(|a| matches!(a, OrgAction::CheckSandbox { .. })).count();
    assert_eq!(checks, 1, "no automatic re-check");
    sim.sandbox_ok = true;
    sim.feed(OrgInput::UserResume);
    assert_eq!(sim.sends("planner-1"), 1);
}

#[test]
fn turn_limit_blocks_and_raising_resumes() {
    let mut sim = Sim::new();
    let limits = Limits { max_turns: 1, ..Limits::default() };
    sim.start(limits);
    sim.complete_turn("planner-1", &plan_json(&[("a", "worker", &[], &[])]));
    // the plan needs 1 turn but 0 are left: the user is asked
    assert_eq!(sim.sends("a"), 0);
    assert!(sim.st().halt() == Some(HaltReason::AwaitingPlanApproval) || sim.t("a").status() == TaskStatus::Blocked(BlockedReason::Limit));
    sim.feed(OrgInput::UserApprovePlan);
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::Limit));
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::TurnLimitReached)));
    sim.feed(OrgInput::UserRaiseTurnLimit { to: 5 });
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn events_replay_to_the_same_state() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    sim.complete_turn("w", REPORT);
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Wake });
    let events: Vec<OrgEvent> = sim.log.iter().filter_map(|a| if let OrgAction::Persist(e) = a { Some(e.clone()) } else { None }).collect();
    let OrgEventKind::Created { workflow_id, limits } = &events[0].kind else { panic!("first event is Created") };
    let mut s = WorkflowState::new(workflow_id.clone(), limits.clone());
    for e in &events[1..] {
        s.apply(e).unwrap_or_else(|err| panic!("replay rejected {e:?}: {err}"));
    }
    assert_eq!(&s, sim.st());
}

#[test]
fn restart_reconciliation_brings_back_only_what_is_observed() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &[], &[]), ("c", "worker", &["a"], &[])]));
    let mut state = sim.st().clone();
    let (events, changed) = classify_after_restart(&mut state, 9).unwrap();
    assert!(!events.is_empty() && changed.len() == 2);
    let mut r = Sim::from_core(OrgCore::restored(state, 10));
    r.feed(OrgInput::Grant { granted: true });
    assert!(r.feed(OrgInput::Tick { now_ms: 11 }).is_empty());
    r.feed(OrgInput::UserResume);
    assert_eq!(r.sends("a") + r.sends("b") + r.sends("c"), 0, "resume does not send to in-flight tasks");
    // the host's read-only reconciliation reports a's end
    r.complete_turn("a", REPORT);
    assert_eq!(r.t("a").status(), TaskStatus::Completed);
    assert_eq!(r.t("b").confirm(), ConfirmState::NeedsReconcile(ReconcileCause::Restart));
    assert_eq!(r.t("b").status(), TaskStatus::Running);
}

#[test]
fn guards_in_the_event_log() {
    let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
    let t = tid("t");
    s.record(OrgEventKind::TaskAdded { task: PlanTask { id: t.clone(), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() } }, 0).unwrap();
    assert!(matches!(
        s.record(OrgEventKind::Dispatched { task: t.clone(), attempt: AttemptRef::new("a1"), client_message_id: "m".into(), kind: TurnKind::Task, notes_upto: None }, 0),
        Err(crate::ApplyError::ChatNotBound(_))
    ));
    s.record(OrgEventKind::ChatRequested { task: t.clone(), cwd: "x".into() }, 0).unwrap();
    assert!(s.record(OrgEventKind::ChatRequested { task: t.clone(), cwd: "y".into() }, 0).is_err());
    s.record(OrgEventKind::ChatBound { task: t.clone(), chat: ChatRef::new("c1") }, 0).unwrap();
    assert!(s.record(OrgEventKind::ChatBound { task: t.clone(), chat: ChatRef::new("c2") }, 0).is_err());
    let d = |a: &str| OrgEventKind::Dispatched { task: t.clone(), attempt: AttemptRef::new(a), client_message_id: "m".into(), kind: TurnKind::Task, notes_upto: None };
    s.record(d("a1"), 0).unwrap();
    assert!(matches!(s.record(d("a2"), 0), Err(crate::ApplyError::AttemptOpen(_))), "no new dispatch while unsettled");
    let settle = |o| OrgEventKind::DispatchSettled { task: t.clone(), attempt: AttemptRef::new("a1"), outcome: o };
    s.record(settle(SettleOutcome::Unknown), 0).unwrap();
    assert!(s.record(d("a2"), 0).is_err(), "no new dispatch after an unknown outcome");
    assert!(s.record(settle(SettleOutcome::Unknown), 0).is_err());
    s.record(settle(SettleOutcome::ReconciledNotFound), 0).unwrap();
    assert!(s.record(settle(SettleOutcome::Rejected), 0).is_err(), "settled once");
    assert!(s.record(OrgEventKind::DispatchSettled { task: t.clone(), attempt: AttemptRef::new("zz"), outcome: SettleOutcome::Rejected }, 0).is_err());
}

#[test]
fn messages_round_trip_as_json() {
    let inputs = vec![
        OrgInput::Start { workflow: WorkflowId::new("w"), request: "r".into(), repo: "p".into(), limits: Limits::default() },
        OrgInput::SendSettled { task: tid("t"), attempt: AttemptRef::new("a"), outcome: SettleOutcome::Accepted { turn: TurnRef::new("x") } },
        OrgInput::TurnEnded { chat: ChatRef::new("c"), turn: TurnRef::new("x"), end: TurnEndKind::Failed { usage_limit: true } },
        OrgInput::ChatCreated { task: tid("t"), result: CreateResult::Created(ChatRef::new("c")) },
        OrgInput::WorkspaceResult { task: tid("t"), result: WorkspaceOutcome::Committed { commit: "abc".into() } },
        OrgInput::Descendants { chat: ChatRef::new("c"), working: 1, unknown: 2, scan_complete: true },
        OrgInput::Freshness { cause: ReconcileCause::Restart },
        OrgInput::UserRaiseTurnLimit { to: 9 },
    ];
    for i in inputs {
        let j = serde_json::to_string(&i).unwrap();
        assert_eq!(serde_json::from_str::<OrgInput>(&j).unwrap(), i, "{j}");
    }
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &["secretsOrAuth"])]));
    for a in &sim.log {
        let j = serde_json::to_string(a).unwrap();
        assert_eq!(&serde_json::from_str::<OrgAction>(&j).unwrap(), a, "{j}");
    }
}

#[test]
fn engine_has_no_input_for_resend_and_no_codex_request_action() {
    // The OrgAction set is closed; this fails to compile if a respond/resume/interrupt/resend action is added
    // without updating the match.
    fn label(a: &OrgAction) -> &'static str {
        match a {
            OrgAction::Persist(_) => "persist",
            OrgAction::PutSlot { .. } => "slot",
            OrgAction::CreateRoleChat { .. } => "create",
            OrgAction::CheckSandbox { .. } => "sandbox",
            OrgAction::Send { .. } => "send",
            OrgAction::Steer { .. } => "steer",
            OrgAction::ReadFinalMessage { .. } => "read",
            OrgAction::Workspace(_) => "workspace",
            OrgAction::Notify(_) => "notify",
            OrgAction::Publish => "publish",
        }
    }
    assert_eq!(label(&OrgAction::Publish), "publish");
}

#[test]
fn shutdown_stops_new_dispatch_and_converts_nothing_to_cancelled_or_failed() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &["a"], &[])]));
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    sim.feed(OrgInput::Shutdown);
    assert_eq!(sim.st().halt(), Some(HaltReason::Shutdown));
    // the interrupt that follows ends the running turn: blocked for the user, never cancelled / failed
    let ca = sim.t("a").chat().unwrap().clone();
    let ta = sim.t("a").accepted_turn().unwrap().clone();
    sim.feed(OrgInput::InterruptRequested { chat: ca.clone() });
    let acts = sim.feed(OrgInput::TurnEnded { chat: ca, turn: ta, end: TurnEndKind::Interrupted });
    assert!(!has_send(&acts));
    assert!(!has_send(&sim.feed(OrgInput::Tick { now_ms: 9000 })));
    for id in ["a", "b"] {
        assert!(!matches!(sim.t(id).status(), TaskStatus::Cancelled | TaskStatus::Failed(_)), "{id}");
    }
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::ManualStop));
    assert_eq!(sim.sends("b"), 0);
}

#[test]
fn shutdown_of_a_finished_workflow_records_nothing() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let ca = sim.t("a").chat().unwrap().clone();
    let ta = sim.t("a").accepted_turn().unwrap().clone();
    sim.feed(OrgInput::TurnEnded { chat: ca, turn: ta, end: TurnEndKind::Failed { usage_limit: false } });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::TurnFailed));
    sim.feed(OrgInput::UserFailTask { task: tid("a") });
    assert_eq!(sim.t("a").status(), TaskStatus::Failed(FailReason::ExecutionFailed));
    sim.feed(OrgInput::Shutdown);
    assert_ne!(sim.st().halt(), Some(HaltReason::Shutdown));
}

// ---------------------------------------------------------------- R-ORG fixes

#[test]
fn a_plan_waiting_for_approval_survives_a_restart_and_resume_does_not_start_it() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"])]));
    assert_eq!(sim.st().halt(), Some(HaltReason::AwaitingPlanApproval));
    let mut state = sim.st().clone();
    let (events, _) = classify_after_restart(&mut state, 9).unwrap();
    assert!(!events.iter().any(|e| matches!(e.kind, OrgEventKind::Halted { .. })), "the approval wait is not overwritten");
    assert_eq!(state.halt(), Some(HaltReason::AwaitingPlanApproval));
    let mut r = Sim::from_core(OrgCore::restored(state, 10));
    r.feed(OrgInput::Grant { granted: true });
    r.feed(OrgInput::UserResume);
    assert_eq!(r.st().halt(), Some(HaltReason::AwaitingPlanApproval), "resume does not approve");
    assert!(!r.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. } | OrgAction::Send { .. } | OrgAction::Workspace(_))));
    r.feed(OrgInput::UserApprovePlan);
    assert_eq!(r.st().halt(), None);
    assert_eq!(r.sends("a"), 1);
}

#[test]
fn a_reviewing_task_whose_turn_ended_is_not_reconciled_and_the_review_completes_after_a_wake() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("w").status(), TaskStatus::Reviewing);
    assert_eq!(sim.t("r").status(), TaskStatus::Running);
    // restart classification: only the running reviewer needs reconciling
    let mut state = sim.st().clone();
    let (_, changed) = classify_after_restart(&mut state, 9).unwrap();
    assert_eq!(changed.iter().map(|c| c.task.0.as_str()).collect::<Vec<_>>(), ["r"]);
    assert_eq!(state.task(&tid("w")).unwrap().confirm(), ConfirmState::Confirmed);
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Wake });
    assert_eq!(sim.t("w").confirm(), ConfirmState::Confirmed);
    assert_eq!(sim.t("r").confirm(), ConfirmState::NeedsReconcile(ReconcileCause::Wake));
    // the read-only reconciliation finds the reviewer's end
    sim.complete_turn("r", PASS);
    sim.feed(OrgInput::UserResume);
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
    assert_eq!(sim.t("w").confirm(), ConfirmState::Confirmed);
    assert_eq!(sim.t("r").status(), TaskStatus::Completed);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
}

#[test]
fn an_interrupt_requested_task_is_settled_by_the_observed_end() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let chat = sim.t("a").chat().unwrap().clone();
    sim.feed(OrgInput::InterruptRequested { chat });
    assert_eq!(sim.t("a").confirm(), ConfirmState::InterruptRequested);
    // a restart keeps it as is (it is not reclassified), and the read-only reconciliation brings the end back
    let mut state = sim.st().clone();
    let (_, changed) = classify_after_restart(&mut state, 9).unwrap();
    assert!(changed.is_empty());
    sim.complete_turn("a", REPORT);
    assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
}

#[test]
fn a_reviewer_is_dispatched_even_when_the_reviewed_worker_would_fill_the_only_slot() {
    let mut sim = Sim::new();
    sim.start(Limits::default().with_parallelism(1, 1).unwrap());
    sim.complete_turn("planner-1", &plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("w").status(), TaskStatus::Reviewing);
    assert_eq!(sim.t("r").status(), TaskStatus::Running, "the review is not starved by the finished worker turn");
    assert_eq!(sim.sends("r"), 1);
}

#[test]
fn a_rereview_moves_the_reviewers_worktree_first_and_never_sends_when_that_fails() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    sim.complete_turn("w", REPORT);
    let refresh = sim
        .log
        .iter()
        .position(|a| matches!(a, OrgAction::Workspace(WorkspaceAction::RefreshDetached { task, commit, .. }) if task.0 == "r" && commit == "c-w"))
        .expect("refresh action");
    let second_send = sim
        .log
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, OrgAction::Send { task, .. } if task.0 == "r"))
        .nth(1)
        .map(|(i, _)| i)
        .expect("second review sent");
    assert!(refresh < second_send, "the worktree is moved before the re-review is sent");
    assert_eq!(sim.t("r").status(), TaskStatus::Running);

    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    sim.refresh_fail = true;
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.sends("r"), 1, "nothing is sent to a stale worktree");
    assert_eq!(sim.t("r").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert!(!matches!(sim.t("r").status(), TaskStatus::Failed(_) | TaskStatus::Cancelled));
}

#[test]
fn child_counts_read_before_an_interruption_do_not_complete_a_task() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[])]));
    let (chat, turn) = (sim.t("w").chat().unwrap().clone(), sim.t("w").accepted_turn().unwrap().clone());
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete: true });
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Disconnect });
    sim.feed(OrgInput::TurnEnded { chat, turn: turn.clone(), end: TurnEndKind::Completed });
    sim.feed(OrgInput::FinalMessage { task: tid("w"), turn, text: Some(REPORT.into()) });
    let acts = sim.feed(OrgInput::UserResume);
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::Workspace(_))), "the old child count is not used");
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    let chat = sim.t("w").chat().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat, working: 0, unknown: 0, scan_complete: true });
    assert_eq!(sim.t("w").status(), TaskStatus::Completed, "a new read lets it go on");
}

#[test]
fn a_task_whose_first_send_certainly_did_not_happen_can_be_retried_by_the_user() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("a"));
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    // still unknown: never retried
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert_eq!(sim.sends("a"), 1);
    let attempt = sim.t("a").last_attempt().unwrap().clone();
    sim.feed(OrgInput::SendSettled { task: tid("a"), attempt, outcome: SettleOutcome::ReconciledNotFound });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    sim.unknown_sends.clear();
    sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert_eq!(sim.sends("a"), 2, "a new attempt, by the user's operation");
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    // a task that was accepted once is not retried this way
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert_eq!(sim.sends("a"), 2);
}

#[test]
fn a_rejected_chat_creation_can_be_retried_but_an_unknown_one_cannot() {
    let mut core = OrgCore::new(1);
    core.handle(OrgInput::Grant { granted: true });
    core.handle(OrgInput::Start { workflow: WorkflowId::new("w1"), request: "r".into(), repo: "p".into(), limits: Limits::default() });
    core.handle(OrgInput::ChatCreated { task: tid("planner-1"), result: CreateResult::Rejected("no".into()) });
    let t = core.state().unwrap().task(&tid("planner-1")).unwrap();
    assert_eq!((t.status(), t.confirm(), t.chat_cwd()), (TaskStatus::Blocked(BlockedReason::NeedsAttention), ConfirmState::Confirmed, None));
    let acts = core.handle(OrgInput::UserRetryTask { task: tid("planner-1") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })), "creation is asked again by the user's operation");

    let mut core = OrgCore::new(1);
    core.handle(OrgInput::Grant { granted: true });
    core.handle(OrgInput::Start { workflow: WorkflowId::new("w1"), request: "r".into(), repo: "p".into(), limits: Limits::default() });
    core.handle(OrgInput::ChatCreated { task: tid("planner-1"), result: CreateResult::Unknown });
    let acts = core.handle(OrgInput::UserRetryTask { task: tid("planner-1") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })));
    // the user picks the chat: the task goes on from the start (nothing was ever sent)
    let acts = core.handle(OrgInput::ChatRecovered { task: tid("planner-1"), chat: ChatRef::new("codex:th") });
    let t = core.state().unwrap().task(&tid("planner-1")).unwrap();
    assert_eq!((t.confirm(), t.chat()), (ConfirmState::Confirmed, Some(&ChatRef::new("codex:th"))));
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. })));
    assert!(acts.iter().any(|a| matches!(a, OrgAction::CheckSandbox { .. })), "it goes on to the usual launch");
}

#[test]
fn role_instructions_go_before_the_work_prompt() {
    let loaded: Vec<LoadedRole> = crate::roledef::standard_definitions()
        .into_iter()
        .map(|mut d| {
            if d.id == "planner" {
                d.instructions = "MY-ROLE-TEXT".into();
            }
            let id = d.id.clone();
            crate::roledef::load_role(d, &id, None).unwrap()
        })
        .collect();
    let mut core = OrgCore::new(1000);
    core.set_roles(&loaded).unwrap();
    let mut sim = Sim::from_core(core);
    sim.feed(OrgInput::Grant { granted: true });
    sim.start(Limits::default());
    let text = sim
        .log
        .iter()
        .find_map(|a| if let OrgAction::Send { task, text, .. } = a { (task.0 == "planner-1").then(|| text.clone()) } else { None })
        .unwrap();
    assert!(text.starts_with("【この役割の指示】\nMY-ROLE-TEXT"));
}

// ---------------------------------------------------------------- unused chat after a restart

/// Sandbox check fails at the start: the planner chat is made and bound, but no turn is sent. Then "restart".
fn restarted_unsent(sandbox_ok_after: bool) -> Sim {
    let mut sim = Sim::new();
    sim.sandbox_ok = false;
    sim.start(Limits::default());
    assert_eq!(sim.st().halt(), Some(HaltReason::SandboxUnhealthy));
    assert!(sim.t("planner-1").chat().is_some());
    assert_eq!(sim.sends("planner-1"), 0);
    let mut state = sim.st().clone();
    classify_after_restart(&mut state, 9).unwrap();
    let mut r = Sim::from_core(OrgCore::restored(state, 10));
    r.sandbox_ok = sandbox_ok_after;
    r.feed(OrgInput::Grant { granted: true });
    r
}

#[test]
fn an_unused_chat_that_cannot_be_resumed_is_replaced_and_the_task_is_sent_once() {
    let mut r = restarted_unsent(true);
    r.feed(OrgInput::ChatUnresumable { task: tid("planner-1") });
    assert!(r.t("planner-1").chat().is_none());
    r.feed(OrgInput::UserResume);
    assert_eq!(r.st().halt(), None);
    assert_eq!(r.log.iter().filter(|a| matches!(a, OrgAction::CreateRoleChat { .. })).count(), 1);
    assert!(r.log.iter().any(|a| matches!(a, OrgAction::Persist(ev) if matches!(ev.kind, OrgEventKind::ChatRequestAbandoned { .. }))));
    assert_eq!(r.sends("planner-1"), 1);
    assert_eq!(r.t("planner-1").status(), TaskStatus::Running);
}

#[test]
fn a_chat_with_a_dispatch_or_an_unknown_send_is_never_replaced() {
    for unknown in [true, false] {
        let mut sim = Sim::new();
        sim.start(Limits::default());
        let mut state = sim.st().clone();
        if !unknown {
            sim.complete_turn("planner-1", &plan_json(&[("a", "worker", &[], &[])]));
            state = sim.st().clone();
        } else {
            // acceptance unknown: the dispatch is on record, the outcome is not
            let t = sim.t("planner-1");
            assert_eq!(t.status(), TaskStatus::Running);
        }
        classify_after_restart(&mut state, 9).unwrap();
        let mut r = Sim::from_core(OrgCore::restored(state, 10));
        r.feed(OrgInput::Grant { granted: true });
        let tasks: Vec<TaskId> = r.st().tasks().iter().map(|t| t.id.clone()).collect();
        for task in tasks {
            let chat_before = r.st().task(&task).unwrap().chat().cloned();
            r.feed(OrgInput::ChatUnresumable { task: task.clone() });
            assert_eq!(r.st().task(&task).unwrap().chat().cloned(), chat_before, "{task} keeps its chat");
        }
        assert!(!r.log.iter().any(|a| matches!(a, OrgAction::CreateRoleChat { .. } | OrgAction::Send { .. })));
    }
}

#[test]
fn a_send_with_unknown_acceptance_stays_stopped_when_the_chat_cannot_be_resumed() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("planner-1"));
    sim.start(Limits::default());
    assert_eq!(sim.t("planner-1").confirm(), ConfirmState::SendUnknown);
    let sends = sim.sends("planner-1");
    sim.feed(OrgInput::ChatUnresumable { task: tid("planner-1") });
    sim.feed(OrgInput::UserResume);
    assert!(sim.t("planner-1").chat().is_some());
    assert_eq!(sim.sends("planner-1"), sends, "no second send");
    assert_eq!(sim.log.iter().filter(|a| matches!(a, OrgAction::CreateRoleChat { .. })).count(), 1);
}

#[test]
fn resuming_from_sandbox_unhealthy_checks_again_first_and_stops_again_when_it_still_fails() {
    let mut r = restarted_unsent(false);
    let checks = |r: &Sim| r.log.iter().filter(|a| matches!(a, OrgAction::CheckSandbox { .. })).count();
    r.feed(OrgInput::ChatUnresumable { task: tid("planner-1") });
    r.feed(OrgInput::UserResume);
    assert_eq!(checks(&r), 1, "re-checked on resume");
    assert_eq!(r.st().halt(), Some(HaltReason::SandboxUnhealthy));
    assert_eq!(r.sends("planner-1"), 0);
    // the user fixes the cause and resumes again: checked again, then sent
    r.sandbox_ok = true;
    r.feed(OrgInput::UserResume);
    assert_eq!(checks(&r), 2);
    assert_eq!(r.st().halt(), None);
    assert_eq!(r.sends("planner-1"), 1);
}

#[test]
fn the_event_log_of_a_replaced_chat_replays_to_the_same_state() {
    let mut r = restarted_unsent(true);
    // the state as restored, then the events recorded after it
    let mut replayed = r.st().clone();
    r.feed(OrgInput::ChatUnresumable { task: tid("planner-1") });
    r.feed(OrgInput::UserResume);
    let events: Vec<OrgEvent> = r.log.iter().filter_map(|a| if let OrgAction::Persist(ev) = a { Some(ev.clone()) } else { None }).collect();
    assert!(events.iter().any(|e| matches!(e.kind, OrgEventKind::ChatRequestAbandoned { .. })));
    for ev in &events {
        replayed.apply(ev).unwrap();
    }
    let live_state = r.st();
    let (a, b) = (replayed.task(&tid("planner-1")).unwrap(), live_state.task(&tid("planner-1")).unwrap());
    assert_eq!((a.chat(), a.chat_cwd(), a.status(), a.confirm()), (b.chat(), b.chat_cwd(), b.status(), b.confirm()));
    assert_eq!(replayed.halt(), live_state.halt());
    assert_eq!(replayed.last_seq(), live_state.last_seq());
}

#[test]
fn a_failed_worktree_blocks_the_task_with_the_first_line_of_the_failure_as_the_reason() {
    let mut sim = Sim::new();
    sim.worktree_fail = Some("fatal: not a valid object name: 'HEAD'
second line".into());
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[])]));
    assert_eq!(sim.t("w").status(), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    let why = sim.notices().into_iter().find_map(|n| match n {
        OrgNotice::TaskBlocked { task, why } if task.0 == "w" => Some(why.clone()),
        _ => None,
    });
    let why = why.expect("a blocked notice with a reason");
    assert!(why.contains("not a valid object name") && !why.contains("second line"), "{why}");
}

// ---------------------------------------------------------------- R1 (DESIGN_ORG_R1): failures, retries, adoption

fn end_turn(sim: &mut Sim, id: &str, end: TurnEndKind) {
    let (chat, turn) = (sim.t(id).chat().unwrap().clone(), sim.t(id).accepted_turn().unwrap().clone());
    sim.feed(OrgInput::TurnEnded { chat, turn, end });
}

fn dispatched_kinds(sim: &Sim, id: &str) -> Vec<TurnKind> {
    sim.log
        .iter()
        .filter_map(|a| match a {
            OrgAction::Persist(ev) => match &ev.kind {
                OrgEventKind::Dispatched { task, kind, .. } if task.0 == id => Some(*kind),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn turn_failures_block_with_their_reason_and_only_the_usage_limit_halts() {
    let cases = [
        (TurnEndKind::Failed { usage_limit: true }, BlockedReason::UsageLimit, Some(HaltReason::UsageLimit)),
        (TurnEndKind::Failed { usage_limit: false }, BlockedReason::TurnFailed, None),
        (TurnEndKind::FailedUnclassified, BlockedReason::TurnFailed, None),
        (TurnEndKind::Interrupted, BlockedReason::ManualStop, None),
    ];
    for (end, reason, halt) in cases {
        let mut sim = Sim::new();
        sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
        end_turn(&mut sim, "a", end);
        assert_eq!(sim.t("a").status(), TaskStatus::Blocked(reason), "{end:?}");
        assert_eq!(sim.t("a").confirm(), ConfirmState::Confirmed);
        assert_eq!(sim.st().halt(), halt, "{end:?}");
        assert!(!sim.notices().iter().any(|n| matches!(n, OrgNotice::TaskFailed { .. })));
    }
}

#[test]
fn retry_kind_covers_every_branch() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let limits = Limits::default();
    // running: not blocked
    assert_eq!(retry_kind(sim.t("a"), &limits, 5), Err(RetryRefusal::NotBlocked));
    end_turn(&mut sim, "a", TurnEndKind::Failed { usage_limit: true });
    assert_eq!(retry_kind(sim.t("a"), &limits, 5), Ok(RetryKind::NewAttempt { reason: RetryReason::UsageLimit }));
    assert_eq!(retry_kind(sim.t("a"), &limits, 0), Err(RetryRefusal::NoTurns));
    assert_eq!(retry_kind(sim.t("a"), &Limits { max_retries: 0, ..limits.clone() }, 5), Err(RetryRefusal::RetryLimit { used: 0, max: 0 }));
    // unconfirmed
    let mut t = sim.t("a").clone();
    t.confirm = ConfirmState::NeedsReconcile(ReconcileCause::Wake);
    assert_eq!(retry_kind(&t, &limits, 5), Err(RetryRefusal::SendUnknown));
    // the end of the accepted turn is not confirmed
    let mut t = sim.t("a").clone();
    t.observed = None;
    assert_eq!(retry_kind(&t, &limits, 5), Err(RetryRefusal::NotEnded));
    let mut t = sim.t("a").clone();
    t.observed = Some(TurnObservation { turn: TurnRef::new("other"), end: TurnEndKind::Interrupted });
    assert_eq!(retry_kind(&t, &limits, 5), Err(RetryRefusal::NotEnded));
    // the turn completed: nothing to retry
    let mut t = sim.t("a").clone();
    t.observed = Some(TurnObservation { turn: t.accepted_turn().unwrap().clone(), end: TurnEndKind::Completed });
    assert_eq!(retry_kind(&t, &limits, 5), Err(RetryRefusal::NotEnded));
    // an open attempt
    let mut t = sim.t("a").clone();
    t.last_settle = None;
    assert_eq!(retry_kind(&t, &limits, 5), Err(RetryRefusal::SendUnknown));
    // reasons by end
    for (end, reason) in [(TurnEndKind::FailedUnclassified, RetryReason::TurnFailed), (TurnEndKind::Interrupted, RetryReason::Interrupted)] {
        let mut t = sim.t("a").clone();
        t.status = TaskStatus::Blocked(BlockedReason::ManualStop);
        t.observed = Some(TurnObservation { turn: t.accepted_turn().unwrap().clone(), end });
        assert_eq!(retry_kind(&t, &limits, 5), Ok(RetryKind::NewAttempt { reason }));
    }
    // a task nothing was sent for keeps the old "try again"
    let mut t = sim.t("a").clone();
    t.status = TaskStatus::Blocked(BlockedReason::NeedsAttention);
    t.accepted_turn = None;
    t.last_attempt = None;
    t.observed = None;
    assert_eq!(retry_kind(&t, &limits, 5), Ok(RetryKind::Unsent));
}

#[test]
fn a_new_attempt_is_sent_once_per_user_action_and_counted() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    end_turn(&mut sim, "a", TurnEndKind::Failed { usage_limit: false });
    assert_eq!(sim.sends("a"), 1);
    // nothing is sent by itself
    for i in 0..3 {
        assert!(!has_send(&sim.feed(OrgInput::Tick { now_ms: 5000 + i })));
    }
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert_eq!(acts.iter().filter(|a| matches!(a, OrgAction::Send { .. })).count(), 1);
    assert_eq!(sim.sends("a"), 2);
    assert_eq!(dispatched_kinds(&sim, "a"), vec![TurnKind::Task, TurnKind::Retry]);
    assert_eq!(sim.t("a").retry_count(), 1);
    assert!(!sim.t("a").retry_pending(), "Dispatched clears the pending flag");
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    let text = acts.iter().find_map(|a| if let OrgAction::Send { text, .. } = a { Some(text.clone()) } else { None }).unwrap();
    assert!(text.contains("失敗") && text.contains("title a") && !text.contains("do a"), "{text}");
    // the second press while it runs does nothing
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert!(!has_send(&acts) && sim.t("a").retry_count() == 1);
    // the new turn completes normally
    sim.complete_turn("a", REPORT);
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
}

#[test]
fn the_retry_limit_stops_new_attempts_and_survives_a_replay() {
    let mut sim = Sim::new();
    sim.start(Limits { max_retries: 1, ..Limits::default() });
    sim.complete_turn("planner-1", &plan_json(&[("a", "worker", &[], &[])]));
    end_turn(&mut sim, "a", TurnEndKind::Failed { usage_limit: false });
    sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert_eq!(sim.sends("a"), 2);
    end_turn(&mut sim, "a", TurnEndKind::FailedUnclassified);
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::TurnFailed));
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert_eq!(sim.sends("a"), 2);
    assert_eq!(sim.t("a").retry_count(), 1);
    // replay (as after a restart) keeps the count
    let events: Vec<OrgEvent> = sim.log.iter().filter_map(|a| if let OrgAction::Persist(e) = a { Some(e.clone()) } else { None }).collect();
    let OrgEventKind::Created { workflow_id, limits } = &events[0].kind else { panic!("first event is Created") };
    let mut s = WorkflowState::new(workflow_id.clone(), limits.clone());
    for e in &events[1..] {
        s.apply(e).unwrap_or_else(|err| panic!("replay rejected {e:?}: {err}"));
    }
    assert_eq!(&s, sim.st());
    assert!(matches!(
        s.record(OrgEventKind::RetryRequested { task: tid("a") }, 0),
        Err(crate::ApplyError::Limit(crate::LimitError::RetryLimit { used: 1, max: 1 }))
    ));
}

#[test]
fn a_usage_limit_stop_is_resumed_by_the_user_and_sends_only_what_the_user_retries() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &[], &[])]));
    end_turn(&mut sim, "a", TurnEndKind::Failed { usage_limit: true });
    assert_eq!(sim.st().halt(), Some(HaltReason::UsageLimit));
    // the retry is accepted but nothing goes out while the workflow is halted
    let acts = sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert!(!has_send(&acts));
    assert_eq!(sim.t("a").status(), TaskStatus::Ready);
    let acts = sim.feed(OrgInput::UserResume);
    assert_eq!(acts.iter().filter(|a| matches!(a, OrgAction::Send { task, .. } if task.0 == "a")).count(), 1);
    assert_eq!(sim.sends("a"), 2);
    // b was running all along and is untouched
    assert_eq!(sim.sends("b"), 1);
}

#[test]
fn settling_as_failed_is_a_user_action_for_stopped_tasks_only() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let acts = sim.feed(OrgInput::UserFailTask { task: tid("a") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    end_turn(&mut sim, "a", TurnEndKind::Interrupted);
    sim.feed(OrgInput::UserFailTask { task: tid("a") });
    assert_eq!(sim.t("a").status(), TaskStatus::Failed(FailReason::ExecutionFailed));
}

#[test]
fn an_unconfirmed_stopped_task_cannot_be_settled_or_retried() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    end_turn(&mut sim, "a", TurnEndKind::Failed { usage_limit: false });
    let idx = sim.st().tasks().iter().position(|t| t.id.0 == "a").unwrap();
    sim.core.state.as_mut().unwrap().tasks[idx].confirm = ConfirmState::StopUnconfirmed;
    sim.feed(OrgInput::UserFailTask { task: tid("a") });
    sim.feed(OrgInput::UserRetryTask { task: tid("a") });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::TurnFailed));
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn adopting_a_manual_result_runs_the_usual_completion_and_never_sends() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    sim.feed(OrgInput::UserManual { task: tid("a") });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::Manual));
    let chat = sim.t("a").chat().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat, working: 0, unknown: 0, scan_complete: true });
    let acts = sim.feed(OrgInput::UserAdoptManual { task: tid("a"), turn: TurnRef::new("manual-1") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::ReadFinalMessage { turn, .. } if turn.0 == "manual-1")));
    assert!(!has_send(&acts));
    assert!(sim.t("a").adopting());
    assert_eq!(sim.t("a").accepted_turn(), Some(&TurnRef::new("manual-1")));
    let acts = sim.feed(OrgInput::FinalMessage { task: tid("a"), turn: TurnRef::new("manual-1"), text: Some(REPORT.into()) });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Workspace(WorkspaceAction::Commit { .. }))), "a worker is committed as usual");
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn an_adopted_result_in_the_wrong_format_goes_back_to_manual_without_a_fix_turn() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    sim.feed(OrgInput::UserManual { task: tid("a") });
    sim.feed(OrgInput::UserAdoptManual { task: tid("a"), turn: TurnRef::new("manual-1") });
    let acts = sim.feed(OrgInput::FinalMessage { task: tid("a"), turn: TurnRef::new("manual-1"), text: Some("done, thanks".into()) });
    assert!(!has_send(&acts));
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::Manual));
    assert!(!sim.t("a").adopting());
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::TaskBlocked { .. }))));
    // an unreadable answer also returns to manual
    sim.feed(OrgInput::UserAdoptManual { task: tid("a"), turn: TurnRef::new("manual-2") });
    sim.feed(OrgInput::FinalMessage { task: tid("a"), turn: TurnRef::new("manual-2"), text: None });
    assert_eq!(sim.t("a").status(), TaskStatus::Blocked(BlockedReason::Manual));
    assert_eq!(sim.sends("a"), 1, "nothing was ever sent into the chat");
    // and it can be adopted again
    let chat = sim.t("a").chat().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat, working: 0, unknown: 0, scan_complete: true });
    sim.feed(OrgInput::UserAdoptManual { task: tid("a"), turn: TurnRef::new("manual-3") });
    sim.feed(OrgInput::FinalMessage { task: tid("a"), turn: TurnRef::new("manual-3"), text: Some(REPORT.into()) });
    assert_eq!(sim.t("a").status(), TaskStatus::Completed);
}

#[test]
fn adoption_requires_a_confirmed_manual_task() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let acts = sim.feed(OrgInput::UserAdoptManual { task: tid("a"), turn: TurnRef::new("m") });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. }))));
    assert_eq!(sim.t("a").status(), TaskStatus::Running);
    assert!(matches!(
        sim.core.state.as_mut().unwrap().record(OrgEventKind::ManualTurnAdopted { task: tid("a"), turn: TurnRef::new("m") }, 0),
        Err(crate::ApplyError::NotAdoptable(_))
    ));
}

#[test]
fn limits_without_max_retries_read_as_three() {
    let mut v = serde_json::to_value(Limits::default()).unwrap();
    v.as_object_mut().unwrap().remove("max_retries");
    let l: Limits = serde_json::from_value(v).unwrap();
    assert_eq!(l.max_retries, 3);
    assert!(Limits::default().with_max_retries(10).is_ok());
    assert!(Limits::default().with_max_retries(11).is_err());
}

// ---------------------------------------------------------------- DESIGN_ORG_R2: interventions

/// Plan w (worker) + r (reviewer), the reviewer rejecting three times: `w` is stopped at the limit.
fn at_rework_limit() -> Sim {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    sim.complete_turn("w", REPORT);
    sim.complete_turn("r", REWORK);
    assert_eq!(sim.t("w").status(), TaskStatus::Blocked(BlockedReason::ReworkLimit));
    sim
}

fn sent_texts(sim: &Sim, id: &str) -> Vec<String> {
    sim.log.iter().filter_map(|a| if let OrgAction::Send { task, text, .. } = a { (task.0 == id).then(|| text.clone()) } else { None }).collect()
}

fn refused(acts: &[OrgAction]) -> bool {
    acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::Refused { .. })))
}

/// Rebuild the state from the persisted events of the simulation (what a restart would read).
fn replay(sim: &Sim) -> WorkflowState {
    let evs: Vec<OrgEvent> = sim.log.iter().filter_map(|a| if let OrgAction::Persist(e) = a { Some(e.clone()) } else { None }).collect();
    let OrgEventKind::Created { workflow_id, limits } = evs[0].kind.clone() else { panic!("no Created") };
    let mut s = WorkflowState::new(workflow_id, limits);
    s.last_seq = evs[0].seq;
    for e in &evs[1..] {
        s.apply(e).unwrap();
    }
    s
}

#[test]
fn rework_limit_stops_for_the_user_and_the_user_can_fail_it() {
    let mut sim = at_rework_limit();
    assert_eq!(sim.t("w").rework_count(), 2);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::TaskBlocked { task, .. } if task.0 == "w")));
    // a restart keeps the findings and the state
    let restored = replay(&sim);
    assert_eq!(restored.task(&tid("w")).unwrap().last_findings(), ["fix the edge case"]);
    assert_eq!(restored.task(&tid("w")).unwrap().status(), TaskStatus::Blocked(BlockedReason::ReworkLimit));
    sim.feed(OrgInput::UserFailTask { task: tid("w") });
    assert_eq!(sim.t("w").status(), TaskStatus::Failed(FailReason::ExecutionFailed));
}

#[test]
fn user_send_back_from_the_limit_does_not_count_as_automatic_and_carries_the_note() {
    let mut sim = at_rework_limit();
    sim.feed(OrgInput::UserSendBack { task: tid("w"), note: Some("handle the empty list too".into()) });
    assert_eq!(sim.t("w").rework_count(), 2, "not an automatic send-back");
    assert_eq!(sim.t("w").user_rework_count(), 1);
    assert_eq!(sim.t("w").status(), TaskStatus::Running);
    let texts = sent_texts(&sim, "w");
    assert_eq!(texts.len(), 4);
    assert!(texts[3].contains("handle the empty list too") && texts[3].contains("-----BEGIN USER NOTE-----"), "{}", texts[3]);
    // the note is used up by the accepted send; the reviewer reviews again
    assert!(crate::notes_for(sim.st(), &tid("w")).0.is_empty());
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("r").status(), TaskStatus::Running);
    sim.complete_turn("r", PASS);
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
}

#[test]
fn user_send_back_needs_a_note_while_waiting_for_review_and_refuses_a_completed_worker() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    // the reviewer is running now
    assert_eq!(sim.t("r").status(), TaskStatus::Running);
    assert!(refused(&sim.feed(OrgInput::UserSendBack { task: tid("w"), note: None })));
    assert!(refused(&sim.feed(OrgInput::UserSendBack { task: tid("w"), note: Some("x".into()) })), "the reviewer is running");
    assert_eq!(sim.t("w").status(), TaskStatus::Reviewing);
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[])]));
    sim.complete_turn("w", REPORT);
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
    assert!(refused(&sim.feed(OrgInput::UserSendBack { task: tid("w"), note: Some("x".into()) })));
}

#[test]
fn accept_as_is_completes_the_worker_and_skips_the_waiting_reviewer() {
    let mut sim = at_rework_limit();
    sim.feed(OrgInput::UserAcceptAsIs { task: tid("w") });
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
    assert_eq!(sim.t("r").status(), TaskStatus::Cancelled);
    assert!(sim.t("r").skipped());
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
}

#[test]
fn skip_a_pending_worker_takes_its_reviewer_and_lets_the_followers_go() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"]), ("r", "reviewer", &["a"], &[]), ("b", "worker", &["a"], &[])]));
    assert_eq!(sim.st().halt(), Some(HaltReason::AwaitingPlanApproval));
    sim.feed(OrgInput::UserSkipTask { task: tid("a") });
    assert_eq!(sim.t("a").status(), TaskStatus::Cancelled);
    assert!(sim.t("a").skipped() && sim.t("r").skipped());
    assert_eq!(sim.t("r").status(), TaskStatus::Cancelled);
    sim.feed(OrgInput::UserApprovePlan);
    // b treats the skipped dependency as satisfied, and its prompt says so
    assert_eq!(sim.t("b").status(), TaskStatus::Running);
    assert!(sent_texts(&sim, "b")[0].contains("スキップしました"));
    sim.complete_turn("b", REPORT);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)), "skipped tasks do not block completion");
}

#[test]
fn skipping_the_only_reviewer_completes_the_reviewing_worker_unreviewed() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("w", "worker", &[], &[]), ("r", "reviewer", &["w"], &[])]));
    sim.complete_turn("w", REPORT);
    // the reviewer is blocked before it started (its worktree could not be made)
    sim.core.state.as_mut().unwrap().tasks.iter_mut().find(|t| t.id.0 == "r").unwrap().status = TaskStatus::Blocked(BlockedReason::NeedsAttention);
    sim.core.launching.clear();
    sim.core.flows.clear();
    sim.feed(OrgInput::UserSkipTask { task: tid("r") });
    assert_eq!(sim.t("r").status(), TaskStatus::Cancelled);
    assert_eq!(sim.t("w").status(), TaskStatus::Completed);
}

#[test]
fn skip_refuses_running_and_unconfirmed_tasks() {
    let mut sim = Sim::new();
    sim.unknown_sends.insert(tid("u"));
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("u", "worker", &[], &[])]));
    for id in ["a", "u"] {
        let acts = sim.feed(OrgInput::UserSkipTask { task: tid(id) });
        assert!(refused(&acts), "{id}");
        assert_eq!(sim.t(id).status(), TaskStatus::Running);
    }
}

#[test]
fn a_worker_whose_reviewer_was_cancelled_completes_at_the_commit() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"]), ("r", "reviewer", &["a"], &[])]));
    sim.feed(OrgInput::UserCancelTask { task: tid("r") });
    sim.feed(OrgInput::UserApprovePlan);
    sim.complete_turn("a", REPORT);
    assert_eq!(sim.t("a").status(), TaskStatus::Completed, "a cancelled reviewer is not counted");
}

#[test]
fn a_held_task_is_not_sent_until_released() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"]), ("b", "worker", &[], &["networkOrInstall"])]));
    sim.feed(OrgInput::UserHoldTask { task: tid("b"), held: true });
    assert!(sim.t("b").held());
    sim.feed(OrgInput::UserApprovePlan);
    assert_eq!(sim.sends("a"), 1);
    assert_eq!(sim.sends("b"), 0);
    sim.feed(OrgInput::UserHoldTask { task: tid("b"), held: false });
    assert_eq!(sim.sends("b"), 1);
    // a running task cannot be held
    assert!(refused(&sim.feed(OrgInput::UserHoldTask { task: tid("a"), held: true })));
}

#[test]
fn a_note_rides_along_once_and_only_an_accepted_send_uses_it_up() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"]), ("b", "worker", &["a"], &[])]));
    sim.feed(OrgInput::UserAddNote { text: "use tabs".into(), task: None });
    sim.feed(OrgInput::UserAddNote { text: "only for b".into(), task: Some(tid("b")) });
    // a withdrawn note never goes
    sim.feed(OrgInput::UserAddNote { text: "never mind".into(), task: None });
    let nid = sim.st().notes().last().unwrap().id.clone();
    sim.feed(OrgInput::UserWithdrawNote { id: nid });
    sim.unknown_sends.insert(tid("a"));
    sim.feed(OrgInput::UserApprovePlan);
    let a = sent_texts(&sim, "a");
    assert!(a[0].contains("use tabs") && !a[0].contains("only for b") && !a[0].contains("never mind"));
    assert!(a[0].contains("利用者からの追加指示"));
    // unknown acceptance does not use the note up
    assert_eq!(sim.t("a").notes_seen_seq(), 0);
    assert_eq!(sim.t("a").notes_sent_upto(), Some(sim.st().notes()[0].seq));
    let attempt = sim.t("a").last_attempt().unwrap().clone();
    sim.feed(OrgInput::SendSettled { task: tid("a"), attempt, outcome: SettleOutcome::ReconciledAccepted { turn: TurnRef::new("tr") } });
    assert_eq!(sim.t("a").notes_seen_seq(), sim.st().notes()[0].seq);
    sim.unknown_sends.clear();
    sim.complete_turn("a", REPORT);
    let b = sent_texts(&sim, "b");
    assert!(b[0].contains("use tabs") && b[0].contains("only for b") && !b[0].contains("never mind"));
    assert_eq!(crate::notes_for(sim.st(), &tid("a")).0.len(), 0);
}

#[test]
fn a_format_fix_turn_carries_no_note() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"])]));
    sim.feed(OrgInput::UserApprovePlan);
    sim.feed(OrgInput::UserAddNote { text: "late wish".into(), task: Some(tid("a")) });
    sim.complete_turn("a", "not json");
    let texts = sent_texts(&sim, "a");
    assert_eq!(texts.len(), 2);
    assert!(!texts[1].contains("late wish"), "format fix: {}", texts[1]);
    assert_eq!(sim.t("a").notes_sent_upto(), None);
}

#[test]
fn note_limits_and_blank_text_are_refused() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &["networkOrInstall"])]));
    assert!(refused(&sim.feed(OrgInput::UserAddNote { text: "   ".into(), task: None })));
    assert!(refused(&sim.feed(OrgInput::UserAddNote { text: "x".repeat(4001), task: None })));
    assert!(refused(&sim.feed(OrgInput::UserAddNote { text: "x".into(), task: Some(tid("zz")) })));
    for _ in 0..crate::MAX_PENDING_NOTES {
        assert!(!refused(&sim.feed(OrgInput::UserAddNote { text: "x".into(), task: None })));
    }
    assert!(refused(&sim.feed(OrgInput::UserAddNote { text: "x".into(), task: None })));
    assert!(refused(&sim.feed(OrgInput::UserWithdrawNote { id: NoteId::new("nope") })));
}

fn interject_sim() -> (Sim, NoteId) {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let turn = sim.t("a").accepted_turn().unwrap().clone();
    let acts = sim.feed(OrgInput::UserInterject { task: tid("a"), text: "also update the README".into(), fallback: true, active_turn: Some(turn.clone()) });
    let steer = acts
        .iter()
        .find_map(|a| if let OrgAction::Steer { id, turn: t, text, task, .. } = a { Some((id.clone(), t.clone(), text.clone(), task.clone())) } else { None })
        .expect("steer");
    assert_eq!((steer.1, steer.2.as_str(), steer.3), (turn, "also update the README", tid("a")));
    (sim, steer.0)
}

#[test]
fn interject_steers_into_the_accepted_turn_without_a_new_turn() {
    let (sim, id) = interject_sim();
    assert_eq!(sim.st().counters().turns_used, 2, "planner + worker only: a steer starts no turn");
    assert!(sim.st().interject(&id).unwrap().outcome.is_none());
    assert_eq!(sim.sends("a"), 1);
}

#[test]
fn interject_is_refused_when_the_turn_is_not_the_running_accepted_one() {
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[]), ("b", "worker", &["a"], &[])]));
    let turn = sim.t("a").accepted_turn().unwrap().clone();
    assert!(refused(&sim.feed(OrgInput::UserInterject { task: tid("a"), text: "x".into(), fallback: false, active_turn: Some(TurnRef::new("other")) })));
    assert!(refused(&sim.feed(OrgInput::UserInterject { task: tid("a"), text: "x".into(), fallback: false, active_turn: None })));
    assert!(refused(&sim.feed(OrgInput::UserInterject { task: tid("b"), text: "x".into(), fallback: false, active_turn: Some(turn.clone()) })));
    assert!(refused(&sim.feed(OrgInput::UserInterject { task: tid("a"), text: "  ".into(), fallback: false, active_turn: Some(turn.clone()) })));
    // the turn ended
    let chat = sim.t("a").chat().unwrap().clone();
    sim.feed(OrgInput::TurnEnded { chat, turn: turn.clone(), end: TurnEndKind::Completed });
    assert!(refused(&sim.feed(OrgInput::UserInterject { task: tid("a"), text: "x".into(), fallback: false, active_turn: Some(turn) })));
    assert!(sim.st().interjects().is_empty());
}

#[test]
fn only_one_interject_is_open_per_task() {
    let (mut sim, _) = interject_sim();
    let turn = sim.t("a").accepted_turn().unwrap().clone();
    let acts = sim.feed(OrgInput::UserInterject { task: tid("a"), text: "again".into(), fallback: false, active_turn: Some(turn) });
    assert!(refused(&acts));
    assert_eq!(sim.st().interjects().len(), 1);
}

#[test]
fn a_certain_failure_falls_back_to_a_note_only_when_chosen() {
    let (mut sim, id) = interject_sim();
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::NotDelivered { reason: "rejected".into() } });
    assert_eq!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::FallbackNoted));
    assert_eq!(sim.st().notes().len(), 1);
    assert!(matches!(&sim.st().notes()[0].scope, NoteScope::Task { task } if task.0 == "a"));
    assert_eq!(sim.st().notes()[0].text, "also update the README");
    // without the fallback choice the failure stays a failure
    let mut sim = Sim::new();
    sim.with_plan(&plan_json(&[("a", "worker", &[], &[])]));
    let turn = sim.t("a").accepted_turn().unwrap().clone();
    let acts = sim.feed(OrgInput::UserInterject { task: tid("a"), text: "t".into(), fallback: false, active_turn: Some(turn) });
    let id = acts.iter().find_map(|a| if let OrgAction::Steer { id, .. } = a { Some(id.clone()) } else { None }).unwrap();
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::NotDelivered { reason: "r".into() } });
    assert!(matches!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::NotDelivered { .. })));
    assert!(sim.st().notes().is_empty());
}

#[test]
fn an_unknown_outcome_does_nothing_by_itself_and_the_user_decides() {
    let (mut sim, id) = interject_sim();
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::Unknown });
    assert!(sim.st().notes().is_empty(), "no automatic note, no resend");
    assert_eq!(sim.sends("a"), 1);
    assert_eq!(sim.log.iter().filter(|a| matches!(a, OrgAction::Steer { .. })).count(), 1, "no second steer");
    // the reconciliation may still settle it; a settled one is final
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::ReconciledNotFound });
    assert_eq!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::ReconciledNotFound));
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::ReconciledDelivered });
    assert_eq!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::ReconciledNotFound));
    sim.feed(OrgInput::UserInterjectFallback { id: id.clone(), add: true });
    assert_eq!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::FallbackNoted));
    assert_eq!(sim.st().notes().len(), 1);
    // already decided: another choice is refused
    assert!(refused(&sim.feed(OrgInput::UserInterjectFallback { id, add: false })));
}

#[test]
fn discarding_an_unknown_interject_adds_no_note() {
    let (mut sim, id) = interject_sim();
    sim.feed(OrgInput::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::Unknown });
    sim.feed(OrgInput::UserInterjectFallback { id: id.clone(), add: false });
    assert_eq!(sim.st().interject(&id).unwrap().outcome, Some(InterjectOutcome::Discarded));
    assert!(sim.st().notes().is_empty());
}

#[test]
fn a_restart_turns_an_open_interject_into_unknown_and_replays_the_new_state() {
    let (sim, id) = interject_sim();
    let mut state = replay(&sim);
    assert_eq!(&state, sim.st());
    let (events, _) = classify_after_restart(&mut state, 9).unwrap();
    assert!(events.iter().any(|e| matches!(&e.kind, OrgEventKind::InterjectSettled { id: i, outcome: InterjectOutcome::Unknown } if *i == id)));
    assert_eq!(state.interject(&id).unwrap().outcome, Some(InterjectOutcome::Unknown));
    // JSON round trip of the snapshot keeps the new fields
    let j = serde_json::to_string(&state).unwrap();
    assert_eq!(serde_json::from_str::<WorkflowState>(&j).unwrap(), state);
}

#[test]
fn old_task_records_without_the_new_fields_still_load() {
    let mut v = serde_json::to_value(Task::from_plan(&PlanTask { id: tid("t"), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() })).unwrap();
    for k in ["notes_sent_upto", "notes_seen_seq", "skipped", "held", "user_rework_count", "last_findings"] {
        v.as_object_mut().unwrap().remove(k);
    }
    let t: Task = serde_json::from_value(v).unwrap();
    assert!(!t.held() && !t.skipped() && t.notes_seen_seq() == 0 && t.last_findings().is_empty());
}
