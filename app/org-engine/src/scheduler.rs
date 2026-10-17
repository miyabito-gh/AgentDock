//! Scheduler: pure functions that say what may be promoted / dispatched.
//! Sending is never done here. Tasks whose confirmation state is not
//! `Confirmed` are never touched, and nothing depending on them advances.
use crate::types::*;
use std::collections::HashMap;

/// Outside conditions that hold dispatching (rate limit pause, user pause...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchGate {
    /// Rate limit observed, workflow paused, restart stop, wake hold, etc.
    pub paused: bool,
    /// Turns left before the workflow turn limit (each dispatch costs one).
    pub turns_remaining: u32,
}

impl DispatchGate {
    pub fn open(turns_remaining: u32) -> Self {
        Self { paused: false, turns_remaining }
    }
}

fn dep_satisfied(by_id: &HashMap<&TaskId, &Task>, dep: &TaskId) -> bool {
    by_id
        .get(dep)
        .is_some_and(|d| d.status == TaskStatus::Completed && d.confirm == ConfirmState::Confirmed)
}

fn deps_satisfied(by_id: &HashMap<&TaskId, &Task>, t: &Task) -> bool {
    t.deps.iter().all(|d| dep_satisfied(by_id, d))
}

/// A task holds a slot while it runs, or while its state is uncertain (it may
/// still be running on the Codex side).
fn occupies(t: &Task) -> bool {
    match t.status {
        TaskStatus::Running | TaskStatus::Reviewing => true,
        TaskStatus::Completed | TaskStatus::Cancelled => false,
        _ => t.confirm != ConfirmState::Confirmed,
    }
}

/// `pending` tasks whose dependencies are all completed (and confirmed).
/// The caller applies the pending -> ready transition.
pub fn promote_ready(tasks: &[Task]) -> Vec<TaskId> {
    let by_id: HashMap<&TaskId, &Task> = tasks.iter().map(|t| (&t.id, t)).collect();
    tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending && t.confirm == ConfirmState::Confirmed)
        .filter(|t| deps_satisfied(&by_id, t))
        .map(|t| t.id.clone())
        .collect()
}

/// `ready` tasks that may be sent now, in input order, within the total,
/// writer and per-role limits and the turn budget.
pub fn next_dispatch(tasks: &[Task], roles: &[Role], limits: &Limits, gate: &DispatchGate) -> Vec<TaskId> {
    if gate.paused || gate.turns_remaining == 0 {
        return vec![];
    }
    let role_of = |id: &RoleId| roles.iter().find(|r| &r.id == id);
    let by_id: HashMap<&TaskId, &Task> = tasks.iter().map(|t| (&t.id, t)).collect();

    let mut total = 0u32;
    let mut writers = 0u32;
    let mut per_role: HashMap<&RoleId, u32> = HashMap::new();
    for t in tasks.iter().filter(|t| occupies(t)) {
        total += 1;
        *per_role.entry(&t.role).or_default() += 1;
        if role_of(&t.role).is_some_and(|r| r.writes) {
            writers += 1;
        }
    }

    let mut out = Vec::new();
    for t in tasks {
        if out.len() as u32 >= gate.turns_remaining || total >= limits.max_total {
            break;
        }
        if t.status != TaskStatus::Ready || t.confirm != ConfirmState::Confirmed || !deps_satisfied(&by_id, t) {
            continue;
        }
        let Some(role) = role_of(&t.role) else { continue };
        if !role.enabled {
            continue;
        }
        if role.writes && writers >= limits.max_writers {
            continue;
        }
        if role.max_concurrent.is_some_and(|m| per_role.get(&t.role).copied().unwrap_or(0) >= m) {
            continue;
        }
        total += 1;
        if role.writes {
            writers += 1;
        }
        *per_role.entry(&t.role).or_default() += 1;
        out.push(t.id.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, role: &str, deps: &[&str], status: TaskStatus) -> Task {
        let mut t = Task::from_plan(&PlanTask {
            id: TaskId::new(id),
            role: RoleId::new(role),
            deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
            effects: vec![],
        });
        t.status = status;
        t
    }
    fn roles() -> Vec<Role> {
        vec![
            Role { id: RoleId::new("worker"), enabled: true, writes: true, max_concurrent: None },
            Role { id: RoleId::new("reviewer"), enabled: true, writes: false, max_concurrent: None },
        ]
    }
    fn ids(v: Vec<TaskId>) -> Vec<String> {
        v.into_iter().map(|t| t.0).collect()
    }

    #[test]
    fn promotes_only_when_deps_completed() {
        let ts = [
            task("a", "worker", &[], TaskStatus::Completed),
            task("b", "worker", &["a"], TaskStatus::Pending),
            task("c", "worker", &["b"], TaskStatus::Pending),
            task("d", "worker", &["x"], TaskStatus::Pending),
            task("e", "worker", &[], TaskStatus::Pending),
        ];
        assert_eq!(ids(promote_ready(&ts)), ["b", "e"]);
    }

    #[test]
    fn failed_or_cancelled_dep_does_not_promote() {
        let ts = [
            task("a", "worker", &[], TaskStatus::Failed(FailReason::ExecutionFailed)),
            task("z", "worker", &[], TaskStatus::Cancelled),
            task("b", "worker", &["a"], TaskStatus::Pending),
            task("c", "worker", &["z"], TaskStatus::Pending),
        ];
        assert!(promote_ready(&ts).is_empty());
    }

    #[test]
    fn unconfirmed_dep_blocks_dependents_but_not_independents() {
        let mut a = task("a", "worker", &[], TaskStatus::Completed);
        a.confirm = ConfirmState::NeedsReconcile(ReconcileCause::Restart);
        let ts = [a, task("b", "worker", &["a"], TaskStatus::Pending), task("e", "worker", &[], TaskStatus::Pending)];
        assert_eq!(ids(promote_ready(&ts)), ["e"]);
    }

    #[test]
    fn unconfirmed_task_is_skipped_and_holds_a_slot() {
        let mut r = task("r", "worker", &[], TaskStatus::Ready);
        r.confirm = ConfirmState::SendUnknown;
        let mut s = task("s", "worker", &[], TaskStatus::Running);
        s.confirm = ConfirmState::StopUnconfirmed;
        let ts = [r, s, task("n", "worker", &[], TaskStatus::Ready)];
        // writers: s holds one (r also holds one via uncertain state) => limit 2 reached
        assert!(next_dispatch(&ts, &roles(), &Limits::default(), &DispatchGate::open(10)).is_empty());
        let limits = Limits::default().with_parallelism(3, 4).unwrap();
        assert_eq!(ids(next_dispatch(&ts, &roles(), &limits, &DispatchGate::open(10))), ["n"]);
    }

    #[test]
    fn respects_writer_and_total_limits() {
        let ts = [
            task("w1", "worker", &[], TaskStatus::Running),
            task("w2", "worker", &[], TaskStatus::Ready),
            task("w3", "worker", &[], TaskStatus::Ready),
            task("v1", "reviewer", &[], TaskStatus::Ready),
            task("v2", "reviewer", &[], TaskStatus::Ready),
        ];
        // writers 2, total 3: w1 running; w2 ok (writers 2), w3 blocked by writers, v1 ok (total 3), v2 total full
        assert_eq!(ids(next_dispatch(&ts, &roles(), &Limits::default(), &DispatchGate::open(10))), ["w2", "v1"]);
    }

    #[test]
    fn never_exceeds_total_even_with_many_ready() {
        let ts: Vec<_> = (0..10).map(|i| task(&format!("v{i}"), "reviewer", &[], TaskStatus::Ready)).collect();
        for total in 1..=4 {
            let limits = Limits::default().with_parallelism(1, total).unwrap();
            assert_eq!(next_dispatch(&ts, &roles(), &limits, &DispatchGate::open(100)).len() as u32, total);
        }
    }

    #[test]
    fn role_cap_gate_and_turn_budget() {
        let mut rs = roles();
        rs[1].max_concurrent = Some(1);
        let ts = [
            task("v1", "reviewer", &[], TaskStatus::Reviewing),
            task("v2", "reviewer", &[], TaskStatus::Ready),
            task("w1", "worker", &[], TaskStatus::Ready),
            task("w2", "worker", &[], TaskStatus::Ready),
        ];
        assert_eq!(ids(next_dispatch(&ts, &rs, &Limits::default(), &DispatchGate::open(10))), ["w1", "w2"]);
        let ts2 = [task("w1", "worker", &[], TaskStatus::Ready), task("w2", "worker", &[], TaskStatus::Ready)];
        assert_eq!(next_dispatch(&ts2, &roles(), &Limits::default(), &DispatchGate::open(1)).len(), 1);
        assert!(next_dispatch(&ts2, &roles(), &Limits::default(), &DispatchGate::open(0)).is_empty());
        let paused = DispatchGate { paused: true, turns_remaining: 5 };
        assert!(next_dispatch(&ts2, &roles(), &Limits::default(), &paused).is_empty());
    }

    #[test]
    fn disabled_role_not_dispatched() {
        let mut rs = roles();
        rs[0].enabled = false;
        let ts = [task("w1", "worker", &[], TaskStatus::Ready)];
        assert!(next_dispatch(&ts, &rs, &Limits::default(), &DispatchGate::open(5)).is_empty());
    }
}
