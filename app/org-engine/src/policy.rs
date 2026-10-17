//! Plan approval policy (DESIGN_ORG 4.3). Decides only whether a *plan* can be
//! approved automatically. It has no relation to Codex approval requests and
//! offers no way to answer them.
use crate::dag::{validate_plan, PlanError};
use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    Initial,
    Replan {
        /// Goal or scope differs from the approved plan.
        scope_changed: bool,
        /// Automatic re-plans already used in this workflow.
        auto_replans_used: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConfirmReason {
    /// A task declares this effect.
    DeclaredEffect { task: TaskId, effect: Effect },
    /// A task uses a role that is not enabled (permission-widening definition).
    DisabledRole { task: TaskId, role: RoleId },
    ReplanScopeChanged,
    AutoReplanLimitExceeded,
    /// Requested parallelism exceeds the configured limits.
    ParallelismExceeded { requested: u32, max: u32 },
    /// Minimum turns needed (one per task) exceed the turns left.
    TurnLimitExceeded { needed: u32, remaining: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    AutoApprove,
    NeedsConfirmation(Vec<ConfirmReason>),
}

pub struct PolicyInput<'a> {
    pub tasks: &'a [PlanTask],
    pub roles: &'a [Role],
    pub limits: &'a Limits,
    pub kind: PlanKind,
    /// Parallelism the plan asks for (None = no request).
    pub requested_parallelism: Option<u32>,
    pub turns_remaining: u32,
}

/// Format validation first (failure is not an approval question), then the
/// "always ask the user" conditions. Anything else is auto-approved.
pub fn evaluate_plan(input: &PolicyInput<'_>) -> Result<Decision, Vec<PlanError>> {
    validate_plan(input.tasks, input.roles, input.limits)?;
    let mut reasons = Vec::new();

    for t in input.tasks {
        for e in &t.effects {
            reasons.push(ConfirmReason::DeclaredEffect { task: t.id.clone(), effect: *e });
        }
        if input.roles.iter().any(|r| r.id == t.role && !r.enabled) {
            reasons.push(ConfirmReason::DisabledRole { task: t.id.clone(), role: t.role.clone() });
        }
    }
    if let PlanKind::Replan { scope_changed, auto_replans_used } = input.kind {
        if scope_changed {
            reasons.push(ConfirmReason::ReplanScopeChanged);
        }
        // This plan would be automatic re-plan number `auto_replans_used + 1`.
        if auto_replans_used >= input.limits.max_auto_replans {
            reasons.push(ConfirmReason::AutoReplanLimitExceeded);
        }
    }
    if let Some(req) = input.requested_parallelism {
        let max = input.limits.max_total;
        if req > max {
            reasons.push(ConfirmReason::ParallelismExceeded { requested: req, max });
        }
    }
    let needed = input.tasks.len() as u32;
    if needed > input.turns_remaining {
        reasons.push(ConfirmReason::TurnLimitExceeded { needed, remaining: input.turns_remaining });
    }

    Ok(if reasons.is_empty() { Decision::AutoApprove } else { Decision::NeedsConfirmation(reasons) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles() -> Vec<Role> {
        vec![
            Role { id: RoleId::new("worker"), enabled: true, writes: true, max_concurrent: None },
            Role { id: RoleId::new("custom"), enabled: false, writes: true, max_concurrent: None },
        ]
    }
    fn task(id: &str, role: &str, deps: &[&str], effects: &[Effect]) -> PlanTask {
        PlanTask {
            id: TaskId::new(id),
            role: RoleId::new(role),
            deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
            effects: effects.to_vec(),
            title: String::new(),
            instructions: String::new(),
        }
    }
    fn eval(tasks: &[PlanTask], kind: PlanKind, par: Option<u32>, turns: u32) -> Decision {
        let limits = Limits::default();
        evaluate_plan(&PolicyInput {
            tasks,
            roles: &roles(),
            limits: &limits,
            kind,
            requested_parallelism: par,
            turns_remaining: turns,
        })
        .unwrap()
    }

    #[test]
    fn clean_plan_auto_approved() {
        let t = [task("a", "worker", &[], &[]), task("b", "worker", &["a"], &[])];
        assert_eq!(eval(&t, PlanKind::Initial, Some(3), 30), Decision::AutoApprove);
    }

    #[test]
    fn each_effect_requires_confirmation() {
        for e in [
            Effect::UserTreeWrite,
            Effect::PushMergeTag,
            Effect::OutsideWorkdir,
            Effect::NetworkOrInstall,
            Effect::SecretsOrAuth,
            Effect::DeletionPrimary,
        ] {
            let t = [task("a", "worker", &[], &[e])];
            assert_eq!(
                eval(&t, PlanKind::Initial, None, 30),
                Decision::NeedsConfirmation(vec![ConfirmReason::DeclaredEffect { task: TaskId::new("a"), effect: e }])
            );
        }
    }

    #[test]
    fn disabled_role_requires_confirmation() {
        let t = [task("a", "custom", &[], &[])];
        assert!(matches!(eval(&t, PlanKind::Initial, None, 30), Decision::NeedsConfirmation(r)
            if r == vec![ConfirmReason::DisabledRole { task: TaskId::new("a"), role: RoleId::new("custom") }]));
    }

    #[test]
    fn replan_conditions() {
        let t = [task("a", "worker", &[], &[])];
        assert_eq!(
            eval(&t, PlanKind::Replan { scope_changed: false, auto_replans_used: 0 }, None, 30),
            Decision::AutoApprove
        );
        assert_eq!(
            eval(&t, PlanKind::Replan { scope_changed: true, auto_replans_used: 0 }, None, 30),
            Decision::NeedsConfirmation(vec![ConfirmReason::ReplanScopeChanged])
        );
        assert_eq!(
            eval(&t, PlanKind::Replan { scope_changed: false, auto_replans_used: 1 }, None, 30),
            Decision::NeedsConfirmation(vec![ConfirmReason::AutoReplanLimitExceeded])
        );
    }

    #[test]
    fn parallelism_and_turn_limits() {
        let t = [task("a", "worker", &[], &[]), task("b", "worker", &[], &[])];
        assert_eq!(
            eval(&t, PlanKind::Initial, Some(4), 30),
            Decision::NeedsConfirmation(vec![ConfirmReason::ParallelismExceeded { requested: 4, max: 3 }])
        );
        assert_eq!(
            eval(&t, PlanKind::Initial, None, 1),
            Decision::NeedsConfirmation(vec![ConfirmReason::TurnLimitExceeded { needed: 2, remaining: 1 }])
        );
    }

    #[test]
    fn invalid_plan_is_not_an_approval_question() {
        let limits = Limits::default();
        let t = [task("a", "worker", &["b"], &[]), task("b", "worker", &["a"], &[])];
        let r = evaluate_plan(&PolicyInput {
            tasks: &t,
            roles: &roles(),
            limits: &limits,
            kind: PlanKind::Initial,
            requested_parallelism: None,
            turns_remaining: 30,
        });
        assert!(r.is_err());
    }

    #[test]
    fn task_count_over_limit_is_format_error() {
        let limits = Limits::default();
        let t: Vec<_> = (0..21).map(|i| task(&format!("t{i}"), "worker", &[], &[])).collect();
        let r = evaluate_plan(&PolicyInput {
            tasks: &t,
            roles: &roles(),
            limits: &limits,
            kind: PlanKind::Initial,
            requested_parallelism: None,
            turns_remaining: 30,
        });
        assert!(matches!(r, Err(e) if e.iter().any(|x| matches!(x, PlanError::TooManyTasks { .. }))));
    }
}
