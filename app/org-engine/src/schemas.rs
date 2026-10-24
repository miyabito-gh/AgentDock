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

/// The plan schema of the plan route (DESIGN_ORG_R4 1.2): like `planner_plan_schema` plus each task's `kind`.
/// The built-in graph schema `plan` keeps the older shape (the graph route is not changed here).
pub fn planner_plan_schema_with_kind() -> Value {
    let mut v = planner_plan_schema();
    let item = &mut v["properties"]["tasks"]["items"];
    item["required"] = json!(["id", "role", "kind", "deps", "effects", "title", "instructions"]);
    item["properties"]["kind"] = json!({ "type": "string", "enum": ["change", "investigate"] });
    v
}

/// What a plan-route investigation task answers with (DESIGN_ORG_R4 1.5).
pub fn investigation_report_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "findings", "references", "openQuestions"],
        "properties": {
            "summary": { "type": "string" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["point", "basis", "refs"],
                    "properties": {
                        "point": { "type": "string" },
                        "basis": { "type": "string" },
                        "refs": { "type": "array", "items": { "type": "string" } }
                    }
                }
            },
            "references": { "type": "array", "items": { "type": "string" } },
            "openQuestions": { "type": "array", "items": { "type": "string" } }
        }
    })
}

/// The verdict schema of the plan route (DESIGN_ORG_R4 2.1): the verdict, the findings and the checks the
/// reviewer ran. The built-in graph schema `reviewerVerdict` keeps the older shape.
pub fn reviewer_verdict_schema_checked() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "findings", "checks", "noCheckReason"],
        "properties": {
            "verdict": { "type": "string", "enum": ["pass", "rework"] },
            "findings": { "type": "array", "items": { "type": "string" } },
            "checks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["command", "outcome", "summary"],
                    "properties": {
                        "command": { "type": "string" },
                        "outcome": { "type": "string", "enum": ["passed", "failed", "notRun"] },
                        "summary": { "type": "string" }
                    }
                }
            },
            "noCheckReason": { "type": "string" }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckOutcome {
    Passed,
    Failed,
    NotRun,
}

/// One command the reviewer ran (or could not run) to check the work.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CheckResult {
    pub command: String,
    pub outcome: CheckOutcome,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReviewerVerdict {
    pub verdict: VerdictKind,
    pub findings: Vec<String>,
    /// None = the output has no such field (the older shape; `parse_verdict` never returns it).
    #[serde(default)]
    pub checks: Option<Vec<CheckResult>>,
    #[serde(default)]
    pub no_check_reason: Option<String>,
}

/// At most this many checks are kept; command and summary are cut to `CHECK_TEXT_MAX_CHARS`.
pub const CHECKS_MAX: usize = 20;
pub const CHECK_TEXT_MAX_CHARS: usize = 2000;

/// What the engine does with a verdict (DESIGN_ORG_R4 2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerdictJudgement {
    /// `unverified` = no check passed (the reason is in `no_check_reason`).
    Pass { unverified: bool },
    Rework,
    /// The reviewer said pass but a check failed: the user decides (never an automatic send-back).
    PassContradicted { failed: Vec<String> },
    /// The reviewer said rework but its own findings say nothing needs fixing: not a send-back, the user decides
    /// (`failed` = the commands whose check failed; may be empty).
    ReworkNoAction { failed: Vec<String> },
}

/// Phrases (compared after lowercasing and removing whitespace) with which a finding says nothing needs fixing.
/// A wrong hit only sends the verdict to the user, so the list errs on the wide side.
pub const NO_FIX_PHRASES: &[&str] = &[
    "追加の修正は求めない",
    "追加の実装修正は求めない",
    "修正は求めない",
    "修正は不要",
    "修正不要",
    "修正の必要はない",
    "修正の必要はありません",
    "修正は必要ない",
    "直すものはない",
    "直す点はない",
    "直すべき点はない",
    "対応は不要",
    "変更は不要",
    "nofurtherfix",
    "nofixneeded",
    "nofixrequired",
    "nothingtofix",
    "nochangesneeded",
    "noactionneeded",
];

/// True when a rework verdict has no finding the worker could act on: all findings are empty, or one of them
/// says no fix is needed.
pub fn findings_demand_no_fix(findings: &[String]) -> bool {
    if findings.iter().all(|f| f.trim().is_empty()) {
        return true;
    }
    findings.iter().any(|f| {
        let n: String = f.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        NO_FIX_PHRASES.iter().any(|p| n.contains(p))
    })
}

pub fn judge_verdict(v: &ReviewerVerdict) -> VerdictJudgement {
    let checks = v.checks.as_deref().unwrap_or(&[]);
    let failed: Vec<String> = checks.iter().filter(|c| c.outcome == CheckOutcome::Failed).map(|c| c.command.clone()).collect();
    match v.verdict {
        VerdictKind::Rework if findings_demand_no_fix(&v.findings) => VerdictJudgement::ReworkNoAction { failed },
        VerdictKind::Rework => VerdictJudgement::Rework,
        VerdictKind::Pass => {
            if !failed.is_empty() {
                VerdictJudgement::PassContradicted { failed }
            } else {
                let passed = checks.iter().any(|c| c.outcome == CheckOutcome::Passed);
                // An older output without `checks` is not called unverified (it never came from a live turn).
                VerdictJudgement::Pass { unverified: v.checks.is_some() && !passed }
            }
        }
    }
}

/// The findings recorded for the worker when a pass is contradicted by failed checks.
pub fn failed_check_findings(v: &ReviewerVerdict) -> Vec<String> {
    v.checks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|c| c.outcome == CheckOutcome::Failed)
        .map(|c| format!("検証のコマンドが失敗しました: {} ／ {}", c.command, c.summary))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvestigationFinding {
    pub point: String,
    pub basis: String,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvestigationReport {
    pub summary: String,
    pub findings: Vec<InvestigationFinding>,
    pub references: Vec<String>,
    pub open_questions: Vec<String>,
}

/// An investigation report larger than this is a format error.
pub const REPORT_MAX_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTask {
    id: String,
    role: String,
    /// Missing = change (the schema requires it, the parser is lenient).
    #[serde(default)]
    kind: Option<String>,
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
        let kind = match r.kind.as_deref() {
            None | Some("change") => TaskKind::Change,
            Some("investigate") => TaskKind::Investigate,
            Some(other) => return Err(format!("タスク {} の種類 {other} は使えません（change か investigate）", r.id)),
        };
        if kind == TaskKind::Investigate {
            if role.0 != ROLE_WORKER {
                return Err(format!("タスク {} は investigate にできません（investigate は worker だけです）", r.id));
            }
            if let Some(e) = effects.iter().find(|e| matches!(e, Effect::UserTreeWrite | Effect::PushMergeTag | Effect::DeletionPrimary)) {
                let name = EFFECTS.iter().find(|(_, x)| x == e).map_or("", |(n, _)| *n);
                return Err(format!("調査タスク {} は読むだけのため、書込みを伴う効果 {name} を宣言できません", r.id));
            }
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
            kind,
        });
    }
    for t in tasks.iter().filter(|t| t.role.0 == ROLE_REVIEWER) {
        let ok = t.deps.len() == 1 && tasks.iter().any(|w| w.id == t.deps[0] && w.role.0 == ROLE_WORKER && w.kind == TaskKind::Change);
        if !ok {
            return Err(format!("reviewer タスク {} は、変更を行う（change の）worker タスク1件だけに依存する必要があります（調査タスクにはレビューを付けません）", t.id));
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

/// Parse the reviewer's verdict of a live turn (strict: `checks` and `noCheckReason` are required).
pub fn parse_verdict(text: &str) -> Result<ReviewerVerdict, String> {
    let mut v: ReviewerVerdict = serde_json::from_str(text.trim()).map_err(|e| format!("レビュー判定のJSONを解析できません: {e}"))?;
    if v.verdict == VerdictKind::Rework && v.findings.iter().all(|f| f.trim().is_empty()) {
        return Err("rework には findings が1件以上必要です".into());
    }
    let (Some(checks), Some(reason)) = (v.checks.as_mut(), v.no_check_reason.as_ref()) else {
        return Err("checks と noCheckReason は必須です（実行した確認がなければ checks を空にして理由を書く）".into());
    };
    checks.truncate(CHECKS_MAX);
    for c in checks.iter_mut() {
        c.command = c.command.chars().take(CHECK_TEXT_MAX_CHARS).collect();
        c.summary = c.summary.chars().take(CHECK_TEXT_MAX_CHARS).collect();
    }
    let reason: String = reason.chars().take(CHECK_TEXT_MAX_CHARS).collect();
    // A pass with a failed check is not a format error: the engine holds it back for the user (`judge_verdict`).
    let basis = checks.iter().any(|c| matches!(c.outcome, CheckOutcome::Passed | CheckOutcome::Failed));
    if v.verdict == VerdictKind::Pass && !basis && reason.trim().is_empty() {
        return Err("pass には、成功した確認（checks の passed）が1件以上、または確認しなかった理由（noCheckReason）が必要です".into());
    }
    v.no_check_reason = Some(reason);
    Ok(v)
}

pub fn parse_investigation_report(text: &str) -> Result<InvestigationReport, String> {
    if text.len() > REPORT_MAX_BYTES {
        return Err("調査報告が大きすぎます（64KBまで）".into());
    }
    let r: InvestigationReport = serde_json::from_str(text.trim()).map_err(|e| format!("調査報告のJSONを解析できません: {e}"))?;
    if r.summary.trim().is_empty() {
        return Err("summary が空です".into());
    }
    Ok(r)
}

/// The text handed to a later task (and shown as the report): cut at `cap_chars` with a note.
pub fn render_investigation(r: &InvestigationReport, cap_chars: usize) -> String {
    let mut s = format!("要約: {}\n", r.summary.trim());
    for (i, f) in r.findings.iter().enumerate() {
        s.push_str(&format!("\n結論{}: {}\n  根拠: {}\n", i + 1, f.point.trim(), f.basis.trim()));
        if !f.refs.is_empty() {
            s.push_str(&format!("  参照: {}\n", f.refs.join(" / ")));
        }
    }
    if !r.references.is_empty() {
        s.push_str(&format!("\n調べた対象: {}\n", r.references.join(" / ")));
    }
    if !r.open_questions.is_empty() {
        s.push_str("\n分からなかったこと:\n");
        for q in &r.open_questions {
            s.push_str(&format!("- {}\n", q.trim()));
        }
    }
    let s = s.trim_end().to_string();
    if s.chars().count() <= cap_chars {
        return s;
    }
    let cut: String = s.chars().take(cap_chars).collect();
    format!("{cut}\n（以下省略）")
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
        assert!(parse_verdict(r#"{"verdict":"pass","findings":[],"checks":[{"command":"c","outcome":"passed","summary":"s"}],"noCheckReason":""}"#).is_ok());
        assert!(parse_verdict(r#"{"verdict":"rework","findings":[],"checks":[],"noCheckReason":""}"#).is_err());
        assert!(parse_verdict(r#"{"verdict":"maybe","findings":[],"checks":[],"noCheckReason":""}"#).is_err());
    }
    fn task(id: &str, role: &str, kind: Option<&str>, deps: &str, effects: &str) -> String {
        let k = kind.map(|k| format!(r#""kind":"{k}","#)).unwrap_or_default();
        format!(r#"{{"id":"{id}","role":"{role}",{k}"deps":{deps},"effects":{effects},"title":"t","instructions":"do"}}"#)
    }
    fn plan(tasks: &[String]) -> String {
        format!(r#"{{"tasks":[{}]}}"#, tasks.join(","))
    }

    #[test]
    fn kind_defaults_to_change_and_investigation_rules_are_checked() {
        let none = HashSet::new();
        let p = parse_plan(&plan(&[task("a", "worker", None, "[]", "[]"), task("b", "worker", Some("investigate"), "[]", "[]")]), &none).unwrap();
        assert_eq!((p[0].kind, p[1].kind), (TaskKind::Change, TaskKind::Investigate));
        assert!(parse_plan(&plan(&[task("a", "worker", Some("bogus"), "[]", "[]")]), &none).is_err());
        // a reviewer cannot be an investigation, and cannot review one
        assert!(parse_plan(&plan(&[task("a", "worker", None, "[]", "[]"), task("r", "reviewer", Some("investigate"), r#"["a"]"#, "[]")]), &none).is_err());
        assert!(parse_plan(&plan(&[task("i", "worker", Some("investigate"), "[]", "[]"), task("r", "reviewer", Some("change"), r#"["i"]"#, "[]")]), &none).is_err());
        assert!(parse_plan(&plan(&[task("c", "worker", Some("change"), "[]", "[]"), task("r", "reviewer", Some("change"), r#"["c"]"#, "[]")]), &none).is_ok());
        // an investigation reads only: no write-like effect
        for e in ["userTreeWrite", "pushMergeTag", "deletionPrimary"] {
            assert!(parse_plan(&plan(&[task("i", "worker", Some("investigate"), "[]", &format!(r#"["{e}"]"#))]), &none).is_err(), "{e}");
        }
        assert!(parse_plan(&plan(&[task("i", "worker", Some("investigate"), "[]", r#"["networkOrInstall"]"#)]), &none).is_ok());
        // the plan-route schema asks for the kind
        let schema = planner_plan_schema_with_kind();
        assert!(schema["properties"]["tasks"]["items"]["required"].as_array().unwrap().iter().any(|v| v == "kind"));
        assert!(!planner_plan_schema()["properties"]["tasks"]["items"]["properties"].as_object().unwrap().contains_key("kind"));
    }

    #[test]
    fn investigation_report_parses_and_renders_within_a_cap() {
        let ok = r#"{"summary":"s","findings":[{"point":"p","basis":"b","refs":["f:1"]}],"references":["f"],"openQuestions":["q"]}"#;
        let r = parse_investigation_report(ok).unwrap();
        let text = render_investigation(&r, 16_000);
        assert!(text.contains("要約: s") && text.contains("根拠: b") && text.contains("参照: f:1") && text.contains("- q"));
        assert_eq!(text, render_investigation(&r, 16_000), "deterministic");
        let cut = render_investigation(&r, 10);
        assert!(cut.ends_with("（以下省略）") && cut.chars().count() < text.chars().count());
        assert!(parse_investigation_report(r#"{"summary":" ","findings":[],"references":[],"openQuestions":[]}"#).is_err());
        assert!(parse_investigation_report(r#"{"summary":"s","findings":[],"references":[]}"#).is_err());
        assert!(parse_investigation_report(&format!(r#"{{"summary":"{}","findings":[],"references":[],"openQuestions":[]}}"#, "x".repeat(REPORT_MAX_BYTES))).is_err());
    }

    fn verdict(v: &str, checks: &str, reason: &str) -> String {
        format!(r#"{{"verdict":"{v}","findings":["f"],"checks":{checks},"noCheckReason":"{reason}"}}"#)
    }
    const PASSED: &str = r#"[{"command":"pytest","outcome":"passed","summary":"3 passed"}]"#;
    const FAILED: &str = r#"[{"command":"pytest","outcome":"failed","summary":"1 failed"}]"#;
    const NOT_RUN: &str = r#"[{"command":"pytest","outcome":"notRun","summary":"read-only"}]"#;

    #[test]
    fn checks_are_required_and_a_pass_needs_a_basis() {
        // missing checks or reason: a format error
        assert!(parse_verdict(r#"{"verdict":"rework","findings":["f"]}"#).is_err());
        assert!(parse_verdict(r#"{"verdict":"pass","findings":[],"checks":[]}"#).is_err());
        // pass: a passed check, or a reason; not-run alone is not enough
        assert!(parse_verdict(&verdict("pass", PASSED, "")).is_ok());
        assert!(parse_verdict(&verdict("pass", "[]", "docs only")).is_ok());
        assert!(parse_verdict(&verdict("pass", NOT_RUN, "")).is_err());
        assert!(parse_verdict(&verdict("pass", NOT_RUN, "cannot write cache")).is_ok());
        assert!(parse_verdict(&verdict("pass", "[]", "  ")).is_err());
        // a pass with a failed check is accepted here and held back by the judgement
        assert!(parse_verdict(&verdict("pass", FAILED, "")).is_ok());
        // rework needs no basis
        assert!(parse_verdict(&verdict("rework", "[]", "")).is_ok());
        // limits: 20 checks, 2000 chars each
        let many = format!("[{}]", vec![r#"{"command":"c","outcome":"passed","summary":"s"}"#; 30].join(","));
        assert_eq!(parse_verdict(&verdict("pass", &many, "")).unwrap().checks.unwrap().len(), CHECKS_MAX);
        let long = format!(r#"[{{"command":"{}","outcome":"passed","summary":"s"}}]"#, "c".repeat(5000));
        assert_eq!(parse_verdict(&verdict("pass", &long, "")).unwrap().checks.unwrap()[0].command.chars().count(), CHECK_TEXT_MAX_CHARS);
    }

    #[test]
    fn the_judgement_follows_the_table() {
        let j = |v: &str, c: &str, r: &str| judge_verdict(&parse_verdict(&verdict(v, c, r)).unwrap());
        assert_eq!(j("pass", PASSED, ""), VerdictJudgement::Pass { unverified: false });
        assert_eq!(j("pass", "[]", "docs only"), VerdictJudgement::Pass { unverified: true });
        assert_eq!(j("pass", NOT_RUN, "no write"), VerdictJudgement::Pass { unverified: true });
        assert_eq!(j("pass", FAILED, ""), VerdictJudgement::PassContradicted { failed: vec!["pytest".into()] });
        let mixed = format!("[{},{}]", &PASSED[1..PASSED.len() - 1], &FAILED[1..FAILED.len() - 1]);
        assert!(matches!(j("pass", &mixed, ""), VerdictJudgement::PassContradicted { .. }), "one failure outweighs a pass");
        // "f" is an actionable finding: a normal send-back
        assert_eq!(j("rework", FAILED, ""), VerdictJudgement::Rework);
        // the real case: rework whose own finding asks for no fix goes to the user, with the failed check
        let selfc = format!(
            r#"{{"verdict":"rework","findings":["test_powerのImportErrorを検出したため差し戻す。追加の実装修正は求めない。"],"checks":{FAILED},"noCheckReason":""}}"#
        );
        assert_eq!(
            judge_verdict(&parse_verdict(&selfc).unwrap()),
            VerdictJudgement::ReworkNoAction { failed: vec!["pytest".into()] }
        );
        assert!(findings_demand_no_fix(&["No further fix is needed".into()]));
        assert!(findings_demand_no_fix(&["修正 は 不要です".into()]));
        assert!(!findings_demand_no_fix(&["power の戻り値を直す".into()]));
        // an older output without checks is judged as it was (display only)
        let old = ReviewerVerdict { verdict: VerdictKind::Pass, findings: vec![], checks: None, no_check_reason: None };
        assert_eq!(judge_verdict(&old), VerdictJudgement::Pass { unverified: false });
    }
}
