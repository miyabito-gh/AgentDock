use crate::types::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    Empty,
    TooManyTasks { count: usize, max: u32 },
    EmptyId,
    DuplicateId(TaskId),
    UnknownRole { task: TaskId, role: RoleId },
    MissingDependency { task: TaskId, dep: TaskId },
    SelfDependency(TaskId),
    /// Task ids on a cycle (first == last).
    Cycle(Vec<TaskId>),
}

/// Format validation (always required before approval). Returns all errors found.
pub fn validate_plan(tasks: &[PlanTask], roles: &[Role], limits: &Limits) -> Result<(), Vec<PlanError>> {
    let mut errs = Vec::new();
    if tasks.is_empty() {
        errs.push(PlanError::Empty);
    }
    if tasks.len() > limits.max_tasks as usize {
        errs.push(PlanError::TooManyTasks { count: tasks.len(), max: limits.max_tasks });
    }
    let mut ids: HashSet<&TaskId> = HashSet::new();
    for t in tasks {
        if t.id.0.trim().is_empty() {
            errs.push(PlanError::EmptyId);
        } else if !ids.insert(&t.id) {
            errs.push(PlanError::DuplicateId(t.id.clone()));
        }
    }
    for t in tasks {
        if !roles.iter().any(|r| r.id == t.role) {
            errs.push(PlanError::UnknownRole { task: t.id.clone(), role: t.role.clone() });
        }
        for d in &t.deps {
            if d == &t.id {
                errs.push(PlanError::SelfDependency(t.id.clone()));
            } else if !ids.contains(d) {
                errs.push(PlanError::MissingDependency { task: t.id.clone(), dep: d.clone() });
            }
        }
    }
    if let Some(c) = find_cycle(tasks) {
        errs.push(PlanError::Cycle(c));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Tasks with neither dependencies nor dependents, when the plan has more than
/// one task. Informational (independent tasks are legal), not a format error.
pub fn isolated_tasks(tasks: &[PlanTask]) -> Vec<TaskId> {
    if tasks.len() < 2 {
        return vec![];
    }
    let depended: HashSet<&TaskId> = tasks.iter().flat_map(|t| t.deps.iter()).collect();
    tasks
        .iter()
        .filter(|t| t.deps.is_empty() && !depended.contains(&t.id))
        .map(|t| t.id.clone())
        .collect()
}

fn find_cycle(tasks: &[PlanTask]) -> Option<Vec<TaskId>> {
    let idx: HashMap<&TaskId, usize> = tasks.iter().enumerate().map(|(i, t)| (&t.id, i)).collect();
    // 0 = unvisited, 1 = on stack, 2 = done
    let mut color = vec![0u8; tasks.len()];
    let mut stack: Vec<usize> = Vec::new();
    fn dfs(
        i: usize,
        tasks: &[PlanTask],
        idx: &HashMap<&TaskId, usize>,
        color: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<TaskId>> {
        color[i] = 1;
        stack.push(i);
        for d in &tasks[i].deps {
            let Some(&j) = idx.get(d) else { continue };
            if color[j] == 1 {
                let start = stack.iter().position(|&x| x == j).unwrap();
                let mut path: Vec<TaskId> = stack[start..].iter().map(|&k| tasks[k].id.clone()).collect();
                path.push(tasks[j].id.clone());
                return Some(path);
            }
            if color[j] == 0 {
                if let Some(c) = dfs(j, tasks, idx, color, stack) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        color[i] = 2;
        None
    }
    for i in 0..tasks.len() {
        if color[i] == 0 {
            if let Some(c) = dfs(i, tasks, &idx, &mut color, &mut stack) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn pt(id: &str, deps: &[&str]) -> PlanTask {
        PlanTask {
            id: TaskId::new(id),
            role: RoleId::new("worker"),
            deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
            effects: vec![],
            title: String::new(),
            instructions: String::new(),
            kind: TaskKind::Change,
        }
    }
    pub fn roles() -> Vec<Role> {
        vec![Role { id: RoleId::new("worker"), enabled: true, writes: true, max_concurrent: None }]
    }

    #[test]
    fn valid_dag() {
        let p = [pt("a", &[]), pt("b", &["a"]), pt("c", &["a", "b"])];
        assert!(validate_plan(&p, &roles(), &Limits::default()).is_ok());
    }

    #[test]
    fn detects_cycles() {
        let p = [pt("a", &["c"]), pt("b", &["a"]), pt("c", &["b"])];
        let e = validate_plan(&p, &roles(), &Limits::default()).unwrap_err();
        assert!(e.iter().any(|x| matches!(x, PlanError::Cycle(c) if c.first() == c.last() && c.len() == 4)));
        let p = [pt("a", &["a"])];
        let e = validate_plan(&p, &roles(), &Limits::default()).unwrap_err();
        assert!(e.contains(&PlanError::SelfDependency(TaskId::new("a"))));
    }

    #[test]
    fn missing_dep_unknown_role_dup_empty() {
        let mut p = vec![pt("a", &["zz"]), pt("a", &[])];
        p[1].role = RoleId::new("ghost");
        let e = validate_plan(&p, &roles(), &Limits::default()).unwrap_err();
        assert!(e.contains(&PlanError::MissingDependency { task: TaskId::new("a"), dep: TaskId::new("zz") }));
        assert!(e.contains(&PlanError::DuplicateId(TaskId::new("a"))));
        assert!(e.iter().any(|x| matches!(x, PlanError::UnknownRole { .. })));
        assert_eq!(validate_plan(&[], &roles(), &Limits::default()), Err(vec![PlanError::Empty]));
    }

    #[test]
    fn task_count_limit() {
        let limits = Limits { max_tasks: 3, ..Limits::default() };
        let p: Vec<_> = (0..4).map(|i| pt(&format!("t{i}"), &[])).collect();
        let e = validate_plan(&p, &roles(), &limits).unwrap_err();
        assert!(e.contains(&PlanError::TooManyTasks { count: 4, max: 3 }));
        assert!(validate_plan(&p[..3], &roles(), &limits).is_ok());
    }

    #[test]
    fn isolated() {
        let p = [pt("a", &[]), pt("b", &["a"]), pt("x", &[])];
        assert_eq!(isolated_tasks(&p), vec![TaskId::new("x")]);
        assert!(isolated_tasks(&[pt("a", &[])]).is_empty());
    }
}
