//! Edge selection (DESIGN_ORG_GRAPH 6.4, 7.3). Pure: given the slot values, pick the next edge.
//!
//! Conditional edges are tried in declaration order and the first that holds wins; otherwise the
//! unconditional (default) edge is taken; with neither, the path ends. If any conditional edge
//! cannot be evaluated (slot value missing, path absent, type mismatch) nothing is guessed: the
//! result is `Unresolved` and the engine must not move on. Feedback allowance (rounds left) is the
//! engine's concern, not decided here.

use serde_json::Value;

use super::def::{Condition, EdgeDef, NodeId, SlotId};
use super::validate::CompiledGraph;

#[derive(Clone, Debug, PartialEq)]
pub enum Route<'a> {
    Edge(&'a EdgeDef),
    /// No edge applies: the flow ends at this node.
    End,
    /// A needed value is missing or has an unexpected type; the reason is for display.
    Unresolved(String),
}

enum Eval {
    Yes,
    No,
    Unknown(String),
}

fn at_path<'v>(root: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

fn eval<'v>(c: &Condition, slot_value: &impl Fn(&SlotId) -> Option<&'v Value>) -> Eval {
    let Some(root) = slot_value(&c.slot) else {
        return Eval::Unknown(format!("スロット「{}」の値が未取得です", c.slot));
    };
    let Some(v) = at_path(root, &c.path) else {
        return Eval::Unknown(format!("スロット「{}」に「{}」がありません", c.slot, c.path));
    };
    if let Some(x) = &c.eq {
        return if v == x { Eval::Yes } else { Eval::No };
    }
    if let Some(x) = &c.ne {
        return if v != x { Eval::Yes } else { Eval::No };
    }
    if let Some(xs) = &c.in_ {
        return if xs.contains(v) { Eval::Yes } else { Eval::No };
    }
    Eval::Unknown("条件に eq・in・ne がありません".to_string())
}

/// Pick the edge to take out of `from`. `slot_value` returns the latest value of a slot.
pub fn select_edge<'g, 'v>(g: &'g CompiledGraph, from: &NodeId, slot_value: impl Fn(&SlotId) -> Option<&'v Value>) -> Route<'g> {
    let mut default: Option<&EdgeDef> = None;
    let mut chosen: Option<&EdgeDef> = None;
    for e in g.def.edges.iter().filter(|e| &e.from == from) {
        match &e.when {
            None => default = default.or(Some(e)),
            Some(c) => match eval(c, &slot_value) {
                Eval::Yes => chosen = chosen.or(Some(e)),
                Eval::No => {}
                // Do not guess: an unevaluable condition blocks the choice even if an earlier one held.
                Eval::Unknown(why) => return Route::Unresolved(why),
            },
        }
    }
    match chosen.or(default) {
        Some(e) => Route::Edge(e),
        None => Route::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::templates::{template_json, TEMPLATE_QUALITY_LOOP};
    use crate::graph::validate::{validate, RoleInfo};
    use crate::outschema::builtin_schemas;
    use crate::types::{Limits, RoleId};
    use serde_json::json;

    fn compiled(text: &str, id: &str) -> CompiledGraph {
        let def = crate::graph::def::parse_graph(text).unwrap();
        let roles = ["planner", "worker", "verifier", "router"]
            .iter()
            .map(|r| {
                let workplace = match *r {
                    "worker" => crate::roledef::Workplace::OwnWorktree,
                    "verifier" => crate::roledef::Workplace::DetachedAtCommit,
                    _ => crate::roledef::Workplace::SharedReadOnly,
                };
                (RoleId::new(*r), RoleInfo { writes: *r == "worker", workplace })
            })
            .collect();
        validate(&def, &roles, &builtin_schemas(), id, &Limits::default()).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn pick<'a>(g: &'a CompiledGraph, from: &str, v: &'a Value) -> Route<'a> {
        select_edge(g, &NodeId::new(from), |s| (s.0 == "verdict").then_some(v))
    }

    #[test]
    fn conditional_edges_are_chosen_and_no_match_ends() {
        let g = compiled(template_json(TEMPLATE_QUALITY_LOOP).unwrap(), "quality-loop");
        let rework = json!({"verdict": "rework", "findings": []});
        let Route::Edge(e) = pick(&g, "verify", &rework) else { panic!() };
        assert_eq!(e.to.0, "build");
        let replan = json!({"verdict": "replan", "findings": []});
        let Route::Edge(e) = pick(&g, "verify", &replan) else { panic!() };
        assert_eq!(e.to.0, "plan");
        assert_eq!(pick(&g, "verify", &json!({"verdict": "pass", "findings": []})), Route::End);
        // A plain single edge is taken without looking at any slot.
        let Route::Edge(e) = select_edge(&g, &NodeId::new("plan"), |_| None) else { panic!() };
        assert_eq!(e.to.0, "build");
    }

    const ROUTER: &str = r#"{
      "schemaVersion": 1, "id": "r", "displayName": "r", "workspace": "git", "entry": "a",
      "slots": { "verdict": { "schema": "verdict3" } },
      "nodes": [
        { "id": "a", "kind": "agent", "role": "planner", "output": "verdict" },
        { "id": "r", "kind": "router" },
        { "id": "x", "kind": "router" },
        { "id": "y", "kind": "router" },
        { "id": "z", "kind": "router" }
      ],
      "edges": [
        { "from": "a", "to": "r" },
        { "from": "r", "to": "x", "when": { "slot": "verdict", "path": "verdict", "in": ["rework", "replan"] } },
        { "from": "r", "to": "y", "when": { "slot": "verdict", "path": "verdict", "ne": "pass" } },
        { "from": "r", "to": "z" }
      ]
    }"#;

    #[test]
    fn declaration_order_decides_when_several_conditions_hold() {
        let g = compiled(ROUTER, "r");
        // Both x (in) and y (ne) hold: the one declared first wins.
        let v = json!({"verdict": "rework", "findings": []});
        let Route::Edge(e) = pick(&g, "r", &v) else { panic!() };
        assert_eq!(e.to.0, "x");
    }

    #[test]
    fn the_unconditional_edge_is_the_default_even_when_declared_last() {
        let g = compiled(ROUTER, "r");
        let v = json!({"verdict": "pass", "findings": []});
        let Route::Edge(e) = pick(&g, "r", &v) else { panic!() };
        assert_eq!(e.to.0, "z");
    }

    #[test]
    fn a_missing_value_is_not_guessed() {
        let g = compiled(ROUTER, "r");
        assert!(matches!(select_edge(&g, &NodeId::new("r"), |_| None), Route::Unresolved(_)));
        let v = json!({"findings": []});
        assert!(matches!(pick(&g, "r", &v), Route::Unresolved(_)));
    }
}
