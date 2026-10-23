//! Template 1 (planner -> task table -> worker <-> reviewer), moved out of `core.rs` unchanged
//! (DESIGN_ORG_GRAPH 7.1, G-A0). The behaviour is the one `OrgCore` always had; only the place changed.
//! `NodeKind` stands in for "which node of the built-in template-1 graph is this task": the planner node
//! or a plannedTasks node (worker or reviewer). It is replaced by the graph types later.
use std::collections::HashSet;

use serde_json::Value;

use crate::core::{OrgAction, OrgCore, OrgNotice, SandboxKind, WorkspaceAction, WorktreeBase};
use crate::core::{Flow, Launch, Output, ReportRecovery};
use crate::graph::NodeId;
use crate::limits::TurnKind;
use crate::persist::{HaltReason, OrgEventKind};
use crate::policy::{evaluate_plan, Decision, PlanKind, PolicyInput};
use crate::prompt::{self, PromptDep};
use crate::scheduler::default_dep_ok;
use crate::schemas::{self, CheckOutcome, ReviewerVerdict, VerdictJudgement, VerdictKind, ROLE_PLANNER, ROLE_REVIEWER, ROLE_WORKER};
use crate::state::Cause;
use crate::types::*;

use crate::prompt::REPORT_PROMPT_CHARS as INVESTIGATION_PROMPT_CHARS;

/// The task of a plannedTasks node: what the plan's tasks do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlannedRole {
    Worker,
    Reviewer,
}

/// Node of the built-in template-1 graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Planner,
    PlannedTasks(PlannedRole),
}

pub(crate) fn node_kind(r: &RoleId) -> Option<NodeKind> {
    match r.0.as_str() {
        ROLE_PLANNER => Some(NodeKind::Planner),
        ROLE_WORKER => Some(NodeKind::PlannedTasks(PlannedRole::Worker)),
        ROLE_REVIEWER => Some(NodeKind::PlannedTasks(PlannedRole::Reviewer)),
        _ => None,
    }
}

impl OrgCore {
    pub(crate) fn kind_of(&self, id: &TaskId) -> Option<NodeKind> {
        self.task(id).and_then(|t| node_kind(&t.role))
    }

    /// The `Committed` arm of the commit result: the worker goes under review, or completes without a reviewer.
    pub(crate) fn committed(&mut self, id: &TaskId, commit: String) {
        self.rec(OrgEventKind::Committed { task: id.clone(), commit });
        if self.is_graph_task(id) {
            self.graph_committed(id);
            return;
        }
        // A reviewer the user skipped or cancelled will never review: it does not hold the worker under review.
        let has_reviewer = self.s().tasks().iter().any(|t| t.role.0 == ROLE_REVIEWER && t.deps.contains(id) && t.status() != TaskStatus::Cancelled);
        let to = if has_reviewer { TaskStatus::Reviewing } else { TaskStatus::Completed };
        if self.set_status(id, to, Cause::Engine) && to == TaskStatus::Completed {
            self.notify(OrgNotice::TaskCompleted { task: id.clone() });
        }
    }

    /// Sandbox of a new role chat when the role definition files give none.
    pub(crate) fn default_sandbox(role: &RoleId) -> SandboxKind {
        match node_kind(role) {
            Some(NodeKind::PlannedTasks(PlannedRole::Worker)) => SandboxKind::WorkspaceWrite,
            _ => SandboxKind::ReadOnly,
        }
    }

    pub(crate) fn user_replan(&mut self, reason: Option<String>) {
        let reason = reason.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
        let s = self.s();
        let planning_done = s.tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).all(|t| t.status() == TaskStatus::Completed);
        let untouched = s
            .tasks()
            .iter()
            .filter(|t| t.role.0 != ROLE_PLANNER)
            .all(|t| matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Cancelled) && t.chat().is_none());
        if !planning_done || !untouched {
            self.notify(OrgNotice::Refused { reason: "作業が始まっているため計画の作り直しはできません".into() });
            return;
        }
        let proposal: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| t.role.0 != ROLE_PLANNER && t.status() != TaskStatus::Cancelled)
            .map(|t| t.id.clone())
            .collect();
        let note: Vec<String> = proposal
            .iter()
            .filter_map(|id| self.task(id))
            .map(|t| format!("{} ({}): {}", t.id, t.role, t.title))
            .collect();
        for id in &proposal {
            self.set_status(id, TaskStatus::Cancelled, Cause::User);
        }
        let n = self.s().tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).count() + 1;
        let id = TaskId::new(format!("planner-{n}"));
        self.replan_notes.insert(id.clone(), note.join("\n"));
        // The user's reason goes to the new Planner turn as a note of its own (never mixed into the material).
        // Recorded before the task, so a restored workflow still has it.
        if let Some(text) = reason.as_deref() {
            self.add_note(text, NoteScope::Node { node: NodeId::new(id.0.clone()) });
        }
        self.clear_halt();
        self.rec(OrgEventKind::TaskAdded {
            task: PlanTask {
                id,
                role: RoleId::new(ROLE_PLANNER),
                deps: vec![],
                effects: vec![],
                title: "再計画".into(),
                instructions: String::new(),
                kind: TaskKind::Change,
            },
        });
    }

    pub(crate) fn parse_output(&self, id: &TaskId, text: &str) -> Result<Output, String> {
        if self.is_graph_task(id) {
            return self.graph_parse_output(id, text);
        }
        match self.kind_of(id) {
            Some(NodeKind::Planner) => {
                let taken: HashSet<TaskId> = self.s().tasks().iter().map(|t| t.id.clone()).collect();
                let tasks = schemas::parse_plan(text, &taken)?;
                let planners = self.s().tasks().iter().filter(|t| t.role.0 == ROLE_PLANNER).count();
                let kind = if planners <= 1 {
                    PlanKind::Initial
                } else {
                    // A user-requested re-plan is always put to the user.
                    PlanKind::Replan { scope_changed: true, auto_replans_used: self.s().counters().auto_replans_used }
                };
                let decision = evaluate_plan(&PolicyInput {
                    tasks: &tasks,
                    roles: &self.roles,
                    limits: self.s().limits(),
                    kind,
                    requested_parallelism: None,
                    turns_remaining: self.turns_remaining(),
                })
                .map_err(|errs| format!("計画の検証に失敗しました: {errs:?}"))?;
                Ok(Output::Plan { tasks, decision })
            }
            Some(NodeKind::PlannedTasks(PlannedRole::Worker)) if self.task(id).is_some_and(Task::is_investigation) => {
                Ok(Output::Investigation(schemas::parse_investigation_report(text)?))
            }
            Some(NodeKind::PlannedTasks(PlannedRole::Worker)) => Ok(Output::Report(schemas::parse_worker_report(text)?)),
            Some(NodeKind::PlannedTasks(PlannedRole::Reviewer)) => Ok(Output::Verdict(schemas::parse_verdict(text)?)),
            None => Err("未知の役割です".into()),
        }
    }

    pub(crate) fn complete_output(&mut self, id: &TaskId, out: Output) {
        match out {
            Output::Plan { tasks, decision } => {
                let n = tasks.len();
                for t in tasks {
                    self.rec(OrgEventKind::TaskAdded { task: t });
                }
                match decision {
                    Decision::AutoApprove => self.notify(OrgNotice::PlanAutoApproved { tasks: n }),
                    Decision::NeedsConfirmation(reasons) => {
                        self.rec(OrgEventKind::Halted { reason: Some(HaltReason::AwaitingPlanApproval) });
                        self.notify(OrgNotice::PlanNeedsApproval { reasons });
                    }
                }
                if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            Output::Report(r) => {
                let attempt = self.task(id).map_or(0, |t| t.rework_count());
                let value = serde_json::json!({
                    "summary": r.summary,
                    "changedFiles": r.changed_files,
                    "tests": { "ran": r.tests.ran, "passed": r.tests.passed },
                });
                self.out.push(OrgAction::PutOutput { task: id.clone(), attempt, value });
                self.test_claims.insert(id.clone(), (r.tests.ran, r.tests.passed));
                self.artifacts.insert(id.clone(), r.summary);
                self.flow_set(id.clone(), Flow::WantCommit);
            }
            Output::Investigation(r) => {
                // No commit: the report is the result. It is written (and the write must succeed) before it is recorded.
                let attempt = self.task(id).map_or(0, |t| t.rework_count());
                let value = serde_json::json!({
                    "summary": r.summary,
                    "findings": r.findings.iter().map(|f| serde_json::json!({ "point": f.point, "basis": f.basis, "refs": f.refs })).collect::<Vec<_>>(),
                    "references": r.references,
                    "openQuestions": r.open_questions,
                });
                let bytes = u32::try_from(value.to_string().len()).unwrap_or(u32::MAX);
                self.out.push(OrgAction::PutReport { task: id.clone(), attempt, value });
                self.artifacts.insert(id.clone(), schemas::render_investigation(&r, INVESTIGATION_PROMPT_CHARS));
                if self.rec(OrgEventKind::ReportRecorded { task: id.clone(), attempt, bytes }) && self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            Output::Verdict(v) => {
                // The verdict belongs to the reviewed worker's current attempt (read before a send-back counts it).
                let attempt = self.task(id).and_then(|t| t.deps.first().cloned()).and_then(|d| self.task(&d).map(|t| t.rework_count())).unwrap_or(0);
                let value = serde_json::json!({
                    "verdict": match v.verdict {
                        VerdictKind::Pass => "pass",
                        VerdictKind::Rework => "rework",
                    },
                    "findings": v.findings,
                    "checks": v.checks.as_ref().map(|cs| {
                        cs.iter()
                            .map(|c| {
                                serde_json::json!({
                                    "command": c.command,
                                    "outcome": match c.outcome {
                                        CheckOutcome::Passed => "passed",
                                        CheckOutcome::Failed => "failed",
                                        CheckOutcome::NotRun => "notRun",
                                    },
                                    "summary": c.summary,
                                })
                            })
                            .collect::<Vec<_>>()
                    }),
                    "noCheckReason": v.no_check_reason,
                });
                self.out.push(OrgAction::PutOutput { task: id.clone(), attempt, value });
                self.complete_verdict(id, v)
            }
            Output::Slot { slot, value } => self.graph_complete_slot(id, slot, value),
        }
    }

    pub(crate) fn complete_verdict(&mut self, id: &TaskId, v: ReviewerVerdict) {
        let target = self.task(id).and_then(|t| t.deps.first().cloned());
        let Some(target) = target else {
            self.block(id, BlockedReason::NeedsAttention, "検証対象がありません");
            return;
        };
        match schemas::judge_verdict(&v) {
            VerdictJudgement::Pass { .. } => {
                // The reviewed task completes only now; then the reviewer itself.
                if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.set_status(&target, TaskStatus::Completed, Cause::Engine);
                    self.notify(OrgNotice::TaskCompleted { task: target });
                }
                if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            VerdictJudgement::PassContradicted { failed } => {
                // The reviewer said pass but a check it ran failed: the pass is held back and the user decides
                // (not an automatic send-back, not counted as one; DESIGN_ORG_R4 2.2).
                if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.rec(OrgEventKind::FindingsRecorded { task: target.clone(), findings: schemas::failed_check_findings(&v) });
                    self.rec(OrgEventKind::VerdictDemoted { task: id.clone(), target: target.clone(), failed });
                    self.block(
                        &target,
                        BlockedReason::CheckFailed,
                        "検証のコマンドが失敗したため、合格を保留しました。もう一度直させる／このまま受け入れる／失敗として確定のどれかを選んでください",
                    );
                }
                // The reviewer waits until the worker task is under review again.
                self.set_status(id, TaskStatus::Rework, Cause::Engine);
            }
            VerdictJudgement::Rework => {
                let limit_reached = {
                    let limits = self.s().limits().clone();
                    self.task(&target).is_some_and(|t| t.rework_limit_reached(&limits))
                };
                if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    // The findings are kept in the log (a restart must not lose them).
                    let findings: Vec<String> = v.findings.into_iter().filter(|f| !f.trim().is_empty()).collect();
                    self.rec(OrgEventKind::FindingsRecorded { task: target.clone(), findings });
                    if limit_reached {
                        // Not a failure: the user chooses (once more / accept as it is / settle as failed).
                        let max = self.s().limits().max_rework;
                        self.block(
                            &target,
                            BlockedReason::ReworkLimit,
                            &format!("検証で不合格が続き、差し戻しの上限（{max}回）に達しました。もう一度直させる／このまま受け入れる／失敗として確定のどれかを選んでください"),
                        );
                    } else {
                        self.rec(OrgEventKind::ReworkCounted { task: target.clone() });
                        self.set_status(&target, TaskStatus::Rework, Cause::Engine);
                        self.set_status(&target, TaskStatus::Ready, Cause::Engine);
                    }
                }
                // The reviewer waits until the worker task is under review again.
                self.set_status(id, TaskStatus::Rework, Cause::Engine);
            }
        }
    }

    pub(crate) fn dep_ok(dep: &Task, dependent: &Task) -> bool {
        if dependent.node.is_none() && dep.skipped() && dep.status() == TaskStatus::Cancelled {
            // The user skipped it: it counts as satisfied (its result is simply absent; the prompt says so).
            return true;
        }
        if dependent.node.is_some() {
            // Graph node: the parent has produced (and fixed) its output, whether or not a loop still holds it open.
            dep.confirm() == ConfirmState::Confirmed && matches!(dep.status(), TaskStatus::Completed | TaskStatus::Reviewing)
        } else if dependent.role.0 == ROLE_REVIEWER {
            // A reviewer may start once the reviewed task is under review (committed).
            dep.confirm() == ConfirmState::Confirmed
                && (dep.status() == TaskStatus::Completed || (dep.status() == TaskStatus::Reviewing && dep.commit().is_some()))
        } else {
            default_dep_ok(dep, dependent)
        }
    }

    pub(crate) fn launch(&mut self, id: &TaskId) {
        if self.is_graph_task(id) {
            self.graph_launch(id);
            return;
        }
        let Some(t) = self.task(id) else { return };
        let Some(kind) = node_kind(&t.role) else { return };
        let (has_chat, has_cwd) = (t.chat().is_some(), t.chat_cwd().is_some());
        let investigation = t.is_investigation();
        let first_turn_worker = kind == NodeKind::PlannedTasks(PlannedRole::Worker) && t.accepted_turn().is_none() && !t.retry_pending();
        // A task that uses an investigation report is not sent without it (DESIGN_ORG_R4 1.6).
        if first_turn_worker && !self.ensure_dep_reports(id) {
            return;
        }
        let Some(t) = self.task(id) else { return };
        let target_commit = t.deps.first().and_then(|d| self.task(d)).and_then(|d| d.commit().map(str::to_string));
        if has_chat {
            let rereview = kind == NodeKind::PlannedTasks(PlannedRole::Reviewer) && self.task(id).is_some_and(|t| t.accepted_turn().is_some());
            if rereview {
                // The reviewer's chat is tied to its worktree: move that to the new commit first, and send nothing
                // until it is verified to be there (R-ORG M1).
                let (Some(commit), Some(cwd)) = (target_commit, self.task(id).and_then(|t| t.chat_cwd().map(str::to_string))) else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットまたは作業場所がありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Refresh);
                self.out.push(OrgAction::Workspace(WorkspaceAction::RefreshDetached { task: id.clone(), cwd, commit }));
                return;
            }
            self.continue_launch(id);
            return;
        }
        if has_cwd {
            // A creation was started earlier and its result was lost (e.g. restart).
            self.block_unknown_create(id);
            return;
        }
        // A new chat for a task whose earlier chat was never used: its working folder (worktree) is reused.
        if let Some(cwd) = self.rechat_cwd.remove(id) {
            self.begin_chat(id, cwd);
            return;
        }
        let repo = self.s().repo().to_string();
        match kind {
            NodeKind::Planner => self.begin_chat(id, repo),
            NodeKind::PlannedTasks(PlannedRole::Worker) => {
                self.launching.insert(id.clone(), Launch::Worktree);
                // An investigation reads the same HEAD a change task starts from, but on a detached read-only worktree
                // (no branch, nothing to commit).
                let base = if investigation { WorktreeBase::DetachedAt("HEAD".into()) } else { WorktreeBase::Head };
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: id.clone(), repo, base }));
            }
            NodeKind::PlannedTasks(PlannedRole::Reviewer) => {
                let Some(commit) = target_commit else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットがありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Worktree);
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree {
                    task: id.clone(),
                    repo,
                    base: WorktreeBase::DetachedAt(commit),
                }));
            }
        }
    }

    /// Turn kind and text for a ready task.
    pub(crate) fn ready_turn(&self, id: &TaskId) -> Option<(TurnKind, String)> {
        // A new attempt after a failed / interrupted turn (graph and plan alike): a short continuation, no repeated request.
        if let Some(t) = self.task(id).filter(|t| t.retry_pending()) {
            return Some((TurnKind::Retry, prompt::retry_prompt(crate::core::retry_reason_of(t), &t.title)));
        }
        if self.is_graph_task(id) {
            return self.graph_ready_turn(id);
        }
        let t = self.task(id)?;
        let s = self.s();
        match node_kind(&t.role)? {
            NodeKind::Planner => {
                let first = s.tasks().iter().filter(|x| x.role.0 == ROLE_PLANNER).count() <= 1;
                if first {
                    Some((TurnKind::Plan, prompt::planner_prompt(s.request(), s.repo())))
                } else {
                    let prev = self.replan_notes.get(id).cloned().unwrap_or_else(|| "(前の計画は未取得)".into());
                    // The user's reason (a note of this Planner task) is added by the common note section.
                    Some((TurnKind::Replan, prompt::replan_prompt(s.request(), s.repo(), &prev, &[])))
                }
            }
            NodeKind::PlannedTasks(PlannedRole::Worker) => {
                if t.accepted_turn().is_some() && !t.is_investigation() {
                    Some((TurnKind::Rework, prompt::worker_rework_prompt(t, Some(t.last_findings()))))
                } else {
                    let deps: Vec<PromptDep> = t
                        .deps
                        .iter()
                        .map(|d| PromptDep { id: d.clone(), text: self.artifacts.get(d).cloned(), investigation: self.task(d).is_some_and(Task::is_investigation) })
                        .collect();
                    let skipped: Vec<TaskId> = t.deps.iter().filter(|d| self.task(d).is_some_and(|x| x.skipped())).cloned().collect();
                    if t.is_investigation() {
                        return Some((TurnKind::Task, prompt::investigate_prompt(t, s.request(), &deps, &skipped)));
                    }
                    // The changes of earlier change tasks are not in this task's working folder.
                    let unincluded: Vec<TaskId> = t
                        .deps
                        .iter()
                        .filter(|d| !skipped.contains(d))
                        .filter(|d| self.task(d).is_some_and(|x| x.role.0 == ROLE_WORKER && x.node().is_none() && !x.is_investigation()))
                        .cloned()
                        .collect();
                    Some((TurnKind::Task, prompt::worker_prompt_ext(t, s.request(), &deps, &skipped, &unincluded)))
                }
            }
            NodeKind::PlannedTasks(PlannedRole::Reviewer) => {
                let target = self.task(t.deps.first()?)?;
                let commit = target.commit()?;
                let summary = self.artifacts.get(&target.id).map(String::as_str);
                let claim = self.test_claims.get(&target.id).copied();
                Some((TurnKind::Review, prompt::reviewer_prompt(t, s.request(), (&target.id, summary, commit), target.rework_count() + 1, claim)))
            }
        }
    }

    pub(crate) fn schema_for(&self, id: &TaskId) -> Value {
        if self.is_graph_task(id) {
            return self.graph_schema_for(id);
        }
        match self.kind_of(id) {
            Some(NodeKind::Planner) => schemas::planner_plan_schema_with_kind(),
            Some(NodeKind::PlannedTasks(PlannedRole::Reviewer)) => schemas::reviewer_verdict_schema_checked(),
            _ if self.task(id).is_some_and(Task::is_investigation) => schemas::investigation_report_schema(),
            _ => schemas::worker_report_schema(),
        }
    }

    /// Before a worker task is sent: every investigation it depends on has its report in hand. A missing one is
    /// read once from the final message of the investigation's accepted turn (read only; nothing is resumed).
    /// Returns true when the task may go on; otherwise it waits (`Launch::Report`) or is blocked.
    pub(crate) fn ensure_dep_reports(&mut self, id: &TaskId) -> bool {
        let Some(t) = self.task(id) else { return false };
        let missing: Vec<TaskId> = t
            .deps
            .iter()
            .filter(|d| self.task(d).is_some_and(|x| x.is_investigation() && !x.skipped()))
            .filter(|d| !self.artifacts.contains_key(*d))
            .cloned()
            .collect();
        let Some(dep) = missing.first().cloned() else { return true };
        let why = format!("調査 {dep} の報告を読めません。調査タスクの結果を確かめてから、やり直してください");
        match self.report_recovery.get(&dep) {
            Some(ReportRecovery::Failed) => self.block(id, BlockedReason::NeedsAttention, &why),
            Some(ReportRecovery::Reading { .. }) => {
                self.launching.insert(id.clone(), Launch::Report);
            }
            None => {
                let found = self.task(&dep).and_then(|d| Some((d.chat().cloned()?, d.accepted_turn().cloned()?)));
                match found {
                    Some((chat, turn)) => {
                        self.report_recovery.insert(dep.clone(), ReportRecovery::Reading { turn: turn.clone() });
                        self.launching.insert(id.clone(), Launch::Report);
                        self.out.push(OrgAction::ReadFinalMessage { task: dep, chat, turn });
                    }
                    None => {
                        self.report_recovery.insert(dep, ReportRecovery::Failed);
                        self.block(id, BlockedReason::NeedsAttention, &why);
                    }
                }
            }
        }
        false
    }

    /// The final message of an investigation (for a later task that lacked its report) arrived, or could not be read.
    pub(crate) fn report_recovered(&mut self, dep: &TaskId, text: Option<String>) {
        let waiting: Vec<TaskId> = self
            .launching
            .iter()
            .filter(|(_, l)| **l == Launch::Report)
            .map(|(t, _)| t.clone())
            .filter(|t| self.task(t).is_some_and(|x| x.deps.contains(dep)))
            .collect();
        let report = text.as_deref().and_then(|t| schemas::parse_investigation_report(t).ok());
        match report {
            Some(r) => {
                self.artifacts.insert(dep.clone(), schemas::render_investigation(&r, INVESTIGATION_PROMPT_CHARS));
                self.report_recovery.remove(dep);
                for w in waiting {
                    self.launching.remove(&w);
                    if self.task(&w).is_some_and(|t| t.status() == TaskStatus::Ready) {
                        self.launch(&w);
                    }
                }
            }
            None => {
                self.report_recovery.insert(dep.clone(), ReportRecovery::Failed);
                for w in waiting {
                    self.launching.remove(&w);
                    self.block(&w, BlockedReason::NeedsAttention, &format!("調査 {dep} の報告を読めません。調査タスクの結果を確かめてから、やり直してください"));
                }
            }
        }
    }
}
