//! Template 1 (planner -> task table -> worker <-> reviewer), moved out of `core.rs` unchanged
//! (DESIGN_ORG_GRAPH 7.1, G-A0). The behaviour is the one `OrgCore` always had; only the place changed.
//! `NodeKind` stands in for "which node of the built-in template-1 graph is this task": the planner node
//! or a plannedTasks node (worker or reviewer). It is replaced by the graph types later.
use std::collections::HashSet;

use serde_json::Value;

use crate::core::{OrgAction, OrgCore, OrgNotice, SandboxKind, WorkspaceAction, WorktreeBase};
use crate::core::{Flow, Launch, Output};
use crate::limits::TurnKind;
use crate::persist::{HaltReason, OrgEventKind};
use crate::policy::{evaluate_plan, Decision, PlanKind, PolicyInput};
use crate::prompt;
use crate::scheduler::default_dep_ok;
use crate::schemas::{self, ReviewerVerdict, VerdictKind, ROLE_PLANNER, ROLE_REVIEWER, ROLE_WORKER};
use crate::state::Cause;
use crate::types::*;

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
        let has_reviewer = self.s().tasks().iter().any(|t| t.role.0 == ROLE_REVIEWER && t.deps.contains(id));
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

    pub(crate) fn user_replan(&mut self) {
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
        self.clear_halt();
        self.rec(OrgEventKind::TaskAdded {
            task: PlanTask {
                id,
                role: RoleId::new(ROLE_PLANNER),
                deps: vec![],
                effects: vec![],
                title: "再計画".into(),
                instructions: String::new(),
            },
        });
    }

    pub(crate) fn parse_output(&self, id: &TaskId, text: &str) -> Result<Output, String> {
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
                self.artifacts.insert(id.clone(), r.summary);
                self.flow_set(id.clone(), Flow::WantCommit);
            }
            Output::Verdict(v) => self.complete_verdict(id, v),
        }
    }

    pub(crate) fn complete_verdict(&mut self, id: &TaskId, v: ReviewerVerdict) {
        let target = self.task(id).and_then(|t| t.deps.first().cloned());
        let Some(target) = target else {
            self.block(id, BlockedReason::NeedsAttention, "検証対象がありません");
            return;
        };
        match v.verdict {
            VerdictKind::Pass => {
                // The reviewed task completes only now; then the reviewer itself.
                if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.set_status(&target, TaskStatus::Completed, Cause::Engine);
                    self.notify(OrgNotice::TaskCompleted { task: target });
                }
                if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
                    self.notify(OrgNotice::TaskCompleted { task: id.clone() });
                }
            }
            VerdictKind::Rework => {
                let limit_reached = {
                    let limits = self.s().limits().clone();
                    self.task(&target).is_some_and(|t| t.rework_limit_reached(&limits))
                };
                if limit_reached {
                    self.fail(&target, FailReason::LimitReached);
                } else if self.task(&target).is_some_and(|t| t.status() == TaskStatus::Reviewing) {
                    self.rec(OrgEventKind::ReworkCounted { task: target.clone() });
                    self.set_status(&target, TaskStatus::Rework, Cause::Engine);
                    self.set_status(&target, TaskStatus::Ready, Cause::Engine);
                    self.findings.insert(target.clone(), v.findings.into_iter().filter(|f| !f.trim().is_empty()).collect());
                }
                // The reviewer waits until the worker task is under review again.
                self.set_status(id, TaskStatus::Rework, Cause::Engine);
            }
        }
    }

    pub(crate) fn dep_ok(dep: &Task, dependent: &Task) -> bool {
        if dependent.role.0 == ROLE_REVIEWER {
            // A reviewer may start once the reviewed task is under review (committed).
            dep.confirm() == ConfirmState::Confirmed
                && (dep.status() == TaskStatus::Completed || (dep.status() == TaskStatus::Reviewing && dep.commit().is_some()))
        } else {
            default_dep_ok(dep, dependent)
        }
    }

    pub(crate) fn launch(&mut self, id: &TaskId) {
        let Some(t) = self.task(id) else { return };
        let Some(kind) = node_kind(&t.role) else { return };
        let (has_chat, has_cwd) = (t.chat().is_some(), t.chat_cwd().is_some());
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
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: id.clone(), repo, base: WorktreeBase::Head }));
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
        let t = self.task(id)?;
        let s = self.s();
        match node_kind(&t.role)? {
            NodeKind::Planner => {
                let first = s.tasks().iter().filter(|x| x.role.0 == ROLE_PLANNER).count() <= 1;
                if first {
                    Some((TurnKind::Plan, prompt::planner_prompt(s.request(), s.repo())))
                } else {
                    let prev = self.replan_notes.get(id).cloned().unwrap_or_else(|| "(前の計画は未取得)".into());
                    Some((TurnKind::Replan, prompt::replan_prompt(s.request(), s.repo(), &prev)))
                }
            }
            NodeKind::PlannedTasks(PlannedRole::Worker) => {
                if t.accepted_turn().is_some() {
                    let f = self.findings.get(id).map(Vec::as_slice);
                    Some((TurnKind::Rework, prompt::worker_rework_prompt(t, f)))
                } else {
                    let deps: Vec<(TaskId, Option<String>)> = t.deps.iter().map(|d| (d.clone(), self.artifacts.get(d).cloned())).collect();
                    Some((TurnKind::Task, prompt::worker_prompt(t, s.request(), &deps)))
                }
            }
            NodeKind::PlannedTasks(PlannedRole::Reviewer) => {
                let target = self.task(t.deps.first()?)?;
                let commit = target.commit()?;
                let summary = self.artifacts.get(&target.id).map(String::as_str);
                Some((TurnKind::Review, prompt::reviewer_prompt(t, s.request(), (&target.id, summary, commit), target.rework_count() + 1)))
            }
        }
    }

    pub(crate) fn schema_for(&self, id: &TaskId) -> Value {
        match self.kind_of(id) {
            Some(NodeKind::Planner) => schemas::planner_plan_schema(),
            Some(NodeKind::PlannedTasks(PlannedRole::Reviewer)) => schemas::reviewer_verdict_schema(),
            _ => schemas::worker_report_schema(),
        }
    }
}
