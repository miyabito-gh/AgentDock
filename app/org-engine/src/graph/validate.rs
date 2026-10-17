//! Graph validation V1 to V12 (DESIGN_ORG_GRAPH 6.4). A pure function: every error is returned,
//! and a definition that passes becomes an immutable `CompiledGraph` (the only thing the engine
//! looks at).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

use serde_json::Value;

use super::def::*;
use crate::outschema::{gate_schema, Node, OutSchema, SCHEMA_PLAN};
use crate::types::Limits;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_NODES: usize = 16;
pub const MAX_EDGES: usize = 32;
pub const MAX_ID_CHARS: usize = 40;
pub const MAX_ROUNDS_PER_EDGE: u32 = 5;
/// V11: the most activations (tasks run) a definition may allow.
pub const MAX_ACTIVATIONS: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphError {
    /// "V1" to "V12".
    pub rule: &'static str,
    pub message: String,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

/// What validation needs to know about a role. Permissions stay in the role definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleInfo {
    /// The role's sandbox is workspaceWrite.
    pub writes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopBody {
    pub edge: EdgeId,
    /// T of the feedback edge S -> T.
    pub target: NodeId,
    pub source: NodeId,
    /// The forward path from `target` to `source`, both included, in that order.
    pub members: Vec<NodeId>,
    pub max_rounds: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationBound {
    pub activations: u32,
    /// Normal case: one turn per activation.
    pub turns_typical: u32,
    /// Worst case: every turn needs one format fix.
    pub turns_max: u32,
}

/// A validated graph (immutable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledGraph {
    pub def: GraphDef,
    /// FNV-1a 64 of the definition text (same method as the role permission fingerprint).
    pub fingerprint: String,
    /// Node -> forward parent (at most one, V5).
    pub parent: HashMap<NodeId, Option<NodeId>>,
    pub loops: Vec<LoopBody>,
    pub bound: ActivationBound,
}

impl CompiledGraph {
    pub fn node(&self, id: &NodeId) -> Option<&NodeDef> {
        self.def.nodes.iter().find(|n| &n.id == id)
    }
    /// Outgoing edges in declaration order.
    pub fn outgoing<'a>(&'a self, from: &'a NodeId) -> impl Iterator<Item = &'a EdgeDef> + 'a {
        self.def.edges.iter().filter(move |e| &e.from == from)
    }
}

#[derive(Default)]
struct Errs(Vec<GraphError>);
impl Errs {
    fn add(&mut self, rule: &'static str, message: impl Into<String>) {
        self.0.push(GraphError { rule, message: message.into() });
    }
}

fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= MAX_ID_CHARS && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn fingerprint_of(def: &GraphDef) -> String {
    let text = serde_json::to_string(def).unwrap_or_default();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Forward path from the ancestor `top` down to `bottom` (both included), following parents.
fn path_down(parent: &HashMap<NodeId, Option<NodeId>>, top: &NodeId, bottom: &NodeId, limit: usize) -> Option<Vec<NodeId>> {
    let mut path = vec![bottom.clone()];
    let mut cur = bottom;
    for _ in 0..=limit {
        if cur == top {
            path.reverse();
            return Some(path);
        }
        match parent.get(cur) {
            Some(Some(p)) => {
                path.push(p.clone());
                cur = p;
            }
            _ => return None,
        }
    }
    None
}

fn leaf_accepts(leaf: &Node, v: &Value) -> bool {
    match leaf {
        Node::String { enum_values: Some(e), .. } => v.as_str().is_some_and(|s| e.iter().any(|x| x == s)),
        Node::Boolean => v.is_boolean(),
        Node::Integer { minimum, maximum } => v.as_i64().is_some_and(|n| minimum.is_none_or(|m| n >= m) && maximum.is_none_or(|m| n <= m)),
        _ => false,
    }
}

fn is_condition_leaf(leaf: &Node) -> bool {
    matches!(leaf, Node::String { enum_values: Some(_), .. } | Node::Boolean | Node::Integer { .. })
}

pub fn validate(
    def: &GraphDef,
    roles: &BTreeMap<RoleId, RoleInfo>,
    schemas: &BTreeMap<String, OutSchema>,
    file_stem: &str,
    limits: &Limits,
) -> Result<CompiledGraph, Vec<GraphError>> {
    let mut e = Errs::default();

    // V1: format and size.
    if def.schema_version != SCHEMA_VERSION {
        e.add("V1", format!("schemaVersion {} は未対応です（対応: {SCHEMA_VERSION}）", def.schema_version));
    }
    if !id_ok(&def.id) {
        e.add("V1", "id は小文字英数字・-・_ の1〜40文字にしてください");
    } else if def.id != file_stem {
        e.add("V1", format!("id「{}」がファイル名「{file_stem}」と一致しません", def.id));
    }
    if def.nodes.is_empty() {
        e.add("V1", "ノードがありません");
    }
    if def.nodes.len() > MAX_NODES {
        e.add("V1", format!("ノードは{MAX_NODES}個までです（{}個）", def.nodes.len()));
    }
    if def.edges.len() > MAX_EDGES {
        e.add("V1", format!("辺は{MAX_EDGES}本までです（{}本）", def.edges.len()));
    }
    for n in &def.nodes {
        if !id_ok(&n.id.0) {
            e.add("V1", format!("ノードID「{}」は小文字英数字・-・_ の1〜40文字にしてください", n.id));
        }
    }
    for s in def.slots.keys() {
        if !id_ok(&s.0) {
            e.add("V1", format!("スロットID「{s}」は小文字英数字・-・_ の1〜40文字にしてください"));
        }
    }
    let mut seen: HashSet<&NodeId> = HashSet::new();
    let mut structural = false;
    for n in &def.nodes {
        if !seen.insert(&n.id) {
            e.add("V1", format!("ノードID「{}」が重複しています", n.id));
            structural = true;
        }
    }
    // References that later steps rely on.
    if !seen.contains(&def.entry) {
        e.add("V2", format!("entry「{}」というノードがありません", def.entry));
        structural = true;
    }
    for ed in &def.edges {
        for end in [&ed.from, &ed.to] {
            if !seen.contains(end) {
                e.add("V2", format!("辺 {} のノード「{end}」がありません", ed.id()));
                structural = true;
            }
        }
    }
    if structural {
        return Err(e.0);
    }

    let n_nodes = def.nodes.len();
    let kind_of = |id: &NodeId| def.nodes.iter().find(|n| &n.id == id).map(|n| &n.kind);
    let out_of = |id: &NodeId| def.edges.iter().filter(move |x| &x.from == id).collect::<Vec<_>>();

    // V5: at most one forward incoming edge per node; the forward part is a tree from the entry.
    let mut parent: HashMap<NodeId, Option<NodeId>> = def.nodes.iter().map(|n| (n.id.clone(), None)).collect();
    let mut fwd_in: HashMap<&NodeId, usize> = HashMap::new();
    for ed in def.edges.iter().filter(|x| x.feedback.is_none()) {
        let c = fwd_in.entry(&ed.to).or_insert(0);
        *c += 1;
        if *c == 1 {
            parent.insert(ed.to.clone(), Some(ed.from.clone()));
        }
    }
    for (id, c) in &fwd_in {
        if *c > 1 {
            e.add("V5", format!("ノード「{id}」に入る前向きの辺が{c}本あります（1本まで）"));
        }
    }

    // V2: everything reachable from the entry by forward edges.
    let mut reach: HashSet<&NodeId> = HashSet::new();
    let mut queue: VecDeque<&NodeId> = VecDeque::new();
    reach.insert(&def.entry);
    queue.push_back(&def.entry);
    while let Some(cur) = queue.pop_front() {
        for ed in def.edges.iter().filter(|x| x.feedback.is_none() && &x.from == cur) {
            if reach.insert(&ed.to) {
                queue.push_back(&ed.to);
            }
        }
    }
    for n in &def.nodes {
        if !reach.contains(&n.id) {
            e.add("V2", format!("ノード「{}」は entry から前向きの辺で到達できません", n.id));
        }
    }

    // V3: no cycle among non-feedback edges (Kahn).
    {
        let mut indeg: HashMap<&NodeId, usize> = def.nodes.iter().map(|n| (&n.id, 0)).collect();
        for ed in def.edges.iter().filter(|x| x.feedback.is_none()) {
            *indeg.get_mut(&ed.to).expect("checked") += 1;
        }
        let mut q: VecDeque<&NodeId> = indeg.iter().filter(|(_, c)| **c == 0).map(|(k, _)| *k).collect();
        let mut done = 0;
        while let Some(cur) = q.pop_front() {
            done += 1;
            for ed in def.edges.iter().filter(|x| x.feedback.is_none() && &x.from == cur) {
                let c = indeg.get_mut(&ed.to).expect("checked");
                *c -= 1;
                if *c == 0 {
                    q.push_back(&ed.to);
                }
            }
        }
        if done < n_nodes {
            e.add("V3", "フィードバックでない辺に循環があります（循環はフィードバックの辺だけで作れます）");
        }
    }

    // V4: feedback edges point backwards; V11 and V7 use the bodies found here.
    let mut loops: Vec<LoopBody> = Vec::new();
    for ed in def.edges.iter().filter(|x| x.feedback.is_some()) {
        let fb = ed.feedback.as_ref().expect("filtered");
        if ed.when.is_none() {
            e.add("V4", format!("フィードバックの辺 {} には when が必要です（無条件の戻りは毎回上限まで回ります）", ed.id()));
        }
        if fb.max_rounds < 1 || fb.max_rounds > MAX_ROUNDS_PER_EDGE {
            e.add("V4", format!("フィードバックの辺 {} の maxRounds は1〜{MAX_ROUNDS_PER_EDGE}にしてください", ed.id()));
        }
        match path_down(&parent, &ed.to, &ed.from, n_nodes) {
            Some(members) => loops.push(LoopBody {
                edge: ed.id(),
                target: ed.to.clone(),
                source: ed.from.clone(),
                members,
                max_rounds: fb.max_rounds.clamp(1, MAX_ROUNDS_PER_EDGE),
            }),
            None => e.add("V4", format!("フィードバックの辺 {} は後ろ向きではありません（{} から {} へ前向きの辺で到達できません）", ed.id(), ed.to, ed.from)),
        }
    }

    // V6: outgoing edges.
    for n in &def.nodes {
        let out = out_of(&n.id);
        let plain = out.iter().filter(|x| x.when.is_none()).count();
        if plain > 1 {
            e.add("V6", format!("ノード「{}」から出る無条件の辺が{plain}本あります（扇状の分岐は未対応。無条件は1本まで）", n.id));
        }
        let mut pairs: HashSet<EdgeId> = HashSet::new();
        for x in &out {
            if !pairs.insert(x.id()) {
                e.add("V6", format!("同じ組の辺 {} が複数あります", x.id()));
            }
        }
    }

    // V7: loop bodies nest or are disjoint, and a body is left only through its source.
    for (i, a) in loops.iter().enumerate() {
        let sa: HashSet<&NodeId> = a.members.iter().collect();
        for b in loops.iter().skip(i + 1) {
            let sb: HashSet<&NodeId> = b.members.iter().collect();
            let inter = sa.intersection(&sb).count();
            if inter > 0 && inter != sa.len() && inter != sb.len() {
                e.add("V7", format!("周回 {} と {} が一部だけ重なっています（入れ子か、交わらないようにしてください）", a.edge, b.edge));
            }
        }
        for m in a.members.iter().filter(|m| **m != a.source) {
            for x in out_of(m) {
                if !sa.contains(&x.to) {
                    e.add("V7", format!("周回 {} の途中のノード「{m}」から本体の外へ出る辺 {} があります（出口は「{}」だけ）", a.edge, x.id(), a.source));
                }
            }
        }
    }

    // V8: agents.
    for (slot_id, slot) in &def.slots {
        if !schemas.contains_key(&slot.schema) {
            e.add("V8", format!("スロット「{slot_id}」のスキーマ「{}」が見つかりません", slot.schema));
        }
    }
    for n in &def.nodes {
        let NodeKind::Agent { role, output, reads, capture, target, memory, .. } = &n.kind else { continue };
        let writes = match roles.get(role) {
            Some(r) => r.writes,
            None => {
                e.add("V8", format!("ノード「{}」の役割「{role}」が役割表にありません", n.id));
                false
            }
        };
        if !def.slots.contains_key(output) {
            e.add("V8", format!("ノード「{}」の output「{output}」が slots にありません", n.id));
        }
        for r in reads {
            if !def.slots.contains_key(r) {
                e.add("V8", format!("ノード「{}」の reads「{r}」が slots にありません", n.id));
            }
        }
        if *memory != MemoryUse::None {
            e.add("V8", format!("ノード「{}」の memory は none のみ使えます", n.id));
        }
        if roles.contains_key(role) {
            match (writes, capture) {
                (true, None) => e.add("V8", format!("書込み役のノード「{}」には capture が必要です", n.id)),
                (true, Some(c)) => {
                    let want = match def.workspace {
                        WorkspaceKind::Git => CaptureKind::Commit,
                        WorkspaceKind::Folder => CaptureKind::Snapshot,
                    };
                    if *c != want {
                        e.add("V8", format!("ノード「{}」の capture は workspace が {:?} のとき {want:?} にしてください", n.id, def.workspace));
                    }
                }
                (false, Some(_)) => e.add("V8", format!("読取り役のノード「{}」に capture は指定できません", n.id)),
                (false, None) => {}
            }
        }
        if let Some(t) = target {
            let t_writer = matches!(kind_of(t), Some(NodeKind::Agent { role: r, .. }) if roles.get(r).is_some_and(|i| i.writes));
            if !t_writer {
                e.add("V8", format!("ノード「{}」の target「{t}」は書込み役のノードではありません", n.id));
            } else {
                let above = parent.get(&n.id).cloned().flatten();
                let found = above.is_some_and(|p| path_down(&parent, t, &p, n_nodes).is_some());
                if !found {
                    e.add("V8", format!("ノード「{}」の target「{t}」は前向きにさかのぼれるノードではありません", n.id));
                }
            }
        }
    }

    // V9: conditions.
    for ed in &def.edges {
        let Some(c) = &ed.when else { continue };
        let gate_from = matches!(kind_of(&ed.from), Some(NodeKind::Gate { .. }));
        let schema: Option<OutSchema> = match kind_of(&NodeId::new(c.slot.0.clone())) {
            Some(NodeKind::Gate { choices, .. }) => {
                Some(gate_schema(&choices.iter().map(|x| x.id.clone()).collect::<Vec<_>>()))
            }
            _ => def.slots.get(&c.slot).and_then(|s| schemas.get(&s.schema)).cloned(),
        };
        if gate_from && c.slot.0 != ed.from.0 {
            e.add("V9", format!("gate からの辺 {} の条件は gate 自身の出力「{}」を参照してください", ed.id(), ed.from));
        }
        let Some(schema) = schema else {
            e.add("V9", format!("辺 {} の条件のスロット「{}」が見つかりません", ed.id(), c.slot));
            continue;
        };
        let given = [c.eq.is_some(), c.in_.is_some(), c.ne.is_some()].iter().filter(|b| **b).count();
        if given != 1 {
            e.add("V9", format!("辺 {} の条件は eq・in・ne のうち1つだけ指定してください", ed.id()));
        }
        let Some(leaf) = schema.lookup(&c.path).filter(|l| is_condition_leaf(l)) else {
            e.add("V9", format!("辺 {} の条件の path「{}」は enum・boolean・integer の項目ではありません", ed.id(), c.path));
            continue;
        };
        let values: Vec<&Value> = c.eq.iter().chain(c.ne.iter()).chain(c.in_.iter().flatten()).collect();
        if matches!(&c.in_, Some(v) if v.is_empty()) {
            e.add("V9", format!("辺 {} の in が空です", ed.id()));
        }
        if let Some(bad) = values.iter().find(|v| !leaf_accepts(leaf, v)) {
            e.add("V9", format!("辺 {} の条件の値 {bad} は「{}」の型・選択肢の外です", ed.id(), c.path));
        }
    }

    // V10: plannedTasks.
    let planned: Vec<&NodeDef> = def.nodes.iter().filter(|n| matches!(n.kind, NodeKind::PlannedTasks { .. })).collect();
    if planned.len() > 1 {
        e.add("V10", "plannedTasks はグラフに1つまでです");
    }
    for n in &planned {
        let NodeKind::PlannedTasks { from, allowed_roles } = &n.kind else { continue };
        if !out_of(&n.id).is_empty() {
            e.add("V10", format!("plannedTasks「{}」は葉でなければなりません（出る辺なし）", n.id));
        }
        match def.slots.get(from) {
            Some(s) if s.schema == SCHEMA_PLAN => {}
            Some(_) => e.add("V10", format!("plannedTasks「{}」の from「{from}」は plan スキーマのスロットではありません", n.id)),
            None => e.add("V10", format!("plannedTasks「{}」の from「{from}」が slots にありません", n.id)),
        }
        if allowed_roles.is_empty() {
            e.add("V10", format!("plannedTasks「{}」の allowedRoles が空です", n.id));
        }
        for r in allowed_roles {
            if !roles.contains_key(r) {
                e.add("V10", format!("plannedTasks「{}」の allowedRoles「{r}」が役割表にありません", n.id));
            }
        }
    }

    // V11: static bound on activations.
    let mut total: u64 = n_nodes as u64;
    for l in &loops {
        total += l.members.len() as u64 * l.max_rounds as u64;
    }
    if !planned.is_empty() {
        total += limits.max_tasks as u64 * (1 + limits.max_rework as u64);
    }
    let activations = u32::try_from(total).unwrap_or(u32::MAX);
    let bound = ActivationBound { activations, turns_typical: activations, turns_max: activations.saturating_mul(2) };
    if activations > MAX_ACTIVATIONS {
        e.add("V11", format!("最大活動数が{activations}で、上限{MAX_ACTIVATIONS}を超えます（周回の回数かノード数を減らしてください）"));
    }

    // V12: gates.
    for n in &def.nodes {
        let NodeKind::Gate { choices, .. } = &n.kind else { continue };
        if choices.is_empty() {
            e.add("V12", format!("gate「{}」に選択肢がありません", n.id));
        }
        let mut ids: HashSet<&str> = HashSet::new();
        for c in choices {
            if c.id.is_empty() || !ids.insert(&c.id) {
                e.add("V12", format!("gate「{}」の選択肢ID「{}」が空または重複しています", n.id, c.id));
            }
            let has_edge = def.edges.iter().any(|x| {
                x.from == n.id
                    && x.when.as_ref().is_some_and(|w| {
                        w.slot.0 == n.id.0 && w.path == "choice" && w.eq.as_ref().and_then(|v| v.as_str()) == Some(c.id.as_str())
                    })
            });
            if !has_edge {
                e.add("V12", format!("gate「{}」の選択肢「{}」に対応する辺がありません", n.id, c.id));
            }
        }
    }

    if !e.0.is_empty() {
        return Err(e.0);
    }
    Ok(CompiledGraph { def: def.clone(), fingerprint: fingerprint_of(def), parent, loops, bound })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::templates::{builtin_templates, template_json, TEMPLATE_QUALITY_LOOP, TEMPLATE_STANDARD};
    use crate::outschema::builtin_schemas;

    fn roles() -> BTreeMap<RoleId, RoleInfo> {
        [("planner", false), ("worker", true), ("reviewer", false), ("verifier", false), ("writer", true)]
            .into_iter()
            .map(|(id, writes)| (RoleId::new(id), RoleInfo { writes }))
            .collect()
    }

    fn check(def: &GraphDef) -> Result<CompiledGraph, Vec<GraphError>> {
        validate(def, &roles(), &builtin_schemas(), &def.id, &Limits::default())
    }

    fn ql() -> GraphDef {
        crate::graph::def::parse_graph(template_json(TEMPLATE_QUALITY_LOOP).unwrap()).unwrap()
    }

    fn rules(r: Result<CompiledGraph, Vec<GraphError>>) -> Vec<&'static str> {
        r.unwrap_err().into_iter().map(|e| e.rule).collect()
    }

    fn agent(id: &str) -> NodeDef {
        NodeDef {
            id: NodeId::new(id),
            label: None,
            kind: NodeKind::Agent {
                role: RoleId::new("planner"),
                output: SlotId::new("v"),
                reads: vec![],
                instructions: String::new(),
                capture: None,
                target: None,
                memory: MemoryUse::None,
            },
        }
    }

    fn edge(from: &str, to: &str) -> EdgeDef {
        EdgeDef { from: NodeId::new(from), to: NodeId::new(to), when: None, feedback: None }
    }

    fn back(from: &str, to: &str, rounds: u32) -> EdgeDef {
        EdgeDef {
            from: NodeId::new(from),
            to: NodeId::new(to),
            when: Some(Condition { slot: SlotId::new("v"), path: "verdict".into(), eq: Some(Value::from("rework")), in_: None, ne: None }),
            feedback: Some(Feedback { max_rounds: rounds, on_exhausted: OnExhausted::Ask }),
        }
    }

    /// n0 -> n1 -> ... -> n(count-1), every node an agent writing slot `v` (verdict3).
    fn chain(count: usize) -> GraphDef {
        let nodes: Vec<NodeDef> = (0..count).map(|i| agent(&format!("n{i}"))).collect();
        let edges = (1..count).map(|i| edge(&format!("n{}", i - 1), &format!("n{i}"))).collect();
        let slots = [(SlotId::new("v"), SlotDef { schema: "verdict3".into(), description: String::new() })].into_iter().collect();
        GraphDef {
            schema_version: 1,
            id: "t".into(),
            display_name: "t".into(),
            description: String::new(),
            workspace: WorkspaceKind::Git,
            entry: NodeId::new("n0"),
            slots,
            nodes,
            edges,
            suggested_max_turns: None,
        }
    }

    #[test]
    fn both_templates_pass_and_quality_loop_bound_is_12_and_24() {
        for t in builtin_templates() {
            check(&t).unwrap_or_else(|e| panic!("{}: {e:?}", t.id));
        }
        let g = check(&ql()).unwrap();
        assert_eq!(g.bound, ActivationBound { activations: 12, turns_typical: 12, turns_max: 24 });
        assert_eq!(g.loops.len(), 2);
        let inner = g.loops.iter().find(|l| l.target.0 == "build").unwrap();
        assert_eq!(inner.members.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["build", "verify"]);
        let std = check(&crate::graph::def::parse_graph(template_json(TEMPLATE_STANDARD).unwrap()).unwrap()).unwrap();
        assert_eq!(std.bound.activations, 2 + 20 * 3);
        assert_eq!(g.fingerprint.len(), 16);
        assert_eq!(g.fingerprint, check(&ql()).unwrap().fingerprint);
    }

    #[test]
    fn parse_refuses_unknown_fields_in_nodes_and_elsewhere() {
        let t = template_json(TEMPLATE_QUALITY_LOOP).unwrap();
        assert!(crate::graph::def::parse_graph(&t.replace("\"role\": \"planner\",", "\"role\": \"planner\", \"sandbox\": \"danger\",")).is_err());
        assert!(crate::graph::def::parse_graph(&t.replace("\"workspace\": \"git\",", "\"workspace\": \"git\", \"approval\": \"never\",")).is_err());
    }

    #[test]
    fn v1_format_and_size() {
        let mut d = ql();
        d.schema_version = 2;
        assert!(rules(check(&d)).contains(&"V1"));
        let mut d = ql();
        assert!(rules(validate(&d, &roles(), &builtin_schemas(), "other", &Limits::default())).contains(&"V1"));
        d.nodes[0].id = NodeId::new("Bad Id");
        assert!(rules(check(&d)).contains(&"V1"));
        let mut d = ql();
        d.nodes.push(d.nodes[0].clone());
        assert!(rules(check(&d)).contains(&"V1"));
        assert!(rules(check(&chain(17))).contains(&"V1"));
        let mut d = chain(2);
        d.edges = (0..33).map(|_| edge("n0", "n1")).collect();
        assert!(rules(check(&d)).contains(&"V1"));
    }

    #[test]
    fn v2_entry_and_reachability() {
        let mut d = chain(2);
        d.nodes.push(agent("orphan"));
        assert!(rules(check(&d)).contains(&"V2"));
        let mut d = chain(2);
        d.entry = NodeId::new("zz");
        assert!(rules(check(&d)).contains(&"V2"));
        let mut d = chain(2);
        d.edges.push(edge("n1", "missing"));
        assert!(rules(check(&d)).contains(&"V2"));
    }

    #[test]
    fn v3_cycle_without_feedback() {
        let mut d = chain(3);
        d.edges.push(edge("n2", "n1"));
        let r = rules(check(&d));
        assert!(r.contains(&"V3"));
    }

    #[test]
    fn v4_feedback_rules() {
        let mut d = chain(3);
        let mut b = back("n2", "n1", 2);
        b.when = None;
        d.edges.push(b);
        assert!(rules(check(&d)).contains(&"V4"));
        for rounds in [0, 6] {
            let mut d = chain(3);
            d.edges.push(back("n2", "n1", rounds));
            assert!(rules(check(&d)).contains(&"V4"), "{rounds}");
        }
        // A feedback edge pointing forward (n0 cannot be reached back from n1 -> n2 direction here).
        let mut d = chain(3);
        d.edges.push(back("n0", "n2", 2));
        assert!(rules(check(&d)).contains(&"V4"));
    }

    #[test]
    fn v5_one_forward_parent() {
        let mut d = chain(3);
        d.edges.push(edge("n0", "n2"));
        assert!(rules(check(&d)).contains(&"V5"));
    }

    #[test]
    fn v6_outgoing_edges() {
        let mut d = chain(3);
        d.nodes.push(agent("n3"));
        d.edges.push(edge("n1", "n3"));
        assert!(rules(check(&d)).contains(&"V6"));
        let mut d = chain(2);
        d.edges.push(edge("n0", "n1"));
        assert!(rules(check(&d)).contains(&"V6"));
    }

    #[test]
    fn v7_loops_nest_or_stay_apart() {
        // Partial overlap: {n1,n2} and {n2,n3}.
        let mut d = chain(4);
        d.edges.push(back("n2", "n1", 1));
        d.edges.push(back("n3", "n2", 1));
        assert!(rules(check(&d)).contains(&"V7"));
        // Leaving the body from a middle node.
        let mut d = chain(4);
        d.edges.push(edge("n1", "n3"));
        d.edges.retain(|x| !(x.from.0 == "n2" && x.to.0 == "n3"));
        d.edges.push(back("n2", "n1", 1));
        assert!(rules(check(&d)).contains(&"V7"));
    }

    #[test]
    fn v8_agent_references_and_capture() {
        let mut d = ql();
        if let NodeKind::Agent { role, .. } = &mut d.nodes[1].kind {
            *role = RoleId::new("nobody");
        }
        assert!(rules(check(&d)).contains(&"V8"));
        let mut d = ql();
        if let NodeKind::Agent { capture, .. } = &mut d.nodes[1].kind {
            *capture = None;
        }
        assert!(rules(check(&d)).contains(&"V8"));
        let mut d = ql();
        d.workspace = WorkspaceKind::Folder;
        assert!(rules(check(&d)).contains(&"V8"));
        let mut d = ql();
        if let NodeKind::Agent { reads, .. } = &mut d.nodes[2].kind {
            reads.push(SlotId::new("ghost"));
        }
        assert!(rules(check(&d)).contains(&"V8"));
        // target must be an earlier writer.
        let mut d = ql();
        if let NodeKind::Agent { target, .. } = &mut d.nodes[2].kind {
            *target = Some(NodeId::new("plan"));
        }
        assert!(rules(check(&d)).contains(&"V8"));
        let mut d = ql();
        d.slots.get_mut(&SlotId::new("spec")).unwrap().schema = "unknown".into();
        assert!(rules(check(&d)).contains(&"V8"));
    }

    #[test]
    fn v9_conditions_stay_inside_the_schema() {
        let mut d = ql();
        d.edges[2].when.as_mut().unwrap().path = "findings".into();
        assert!(rules(check(&d)).contains(&"V9"));
        let mut d = ql();
        d.edges[2].when.as_mut().unwrap().eq = Some(Value::from("maybe"));
        assert!(rules(check(&d)).contains(&"V9"));
        let mut d = ql();
        d.edges[2].when.as_mut().unwrap().in_ = Some(vec![Value::from("pass")]);
        assert!(rules(check(&d)).contains(&"V9"));
        let mut d = ql();
        d.edges[2].when.as_mut().unwrap().slot = SlotId::new("ghost");
        assert!(rules(check(&d)).contains(&"V9"));
        // Integer and boolean leaves are allowed.
        let mut d = ql();
        let w = d.edges[2].when.as_mut().unwrap();
        w.path = "score".into();
        w.eq = Some(Value::from(5));
        check(&d).unwrap();
    }

    #[test]
    fn v10_planned_tasks_scope() {
        let std = || crate::graph::def::parse_graph(template_json(TEMPLATE_STANDARD).unwrap()).unwrap();
        let mut d = std();
        d.nodes.push(agent("after"));
        d.edges.push(edge("tasks", "after"));
        assert!(rules(check(&d)).contains(&"V10"));
        let mut d = std();
        d.slots.get_mut(&SlotId::new("plan")).unwrap().schema = "spec".into();
        assert!(rules(check(&d)).contains(&"V10"));
        let mut d = std();
        if let NodeKind::PlannedTasks { allowed_roles, .. } = &mut d.nodes[1].kind {
            allowed_roles.push(RoleId::new("nobody"));
        }
        assert!(rules(check(&d)).contains(&"V10"));
        let mut d = std();
        d.nodes.push(NodeDef { id: NodeId::new("tasks2"), label: None, kind: d.nodes[1].kind.clone() });
        d.edges.push(edge("planner", "tasks2"));
        assert!(rules(check(&d)).contains(&"V10"));
    }

    #[test]
    fn v11_bound_nested_and_independent_loops() {
        // Nested: 4 nodes + inner {n1,n2} x2 + outer {n0..n3} x1 = 4 + 4 + 4.
        let mut d = chain(4);
        d.edges.push(back("n2", "n1", 2));
        d.edges.push(back("n3", "n0", 1));
        let g = check(&d).unwrap();
        assert_eq!(g.bound.activations, 12);
        // Independent: 5 nodes + {n1,n2} x2 + {n3,n4} x3 = 5 + 4 + 6.
        let mut d = chain(5);
        d.edges.push(back("n2", "n1", 2));
        d.edges.push(back("n4", "n3", 3));
        let g = check(&d).unwrap();
        assert_eq!(g.bound, ActivationBound { activations: 15, turns_typical: 15, turns_max: 30 });
        // Too many: 16 nodes + 16*5 + 15*5 = 171.
        let mut d = chain(16);
        d.edges.push(back("n15", "n0", 5));
        d.edges.push(back("n15", "n1", 5));
        assert!(rules(check(&d)).contains(&"V11"));
    }

    #[test]
    fn v11_counts_planned_tasks_with_the_limits() {
        let std = crate::graph::def::parse_graph(template_json(TEMPLATE_STANDARD).unwrap()).unwrap();
        let big = Limits { max_tasks: 40, ..Limits::default() };
        assert!(rules(validate(&std, &roles(), &builtin_schemas(), "standard", &big)).contains(&"V11"));
    }

    #[test]
    fn v12_gate_choices_need_edges() {
        let gate = |ids: &[&str]| NodeDef {
            id: NodeId::new("ask"),
            label: None,
            kind: NodeKind::Gate {
                prompt: "?".into(),
                choices: ids.iter().map(|i| GateChoice { id: i.to_string(), label: i.to_string(), needs_note: false }).collect(),
                shows: vec![],
            },
        };
        let when = |id: &str| Condition { slot: SlotId::new("ask"), path: "choice".into(), eq: Some(Value::from(id)), in_: None, ne: None };
        let build = |ids: &[&str], edge_ids: &[&str]| {
            let mut d = chain(2);
            d.nodes.push(gate(ids));
            d.edges.push(edge("n1", "ask"));
            for id in edge_ids {
                d.nodes.push(agent(&format!("to-{id}")));
                d.edges.push(EdgeDef { from: NodeId::new("ask"), to: NodeId::new(format!("to-{id}")), when: Some(when(id)), feedback: None });
            }
            d
        };
        check(&build(&["save", "fix"], &["save", "fix"])).unwrap();
        assert!(rules(check(&build(&["save", "fix"], &["save"]))).contains(&"V12"));
        assert!(rules(check(&build(&["save", "save"], &["save"]))).contains(&"V12"));
        assert!(rules(check(&build(&[], &[]))).contains(&"V12"));
    }
}
