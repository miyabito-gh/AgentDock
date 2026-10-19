//! What the user may do to a single task of the plan route: skip, send back, accept as it is (DESIGN_ORG_R2 2.3).
//! Pure checks that the screen also uses for its labels and confirmation texts. The core adds the conditions that
//! depend on its bookkeeping (nothing launching, no flow running).
use crate::persist::WorkflowState;
use crate::schemas::{ROLE_REVIEWER, ROLE_WORKER};
use crate::types::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipPlan {
    /// Every task to skip (the asked one first).
    pub skip: Vec<TaskId>,
    /// A worker under review whose only reviewer is skipped: it completes without a review.
    pub complete_unreviewed: Option<TaskId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendBackPlan {
    /// Reviewers waiting `Ready` go back to `Pending`.
    pub to_pending: Vec<TaskId>,
    /// Reviewers `Blocked` go to the waiting state `Rework`.
    pub to_rework: Vec<TaskId>,
}

fn terminal(t: &Task) -> bool {
    matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)
}

fn attempt_open(t: &Task) -> bool {
    t.last_attempt().is_some() && matches!(t.last_settle(), None | Some(SettleOutcome::Unknown))
}

pub fn is_worker(t: &Task) -> bool {
    t.node().is_none() && t.role.0 == ROLE_WORKER
}

pub fn is_reviewer(t: &Task) -> bool {
    t.node().is_none() && t.role.0 == ROLE_REVIEWER
}

/// The reviewers that check `worker` (all of them, finished ones too).
pub fn reviewers_of<'a>(state: &'a WorkflowState, worker: &TaskId) -> Vec<&'a Task> {
    state.tasks().iter().filter(|t| is_reviewer(t) && t.deps.contains(worker)).collect()
}

/// A task that has not started anything that is still unsettled and waits for the user or the scheduler.
fn skippable_state(t: &Task) -> Result<(), String> {
    if !matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Blocked(_) | TaskStatus::Rework) {
        return Err(match t.status() {
            TaskStatus::Running => format!("{} は実行中のためスキップできません", t.id),
            _ => format!("{} はスキップできる状態ではありません", t.id),
        });
    }
    if t.confirm() != ConfirmState::Confirmed {
        return Err(format!("{} は送信または停止の結果を確認できていないためスキップできません", t.id));
    }
    if attempt_open(t) {
        return Err(format!("{} は送信の結果が確定していないためスキップできません", t.id));
    }
    if t.chat_cwd().is_some() && t.chat().is_none() {
        return Err(format!("{} は役割チャットを作成中のためスキップできません", t.id));
    }
    Ok(())
}

pub fn skip_plan(state: &WorkflowState, task: &TaskId) -> Result<SkipPlan, String> {
    let t = state.task(task).ok_or_else(|| "タスクが見つかりません".to_string())?;
    if !(is_worker(t) || is_reviewer(t)) {
        return Err("このタスクはスキップできません（計画の作業・検証タスクだけです）".into());
    }
    skippable_state(t)?;
    if is_worker(t) {
        let mut skip = vec![t.id.clone()];
        for r in reviewers_of(state, &t.id).into_iter().filter(|r| !terminal(r)) {
            skippable_state(r).map_err(|e| format!("検証役も一緒に飛ばす必要がありますが、{e}"))?;
            skip.push(r.id.clone());
        }
        return Ok(SkipPlan { skip, complete_unreviewed: None });
    }
    // A reviewer: its worker is completed without a review when this was the only live one.
    let mut complete_unreviewed = None;
    if let Some(w) = t.deps.first().and_then(|d| state.task(d)).filter(|w| is_worker(w)) {
        if w.status() == TaskStatus::Reviewing {
            if w.confirm() != ConfirmState::Confirmed {
                return Err(format!("検証対象の {} の確認状態が確定していないためスキップできません", w.id));
            }
            let others = reviewers_of(state, &w.id).into_iter().any(|r| r.id != t.id && !terminal(r));
            if !others {
                complete_unreviewed = Some(w.id.clone());
            }
        }
    }
    Ok(SkipPlan { skip: vec![t.id.clone()], complete_unreviewed })
}

/// Whether a worker can be sent back by the user, and what happens to its reviewers.
/// `note`: the user's text (required unless the task stopped at the send-back limit).
pub fn send_back_check(state: &WorkflowState, task: &TaskId, turns_remaining: u32, note: Option<&str>) -> Result<SendBackPlan, String> {
    let t = state.task(task).ok_or_else(|| "タスクが見つかりません".to_string())?;
    if !is_worker(t) {
        return Err("このタスクは差し戻せません（計画の作業タスクだけです）".into());
    }
    let at_limit = t.status() == TaskStatus::Blocked(BlockedReason::ReworkLimit);
    if !(t.status() == TaskStatus::Reviewing || at_limit) {
        return Err(if t.status() == TaskStatus::Completed {
            "完了したタスクは差し戻せません。後続のタスクへは追加指示で伝えてください".to_string()
        } else {
            "このタスクは差し戻せる状態ではありません（検証待ち、または差し戻し上限で止まっている間だけです）".to_string()
        });
    }
    if t.confirm() != ConfirmState::Confirmed {
        return Err("確認状態が確定していないため差し戻せません".into());
    }
    if note.is_none_or(|n| n.trim().is_empty()) && !at_limit {
        return Err("差し戻しの指摘を書いてください".into());
    }
    if turns_remaining == 0 {
        return Err("turn上限に達しているため差し戻せません".into());
    }
    let mut plan = SendBackPlan { to_pending: vec![], to_rework: vec![] };
    for r in reviewers_of(state, &t.id).into_iter().filter(|r| !terminal(r)) {
        if r.confirm() != ConfirmState::Confirmed || attempt_open(r) {
            return Err(format!("検証役 {} の送信結果を確認できていないため差し戻せません", r.id));
        }
        match r.status() {
            TaskStatus::Running | TaskStatus::Reviewing => return Err(format!("検証役 {} が実行中のため差し戻せません", r.id)),
            TaskStatus::Ready => plan.to_pending.push(r.id.clone()),
            TaskStatus::Blocked(_) => plan.to_rework.push(r.id.clone()),
            _ => {}
        }
    }
    Ok(plan)
}

/// "Accept as it is" for a worker stopped at the send-back limit: the reviewers that are still waiting are skipped.
pub fn accept_as_is_check(state: &WorkflowState, task: &TaskId) -> Result<Vec<TaskId>, String> {
    let t = state.task(task).ok_or_else(|| "タスクが見つかりません".to_string())?;
    if !is_worker(t) || t.status() != TaskStatus::Blocked(BlockedReason::ReworkLimit) {
        return Err("差し戻し上限で止まっている作業タスクだけ、このまま受け入れられます".into());
    }
    if t.confirm() != ConfirmState::Confirmed {
        return Err("確認状態が確定していないため受け入れられません".into());
    }
    let mut out = Vec::new();
    for r in reviewers_of(state, &t.id).into_iter().filter(|r| !terminal(r)) {
        skippable_state(r)?;
        out.push(r.id.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::OrgEventKind;
    use crate::state::Cause;

    fn st(spec: &[(&str, &str, &[&str], TaskStatus)]) -> WorkflowState {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        for (id, role, deps, status) in spec {
            s.record(
                OrgEventKind::TaskAdded {
                    task: PlanTask {
                        id: TaskId::new(*id),
                        role: RoleId::new(*role),
                        deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
                        effects: vec![],
                        title: String::new(),
                        instructions: String::new(),
                    },
                },
                0,
            )
            .unwrap();
            let t = s.tasks.iter_mut().find(|t| t.id.0 == *id).unwrap();
            t.status = *status;
        }
        s
    }
    fn tid(s: &str) -> TaskId {
        TaskId::new(s)
    }

    #[test]
    fn skipping_a_worker_takes_its_reviewer() {
        let s = st(&[("w", "worker", &[], TaskStatus::Pending), ("r", "reviewer", &["w"], TaskStatus::Pending)]);
        let p = skip_plan(&s, &tid("w")).unwrap();
        assert_eq!(p.skip, vec![tid("w"), tid("r")]);
        assert_eq!(p.complete_unreviewed, None);
    }

    #[test]
    fn skipping_the_only_reviewer_completes_a_reviewing_worker_unreviewed() {
        let s = st(&[("w", "worker", &[], TaskStatus::Reviewing), ("r", "reviewer", &["w"], TaskStatus::Ready)]);
        let p = skip_plan(&s, &tid("r")).unwrap();
        assert_eq!(p, SkipPlan { skip: vec![tid("r")], complete_unreviewed: Some(tid("w")) });
        // a second live reviewer keeps the worker under review
        let s = st(&[("w", "worker", &[], TaskStatus::Reviewing), ("r", "reviewer", &["w"], TaskStatus::Ready), ("r2", "reviewer", &["w"], TaskStatus::Pending)]);
        assert_eq!(skip_plan(&s, &tid("r")).unwrap().complete_unreviewed, None);
    }

    #[test]
    fn skip_refusals() {
        for status in [TaskStatus::Running, TaskStatus::Reviewing, TaskStatus::Completed, TaskStatus::Cancelled] {
            let s = st(&[("w", "worker", &[], status)]);
            assert!(skip_plan(&s, &tid("w")).is_err(), "{status:?}");
        }
        let mut s = st(&[("w", "worker", &[], TaskStatus::Ready)]);
        s.tasks[0].confirm = ConfirmState::SendUnknown;
        assert!(skip_plan(&s, &tid("w")).is_err());
        let mut s = st(&[("w", "worker", &[], TaskStatus::Blocked(BlockedReason::NeedsAttention))]);
        s.tasks[0].last_attempt = Some(AttemptRef::new("a"));
        assert!(skip_plan(&s, &tid("w")).is_err());
        let mut s = st(&[("w", "worker", &[], TaskStatus::Ready)]);
        s.tasks[0].chat_cwd = Some("c".into());
        assert!(skip_plan(&s, &tid("w")).is_err(), "chat being created");
        // a running reviewer stops the whole skip
        let s = st(&[("w", "worker", &[], TaskStatus::Pending), ("r", "reviewer", &["w"], TaskStatus::Running)]);
        assert!(skip_plan(&s, &tid("w")).is_err());
        // graph tasks and planners are out
        let mut s = st(&[("w", "worker", &[], TaskStatus::Ready)]);
        s.tasks[0].node = Some(crate::graph::def::NodeId::new("n"));
        assert!(skip_plan(&s, &tid("w")).is_err());
        let s = st(&[("p", "planner", &[], TaskStatus::Ready)]);
        assert!(skip_plan(&s, &tid("p")).is_err());
    }

    #[test]
    fn send_back_rules() {
        let s = st(&[("w", "worker", &[], TaskStatus::Reviewing), ("r", "reviewer", &["w"], TaskStatus::Ready)]);
        assert!(send_back_check(&s, &tid("w"), 5, None).is_err(), "a note is required");
        assert!(send_back_check(&s, &tid("w"), 0, Some("x")).is_err(), "no turns");
        let p = send_back_check(&s, &tid("w"), 5, Some("fix")).unwrap();
        assert_eq!(p, SendBackPlan { to_pending: vec![tid("r")], to_rework: vec![] });
        // at the limit the note is optional and a blocked reviewer waits
        let s = st(&[("w", "worker", &[], TaskStatus::Blocked(BlockedReason::ReworkLimit)), ("r", "reviewer", &["w"], TaskStatus::Blocked(BlockedReason::Manual))]);
        assert_eq!(send_back_check(&s, &tid("w"), 5, None).unwrap().to_rework, vec![tid("r")]);
        // a running reviewer, a completed worker
        let s = st(&[("w", "worker", &[], TaskStatus::Reviewing), ("r", "reviewer", &["w"], TaskStatus::Running)]);
        assert!(send_back_check(&s, &tid("w"), 5, Some("x")).is_err());
        let s = st(&[("w", "worker", &[], TaskStatus::Completed)]);
        assert!(send_back_check(&s, &tid("w"), 5, Some("x")).is_err());
    }

    #[test]
    fn accept_as_is_skips_waiting_reviewers() {
        let s = st(&[("w", "worker", &[], TaskStatus::Blocked(BlockedReason::ReworkLimit)), ("r", "reviewer", &["w"], TaskStatus::Rework)]);
        assert_eq!(accept_as_is_check(&s, &tid("w")).unwrap(), vec![tid("r")]);
        let s = st(&[("w", "worker", &[], TaskStatus::Reviewing)]);
        assert!(accept_as_is_check(&s, &tid("w")).is_err());
        let _ = Cause::User;
    }
}
