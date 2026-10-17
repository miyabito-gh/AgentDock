//! Graph definition file types (`org\graphs\<id>.json`, DESIGN_ORG_GRAPH 6.3).
//!
//! Sandbox, approval policy and network settings are intentionally absent: permissions come from the
//! role definitions only, so a graph can never widen them.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use crate::types::RoleId;

macro_rules! graph_id {
    ($n:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $n(pub String);
        impl $n {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
        }
        impl fmt::Display for $n {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
graph_id!(NodeId);
graph_id!(SlotId);
// `"<from>-><to>"`. Unique because a (from, to) pair may appear only once (V6).
graph_id!(EdgeId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphDef {
    pub schema_version: u32,
    /// Must equal the file name (lowercase alphanumerics, `-`, `_`; 1 to 40 characters).
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub workspace: WorkspaceKind,
    pub entry: NodeId,
    #[serde(default)]
    pub slots: BTreeMap<SlotId, SlotDef>,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
    /// A suggestion for the start dialog only (the user picks the value above the default 30).
    #[serde(default)]
    pub suggested_max_turns: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceKind {
    /// Needs a Git repository. Writers use their own worktree; commits are made by AgentDock.
    Git,
    /// No Git. Writers use a dedicated work folder; results are fixed by snapshot (G-B).
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlotDef {
    /// Output schema id (built in, or `org\schemas\<id>.json`).
    pub schema: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDef {
    pub id: NodeId,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum NodeKind {
    /// One model call by a role (plus at most one format fix). The output goes to `output`.
    Agent {
        role: RoleId,
        output: SlotId,
        /// Slots handed to the role as material (latest version).
        #[serde(default)]
        reads: Vec<SlotId>,
        /// Node-specific instruction, placed after the role's own.
        #[serde(default)]
        instructions: String,
        /// How a writing role's result is fixed. Required for writers.
        #[serde(default)]
        capture: Option<CaptureKind>,
        /// The writer node whose fixed result (commit/snapshot) a read-only role verifies.
        #[serde(default)]
        target: Option<NodeId>,
        /// How project memory is passed (G-C; `none` only in G-A).
        #[serde(default)]
        memory: MemoryUse,
    },
    /// A human decision. Its implicit output slot has the same id as the node: `{choice, note}`.
    Gate {
        prompt: String,
        choices: Vec<GateChoice>,
        #[serde(default)]
        shows: Vec<SlotId>,
    },
    /// Deterministic branch; runs nothing.
    Router,
    /// Runs the Planner's plan with the existing task-table method (template 1). A leaf.
    PlannedTasks { from: SlotId, allowed_roles: Vec<RoleId> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureKind {
    Commit,
    Snapshot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryUse {
    #[default]
    None,
    Pinned,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateChoice {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub needs_note: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeDef {
    pub from: NodeId,
    pub to: NodeId,
    /// None = unconditional (the default edge, or the only plain edge).
    #[serde(default)]
    pub when: Option<Condition>,
    /// Some = a feedback (backward) edge. Only these may close a cycle.
    #[serde(default)]
    pub feedback: Option<Feedback>,
}

impl EdgeDef {
    pub fn id(&self) -> EdgeId {
        EdgeId::new(format!("{}->{}", self.from, self.to))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Feedback {
    /// 1 to 5. Counted over the whole workflow; an outer loop does not reset it.
    pub max_rounds: u32,
    #[serde(default)]
    pub on_exhausted: OnExhausted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnExhausted {
    /// The target fails with the limit reason.
    Fail,
    /// The target is blocked and the user chooses: one more round / accept / cancel.
    #[default]
    Ask,
}

/// A condition. No expressions (there is no evaluator). The path must name an enum, boolean or
/// integer field of the slot's schema (checked by V9). Exactly one of eq / in / ne is given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Condition {
    pub slot: SlotId,
    /// Dot separated (`verdict`, `checks.tests`); no array indexes.
    pub path: String,
    #[serde(default)]
    pub eq: Option<serde_json::Value>,
    #[serde(default, rename = "in")]
    pub in_: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub ne: Option<serde_json::Value>,
}

/// Parse a definition file. Unknown fields are refused, including inside nodes (serde cannot
/// combine `deny_unknown_fields` with `flatten`, so node keys are checked here).
pub fn parse_graph(text: &str) -> Result<GraphDef, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("JSONを解析できません: {e}"))?;
    if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
        for n in nodes {
            let Some(obj) = n.as_object() else { continue };
            let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let known: &[&str] = match kind {
                "agent" => &["id", "label", "kind", "role", "output", "reads", "instructions", "capture", "target", "memory"],
                "gate" => &["id", "label", "kind", "prompt", "choices", "shows"],
                "router" => &["id", "label", "kind"],
                "plannedTasks" => &["id", "label", "kind", "from", "allowedRoles"],
                _ => continue,
            };
            if let Some(k) = obj.keys().find(|k| !known.contains(&k.as_str())) {
                let id = obj.get("id").and_then(|i| i.as_str()).unwrap_or("?");
                return Err(format!("ノード「{id}」に未知の項目「{k}」があります"));
            }
        }
    }
    serde_json::from_value(v).map_err(|e| format!("定義を読めません: {e}"))
}
