//! Output schemas (sent as `outputSchema` on every turn; they do not persist across
//! turns, DESIGN_ORG 4.3 / 5.4) and the parsers/validators for the artifacts.
//! Parsing is strict: anything that does not match is a format error (one format fix
//! is allowed by the engine, then the task fails with `InvalidArtifact`).
use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::types::*;

pub const ROLE_PLANNER: &str = "planner";
pub const ROLE_WORKER: &str = "worker";
pub const ROLE_REVIEWER: &str = "reviewer";

/// Effect names as they appear in the plan JSON.
const EFFECTS: &[(&str, Effect)] = &[
    ("userTreeWrite", Effect::UserTreeWrite),
    ("pushMergeTag", Effect::PushMergeTag),
    ("outsideWorkdir", Effect::OutsideWorkdir),
    ("networkOrInstall", Effect::NetworkOrInstall),
    ("secretsOrAuth", Effect::SecretsOrAuth),
    ("deletionPrimary", Effect::DeletionPrimary),
];

pub fn planner_plan_schema() -> Value {
    let effect_names: Vec<&str> = EFFECTS.iter().map(|(n, _)| *n).collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["tasks"],
        "properties": {
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "role", "deps", "effects", "title", "instructions"],
                    "properties": {
                        "id": { "type": "string" },
                        "role": { "type": "string", "enum": [ROLE_WORKER, ROLE_REVIEWER] },
                        "deps": { "type": "array", "items": { "type": "string" } },
                        "effects": { "type": "array", "items": { "type": "string", "enum": effect_names } },
                        "title": { "type": "string" },
                        "instructions": { "type": "string" }
                    }
                }
            }
        }
    })
}

pub fn worker_report_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "changedFiles", "tests"],
        "properties": {
            "summary": { "type": "string" },
            "changedFiles": { "type": "array", "items": { "type": "string" } },
            "tests": {
                "type": "object",
                "additionalProperties": false,
                "required": ["ran", "passed"],
                "properties": {
                    "ran": { "type": "boolean" },
                    "passed": { "type": "boolean" }
                }
            }
        }
    })
}

pub fn reviewer_verdict_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "findings"],
        "properties": {
            "verdict": { "type": "string", "enum": ["pass", "rework"] },
            "findings": { "type": "array", "items": { "type": "string" } }
        }
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestsReport {
    pub ran: bool,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerReport {
    pub summary: String,
    pub changed_files: Vec<String>,
    pub tests: TestsReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerdictKind {
    Pass,
    Rework,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReviewerVerdict {
    pub verdict: VerdictKind,
    pub findings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTask {
    id: String,
    role: String,
    deps: Vec<String>,
    effects: Vec<String>,
    title: String,
    instructions: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    tasks: Vec<RawTask>,
}

/// Parse the planner's JSON into plan tasks. Structure rules checked here (on top of
/// `validate_plan`): roles are worker/reviewer only, ids do not collide with `taken`,
/// every task has instructions, a reviewer depends on exactly one worker task.
pub fn parse_plan(text: &str, taken: &HashSet<TaskId>) -> Result<Vec<PlanTask>, String> {
    let raw: RawPlan = serde_json::from_str(text.trim()).map_err(|e| format!("計画のJSONを解析できません: {e}"))?;
    let mut tasks = Vec::new();
    for r in raw.tasks {
        let role = match r.role.as_str() {
            ROLE_WORKER | ROLE_REVIEWER => RoleId::new(r.role.clone()),
            other => return Err(format!("タスク {} の役割 {other} は使えません（worker か reviewer）", r.id)),
        };
        let mut effects = Vec::new();
        for e in &r.effects {
            let Some((_, eff)) = EFFECTS.iter().find(|(n, _)| n == e) else {
                return Err(format!("タスク {} の効果 {e} は未知です", r.id));
            };
            effects.push(*eff);
        }
        if r.instructions.trim().is_empty() {
            return Err(format!("タスク {} の instructions が空です", r.id));
        }
        let id = TaskId::new(r.id.trim());
        if taken.contains(&id) {
            return Err(format!("タスクID {id} は既に使われています"));
        }
        tasks.push(PlanTask {
            id,
            role,
            deps: r.deps.iter().map(|d| TaskId::new(d.trim())).collect(),
            effects,
            title: r.title,
            instructions: r.instructions,
        });
    }
    for t in tasks.iter().filter(|t| t.role.0 == ROLE_REVIEWER) {
        let ok = t.deps.len() == 1 && tasks.iter().any(|w| w.id == t.deps[0] && w.role.0 == ROLE_WORKER);
        if !ok {
            return Err(format!("reviewer タスク {} は worker タスク1件だけに依存する必要があります", t.id));
        }
    }
    Ok(tasks)
}

pub fn parse_worker_report(text: &str) -> Result<WorkerReport, String> {
    let r: WorkerReport = serde_json::from_str(text.trim()).map_err(|e| format!("作業報告のJSONを解析できません: {e}"))?;
    if r.summary.trim().is_empty() {
        return Err("summary が空です".into());
    }
    Ok(r)
}

pub fn parse_verdict(text: &str) -> Result<ReviewerVerdict, String> {
    let v: ReviewerVerdict = serde_json::from_str(text.trim()).map_err(|e| format!("レビュー判定のJSONを解析できません: {e}"))?;
    if v.verdict == VerdictKind::Rework && v.findings.iter().all(|f| f.trim().is_empty()) {
        return Err("rework には findings が1件以上必要です".into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_parses_and_checks_structure() {
        let ok = r#"{"tasks":[
            {"id":"a","role":"worker","deps":[],"effects":[],"title":"A","instructions":"do a"},
            {"id":"r","role":"reviewer","deps":["a"],"effects":["pushMergeTag"],"title":"R","instructions":"check a"}]}"#;
        let p = parse_plan(ok, &HashSet::new()).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].effects, vec![Effect::PushMergeTag]);
        assert!(parse_plan(ok, &HashSet::from([TaskId::new("a")])).is_err());
        assert!(parse_plan(&ok.replace("\"worker\"", "\"planner\""), &HashSet::new()).is_err());
        assert!(parse_plan(&ok.replace("\"deps\":[\"a\"]", "\"deps\":[]"), &HashSet::new()).is_err());
        assert!(parse_plan(&ok.replace("pushMergeTag", "bogus"), &HashSet::new()).is_err());
        assert!(parse_plan("not json", &HashSet::new()).is_err());
        assert!(parse_plan(r#"{"tasks":[],"extra":1}"#, &HashSet::new()).is_err());
    }

    #[test]
    fn report_and_verdict_validation() {
        assert!(parse_worker_report(r#"{"summary":"x","changedFiles":["a"],"tests":{"ran":true,"passed":true}}"#).is_ok());
        assert!(parse_worker_report(r#"{"summary":" ","changedFiles":[],"tests":{"ran":false,"passed":false}}"#).is_err());
        assert!(parse_worker_report(r#"{"summary":"x"}"#).is_err());
        assert!(parse_verdict(r#"{"verdict":"pass","findings":[]}"#).is_ok());
        assert!(parse_verdict(r#"{"verdict":"rework","findings":[]}"#).is_err());
        assert!(parse_verdict(r#"{"verdict":"maybe","findings":[]}"#).is_err());
    }
}
