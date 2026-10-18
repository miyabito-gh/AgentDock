//! グラフ定義ファイルの読み書きと、開始時の定義の写し（G-A4、DESIGN_ORG_GRAPH §6.1）。検証そのものは `org-engine::graph`。
//!
//! - 置き場所: `<root>\org\graphs\<id>.json`（1テンプレート1ファイル）。組込みの `standard`・`quality-loop` は、ファイルが無いときだけ作る
//!   （`create_new`。既存ファイルは上書きしない）。
//! - 読めないファイル・検証に通らないファイルは、動かさず（退避せず）無効として理由を返す。他のテンプレートは読む。
//! - 開始時に、ワークフローのフォルダへ `graph.json`（グラフ定義の写し）と `roles.json`（使う役割の定義と権限の指紋）を保存する。
//!   復元はこの写しで行う。定義ファイルを後で変えても、動いているワークフローの流れ・権限は変わらない。
//! - `~/.codex` は読まない・書かない。

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use org_engine::graph::{parse_graph, template_json, validate, CompiledGraph, NodeKind, RoleInfo, SlotId, TEMPLATE_QUALITY_LOOP, TEMPLATE_STANDARD};
use org_engine::outschema::builtin_schemas;
use org_engine::{permission_fingerprint, GraphRef, Limits, LoadedRole, OrgStore, RoleDefinition, RoleId, SandboxKind, WorkflowState};
use serde::{Deserialize, Serialize};

use crate::store::atomic::write_atomic;

/// 定義ファイルの大きさの上限（これを超えるものは読まない）。
const MAX_FILE_BYTES: u64 = 512 * 1024;
pub const GRAPH_COPY_FILE: &str = "graph.json";
pub const ROLES_COPY_FILE: &str = "roles.json";
const COPY_SCHEMA_VERSION: u32 = 1;

pub fn graphs_dir(root: &Path) -> PathBuf {
    root.join("org").join("graphs")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphProblem {
    pub file: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GraphReport {
    /// 検証に通ったテンプレート（ファイル名順）。
    pub graphs: Vec<CompiledGraph>,
    /// 使えない定義（読めない・検証に通らない・役割が足りない）。理由つき。
    pub problems: Vec<GraphProblem>,
    /// この回に作った組込みテンプレートのID。
    pub created: Vec<String>,
}

impl GraphReport {
    pub fn find(&self, id: &str) -> Option<&CompiledGraph> {
        self.graphs.iter().find(|g| g.def.id == id)
    }
    /// そのIDのファイル（`<id>.json`）に問題があるときの理由。
    pub fn problem_for(&self, id: &str) -> Option<&GraphProblem> {
        let name = format!("{id}.json");
        self.problems.iter().find(|p| p.file == name)
    }
}

/// 組込みテンプレートのうち、ファイルが無いものだけを作る。既存ファイルには触れない。作ったIDを返す。
pub fn ensure_builtin_graphs(dir: &Path) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut created = Vec::new();
    for id in [TEMPLATE_STANDARD, TEMPLATE_QUALITY_LOOP] {
        let Some(text) = template_json(id) else { continue };
        let path = dir.join(format!("{id}.json"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let r = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
                drop(f);
                if let Err(e) = r {
                    // 書きかけを残さない（次回また作れるように）。
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                created.push(id.to_string());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(created)
}

/// 検証に渡す役割の表。有効な役割だけ（無効な役割を使う定義は、動かない役割を含むので使えない）。
pub fn role_infos(roles: &[LoadedRole]) -> BTreeMap<RoleId, RoleInfo> {
    roles
        .iter()
        .filter(|r| r.enabled())
        .map(|r| (RoleId::new(r.def.id.clone()), RoleInfo { writes: r.def.sandbox == SandboxKind::WorkspaceWrite, workplace: r.def.workplace }))
        .collect()
}

/// 今のホストがそのテンプレートを開始できない理由（できるなら None）。画面で選べなくするためと、開始のときの拒否に同じ文言を使う。
/// 計画（plannedTasks）を含むのは組込みの `standard` だけが対応（従来の計画経路）。Git を使わない（folder）ものは未対応。
pub fn start_blocker(g: &CompiledGraph) -> Option<&'static str> {
    let planned = g.def.nodes.iter().any(|n| matches!(n.kind, NodeKind::PlannedTasks { .. }));
    if planned && g.def.id != TEMPLATE_STANDARD {
        Some("計画（plannedTasks）を含むテンプレートは、組込みの standard だけが対応しています")
    } else if !planned && g.def.workspace != org_engine::graph::WorkspaceKind::Git {
        Some("Git を使わない（folder）テンプレートには、まだ対応していません")
    } else {
        None
    }
}

fn graph_roles(def: &org_engine::graph::GraphDef) -> Vec<RoleId> {
    let mut v = Vec::new();
    for n in &def.nodes {
        match &n.kind {
            NodeKind::Agent { role, .. } => v.push(role.clone()),
            NodeKind::PlannedTasks { allowed_roles, .. } => v.extend(allowed_roles.iter().cloned()),
            _ => {}
        }
    }
    v
}

/// 定義ファイルを読んで検証する（作成はしない）。誤りのあるファイルは動かさず、理由を返す。
pub fn load_graphs(root: &Path, roles: &[LoadedRole]) -> GraphReport {
    let dir = graphs_dir(root);
    let mut report = GraphReport::default();
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json")).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            report.problems.push(GraphProblem { file: dir.to_string_lossy().into_owned(), reasons: vec![format!("グラフ定義のフォルダを読めません: {e}")] });
            return report;
        }
    };
    files.sort();
    let infos = role_infos(roles);
    let schemas = builtin_schemas();
    for path in files {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let problem = |reasons: Vec<String>| GraphProblem { file: name.clone(), reasons };
        if std::fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            report.problems.push(problem(vec!["大きすぎるため読みません（512KBまで）".to_string()]));
            continue;
        }
        let text = match std::fs::read(&path) {
            Ok(b) => match String::from_utf8(b) {
                Ok(t) => t,
                Err(_) => {
                    report.problems.push(problem(vec!["UTF-8の文字列として読めません".to_string()]));
                    continue;
                }
            },
            Err(e) => {
                report.problems.push(problem(vec![format!("読めません: {e}")]));
                continue;
            }
        };
        let def = match parse_graph(text.trim_start_matches('\u{feff}')) {
            Ok(d) => d,
            Err(e) => {
                report.problems.push(problem(vec![e]));
                continue;
            }
        };
        // 無効な役割（権限を広げる定義で、まだ有効にしていない）を使う定義は、理由を分けて示す。
        let disabled: Vec<String> = graph_roles(&def)
            .into_iter()
            .filter(|r| roles.iter().any(|l| l.def.id == r.0 && !l.enabled()))
            .map(|r| format!("役割「{r}」は、権限を広げる定義のため無効です（役割の設定で確認して有効にするまで使えません）"))
            .collect();
        if !disabled.is_empty() {
            report.problems.push(problem(disabled));
            continue;
        }
        match validate(&def, &infos, &schemas, &stem, &Limits::default()) {
            Ok(g) => report.graphs.push(g),
            Err(errs) => report.problems.push(problem(errs.iter().map(|e| e.to_string()).collect())),
        }
    }
    report
}

/// 読込み → 足りない組込みテンプレートの作成 → 作った場合は読み直し。作成に失敗したら問題として報告する。
pub fn prepare_graphs(root: &Path, roles: &[LoadedRole]) -> GraphReport {
    match ensure_builtin_graphs(&graphs_dir(root)) {
        Ok(created) => {
            let mut r = load_graphs(root, roles);
            r.created = created;
            r
        }
        Err(e) => {
            let mut r = load_graphs(root, roles);
            r.problems.push(GraphProblem { file: graphs_dir(root).to_string_lossy().into_owned(), reasons: vec![format!("組込みのテンプレートを作れません: {e}")] });
            r
        }
    }
}

// ───────────── 開始時の写し ─────────────

fn fnv(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 使う役割の定義全体の指紋（権限だけでなく指示も含む。どれかが変われば変わる）。
pub fn roles_fingerprint(roles: &[LoadedRole]) -> String {
    let mut defs: Vec<&RoleDefinition> = roles.iter().map(|r| &r.def).collect();
    defs.sort_by(|a, b| a.id.cmp(&b.id));
    fnv(&serde_json::to_string(&defs).unwrap_or_default())
}

/// グラフが使う役割を、読み込んだ役割から選ぶ。足りない・無効なものがあれば理由を返す。
pub fn used_roles(graph: &CompiledGraph, loaded: &[LoadedRole]) -> Result<Vec<LoadedRole>, String> {
    let mut out = Vec::new();
    for id in graph.roles_used() {
        match loaded.iter().find(|l| l.def.id == id.0) {
            Some(l) if l.enabled() => out.push(l.clone()),
            Some(_) => return Err(format!("役割「{id}」は、権限を広げる定義のため無効です")),
            None => return Err(format!("役割「{id}」の定義がありません")),
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoleCopy {
    def: RoleDefinition,
    /// 開始時の「基準より広い」理由（有効だった役割は、確認済みか、空）。
    widened: Vec<String>,
    confirmed: bool,
    /// 開始時の権限の指紋（確認の印と同じもの）。
    permission_fingerprint: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolesFile {
    schema_version: u32,
    fingerprint: String,
    roles: Vec<RoleCopy>,
}

/// 開始時の定義の写し（`graph.json`・`roles.json`）を保存する。どちらかを保存できなければ失敗（ワークフローを始めない）。
/// 役割の指紋（`GraphBound` に記録する値）を返す。
pub fn save_copies(workflow_dir: &Path, graph: &CompiledGraph, used: &[LoadedRole]) -> std::io::Result<String> {
    let graph_bytes = serde_json::to_vec_pretty(&graph.def).map_err(std::io::Error::other)?;
    let fingerprint = roles_fingerprint(used);
    let file = RolesFile {
        schema_version: COPY_SCHEMA_VERSION,
        fingerprint: fingerprint.clone(),
        roles: used
            .iter()
            .map(|r| RoleCopy { def: r.def.clone(), widened: r.widened.clone(), confirmed: r.confirmed, permission_fingerprint: permission_fingerprint(&r.def) })
            .collect(),
    };
    let roles_bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    write_atomic(&workflow_dir.join(GRAPH_COPY_FILE), &graph_bytes)?;
    write_atomic(&workflow_dir.join(ROLES_COPY_FILE), &roles_bytes)?;
    Ok(fingerprint)
}

/// 写しから、開始時のグラフと役割を復元する。写しが記録（`GraphBound`）の指紋と一致しないとき・読めないときは、使わずに理由を返す
/// （今の定義ファイルで代用しない）。
pub fn load_copies(workflow_dir: &Path, bound: &GraphRef) -> Result<(CompiledGraph, Vec<LoadedRole>), String> {
    let read = |name: &str| std::fs::read(workflow_dir.join(name)).map_err(|e| format!("{name} を読めません: {e}"));
    let roles_file: RolesFile = serde_json::from_slice(&read(ROLES_COPY_FILE)?).map_err(|e| format!("{ROLES_COPY_FILE} を解析できません: {e}"))?;
    let loaded: Vec<LoadedRole> = roles_file.roles.into_iter().map(|c| LoadedRole { def: c.def, widened: c.widened, confirmed: c.confirmed }).collect();
    if roles_fingerprint(&loaded) != bound.roles_fingerprint {
        return Err("役割の写しが、開始時の記録と一致しません".to_string());
    }
    let text = String::from_utf8(read(GRAPH_COPY_FILE)?).map_err(|_| format!("{GRAPH_COPY_FILE} が文字列として読めません"))?;
    let def = parse_graph(&text).map_err(|e| format!("{GRAPH_COPY_FILE}: {e}"))?;
    let id = def.id.clone();
    let compiled = validate(&def, &role_infos(&loaded), &builtin_schemas(), &id, &Limits::default())
        .map_err(|errs| format!("{GRAPH_COPY_FILE} を検証できません: {}", errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(" / ")))?;
    if compiled.fingerprint != bound.fingerprint {
        return Err("グラフの写しが、開始時の記録と一致しません".to_string());
    }
    Ok((compiled, loaded))
}

/// 今の定義ファイル・役割定義が、開始時の記録と違うか。違えば画面に出す案内文（動いているワークフローは写しのまま続ける）。
pub fn definitions_changed(bound: &GraphRef, used_role_ids: &[RoleId], current_graphs: &GraphReport, current_roles: &[LoadedRole]) -> Option<String> {
    let graph_same = current_graphs.find(&bound.graph_id).is_some_and(|g| g.fingerprint == bound.fingerprint);
    let now: Vec<LoadedRole> = used_role_ids.iter().filter_map(|id| current_roles.iter().find(|l| l.def.id == id.0).cloned()).collect();
    let roles_same = now.len() == used_role_ids.len() && roles_fingerprint(&now) == bound.roles_fingerprint;
    (!(graph_same && roles_same)).then(|| "定義が変わっています（このワークフローは開始時の定義で続けます）".to_string())
}

/// 復元時にスロットの値を読み戻す（最新の版だけ。エンジンが使うのは最新の版）。読めなかったものは入れない
/// （エンジンは「未取得」として扱い、その値で辺を選ばず止まる）。読めなかったスロットのIDも返す。
pub fn read_slots(store: &dyn OrgStore, state: &WorkflowState) -> (HashMap<(SlotId, u32), serde_json::Value>, Vec<String>) {
    let mut values = HashMap::new();
    let mut unreadable = Vec::new();
    for (slot, version) in state.slot_versions() {
        match store.read_slot(slot, version) {
            Ok(Some(v)) => {
                values.insert((slot.clone(), version), v);
            }
            Ok(None) | Err(_) => unreadable.push(slot.0.clone()),
        }
    }
    (values, unreadable)
}

#[cfg(test)]
mod tests;
