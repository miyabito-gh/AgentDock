//! Counters for turns, rework and automatic re-planning. A turn is counted
//! before sending; format-fix requests and rework each count as one turn.
use crate::types::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnKind {
    Plan,
    Task,
    Review,
    FormatFix,
    Rework,
    Replan,
    /// A new attempt after a failed / interrupted turn (counts as a turn).
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitError {
    TurnLimit { used: u32, max: u32 },
    ReworkLimit { used: u32, max: u32 },
    AutoReplanLimit { used: u32, max: u32 },
    RetryLimit { used: u32, max: u32 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub turns_used: u32,
    pub auto_replans_used: u32,
}

impl Counters {
    pub fn turns_remaining(&self, limits: &Limits) -> u32 {
        limits.max_turns.saturating_sub(self.turns_used)
    }

    /// Count one turn. On error nothing is counted and nothing may be sent.
    pub fn spend_turn(&mut self, _kind: TurnKind, limits: &Limits) -> Result<(), LimitError> {
        if self.turns_used >= limits.max_turns {
            return Err(LimitError::TurnLimit { used: self.turns_used, max: limits.max_turns });
        }
        self.turns_used += 1;
        Ok(())
    }

    /// Count an automatic re-plan. Beyond the limit the user must confirm.
    pub fn try_auto_replan(&mut self, limits: &Limits) -> Result<(), LimitError> {
        if self.auto_replans_used >= limits.max_auto_replans {
            return Err(LimitError::AutoReplanLimit { used: self.auto_replans_used, max: limits.max_auto_replans });
        }
        self.auto_replans_used += 1;
        Ok(())
    }

    /// "Raise the limit and resume": the new maximum must be larger than the current one.
    pub fn raised_turn_limit(limits: &Limits, new_max: u32) -> Option<Limits> {
        (new_max > limits.max_turns).then(|| Limits { max_turns: new_max, ..limits.clone() })
    }
}

impl Task {
    pub fn rework_limit_reached(&self, limits: &Limits) -> bool {
        self.rework_count >= limits.max_rework
    }

    /// Count one automatic send-back. At the limit the caller sets
    /// `failed{LimitReached}` and hands over to the user.
    #[cfg(test)]
    pub(crate) fn try_rework(&mut self, limits: &Limits) -> Result<(), LimitError> {
        if self.rework_limit_reached(limits) {
            return Err(LimitError::ReworkLimit { used: self.rework_count, max: limits.max_rework });
        }
        self.rework_count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_limit_counts_all_kinds_and_stops() {
        let limits = Limits { max_turns: 4, ..Limits::default() };
        let mut c = Counters::default();
        for k in [TurnKind::Plan, TurnKind::FormatFix, TurnKind::Task, TurnKind::Rework] {
            c.spend_turn(k, &limits).unwrap();
        }
        assert_eq!(c.turns_remaining(&limits), 0);
        assert_eq!(c.spend_turn(TurnKind::Review, &limits), Err(LimitError::TurnLimit { used: 4, max: 4 }));
        assert_eq!(c.turns_used, 4);
        let raised = Counters::raised_turn_limit(&limits, 6).unwrap();
        c.spend_turn(TurnKind::Review, &raised).unwrap();
        assert!(Counters::raised_turn_limit(&limits, 4).is_none());
    }

    #[test]
    fn default_turn_limit_is_30() {
        let limits = Limits::default();
        let mut c = Counters::default();
        for _ in 0..30 {
            c.spend_turn(TurnKind::Task, &limits).unwrap();
        }
        assert!(c.spend_turn(TurnKind::Task, &limits).is_err());
    }

    #[test]
    fn rework_twice_then_limit() {
        let limits = Limits::default();
        let mut t = Task::from_plan(&PlanTask {
            id: TaskId::new("t"),
            role: RoleId::new("worker"),
            deps: vec![],
            effects: vec![],
            title: String::new(),
            instructions: String::new(),
        });
        assert_eq!(t.attempts(), 1);
        t.try_rework(&limits).unwrap();
        t.try_rework(&limits).unwrap();
        assert_eq!(t.attempts(), 3);
        assert_eq!(t.try_rework(&limits), Err(LimitError::ReworkLimit { used: 2, max: 2 }));
        assert_eq!(t.rework_count, 2);
    }

    #[test]
    fn auto_replan_once() {
        let limits = Limits::default();
        let mut c = Counters::default();
        c.try_auto_replan(&limits).unwrap();
        assert_eq!(c.try_auto_replan(&limits), Err(LimitError::AutoReplanLimit { used: 1, max: 1 }));
    }
}
