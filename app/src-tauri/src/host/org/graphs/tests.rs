use super::*;
use crate::host::org::roles::prepare_roles;
use crate::store::org::FileOrgStore;
use org_engine::graph::{EdgeId, NodeId};
use org_engine::{OrgEventKind, PlanTask, TaskId, WorkflowId};

fn temp_root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agentdock-graphs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn roles(root: &Path) -> Vec<LoadedRole> {
    prepare_roles(root, 1).roles
}

fn write_graph(root: &Path, file: &str, text: &str) {
    let dir = graphs_dir(root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

#[test]
fn first_start_creates_the_builtin_templates_and_never_overwrites() {
    let root = temp_root("create");
    let roles = roles(&root);
    let r = prepare_graphs(&root, &roles);
    assert_eq!(r.created, vec!["standard".to_string(), "quality-loop".to_string()]);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(r.find("standard").is_some() && r.find("quality-loop").is_some());
    // 利用者が書き換えたファイルは、再実行しても変わらない。
    let p = graphs_dir(&root).join("quality-loop.json");
    let edited = std::fs::read_to_string(&p).unwrap().replace("\"maxRounds\": 3", "\"maxRounds\": 2");
    std::fs::write(&p, &edited).unwrap();
    let again = prepare_graphs(&root, &roles);
    assert!(again.created.is_empty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), edited);
    assert_ne!(again.find("quality-loop").unwrap().fingerprint, r.find("quality-loop").unwrap().fingerprint);
    // 消した1つだけが作り直される。
    std::fs::remove_file(graphs_dir(&root).join("standard.json")).unwrap();
    let third = prepare_graphs(&root, &roles);
    assert_eq!(third.created, vec!["standard".to_string()]);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), edited);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_bad_definition_is_reported_with_reasons_and_left_where_it_is() {
    let root = temp_root("bad");
    let roles = roles(&root);
    prepare_graphs(&root, &roles);
    // 条件のないフィードバック辺、JSONでないファイル、ファイル名と違うid、未知の項目。
    let text = std::fs::read_to_string(graphs_dir(&root).join("quality-loop.json")).unwrap();
    let no_condition = text.replace(r#""when": { "slot": "verdict", "path": "verdict", "eq": "rework" },"#, "");
    assert_ne!(no_condition, text);
    write_graph(&root, "no-cond.json", &no_condition.replace(r#""id": "quality-loop""#, r#""id": "no-cond""#));
    write_graph(&root, "broken.json", "{ not json");
    write_graph(&root, "other-name.json", &text);
    write_graph(&root, "extra.json", &text.replace(r#""id": "quality-loop","#, r#""id": "extra", "bogus": 1,"#));
    let r = load_graphs(&root, &roles);
    assert!(r.find("quality-loop").is_some(), "the good ones still load");
    for (file, id) in [("no-cond.json", "no-cond"), ("broken.json", "broken"), ("other-name.json", "other-name"), ("extra.json", "extra")] {
        let p = r.problem_for(id).unwrap_or_else(|| panic!("no problem for {file}"));
        assert_eq!(p.file, file);
        assert!(!p.reasons.is_empty() && p.reasons.iter().all(|m| !m.is_empty()), "{file}: {:?}", p.reasons);
        assert!(r.find(id).is_none());
        // 退避しない: ファイルはそのまま残る。
        assert!(graphs_dir(&root).join(file).exists());
    }
    assert!(r.problem_for("no-cond").unwrap().reasons.iter().any(|m| m.starts_with('V')), "{:?}", r.problem_for("no-cond"));
    assert!(r.problem_for("other-name").unwrap().reasons.iter().any(|m| m.contains("一致しません")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_definition_that_uses_a_disabled_or_missing_role_is_not_usable() {
    let root = temp_root("roles");
    let mut loaded = roles(&root);
    prepare_graphs(&root, &loaded);
    assert!(load_graphs(&root, &loaded).find("quality-loop").is_some());
    // verifier が「権限を広げる定義」で未確認 → 無効。理由が役割名つきで出る。
    let v = loaded.iter_mut().find(|r| r.def.id == "verifier").unwrap();
    v.widened = vec!["x".into()];
    v.confirmed = false;
    let r = load_graphs(&root, &loaded);
    assert!(r.find("quality-loop").is_none());
    assert!(r.problem_for("quality-loop").unwrap().reasons.iter().any(|m| m.contains("verifier") && m.contains("無効")));
    // 役割そのものが無い。
    loaded.retain(|r| r.def.id != "verifier");
    let r = load_graphs(&root, &loaded);
    assert!(r.find("quality-loop").is_none() && r.problem_for("quality-loop").is_some());
    assert!(r.find("standard").is_some(), "a graph that does not use it is fine");
    let _ = std::fs::remove_dir_all(&root);
}

fn bound_of(g: &CompiledGraph, roles_fp: &str) -> GraphRef {
    GraphRef { graph_id: g.def.id.clone(), fingerprint: g.fingerprint.clone(), roles_fingerprint: roles_fp.to_string() }
}

#[test]
fn a_workflow_restores_from_its_copy_even_when_the_definition_files_change_later() {
    let root = temp_root("copy");
    let roles = roles(&root);
    let report = prepare_graphs(&root, &roles);
    let g = report.find("quality-loop").unwrap().clone();
    let used = used_roles(&g, &roles).unwrap();
    assert_eq!(used.iter().map(|r| r.def.id.as_str()).collect::<Vec<_>>(), ["planner", "verifier", "worker"]);
    let wf_dir = crate::store::org::workflow_dir(&root, "wf-1");
    let fp = save_copies(&wf_dir, &g, &used).unwrap();
    let bound = bound_of(&g, &fp);

    // 定義ファイルと役割定義を書き換える（maxRounds と worker の指示）。
    let p = graphs_dir(&root).join("quality-loop.json");
    std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("\"maxRounds\": 3", "\"maxRounds\": 5")).unwrap();
    let wp = crate::host::org::roles::roles_dir(&root).join("worker.json");
    let mut wdef: RoleDefinition = serde_json::from_slice(&std::fs::read(&wp).unwrap()).unwrap();
    wdef.instructions = "changed afterwards".into();
    std::fs::write(&wp, serde_json::to_vec_pretty(&wdef).unwrap()).unwrap();
    let now_roles = prepare_roles(&root, 2).roles;
    let now_graphs = load_graphs(&root, &now_roles);
    assert_ne!(now_graphs.find("quality-loop").unwrap().fingerprint, g.fingerprint);

    // 写しからは、開始時のものがそのまま戻る。
    let (restored, restored_roles) = load_copies(&wf_dir, &bound).unwrap();
    assert_eq!(restored.def, g.def);
    assert_eq!(restored.fingerprint, g.fingerprint);
    assert_eq!(restored.loops.iter().map(|l| l.max_rounds).max(), Some(3));
    assert_eq!(restored_roles.len(), 3);
    assert_ne!(restored_roles.iter().find(|r| r.def.id == "worker").unwrap().def.instructions, "changed afterwards");
    assert!(restored_roles.iter().all(|r| r.enabled()));
    // 違いは案内として出る。同じなら出ない。
    let ids: Vec<RoleId> = restored.roles_used().into_iter().collect();
    let note = definitions_changed(&bound, &ids, &now_graphs, &now_roles).unwrap();
    assert!(note.contains("定義が変わっています") && note.contains("開始時の定義で続けます"));
    assert!(definitions_changed(&bound, &ids, &report, &roles).is_none());
    // 現在の定義ファイルが消えているときも「変わっている」。
    std::fs::remove_file(&p).unwrap();
    assert!(definitions_changed(&bound, &ids, &load_graphs(&root, &roles), &roles).is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_copy_that_does_not_match_the_record_is_not_used_and_nothing_stands_in_for_it() {
    let root = temp_root("copy-bad");
    let roles = roles(&root);
    let g = prepare_graphs(&root, &roles).find("quality-loop").unwrap().clone();
    let used = used_roles(&g, &roles).unwrap();
    let wf_dir = crate::store::org::workflow_dir(&root, "wf-2");
    let fp = save_copies(&wf_dir, &g, &used).unwrap();
    let bound = bound_of(&g, &fp);
    assert!(load_copies(&wf_dir, &bound).is_ok());
    // 記録の指紋と違う（別の定義で上書きされた・記録が別のもの）。
    let mut other = bound.clone();
    other.fingerprint = "0000000000000000".into();
    assert!(load_copies(&wf_dir, &other).unwrap_err().contains("グラフの写し"));
    let mut other = bound.clone();
    other.roles_fingerprint = "0000000000000000".into();
    assert!(load_copies(&wf_dir, &other).unwrap_err().contains("役割の写し"));
    // 壊れている・無い。
    std::fs::write(wf_dir.join(GRAPH_COPY_FILE), b"{ broken").unwrap();
    assert!(load_copies(&wf_dir, &bound).is_err());
    std::fs::remove_file(wf_dir.join(ROLES_COPY_FILE)).unwrap();
    assert!(load_copies(&wf_dir, &bound).unwrap_err().contains(ROLES_COPY_FILE));
    // 写しを保存できないとき（保存先がファイル）は失敗する。
    let blocked = root.join("blocked");
    std::fs::write(&blocked, b"x").unwrap();
    assert!(save_copies(&blocked.join("wf"), &g, &used).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

fn state_with_slots(slots: &[(&str, u32)]) -> WorkflowState {
    let mut s = WorkflowState::new(WorkflowId::new("wf-s"), Limits::default());
    let task = PlanTask { id: TaskId::new("plan-1"), role: RoleId::new("planner"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new(), kind: org_engine::TaskKind::Change };
    s.record(OrgEventKind::ActivationAdded { task, node: NodeId::new("plan"), via: None::<EdgeId> }, 1).unwrap();
    for (slot, version) in slots {
        for v in 1..=*version {
            s.record(OrgEventKind::SlotWritten { task: TaskId::new("plan-1"), slot: SlotId::new(*slot), version: v, bytes: 1 }, 1).unwrap();
        }
    }
    s
}

#[test]
fn slot_values_are_written_and_read_back_and_an_unreadable_one_is_reported_not_guessed() {
    let root = temp_root("slots");
    let dir = root.join("wf");
    let store = FileOrgStore::new(dir.clone());
    let value = serde_json::json!({"goal": "g", "steps": ["s"], "acceptance": ["a"]});
    store.put_slot(&SlotId::new("spec"), 1, &value).unwrap();
    store.put_slot(&SlotId::new("spec"), 2, &serde_json::json!({"goal": "newer"})).unwrap();
    store.put_slot(&SlotId::new("verdict"), 1, &serde_json::json!({"verdict": "pass"})).unwrap();
    assert!(dir.join("slots").join("spec.2.json").exists());
    // 同じ版の書き直しは置き換える（SlotWritten が記録されなかった版の再書込み）。
    store.put_slot(&SlotId::new("spec"), 2, &serde_json::json!({"goal": "rewritten"})).unwrap();

    let state = state_with_slots(&[("spec", 2), ("verdict", 1)]);
    let (values, unreadable) = read_slots(&store, &state);
    assert!(unreadable.is_empty());
    assert_eq!(values.len(), 2, "only the newest version of each slot is read");
    assert_eq!(values[&(SlotId::new("spec"), 2)]["goal"], "rewritten");

    // 消えた・壊れた・大きすぎる値は読めなかったものとして返り、別の版で代用しない。
    std::fs::remove_file(dir.join("slots").join("verdict.1.json")).unwrap();
    std::fs::write(dir.join("slots").join("spec.2.json"), b"{ broken").unwrap();
    let (values, mut unreadable) = read_slots(&store, &state);
    unreadable.sort();
    assert_eq!(unreadable, ["spec", "verdict"]);
    assert!(values.is_empty(), "the older spec.1 is not used in place of spec.2");
    std::fs::write(dir.join("slots").join("spec.2.json"), vec![b' '; 600 * 1024]).unwrap();
    assert!(store.read_slot(&SlotId::new("spec"), 2).unwrap().is_none());

    // 大きすぎる値は書かない。読取り専用の保存口は書けない。
    let big = serde_json::json!({ "x": "a".repeat(300 * 1024) });
    assert!(store.put_slot(&SlotId::new("spec"), 3, &big).is_err());
    assert!(!dir.join("slots").join("spec.3.json").exists());
    let ro = crate::store::org::ReadOnlyOrgStore::new(dir.clone());
    assert!(ro.put_slot(&SlotId::new("spec"), 3, &value).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stored_reports_are_read_back_and_an_unreadable_investigation_is_named() {
    let root = temp_root("reports");
    let dir = root.join("wf");
    std::fs::create_dir_all(&dir).unwrap();
    let store = FileOrgStore::new(dir.clone());
    let mut state = WorkflowState::new(WorkflowId::new("wf"), Limits::default());
    let add = |state: &mut WorkflowState, id: &str, kind: org_engine::TaskKind| {
        let task = PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new(), kind };
        state.record(OrgEventKind::TaskAdded { task }, 0).unwrap();
    };
    add(&mut state, "i1", org_engine::TaskKind::Investigate);
    add(&mut state, "i2", org_engine::TaskKind::Investigate);
    add(&mut state, "c", org_engine::TaskKind::Change);
    state.record(OrgEventKind::ReportRecorded { task: TaskId::new("i1"), attempt: 0, bytes: 1 }, 0).unwrap();
    state.record(OrgEventKind::ReportRecorded { task: TaskId::new("i2"), attempt: 0, bytes: 1 }, 0).unwrap();
    state.record(OrgEventKind::Committed { task: TaskId::new("c"), commit: "abc".into() }, 0).unwrap();
    // i1 has its file, i2 does not (not obtained), c has its worker report
    store.put_output(&TaskId::new("i1"), 0, &serde_json::json!({"summary":"s1","findings":[{"point":"p","basis":"b","refs":["f:1"]}],"references":[],"openQuestions":[]})).unwrap();
    store.put_output(&TaskId::new("c"), 0, &serde_json::json!({"summary":"did it","changedFiles":[],"tests":{"ran":true,"passed":true}})).unwrap();
    let (reports, unreadable) = read_reports(&store, &state);
    assert!(reports[&TaskId::new("i1")].contains("要約: s1") && reports[&TaskId::new("i1")].contains("根拠: b"));
    assert_eq!(unreadable, vec!["i2".to_string()]);
    assert!(!reports.contains_key(&TaskId::new("i2")), "not guessed");
    assert_eq!(reports[&TaskId::new("c")], "did it");
    let _ = std::fs::remove_dir_all(&root);
}
