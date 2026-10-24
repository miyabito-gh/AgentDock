use super::*;
use crate::rules::queue::AgentFact;
use std::process::Command;

fn temp(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    std::env::temp_dir().join(format!("agentdock-subagent-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)))
}
fn key(s: &str) -> AgentKey { AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) } }
fn run() -> SubagentRun { SubagentRun { schema_version: 1, id: "run-test".into(), title: "t".into(), created_at: UnixMillis(1), repo: "repo".into(), worktree: None, parent: Some(ChatKey { backend: BackendKind::Codex, id: ExternalId("parent".into()) }), first_attempt: None, phase: "ready".into(), detail: None, observed_children: vec![] } }
fn fact(name: &str) -> AgentFact { AgentFact { agent: key(name), state: AgentState::Done, freshness: Freshness::Live, latest_turn: Some(ExternalId("turn".into())), latest_end: Some((TurnEnd::Completed, UnixMillis(2))), terminal_explicit: false, observed_at: UnixMillis(2) } }
fn gate() -> GateInput { GateInput { connected: true, root: Some(fact("parent")), descendants: vec![fact("child")], descendant_scan_complete: true, open_stop: None, unresolved_attempt: None, unresolved_op: None, delete_pending: false, quitting: false, manual_turn_pending: false, user_confirmed_unknown: false } }

#[test]
fn completed_parent_never_relaxes_unknown_missing_or_unscanned_children() {
    let mut r=run(); r.observed_children.push(key("child")); let mut g=gate();
    assert!(gate_blockers(&r,&g,false).is_empty());
    g.descendants[0].state=AgentState::Unknown; g.descendants[0].latest_end=None;
    assert!(!gate_blockers(&r,&g,false).is_empty());
    g=gate(); g.descendants.clear(); assert!(!gate_blockers(&r,&g,false).is_empty());
    g=gate(); g.descendant_scan_complete=false; assert!(!gate_blockers(&r,&g,false).is_empty());
    g=gate(); g.unresolved_attempt=Some(LocalId("unknown".into())); assert!(!gate_blockers(&r,&g,false).is_empty());
    g=gate(); g.descendants[0].freshness=Freshness::Disconnected; assert!(!gate_blockers(&r,&g,false).is_empty());
    assert!(!gate_blockers(&r,&gate(),true).is_empty());
}

#[tokio::test]
async fn stale_run_save_preserves_new_children_and_future_records_are_protected() {
    let base=temp("persistence"); let store=Arc::new(crate::store::Store::open(base.clone()).unwrap());
    let host=Arc::new(Host::with_store(base.clone(),store)); let stale=run();
    host.subagent_save(&stale).await.unwrap();
    let mut newer=stale.clone(); newer.observed_children.push(key("new-child")); host.subagent_save(&newer).await.unwrap();
    host.subagent_save(&stale).await.unwrap(); assert_eq!(host.subagent_read(stale.id.clone()).await.unwrap().observed_children,vec![key("new-child")]);
    let root=host.subagent_root().unwrap(); let path=run_path(&root,&stale.id).unwrap();
    newer.schema_version=999; let future=serde_json::to_vec(&newer).unwrap(); std::fs::write(&path,&future).unwrap();
    assert!(host.subagent_save(&stale).await.is_err()); assert_eq!(std::fs::read(&path).unwrap(),future);
    std::fs::write(&path,b"truncated").unwrap(); assert!(host.subagent_save(&stale).await.is_err()); assert_eq!(std::fs::read(&path).unwrap(),b"truncated");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn incomplete_and_corrupt_import_ledgers_never_report_success() {
    let root=temp("ledger"); let r=run(); save_run(&root,&r).unwrap();
    let rec=SubagentImportRecord { id:"i".into(), at:UnixMillis(1), before:"a".into(), result:"b".into(), source:"c".into(), outcome:None };
    append(&root,&r.id,&Ledger::Begin { record:rec.clone() }).unwrap(); assert_eq!(imports(&root,&r.id).unwrap()[0].outcome,None);
    append(&root,&r.id,&Ledger::End { id:"i".into(), outcome:OrgImportOutcome::Unknown { head_now:Known::NotFetched } }).unwrap();
    assert!(matches!(imports(&root,&r.id).unwrap()[0].outcome,Some(OrgImportOutcome::Unknown { .. })));
    append(&root,&r.id,&Ledger::End { id:"i".into(), outcome:OrgImportOutcome::Applied { head:"b".into() } }).unwrap();
    assert!(matches!(imports(&root,&r.id).unwrap()[0].outcome,Some(OrgImportOutcome::Applied { .. })));
    append(&root,&r.id,&Ledger::Begin { record:rec }).unwrap(); assert!(imports(&root,&r.id).is_err());
    std::fs::write(ledger_path(&root,&r.id).unwrap(),"{broken").unwrap(); assert!(imports(&root,&r.id).is_err());
    for id in ["../run-a","run-a/../b","run-a\\b","run-:"] { assert!(run_path(&root,id).is_err()); }
    std::fs::remove_dir_all(root).unwrap();
}

fn git(repo: &Path,args: &[&str]) -> String {
    let o=Command::new("git").arg("-C").arg(repo).args(args).output().expect("git required for this test");
    assert!(o.status.success(),"{:?}: {}",args,String::from_utf8_lossy(&o.stderr)); String::from_utf8_lossy(&o.stdout).trim().into()
}
#[tokio::test]
async fn import_checks_source_identity_dirty_and_target_head_before_applying() {
    let base=temp("git"); let repo=base.join("repo"); std::fs::create_dir_all(&repo).unwrap();
    git(&repo,&["init","-q","-b","main"]); git(&repo,&["config","core.autocrlf","false"]); git(&repo,&["config","user.name","Test"]); git(&repo,&["config","user.email","test@localhost"]);
    std::fs::write(repo.join("a.txt"),"base\n").unwrap(); git(&repo,&["add","a.txt"]); git(&repo,&["commit","-qm","base"]);
    let data=base.join("data"); let store=Arc::new(crate::store::Store::open(data.clone()).unwrap());
    let host=Arc::new(Host::with_store(data.clone(),store.clone())); let wt=store.worktrees_dir().join("test"); std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(&repo,&["worktree","add","-q","-b","agentdock/test",wt.to_str().unwrap()]);
    let original=git(&repo,&["rev-parse","HEAD"]);
    let record=WorktreeRecord { id:LocalId("wt".into()),repo_root:repo.to_string_lossy().into(),path:wt.to_string_lossy().into(),branch:"agentdock/test".into(),base_commit:original.clone(),created_at:UnixMillis(1),created_for_chat:None,state:WorktreeState::Ready,org:None };
    host.mutate(|d| {d.worktrees.push(record.clone());((),vec![])});
    let mut r=run(); r.repo=record.repo_root.clone(); r.worktree=Some(record.clone());
    assert!(host.import_source(&r).await.is_err(),"no result commit yet");
    std::fs::write(wt.join("a.txt"),"result\n").unwrap(); assert!(host.import_source(&r).await.is_err(),"dirty result blocked");
    git(&wt,&["commit","-qam","result"]); let source=host.import_source(&r).await.unwrap();
    let g=host.git_for_worktree(); let facts=gi::repo_facts(&g,&repo).await;
    let gi::PrepareOutcome::Ready(prepared)=gi::prepare(&g,&repo,&facts,"test",&[gi::Candidate {task:r.id.clone(),title:r.title.clone(),commit:source.clone()}]).await.unwrap() else {panic!("prepare")};
    std::fs::write(repo.join("a.txt"),"local edit\n").unwrap();
    let outcome=gi::execute(&g,&repo,&prepared,|| panic!("begin must not be called on overlapping dirty files")).await;
    assert!(matches!(outcome,gi::ExecOutcome::Refused(_))); assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(),"local edit\n");
    std::fs::write(repo.join("a.txt"),"base\n").unwrap();
    let outcome=gi::execute(&g,&repo,&prepared,||Err("ledger unavailable".into())).await;
    assert!(matches!(outcome,gi::ExecOutcome::BeginFailed(_))); assert_eq!(git(&repo,&["rev-parse","HEAD"]),original);
    let outcome=gi::execute(&g,&repo,&prepared,||Ok(())).await; assert!(matches!(outcome,gi::ExecOutcome::Done(OrgImportOutcome::Applied { .. })));
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(),"result\n");
    let mut wrong=record; wrong.path=repo.to_string_lossy().into(); r.worktree=Some(wrong); assert!(host.import_source(&r).await.is_err());
    // All files are in this exact test-only directory; no managed user data is touched.
    std::fs::remove_dir_all(base).unwrap();
}
