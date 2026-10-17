//! Output schemas (DESIGN_ORG_GRAPH 6.6). One definition produces both the JSON sent as
//! `outputSchema` and the validation of the value received, so the two cannot drift.
//!
//! A small own subset of JSON Schema (no new dependency): `object` (properties, required,
//! additionalProperties fixed to false), `array` (items, maxItems), `string` (maxLength, enum),
//! `integer` (minimum, maximum) and `boolean`. Depth at most 4 containers, at most 32 properties.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

pub const MAX_DEPTH: usize = 4;
pub const MAX_PROPERTIES: usize = 32;
pub const DOCUMENT_BODY_MAX_CHARS: u32 = 60_000;

pub const SCHEMA_PLAN: &str = "plan";
pub const SCHEMA_WORKER_REPORT: &str = "workerReport";
pub const SCHEMA_REVIEWER_VERDICT: &str = "reviewerVerdict";
pub const SCHEMA_VERDICT3: &str = "verdict3";
pub const SCHEMA_SPEC: &str = "spec";
pub const SCHEMA_DOCUMENT: &str = "document";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Object { properties: Vec<(String, Node)>, required: Vec<String> },
    Array { items: Box<Node>, max_items: Option<u32> },
    String { max_length: Option<u32>, enum_values: Option<Vec<String>> },
    Integer { minimum: Option<i64>, maximum: Option<i64> },
    Boolean,
}

/// A schema is its root node (an object for everything sent to the model).
pub type OutSchema = Node;

impl Node {
    pub fn string() -> Node {
        Node::String { max_length: None, enum_values: None }
    }
    pub fn string_max(max_length: u32) -> Node {
        Node::String { max_length: Some(max_length), enum_values: None }
    }
    pub fn enumeration(values: &[&str]) -> Node {
        Node::String { max_length: None, enum_values: Some(values.iter().map(|s| s.to_string()).collect()) }
    }
    pub fn array(items: Node) -> Node {
        Node::Array { items: Box::new(items), max_items: None }
    }
    pub fn integer(minimum: i64, maximum: i64) -> Node {
        Node::Integer { minimum: Some(minimum), maximum: Some(maximum) }
    }
    /// An object. `required` names must be properties.
    pub fn object(properties: Vec<(&str, Node)>, required: &[&str]) -> Node {
        Node::Object {
            properties: properties.into_iter().map(|(n, v)| (n.to_string(), v)).collect(),
            required: required.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The JSON sent as `outputSchema`.
    pub fn to_json(&self) -> Value {
        match self {
            Node::Object { properties, required } => {
                let mut props = Map::new();
                for (n, v) in properties {
                    props.insert(n.clone(), v.to_json());
                }
                json!({ "type": "object", "additionalProperties": false, "required": required, "properties": props })
            }
            Node::Array { items, max_items } => {
                let mut m = Map::new();
                m.insert("type".into(), json!("array"));
                m.insert("items".into(), items.to_json());
                if let Some(n) = max_items {
                    m.insert("maxItems".into(), json!(n));
                }
                Value::Object(m)
            }
            Node::String { max_length, enum_values } => {
                let mut m = Map::new();
                m.insert("type".into(), json!("string"));
                if let Some(n) = max_length {
                    m.insert("maxLength".into(), json!(n));
                }
                if let Some(e) = enum_values {
                    m.insert("enum".into(), json!(e));
                }
                Value::Object(m)
            }
            Node::Integer { minimum, maximum } => {
                let mut m = Map::new();
                m.insert("type".into(), json!("integer"));
                if let Some(n) = minimum {
                    m.insert("minimum".into(), json!(n));
                }
                if let Some(n) = maximum {
                    m.insert("maximum".into(), json!(n));
                }
                Value::Object(m)
            }
            Node::Boolean => json!({ "type": "boolean" }),
        }
    }

    /// Check a received value against the same definition. Returns every mismatch with its path.
    pub fn validate(&self, value: &Value) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        self.check(value, "$", &mut errs);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }

    fn check(&self, v: &Value, path: &str, errs: &mut Vec<String>) {
        match self {
            Node::Object { properties, required } => {
                let Some(obj) = v.as_object() else {
                    errs.push(format!("{path}: オブジェクトではありません"));
                    return;
                };
                for k in obj.keys() {
                    if !properties.iter().any(|(n, _)| n == k) {
                        errs.push(format!("{path}: 未知の項目「{k}」"));
                    }
                }
                for r in required {
                    if !obj.contains_key(r) {
                        errs.push(format!("{path}: 必須項目「{r}」がありません"));
                    }
                }
                for (n, node) in properties {
                    if let Some(child) = obj.get(n) {
                        node.check(child, &format!("{path}.{n}"), errs);
                    }
                }
            }
            Node::Array { items, max_items } => {
                let Some(a) = v.as_array() else {
                    errs.push(format!("{path}: 配列ではありません"));
                    return;
                };
                if let Some(m) = max_items {
                    if a.len() > *m as usize {
                        errs.push(format!("{path}: 要素が{m}個を超えています"));
                    }
                }
                for (i, e) in a.iter().enumerate() {
                    items.check(e, &format!("{path}[{i}]"), errs);
                }
            }
            Node::String { max_length, enum_values } => {
                let Some(s) = v.as_str() else {
                    errs.push(format!("{path}: 文字列ではありません"));
                    return;
                };
                if let Some(m) = max_length {
                    if s.chars().count() > *m as usize {
                        errs.push(format!("{path}: {m}文字を超えています"));
                    }
                }
                if let Some(e) = enum_values {
                    if !e.iter().any(|x| x == s) {
                        errs.push(format!("{path}: 「{s}」は選択肢にありません"));
                    }
                }
            }
            Node::Integer { minimum, maximum } => {
                let Some(n) = v.as_i64() else {
                    errs.push(format!("{path}: 整数ではありません"));
                    return;
                };
                if minimum.is_some_and(|m| n < m) || maximum.is_some_and(|m| n > m) {
                    errs.push(format!("{path}: 範囲外です（{n}）"));
                }
            }
            Node::Boolean => {
                if !v.is_boolean() {
                    errs.push(format!("{path}: 真偽値ではありません"));
                }
            }
        }
    }

    /// Depth (containers nested) and total property count must stay within the limits.
    pub fn check_limits(&self) -> Result<(), String> {
        fn walk(n: &Node, depth: usize, props: &mut usize) -> usize {
            match n {
                Node::Object { properties, .. } => {
                    *props += properties.len();
                    1 + properties.iter().map(|(_, c)| walk(c, depth + 1, props)).max().unwrap_or(0)
                }
                Node::Array { items, .. } => 1 + walk(items, depth + 1, props),
                _ => 0,
            }
        }
        let mut props = 0;
        let depth = walk(self, 1, &mut props);
        if depth > MAX_DEPTH {
            return Err(format!("スキーマの深さが{MAX_DEPTH}を超えています"));
        }
        if props > MAX_PROPERTIES {
            return Err(format!("スキーマの項目数が{MAX_PROPERTIES}を超えています"));
        }
        Ok(())
    }

    /// The node at a dot separated path (object properties only; no array indexes).
    pub fn lookup(&self, path: &str) -> Option<&Node> {
        let mut cur = self;
        for part in path.split('.') {
            let Node::Object { properties, .. } = cur else { return None };
            cur = &properties.iter().find(|(n, _)| n == part)?.1;
        }
        Some(cur)
    }
}

/// The implicit output schema of a gate: `{choice, note}`.
pub fn gate_schema(choice_ids: &[String]) -> OutSchema {
    Node::Object {
        properties: vec![
            ("choice".into(), Node::String { max_length: None, enum_values: Some(choice_ids.to_vec()) }),
            ("note".into(), Node::string()),
        ],
        required: vec!["choice".into(), "note".into()],
    }
}

fn plan() -> OutSchema {
    // Same shape as `schemas::planner_plan_schema` (a test keeps them equal).
    Node::object(
        vec![(
            "tasks",
            Node::array(Node::object(
                vec![
                    ("id", Node::string()),
                    ("role", Node::enumeration(&["worker", "reviewer"])),
                    ("deps", Node::array(Node::string())),
                    (
                        "effects",
                        Node::array(Node::enumeration(&[
                            "userTreeWrite",
                            "pushMergeTag",
                            "outsideWorkdir",
                            "networkOrInstall",
                            "secretsOrAuth",
                            "deletionPrimary",
                        ])),
                    ),
                    ("title", Node::string()),
                    ("instructions", Node::string()),
                ],
                &["id", "role", "deps", "effects", "title", "instructions"],
            )),
        )],
        &["tasks"],
    )
}

fn worker_report() -> OutSchema {
    Node::object(
        vec![
            ("summary", Node::string()),
            ("changedFiles", Node::array(Node::string())),
            ("tests", Node::object(vec![("ran", Node::Boolean), ("passed", Node::Boolean)], &["ran", "passed"])),
        ],
        &["summary", "changedFiles", "tests"],
    )
}

fn reviewer_verdict() -> OutSchema {
    Node::object(
        vec![("verdict", Node::enumeration(&["pass", "rework"])), ("findings", Node::array(Node::string()))],
        &["verdict", "findings"],
    )
}

/// pass / rework / replan with findings and an optional score (0 to 10, not required).
fn verdict3() -> OutSchema {
    Node::object(
        vec![
            ("verdict", Node::enumeration(&["pass", "rework", "replan"])),
            ("findings", Node::array(Node::string())),
            ("score", Node::integer(0, 10)),
        ],
        &["verdict", "findings"],
    )
}

fn spec() -> OutSchema {
    Node::object(
        vec![("goal", Node::string()), ("steps", Node::array(Node::string())), ("acceptance", Node::array(Node::string()))],
        &["goal", "steps", "acceptance"],
    )
}

fn document() -> OutSchema {
    Node::object(
        vec![
            ("title", Node::string()),
            ("body", Node::string_max(DOCUMENT_BODY_MAX_CHARS)),
            ("notes", Node::array(Node::string())),
        ],
        &["title", "body", "notes"],
    )
}

/// The six built-in schemas by id.
pub fn builtin_schemas() -> BTreeMap<String, OutSchema> {
    [
        (SCHEMA_PLAN, plan()),
        (SCHEMA_WORKER_REPORT, worker_report()),
        (SCHEMA_REVIEWER_VERDICT, reviewer_verdict()),
        (SCHEMA_VERDICT3, verdict3()),
        (SCHEMA_SPEC, spec()),
        (SCHEMA_DOCUMENT, document()),
    ]
    .into_iter()
    .map(|(id, s)| (id.to_string(), s))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas;

    #[test]
    fn builtin_schemas_are_six_and_within_limits() {
        let all = builtin_schemas();
        assert_eq!(all.len(), 6);
        for (id, s) in &all {
            s.check_limits().unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    #[test]
    fn existing_three_are_identical_to_what_is_sent_today() {
        let all = builtin_schemas();
        assert_eq!(all[SCHEMA_PLAN].to_json(), schemas::planner_plan_schema());
        assert_eq!(all[SCHEMA_WORKER_REPORT].to_json(), schemas::worker_report_schema());
        assert_eq!(all[SCHEMA_REVIEWER_VERDICT].to_json(), schemas::reviewer_verdict_schema());
    }

    #[test]
    fn values_built_from_the_same_definition_pass() {
        let all = builtin_schemas();
        let ok = [
            (SCHEMA_PLAN, json!({"tasks": [{"id": "t1", "role": "worker", "deps": [], "effects": ["userTreeWrite"], "title": "t", "instructions": "i"}]})),
            (SCHEMA_WORKER_REPORT, json!({"summary": "s", "changedFiles": ["a"], "tests": {"ran": true, "passed": true}})),
            (SCHEMA_REVIEWER_VERDICT, json!({"verdict": "rework", "findings": ["x"]})),
            (SCHEMA_VERDICT3, json!({"verdict": "replan", "findings": [], "score": 7})),
            (SCHEMA_VERDICT3, json!({"verdict": "pass", "findings": []})),
            (SCHEMA_SPEC, json!({"goal": "g", "steps": ["a"], "acceptance": ["b"]})),
            (SCHEMA_DOCUMENT, json!({"title": "t", "body": "# md", "notes": []})),
        ];
        for (id, v) in ok {
            all[id].validate(&v).unwrap_or_else(|e| panic!("{id}: {e:?}"));
        }
    }

    #[test]
    fn sent_json_declares_exactly_what_validation_enforces() {
        // Every object in the sent JSON closes additionalProperties and lists its required names.
        fn walk(v: &Value) {
            if v["type"] == "object" {
                assert_eq!(v["additionalProperties"], json!(false));
                for r in v["required"].as_array().unwrap() {
                    assert!(v["properties"].get(r.as_str().unwrap()).is_some());
                }
                v["properties"].as_object().unwrap().values().for_each(walk);
            } else if v["type"] == "array" {
                walk(&v["items"]);
            }
        }
        for s in builtin_schemas().values() {
            walk(&s.to_json());
        }
        let sent = builtin_schemas()[SCHEMA_VERDICT3].to_json();
        assert_eq!(sent["properties"]["score"], json!({"type": "integer", "minimum": 0, "maximum": 10}));
        assert_eq!(sent["required"], json!(["verdict", "findings"]));
        assert_eq!(builtin_schemas()[SCHEMA_DOCUMENT].to_json()["properties"]["body"]["maxLength"], json!(60000));
    }

    #[test]
    fn mismatching_values_fail_with_paths() {
        let all = builtin_schemas();
        let v3 = &all[SCHEMA_VERDICT3];
        assert!(v3.validate(&json!({"verdict": "maybe", "findings": []})).is_err());
        assert!(v3.validate(&json!({"verdict": "pass"})).is_err());
        assert!(v3.validate(&json!({"verdict": "pass", "findings": [], "extra": 1})).is_err());
        assert!(v3.validate(&json!({"verdict": "pass", "findings": [], "score": 11})).is_err());
        assert!(v3.validate(&json!({"verdict": "pass", "findings": [], "score": 1.5})).is_err());
        assert!(v3.validate(&json!({"verdict": "pass", "findings": [1]})).is_err());
        let doc = &all[SCHEMA_DOCUMENT];
        let long = "あ".repeat(60_001);
        assert!(doc.validate(&json!({"title": "t", "body": long, "notes": []})).is_err());
        let ok = "あ".repeat(60_000);
        assert!(doc.validate(&json!({"title": "t", "body": ok, "notes": []})).is_ok());
        let wr = &all[SCHEMA_WORKER_REPORT];
        let errs = wr.validate(&json!({"summary": "s", "changedFiles": [], "tests": {"ran": "yes", "passed": true}})).unwrap_err();
        assert!(errs[0].contains("$.tests.ran"));
    }

    #[test]
    fn lookup_follows_dot_paths_and_gate_schema_is_choice_and_note() {
        let all = builtin_schemas();
        assert!(matches!(all[SCHEMA_WORKER_REPORT].lookup("tests.passed"), Some(Node::Boolean)));
        assert!(all[SCHEMA_WORKER_REPORT].lookup("changedFiles.0").is_none());
        assert!(all[SCHEMA_WORKER_REPORT].lookup("nope").is_none());
        let g = gate_schema(&["save".to_string(), "fix".to_string()]);
        assert!(g.validate(&json!({"choice": "fix", "note": "理由"})).is_ok());
        assert!(g.validate(&json!({"choice": "other", "note": ""})).is_err());
    }

    #[test]
    fn limits_reject_deep_or_wide_schemas() {
        let deep = Node::array(Node::array(Node::array(Node::array(Node::array(Node::string())))));
        assert!(deep.check_limits().is_err());
        let wide = Node::Object {
            properties: (0..33).map(|i| (format!("p{i}"), Node::Boolean)).collect(),
            required: vec![],
        };
        assert!(wide.check_limits().is_err());
    }
}
