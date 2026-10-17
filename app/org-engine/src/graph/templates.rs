//! Built-in graph templates (DESIGN_ORG_GRAPH 6.5): template 1 `standard` and template 2
//! `quality-loop`. The host writes these files once with `create_new` (never overwriting) and
//! restores workflows that have no recorded graph with the built-in `standard`.

use super::def::{parse_graph, GraphDef};

pub const TEMPLATE_STANDARD: &str = "standard";
pub const TEMPLATE_QUALITY_LOOP: &str = "quality-loop";

const STANDARD_JSON: &str = r#"{
  "schemaVersion": 1,
  "id": "standard",
  "displayName": "計画して実行（標準）",
  "description": "Planner が依存関係つきのタスク計画を作り、Worker と Reviewer が実行・検証する。",
  "workspace": "git",
  "entry": "planner",
  "slots": {
    "plan": { "schema": "plan" }
  },
  "nodes": [
    { "id": "planner", "kind": "agent", "role": "planner", "output": "plan" },
    { "id": "tasks", "kind": "plannedTasks", "from": "plan", "allowedRoles": ["worker", "reviewer"] }
  ],
  "edges": [
    { "from": "planner", "to": "tasks" }
  ]
}
"#;

const QUALITY_LOOP_JSON: &str = r#"{
  "schemaVersion": 1,
  "id": "quality-loop",
  "displayName": "計画→実装→検証を周回（品質向上）",
  "description": "検証で差し戻されたら実装をやり直し、方針の誤りなら計画から作り直す。",
  "workspace": "git",
  "entry": "plan",
  "slots": {
    "spec":    { "schema": "spec" },
    "report":  { "schema": "workerReport" },
    "verdict": { "schema": "verdict3" }
  },
  "nodes": [
    { "id": "plan",   "kind": "agent", "role": "planner",  "output": "spec",
      "instructions": "依頼を、実装方針と受入条件に整理する。" },
    { "id": "build",  "kind": "agent", "role": "worker",   "output": "report", "capture": "commit",
      "reads": ["spec"] },
    { "id": "verify", "kind": "agent", "role": "verifier", "output": "verdict", "target": "build",
      "reads": ["spec", "report"] }
  ],
  "edges": [
    { "from": "plan",   "to": "build" },
    { "from": "build",  "to": "verify" },
    { "from": "verify", "to": "build", "when": { "slot": "verdict", "path": "verdict", "eq": "rework" },
      "feedback": { "maxRounds": 3, "onExhausted": "ask" } },
    { "from": "verify", "to": "plan",  "when": { "slot": "verdict", "path": "verdict", "eq": "replan" },
      "feedback": { "maxRounds": 1, "onExhausted": "ask" } }
  ]
}
"#;

/// The file text of a built-in template (what the host writes on first start).
pub fn template_json(id: &str) -> Option<&'static str> {
    match id {
        TEMPLATE_STANDARD => Some(STANDARD_JSON),
        TEMPLATE_QUALITY_LOOP => Some(QUALITY_LOOP_JSON),
        _ => None,
    }
}

/// The built-in templates as definitions, in display order.
pub fn builtin_templates() -> Vec<GraphDef> {
    [TEMPLATE_STANDARD, TEMPLATE_QUALITY_LOOP]
        .iter()
        .map(|id| parse_graph(template_json(id).expect("built-in")).expect("built-in template parses"))
        .collect()
}
