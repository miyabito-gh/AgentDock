//! Helper for probes/app-server/reviewer-probe.mjs. Uses the product's own Reviewer prompt, output schema,
//! parse_verdict and judge_verdict.
//!   reviewer_probe_tool prompt  < {"title","instructions","request","summary","commit","claimRan","claimPassed"}
//!   reviewer_probe_tool schema
//!   reviewer_probe_tool judge   < <reviewer final message text>
use org_engine::prompt::reviewer_prompt;
use org_engine::schemas::{judge_verdict, parse_verdict, reviewer_verdict_schema_checked, VerdictJudgement};
use org_engine::{PlanTask, RoleId, Task, TaskId, TaskKind};
use serde_json::{json, Value};
use std::io::Read;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    match mode.as_str() {
        "schema" => println!("{}", reviewer_verdict_schema_checked()),
        "prompt" => {
            let v: Value = serde_json::from_str(&input).expect("json");
            let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
            let t = Task::from_plan(&PlanTask {
                id: TaskId::new("r1"),
                role: RoleId::new("reviewer"),
                deps: vec![TaskId::new("w1")],
                effects: vec![],
                title: s("title"),
                instructions: s("instructions"),
                kind: TaskKind::Change,
            });
            let w = TaskId::new("w1");
            let summary = s("summary");
            let claim = Some((v["claimRan"].as_bool().unwrap_or(false), v["claimPassed"].as_bool().unwrap_or(false)));
            print!("{}", reviewer_prompt(&t, &s("request"), (&w, Some(&summary), &s("commit")), 1, claim));
        }
        "judge" => match parse_verdict(&input) {
            Err(e) => println!("{}", json!({"parsed": false, "error": e})),
            Ok(v) => {
                let j = judge_verdict(&v);
                let out = match j {
                    VerdictJudgement::Pass { unverified } => json!({"kind": "pass", "unverified": unverified}),
                    VerdictJudgement::Rework => json!({"kind": "rework"}),
                    VerdictJudgement::PassContradicted { failed } => json!({"kind": "passContradicted", "failed": failed}),
                    VerdictJudgement::ReworkNoAction { failed } => json!({"kind": "reworkNoAction", "failed": failed}),
                };
                println!("{}", json!({"parsed": true, "judgement": out}));
            }
        },
        _ => eprintln!("usage: prompt|schema|judge"),
    }
}
