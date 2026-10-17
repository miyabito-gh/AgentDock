//! State machine. Only listed transitions exist; there is no transition that
//! turns termination / disconnect / unknown acceptance into cancelled/failed.
use crate::types::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Pending,
    Ready,
    Running,
    Blocked,
    Reviewing,
    Rework,
    Completed,
    Failed,
    Cancelled,
}

fn kind(s: &TaskStatus) -> K {
    match s {
        TaskStatus::Pending => K::Pending,
        TaskStatus::Ready => K::Ready,
        TaskStatus::Running => K::Running,
        TaskStatus::Blocked(_) => K::Blocked,
        TaskStatus::Reviewing => K::Reviewing,
        TaskStatus::Rework => K::Rework,
        TaskStatus::Completed => K::Completed,
        TaskStatus::Failed(_) => K::Failed,
        TaskStatus::Cancelled => K::Cancelled,
    }
}

use K::*;
/// Allowed (from, to) pairs. Completed / Failed / Cancelled are terminal.
const ALLOWED: &[(K, K)] = &[
    (Pending, Ready),
    (Pending, Blocked),
    (Pending, Cancelled),
    (Ready, Pending),
    (Ready, Running),
    (Ready, Blocked),
    (Ready, Cancelled),
    (Running, Reviewing),
    (Running, Completed),
    (Running, Rework),
    (Running, Blocked),
    (Running, Failed),
    (Running, Cancelled),
    (Reviewing, Completed),
    (Reviewing, Rework),
    (Reviewing, Blocked),
    (Reviewing, Failed),
    (Reviewing, Cancelled),
    (Rework, Ready),
    (Rework, Running),
    (Rework, Blocked),
    (Rework, Failed),
    (Rework, Cancelled),
    (Blocked, Pending),
    (Blocked, Ready),
    (Blocked, Running),
    (Blocked, Reviewing),
    (Blocked, Rework),
    (Blocked, Failed),
    (Blocked, Cancelled),
    (Blocked, Blocked),
];

/// Pure table lookup (ignores confirmation state and cause).
pub fn is_allowed(from: &TaskStatus, to: &TaskStatus) -> bool {
    if from == to {
        return false;
    }
    ALLOWED.contains(&(kind(from), kind(to)))
}

/// Who or what drives the transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cause {
    /// Engine observation (turn result, validation...).
    Engine,
    /// Explicit user operation.
    User,
    /// Stop was confirmed (read-back).
    StopConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionError {
    Forbidden { from: TaskStatus, to: TaskStatus },
    /// Task is not `Confirmed`: only holding (blocked) or user/stop-confirmed cancel is possible.
    NotConfirmed(ConfirmState),
    /// `cancelled` only by user action or confirmed stop.
    CancelNeedsUserOrStopConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmError {
    Unchanged,
}

impl Task {
    pub fn transition(&mut self, to: TaskStatus, cause: Cause) -> Result<(), TransitionError> {
        if !is_allowed(&self.status, &to) {
            return Err(TransitionError::Forbidden { from: self.status, to });
        }
        let cancel = to == TaskStatus::Cancelled;
        if cancel && cause == Cause::Engine {
            return Err(TransitionError::CancelNeedsUserOrStopConfirmed);
        }
        if self.confirm != ConfirmState::Confirmed && !cancel && !matches!(to, TaskStatus::Blocked(_)) {
            return Err(TransitionError::NotConfirmed(self.confirm));
        }
        self.status = to;
        Ok(())
    }

    /// Change the confirmation axis. Any state may move to any different state
    /// (observations arrive in either order); the business state is untouched.
    pub fn set_confirm(&mut self, to: ConfirmState) -> Result<(), ConfirmError> {
        if self.confirm == to {
            return Err(ConfirmError::Unchanged);
        }
        self.confirm = to;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: TaskStatus) -> Task {
        let mut t = Task::from_plan(&PlanTask {
            id: TaskId::new("t"),
            role: RoleId::new("worker"),
            deps: vec![],
            effects: vec![],
        });
        t.status = status;
        t
    }

    #[test]
    fn happy_path_and_terminals() {
        let mut t = task(TaskStatus::Pending);
        for s in [TaskStatus::Ready, TaskStatus::Running, TaskStatus::Reviewing, TaskStatus::Completed] {
            t.transition(s, Cause::Engine).unwrap();
        }
        for to in [TaskStatus::Ready, TaskStatus::Running, TaskStatus::Rework, TaskStatus::Cancelled] {
            assert!(t.transition(to, Cause::User).is_err());
        }
    }

    #[test]
    fn forbidden_pairs() {
        assert!(!is_allowed(&TaskStatus::Pending, &TaskStatus::Running));
        assert!(!is_allowed(&TaskStatus::Pending, &TaskStatus::Completed));
        assert!(!is_allowed(&TaskStatus::Ready, &TaskStatus::Completed));
        assert!(!is_allowed(&TaskStatus::Failed(FailReason::ExecutionFailed), &TaskStatus::Ready));
        assert!(!is_allowed(&TaskStatus::Cancelled, &TaskStatus::Ready));
        assert!(!is_allowed(&TaskStatus::Running, &TaskStatus::Running));
        assert!(!is_allowed(&TaskStatus::Rework, &TaskStatus::Completed));
        assert!(is_allowed(&TaskStatus::Blocked(BlockedReason::Manual), &TaskStatus::Blocked(BlockedReason::Limit)));
        assert!(!is_allowed(&TaskStatus::Blocked(BlockedReason::Manual), &TaskStatus::Blocked(BlockedReason::Manual)));
    }

    #[test]
    fn every_pair_matches_table() {
        let all = [
            TaskStatus::Pending,
            TaskStatus::Ready,
            TaskStatus::Running,
            TaskStatus::Blocked(BlockedReason::Manual),
            TaskStatus::Reviewing,
            TaskStatus::Rework,
            TaskStatus::Completed,
            TaskStatus::Failed(FailReason::ExecutionFailed),
            TaskStatus::Cancelled,
        ];
        let mut n = 0;
        for a in &all {
            for b in &all {
                if is_allowed(a, b) {
                    n += 1;
                }
            }
        }
        // Blocked->Blocked with the same reason is excluded here.
        assert_eq!(n, ALLOWED.len() - 1);
    }

    #[test]
    fn unconfirmed_task_cannot_advance() {
        for c in [
            ConfirmState::SendUnknown,
            ConfirmState::NeedsReconcile(ReconcileCause::Restart),
            ConfirmState::InterruptRequested,
            ConfirmState::StopUnconfirmed,
        ] {
            let mut t = task(TaskStatus::Running);
            t.confirm = c;
            for to in [
                TaskStatus::Reviewing,
                TaskStatus::Completed,
                TaskStatus::Rework,
                TaskStatus::Failed(FailReason::ExecutionFailed),
            ] {
                assert_eq!(t.transition(to, Cause::Engine), Err(TransitionError::NotConfirmed(c)));
            }
            assert_eq!(t.status, TaskStatus::Running);
            t.transition(TaskStatus::Blocked(BlockedReason::NeedsAttention), Cause::Engine).unwrap();
        }
    }

    #[test]
    fn cancel_only_by_user_or_stop_confirmed() {
        let mut t = task(TaskStatus::Running);
        t.confirm = ConfirmState::StopUnconfirmed;
        assert_eq!(
            t.transition(TaskStatus::Cancelled, Cause::Engine),
            Err(TransitionError::CancelNeedsUserOrStopConfirmed)
        );
        t.transition(TaskStatus::Cancelled, Cause::User).unwrap();
        let mut t = task(TaskStatus::Running);
        t.set_confirm(ConfirmState::InterruptRequested).unwrap();
        t.set_confirm(ConfirmState::Confirmed).unwrap();
        t.transition(TaskStatus::Cancelled, Cause::StopConfirmed).unwrap();
    }

    #[test]
    fn disconnect_cannot_fail_or_cancel() {
        let mut t = task(TaskStatus::Running);
        t.set_confirm(ConfirmState::NeedsReconcile(ReconcileCause::Disconnect)).unwrap();
        assert!(t.transition(TaskStatus::Failed(FailReason::ExecutionFailed), Cause::Engine).is_err());
        assert!(t.transition(TaskStatus::Cancelled, Cause::Engine).is_err());
        assert_eq!(
            t.set_confirm(ConfirmState::NeedsReconcile(ReconcileCause::Disconnect)),
            Err(ConfirmError::Unchanged)
        );
    }
}
