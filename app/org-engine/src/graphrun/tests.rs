use std::collections::{HashMap, HashSet, VecDeque};

use super::*;
use crate::core::{CreateResult, OrgInput, WorkspaceOutcome};
use crate::graph::{parse_graph, template_json, validate, RoleInfo, TEMPLATE_QUALITY_LOOP};
use crate::outschema::builtin_schemas;
use crate::persist::HaltReason;

const SPEC: &str = r#"{"goal":"g","steps":["s"],"acceptance":["a"]}"#;
const REPORT: &str = r#"{"summary":"done","changedFiles":["a.rs"],"tests":{"ran":true,"passed":true}}"#;
const PASS: &str = r#"{"verdict":"pass","findings":[]}"#;
const REWORK: &str = r#"{"verdict":"rework","findings":["fix it"]}"#;
const REPLAN: &str = r#"{"verdict":"replan","findings":["wrong approach"]}"#;

fn tid(s: &str) -> TaskId {
    TaskId::new(s)
}

fn compile(text: &str, id: &str) -> CompiledGraph {
    let def = parse_graph(text).unwrap();
    let roles = ["planner", "worker", "verifier"].iter().map(|r| (RoleId::new(*r), RoleInfo { writes: *r == "worker" })).collect();
    validate(&def, &roles, &builtin_schemas(), id, &Limits::default()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn quality(max_rounds: u32, on_exhausted: &str) -> CompiledGraph {
    let text = template_json(TEMPLATE_QUALITY_LOOP).unwrap().replace(
        r#""maxRounds": 3, "onExhausted": "ask""#,
        &format!(r#""maxRounds": {max_rounds}, "onExhausted": "{on_exhausted}""#),
    );
    compile(&text, "quality-loop")
}

fn roles() -> Vec<Role> {
    let mut r = crate::core::standard_roles();
    r.push(Role { id: RoleId::new("verifier"), enabled: true, writes: false, max_concurrent: None });
    r
}

struct Sim {
    core: OrgCore,
    log: Vec<OrgAction>,
    unknown_sends: HashSet<TaskId>,
}

impl Sim {
    fn new(g: CompiledGraph) -> Self {
        let mut core = OrgCore::new(1000);
        core.roles = roles();
        core.set_graph(g).unwrap();
        let mut s = Self { core, log: Vec::new(), unknown_sends: HashSet::new() };
        s.feed(OrgInput::Grant { granted: true });
        s
    }

    fn start(&mut self) {
        self.feed(OrgInput::StartGraph {
            workflow: WorkflowId::new("w1"),
            request: "make it".into(),
            repo: "C:/repo".into(),
            limits: Limits::default(),
            roles_fingerprint: "rf".into(),
        });
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
            OrgAction::Workspace(WorkspaceAction::CreateWorktree { task, .. }) => {
                Some(OrgInput::WorkspaceResult { task: task.clone(), result: WorkspaceOutcome::WorktreeCreated { path: format!("wt/{task}") } })
            }
            OrgAction::Workspace(WorkspaceAction::Commit { task, .. }) => {
                Some(OrgInput::WorkspaceResult { task: task.clone(), result: WorkspaceOutcome::Committed { commit: format!("c-{task}-{}", self.log.len()) } })
            }
            OrgAction::Workspace(WorkspaceAction::RefreshDetached { task, cwd, .. }) => {
                Some(OrgInput::WorkspaceResult { task: task.clone(), result: WorkspaceOutcome::WorktreeCreated { path: cwd.clone() } })
            }
            OrgAction::CheckSandbox { .. } => Some(OrgInput::SandboxChecked { ok: true, detail: "d".into() }),
            OrgAction::Send { task, attempt, .. } => {
                let outcome = if self.unknown_sends.contains(task) {
                    SettleOutcome::Unknown
                } else {
                    SettleOutcome::Accepted { turn: TurnRef::new(format!("turn-{attempt}")) }
                };
                Some(OrgInput::SendSettled { task: task.clone(), attempt: attempt.clone(), outcome })
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

    fn status(&self, id: &str) -> TaskStatus {
        self.t(id).status()
    }

    fn turn_end(&mut self, id: &str, text: &str, scan_complete: bool) {
        let (chat, turn) = {
            let t = self.t(id);
            (t.chat().unwrap().clone(), t.accepted_turn().unwrap().clone())
        };
        self.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete });
        self.feed(OrgInput::TurnEnded { chat, turn: turn.clone(), end: TurnEndKind::Completed });
        self.feed(OrgInput::FinalMessage { task: tid(id), turn, text: Some(text.to_string()) });
    }

    fn complete(&mut self, id: &str, text: &str) {
        self.turn_end(id, text, true);
    }

    fn events(&self) -> Vec<OrgEvent> {
        self.log.iter().filter_map(|a| if let OrgAction::Persist(e) = a { Some(e.clone()) } else { None }).collect()
    }

    fn count(&self, f: impl Fn(&OrgEventKind) -> bool) -> usize {
        self.events().iter().filter(|e| f(&e.kind)).count()
    }

    fn notices(&self) -> Vec<&OrgNotice> {
        self.log.iter().filter_map(|a| if let OrgAction::Notify(n) = a { Some(n) } else { None }).collect()
    }

    fn fb(&self, edge: &str) -> (u32, u32) {
        self.st().feedback_of(&EdgeId::new(edge))
    }

    /// plan and build are done; verify-1 is running.
    fn to_verify(&mut self) {
        self.start();
        self.complete("plan-1", SPEC);
        self.complete("build-1", REPORT);
        assert_eq!(self.status("verify-1"), TaskStatus::Running);
    }
}

// ---------------------------------------------------------------- loops

#[test]
fn three_rounds_then_pass_completes_the_body() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    assert_eq!(sim.status("plan-1"), TaskStatus::Reviewing);
    assert_eq!(sim.status("build-1"), TaskStatus::Reviewing);
    for round in 1..=2u32 {
        sim.complete("verify-1", REWORK);
        assert_eq!(sim.fb("verify->build"), (round, 0));
        assert_eq!(sim.status("verify-1"), TaskStatus::Rework);
        assert_eq!(sim.status("build-1"), TaskStatus::Running, "the same task goes round");
        sim.complete("build-1", REPORT);
        assert_eq!(sim.status("verify-1"), TaskStatus::Running);
    }
    assert_eq!(sim.sends("build-1"), 3);
    sim.complete("verify-1", PASS);
    for id in ["plan-1", "build-1", "verify-1"] {
        assert_eq!(sim.status(id), TaskStatus::Completed, "{id}");
    }
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
    assert_eq!(sim.fb("verify->build"), (2, 0));
    assert_eq!(sim.st().tasks().len(), 3, "no second task is made for a round");
}

impl Sim {
    fn sends(&self, id: &str) -> usize {
        self.log.iter().filter(|a| matches!(a, OrgAction::Send { task, .. } if task.0 == id)).count()
    }
}

#[test]
fn ask_at_the_limit_blocks_the_target_and_an_extra_round_adds_one() {
    let mut sim = Sim::new(quality(1, "ask"));
    sim.to_verify();
    sim.complete("verify-1", REWORK);
    sim.complete("build-1", REPORT);
    sim.complete("verify-1", REWORK);
    assert_eq!(sim.status("build-1"), TaskStatus::Blocked(BlockedReason::RoundLimit));
    assert_eq!(sim.status("verify-1"), TaskStatus::Reviewing);
    assert_eq!(sim.fb("verify->build"), (1, 0));
    // nothing goes on by itself
    let before = sim.sends("build-1");
    sim.feed(OrgInput::Tick { now_ms: 5000 });
    sim.feed(OrgInput::UserResume);
    assert_eq!(sim.sends("build-1"), before);
    assert_eq!(sim.status("build-1"), TaskStatus::Blocked(BlockedReason::RoundLimit));

    sim.feed(OrgInput::UserExtraRound { task: tid("build-1"), note: Some("handle the empty case".into()) });
    assert_eq!(sim.fb("verify->build"), (2, 1));
    assert_eq!(sim.status("build-1"), TaskStatus::Running);
    assert_eq!(sim.count(|k| matches!(k, OrgEventKind::FeedbackAllowanceRaised { by: 1, .. })), 1);
    let text = sim.log.iter().rev().find_map(|a| if let OrgAction::Send { task, text, .. } = a { (task.0 == "build-1").then(|| text.clone()) } else { None }).unwrap();
    assert!(text.contains("-----BEGIN USER NOTE-----") && text.contains("handle the empty case"));
    // the extra round is the last: the next rework asks again
    sim.complete("build-1", REPORT);
    sim.complete("verify-1", REWORK);
    assert_eq!(sim.status("build-1"), TaskStatus::Blocked(BlockedReason::RoundLimit));
}

#[test]
fn fail_at_the_limit_fails_the_target() {
    let mut sim = Sim::new(quality(1, "fail"));
    sim.to_verify();
    sim.complete("verify-1", REWORK);
    sim.complete("build-1", REPORT);
    sim.complete("verify-1", REWORK);
    assert_eq!(sim.status("build-1"), TaskStatus::Failed(FailReason::LimitReached));
    assert!(!sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
}

#[test]
fn an_outer_return_does_not_reset_the_inner_count() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    sim.complete("verify-1", REWORK);
    sim.complete("build-1", REPORT);
    assert_eq!(sim.fb("verify->build"), (1, 0));
    sim.complete("verify-1", REPLAN);
    assert_eq!(sim.fb("verify->plan"), (1, 0));
    assert_eq!(sim.fb("verify->build"), (1, 0), "the inner count stays");
    assert_eq!(sim.status("plan-1"), TaskStatus::Running);
    assert_eq!(sim.status("build-1"), TaskStatus::Rework, "waits for the new plan");
    assert_eq!(sim.status("verify-1"), TaskStatus::Rework);
    sim.complete("plan-1", SPEC);
    assert_eq!(sim.status("build-1"), TaskStatus::Running);
    sim.complete("build-1", REPORT);
    assert_eq!(sim.status("verify-1"), TaskStatus::Running);
    sim.complete("verify-1", REWORK);
    assert_eq!(sim.fb("verify->build"), (2, 0), "counted over the whole workflow");
    sim.complete("build-1", REPORT);
    sim.complete("verify-1", PASS);
    for id in ["plan-1", "build-1", "verify-1"] {
        assert_eq!(sim.status(id), TaskStatus::Completed, "{id}");
    }
}

// ---------------------------------------------------------------- safety

#[test]
fn no_edge_is_chosen_for_an_unconfirmed_task() {
    // an unknown send: the turn end is held back, nothing is decided
    let mut sim = Sim::new(quality(3, "ask"));
    sim.unknown_sends.insert(tid("plan-1"));
    sim.start();
    assert_eq!(sim.t("plan-1").confirm(), ConfirmState::SendUnknown);
    let chat = sim.t("plan-1").chat().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat: chat.clone(), working: 0, unknown: 0, scan_complete: true });
    let acts = sim.feed(OrgInput::TurnEnded { chat, turn: TurnRef::new("t"), end: TurnEndKind::Completed });
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::ReadFinalMessage { .. })));
    assert_eq!(sim.count(|k| matches!(k, OrgEventKind::SlotWritten { .. } | OrgEventKind::EdgeTaken { .. })), 0);

    // a disconnect while the children are being read: the result is not decided from it
    let mut sim = Sim::new(quality(3, "ask"));
    sim.start();
    sim.turn_end("plan-1", SPEC, false);
    sim.feed(OrgInput::Freshness { cause: ReconcileCause::Disconnect });
    assert_eq!(sim.t("plan-1").confirm(), ConfirmState::NeedsReconcile(ReconcileCause::Disconnect));
    let chat = sim.t("plan-1").chat().unwrap().clone();
    sim.feed(OrgInput::Descendants { chat, working: 0, unknown: 0, scan_complete: true });
    sim.feed(OrgInput::Tick { now_ms: 2000 });
    sim.core.settle_node(&tid("plan-1"));
    sim.core.out.clear();
    assert_eq!(sim.status("plan-1"), TaskStatus::Running);
    assert_eq!(sim.count(|k| matches!(k, OrgEventKind::SlotWritten { .. } | OrgEventKind::EdgeTaken { .. })), 0);
    assert!(sim.st().task(&tid("build-1")).is_none());
}

#[test]
fn put_slot_precedes_slot_written_and_dispatched_precedes_send() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    sim.complete("verify-1", REWORK);
    sim.complete("build-1", REPORT);
    sim.complete("verify-1", PASS);
    let mut slots = 0;
    for (i, a) in sim.log.iter().enumerate() {
        match a {
            OrgAction::Persist(ev) => {
                if let OrgEventKind::SlotWritten { slot, version, .. } = &ev.kind {
                    let put = sim.log[..i].iter().rposition(|p| matches!(p, OrgAction::PutSlot { slot: s, version: v, .. } if s == slot && v == version));
                    assert!(put.is_some(), "no PutSlot before SlotWritten #{i}");
                    assert_eq!(put.unwrap() + 1, i, "PutSlot is right before its event");
                    slots += 1;
                }
            }
            OrgAction::Send { task, attempt, .. } => {
                let ok = sim.log[..i].iter().any(|p| matches!(p, OrgAction::Persist(ev) if matches!(&ev.kind, OrgEventKind::Dispatched { task: t, attempt: at, .. } if t == task && at == attempt)));
                assert!(ok, "Send #{i} has no earlier Dispatched");
            }
            _ => {}
        }
    }
    assert_eq!(slots, 5);
}

// ---------------------------------------------------------------- restore

/// Replay the events into a state (like `restore`), and the slot values the host would have on disk.
fn rebuild(events: &[OrgEvent]) -> WorkflowState {
    let mut it = events.iter();
    let first = it.next().unwrap();
    let OrgEventKind::Created { workflow_id, limits } = &first.kind else { panic!() };
    let mut s = WorkflowState::new(workflow_id.clone(), limits.clone());
    for e in it {
        s.apply(e).unwrap();
    }
    s
}

fn slot_files(log: &[OrgAction]) -> HashMap<(SlotId, u32), Value> {
    log.iter().filter_map(|a| if let OrgAction::PutSlot { slot, version, value } = a { Some(((slot.clone(), *version), value.clone())) } else { None }).collect()
}

fn restore_sim(g: CompiledGraph, events: &[OrgEvent], slots: HashMap<(SlotId, u32), Value>) -> Sim {
    let mut state = rebuild(events);
    crate::classify_after_restart(&mut state, 5).unwrap();
    let mut core = OrgCore::restored_with(state, Some(g), slots, 1000);
    core.roles = roles();
    Sim { core, log: Vec::new(), unknown_sends: HashSet::new() }
}

#[test]
fn legacy_v1_events_restore_as_standard() {
    let lines = [
        r#"{"schemaVersion":1,"seq":0,"atMs":1,"type":"created","workflowId":"w","limits":{"max_writers":2,"max_total":3,"max_rework":2,"max_auto_replans":1,"max_turns":30,"max_tasks":20,"stagnation_minutes":30}}"#,
        r#"{"schemaVersion":1,"seq":1,"atMs":1,"type":"requested","request":"r","repo":"C:/r"}"#,
        r#"{"schemaVersion":1,"seq":2,"atMs":1,"type":"taskAdded","task":{"id":"planner-1","role":"planner","deps":[],"effects":[],"title":"plan","instructions":""}}"#,
    ]
    .join("\n");
    let read = crate::parse_events(&lines);
    assert_eq!(read.events.len(), 3, "{:?}", read.dropped_tail);
    let state = rebuild(&read.events);
    assert!(state.graph().is_none());
    assert_eq!(state.graph_id(), "standard");
    let core = OrgCore::restored_with(state, None, HashMap::new(), 0);
    assert!(!core.is_graph_mode());
    assert_eq!(core.state().unwrap().graph_id(), crate::LEGACY_GRAPH_ID);
    // a snapshot written before graphs existed has none of the new fields
    let mut v = serde_json::to_value(core.state().unwrap()).unwrap();
    for k in ["graph", "feedback", "slots", "notes"] {
        v.as_object_mut().unwrap().remove(k);
    }
    v["schemaVersion"] = json!(1);
    let old: WorkflowState = serde_json::from_value(v).unwrap();
    assert!(old.validate().is_ok());
    assert_eq!(old.graph_id(), "standard");
}

#[test]
fn a_completed_task_without_an_edge_chooses_the_same_edge_again() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.start();
    sim.complete("plan-1", SPEC);
    let events = sim.events();
    let slots = slot_files(&sim.log);
    let full_edge = events.iter().find_map(|e| if let OrgEventKind::EdgeTaken { from, edge, .. } = &e.kind { (from.0 == "plan-1").then(|| edge.clone()) } else { None }).unwrap();
    let cut_activation = events.iter().position(|e| matches!(&e.kind, OrgEventKind::ActivationAdded { node, .. } if node.0 == "build")).unwrap();

    // lost: the next task and the edge
    let mut r = restore_sim(quality(3, "ask"), &events[..cut_activation], slots.clone());
    assert_eq!(r.status("plan-1"), TaskStatus::Reviewing);
    assert_eq!(r.t("plan-1").confirm(), ConfirmState::Confirmed, "an idle task under review is not reclassified");
    r.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(r.st().task(&tid("build-1")).is_some());
    let again = r.events().iter().find_map(|e| if let OrgEventKind::EdgeTaken { edge, .. } = &e.kind { Some(edge.clone()) } else { None }).unwrap();
    assert_eq!(again, full_edge);
    assert_eq!(again, Some(EdgeId::new("plan->build")));

    // lost: only the edge. The task that exists is reused.
    let cut_edge = events.iter().position(|e| matches!(&e.kind, OrgEventKind::EdgeTaken { .. })).unwrap();
    let mut r = restore_sim(quality(3, "ask"), &events[..cut_edge], slots);
    r.feed(OrgInput::Tick { now_ms: 2000 });
    assert_eq!(r.st().tasks().iter().filter(|t| t.node().is_some_and(|n| n.0 == "build")).count(), 1);
    assert_eq!(r.count(|k| matches!(k, OrgEventKind::EdgeTaken { .. })), 1);
    assert_eq!(r.count(|k| matches!(k, OrgEventKind::ActivationAdded { .. })), 0);
}

#[test]
fn an_unreadable_slot_does_not_move_the_flow_on() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    sim.complete("verify-1", PASS);
    let events = sim.events();
    let slots = slot_files(&sim.log);
    let cut = events.iter().position(|e| matches!(&e.kind, OrgEventKind::EdgeTaken { from, .. } if from.0 == "verify-1")).unwrap();

    // the verdict file cannot be read: nothing is decided, the reason is shown
    let mut no_slots = slots.clone();
    no_slots.retain(|(s, _), _| s.0 != "verdict");
    let mut r = restore_sim(quality(3, "ask"), &events[..cut], no_slots);
    assert_eq!(r.status("verify-1"), TaskStatus::Completed);
    let acts = r.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::Persist(e) if matches!(e.kind, OrgEventKind::EdgeTaken { .. }))));
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::TaskBlocked { task, .. }) if task.0 == "verify-1")));
    assert_eq!(r.status("verify-1"), TaskStatus::Completed);
    // and it does not ask again and again
    let acts = r.feed(OrgInput::Tick { now_ms: 3000 });
    assert!(acts.is_empty());

    // with the value the same decision is made
    let mut r = restore_sim(quality(3, "ask"), &events[..cut], slots);
    r.feed(OrgInput::Tick { now_ms: 2000 });
    assert_eq!(r.count(|k| matches!(k, OrgEventKind::EdgeTaken { edge: None, .. })), 1);
}

// ---------------------------------------------------------------- gate

const GATE_GRAPH: &str = r#"{
  "schemaVersion": 1, "id": "gated", "displayName": "g", "workspace": "git", "entry": "draft",
  "slots": { "spec": { "schema": "spec" } },
  "nodes": [
    { "id": "draft", "kind": "agent", "role": "planner", "output": "spec" },
    { "id": "confirm", "kind": "gate", "prompt": "OK?", "choices": [
        { "id": "ok", "label": "ok" }, { "id": "fix", "label": "fix", "needsNote": true } ] },
    { "id": "end", "kind": "router" }
  ],
  "edges": [
    { "from": "draft", "to": "confirm" },
    { "from": "confirm", "to": "end", "when": { "slot": "confirm", "path": "choice", "eq": "ok" } },
    { "from": "confirm", "to": "draft", "when": { "slot": "confirm", "path": "choice", "eq": "fix" },
      "feedback": { "maxRounds": 2, "onExhausted": "ask" } }
  ]
}"#;

#[test]
fn a_gate_is_decided_only_by_the_user() {
    let mut sim = Sim::new(compile(GATE_GRAPH, "gated"));
    sim.start();
    sim.complete("draft-1", SPEC);
    let waiting = TaskStatus::Blocked(BlockedReason::AwaitingApproval);
    assert_eq!(sim.status("confirm-1"), waiting);
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::TaskBlocked { task, .. } if task.0 == "confirm-1")));
    for i in 0..3 {
        sim.feed(OrgInput::Tick { now_ms: 5000 + i });
    }
    sim.feed(OrgInput::UserResume);
    sim.feed(OrgInput::Grant { granted: true });
    assert_eq!(sim.status("confirm-1"), waiting);
    assert_eq!(sim.count(|k| matches!(k, OrgEventKind::GateDecided { .. }) || matches!(k, OrgEventKind::EdgeTaken { from, .. } if from.0 == "confirm-1")), 0);

    // a choice that needs a reason, without one
    sim.feed(OrgInput::UserGate { task: tid("confirm-1"), choice: "fix".into(), note: Some("  ".into()) });
    assert_eq!(sim.status("confirm-1"), waiting);
    // an unknown choice
    sim.feed(OrgInput::UserGate { task: tid("confirm-1"), choice: "nope".into(), note: None });
    assert_eq!(sim.status("confirm-1"), waiting);

    sim.feed(OrgInput::UserGate { task: tid("confirm-1"), choice: "fix".into(), note: Some("make it shorter".into()) });
    assert_eq!(sim.fb("confirm->draft"), (1, 0));
    assert_eq!(sim.status("draft-1"), TaskStatus::Running);
    let text = sim.log.iter().rev().find_map(|a| if let OrgAction::Send { task, text, .. } = a { (task.0 == "draft-1").then(|| text.clone()) } else { None }).unwrap();
    assert!(text.contains("make it shorter") && text.contains("USER NOTE"));

    sim.complete("draft-1", SPEC);
    assert_eq!(sim.status("confirm-1"), waiting, "asks again after the new output");
    sim.feed(OrgInput::UserGate { task: tid("confirm-1"), choice: "ok".into(), note: None });
    for id in ["draft-1", "confirm-1", "end-1"] {
        assert_eq!(sim.status(id), TaskStatus::Completed, "{id}");
    }
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::WorkflowCompleted)));
}

#[test]
fn graph_start_is_refused_for_a_graph_with_planned_tasks() {
    let g = {
        let def = parse_graph(template_json(crate::graph::TEMPLATE_STANDARD).unwrap()).unwrap();
        let roles = ["planner", "worker", "reviewer"].iter().map(|r| (RoleId::new(*r), RoleInfo { writes: *r == "worker" })).collect();
        validate(&def, &roles, &builtin_schemas(), "standard", &Limits::default()).unwrap()
    };
    let mut sim = Sim::new(g);
    sim.start();
    assert!(sim.core.state().is_none());
    assert!(sim.notices().iter().any(|n| matches!(n, OrgNotice::Refused { .. })));
    let _ = HaltReason::User;
}

// ---------------------------------------------------------------- writer bases, audit, roles

const TWO_WRITERS: &str = r#"{
  "schemaVersion": 1, "id": "two-writers", "displayName": "t", "workspace": "git", "entry": "plan",
  "slots": { "spec": { "schema": "spec" }, "r1": { "schema": "workerReport" }, "r2": { "schema": "workerReport" } },
  "nodes": [
    { "id": "plan", "kind": "agent", "role": "planner", "output": "spec" },
    { "id": "build", "kind": "agent", "role": "worker", "output": "r1", "capture": "commit", "reads": ["spec"] },
    { "id": "polish", "kind": "agent", "role": "worker", "output": "r2", "capture": "commit", "reads": ["r1"] }
  ],
  "edges": [ { "from": "plan", "to": "build" }, { "from": "build", "to": "polish" } ]
}"#;

fn worktree_base(sim: &Sim, task: &str) -> WorktreeBase {
    sim.log
        .iter()
        .find_map(|a| if let OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: t, base, .. }) = a { (t.0 == task).then(|| base.clone()) } else { None })
        .unwrap_or_else(|| panic!("no worktree for {task}"))
}

#[test]
fn a_writer_starts_from_the_nearest_earlier_writers_newest_commit() {
    let mut sim = Sim::new(compile(TWO_WRITERS, "two-writers"));
    sim.start();
    sim.complete("plan-1", SPEC);
    assert_eq!(worktree_base(&sim, "build-1"), WorktreeBase::Head, "no writer before it");
    sim.complete("build-1", REPORT);
    let commit = sim.t("build-1").commit().unwrap().to_string();
    assert_eq!(worktree_base(&sim, "polish-1"), WorktreeBase::FromCommit(commit));
}

#[test]
fn a_writer_is_held_when_the_earlier_writer_has_no_commit() {
    let mut sim = Sim::new(compile(TWO_WRITERS, "two-writers"));
    sim.start();
    sim.complete("plan-1", SPEC);
    sim.complete("build-1", REPORT);
    // the earlier writer's commit is lost (as after a damaged record): the later writer is not started from HEAD
    let events: Vec<OrgEvent> = sim.events().into_iter().filter(|e| !matches!(&e.kind, OrgEventKind::Committed { .. })).collect();
    let g = compile(TWO_WRITERS, "two-writers");
    let mut r = restore_sim(g.clone(), &events[..events.iter().position(|e| matches!(&e.kind, OrgEventKind::ActivationAdded { node, .. } if node.0 == "polish")).unwrap_or(events.len())], slot_files(&sim.log));
    assert!(r.t("build-1").commit().is_none());
    let base = r.core.writer_base(&g, &NodeId::new("polish"));
    assert!(base.is_err(), "{base:?}");
}

#[test]
fn the_restore_audit_holds_what_a_crash_left_unmoved_and_does_not_move_it_on() {
    // verify passes and the loops are closed, but the crash came before the members were completed
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    sim.complete("verify-1", PASS);
    let events = sim.events();
    let slots = slot_files(&sim.log);
    let last_edge = events.iter().rposition(|e| matches!(&e.kind, OrgEventKind::EdgeTaken { from, .. } if from.0 == "verify-1")).unwrap();
    // keep the edge, drop the later status changes that complete the members
    let cut: Vec<OrgEvent> = events[..=last_edge].to_vec();
    let mut r = restore_sim(quality(3, "ask"), &cut, slots.clone());
    assert_eq!(r.status("build-1"), TaskStatus::Reviewing);
    let acts = r.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::TaskBlocked { why, .. }) if why.contains("復元時の確認"))), "{acts:?}");
    assert_eq!(r.status("build-1"), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert_eq!(r.count(|k| matches!(k, OrgEventKind::EdgeTaken { .. } | OrgEventKind::FeedbackCounted { .. })), 0, "no edge is chosen again");
    // reported once
    assert!(r.feed(OrgInput::Tick { now_ms: 3000 }).is_empty());

    // an undecided edge is not reported: it is chosen again from the stored value
    let cut_edge = events.iter().position(|e| matches!(&e.kind, OrgEventKind::EdgeTaken { from, .. } if from.0 == "verify-1")).unwrap();
    let mut r = restore_sim(quality(3, "ask"), &events[..cut_edge], slots);
    let acts = r.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(!acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::TaskBlocked { why, .. }) if why.contains("復元時の確認"))));
}

#[test]
fn set_roles_asks_for_the_roles_the_graph_uses() {
    use crate::{standard_definitions, LoadedRole};
    let load = |ids: &[&str]| -> Vec<LoadedRole> {
        standard_definitions().into_iter().filter(|d| ids.contains(&d.id.as_str())).map(|def| LoadedRole { def, widened: vec![], confirmed: false }).collect()
    };
    // without a graph: the three standard roles (the built-in standard workflow)
    let mut core = OrgCore::new(0);
    assert!(core.set_roles(&load(&["planner", "worker", "reviewer"])).is_ok());
    assert!(core.set_roles(&load(&["planner", "worker"])).is_err());
    // with a graph: the roles it uses, not the standard three
    let mut core = OrgCore::new(0);
    core.set_graph(quality(3, "ask")).unwrap();
    assert!(core.set_roles(&load(&["planner", "worker", "verifier"])).is_ok());
    let e = core.set_roles(&load(&["planner", "worker", "reviewer"])).unwrap_err();
    assert!(e.contains("verifier"), "{e}");
}

#[test]
fn a_feedback_that_stopped_between_the_count_and_the_target_becoming_ready_is_held_with_a_reason() {
    let mut sim = Sim::new(quality(3, "ask"));
    sim.to_verify();
    sim.complete("verify-1", REWORK);
    let events = sim.events();
    let slots = slot_files(&sim.log);
    let counted = events.iter().position(|e| matches!(&e.kind, OrgEventKind::FeedbackCounted { .. })).unwrap();
    let to_rework = counted + events[counted..].iter().position(|e| matches!(&e.kind, OrgEventKind::StatusChanged { task, to: TaskStatus::Rework, .. } if task.0 == "build-1")).unwrap();
    let mut r = restore_sim(quality(3, "ask"), &events[..=to_rework], slots);
    assert_eq!(r.status("build-1"), TaskStatus::Rework);
    let acts = r.feed(OrgInput::Tick { now_ms: 2000 });
    assert!(acts.iter().any(|a| matches!(a, OrgAction::Notify(OrgNotice::TaskBlocked { task, why }) if task.0 == "build-1" && why.contains("差し戻しが途中で止まっています"))), "{acts:?}");
    assert_eq!(r.status("build-1"), TaskStatus::Blocked(BlockedReason::NeedsAttention));
    assert_eq!(r.count(|k| matches!(k, OrgEventKind::EdgeTaken { .. } | OrgEventKind::FeedbackCounted { .. })), 0);
    assert_eq!(r.sends("build-1"), 0, "nothing is sent");
}
