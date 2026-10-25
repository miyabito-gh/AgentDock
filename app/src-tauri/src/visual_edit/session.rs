use super::{ChangeKind, FileAccess, FileDecision, FileScope, ProjectRoot, SelectedFile, Snapshot, SnapshotManifest, VisualError, VisualResult};
use crate::backend::model::ChatKey;
use serde::{Deserialize, Serialize};
use std::{fs::{self, File, OpenOptions}, io::{BufRead, BufReader, Write}, path::{Path, PathBuf}};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub version: u32,
    pub session_id: String,
    pub chat: ChatKey,
    pub source_root: String,
    pub draft_root: String,
    pub html_path: String,
    pub scope: Vec<SelectedFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingBinding { pub version:u32, pub session_id:String, pub chat:ChatKey, pub draft_root:String }#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionEvent {
    Prepared { attempt_id: String, client_message_id: String, before: String },
    Accepted { attempt_id: String, turn_id: String },
    Rejected { attempt_id: String },
    Unknown { attempt_id: String },
    Captured { attempt_id: String, after: String },
    OperationIntent { operation_id: String, purpose: String, revision: u64, desired: String },
    OperationResult { operation_id: String, outcome: String },
    Adopted { after: String },
    UndoApplied { attempt_id: String, before: String },
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub session_id: String,
    pub chat: ChatKey,
    pub source_root: String,
    pub draft_root: String,
    pub html_path: String,
    pub scope: Vec<SelectedFile>,
    pub phase: String,
    pub revision: u64,
    pub changes: Vec<FileDecision>,
    pub blockers: Vec<String>,
    pub can_adopt: bool,
    pub can_undo: bool,
    pub last_operation: Option<LastOperationView>,
}
#[derive(Clone, Debug)]
pub struct LoadedSession {
    pub meta: SessionMeta,
    pub events: Vec<SessionEvent>,
    pub folder: PathBuf,
}
impl LoadedSession {
    pub fn scope(&self) -> VisualResult<FileScope> { FileScope::new(self.meta.scope.clone()) }
    pub fn initial(&self) -> VisualResult<Snapshot> { load_snapshot(&self.folder.join("initial"), &self.scope()?) }
    pub fn latest(&self) -> Option<(&str, &str, &str)> {
        self.events.iter().rev().find_map(|e| match e {
            SessionEvent::Prepared { attempt_id, client_message_id, before } => Some((attempt_id.as_str(), client_message_id.as_str(), before.as_str())),
            _ => None,
        })
    }
    pub fn accepted_turn(&self, attempt: &str) -> Option<&str> {
        self.events.iter().rev().find_map(|e| match e { SessionEvent::Accepted {attempt_id,turn_id} if attempt_id == attempt => Some(turn_id.as_str()), _ => None })
    }
    pub fn after(&self, attempt: &str) -> Option<&str> {
        self.events.iter().rev().find_map(|e| match e { SessionEvent::Captured {attempt_id,after} if attempt_id == attempt => Some(after.as_str()), _ => None })
    }
    pub fn outcome(&self, operation: &str) -> Option<&str> {
        self.events.iter().rev().find_map(|e| match e { SessionEvent::OperationResult {operation_id,outcome} if operation_id == operation => Some(outcome.as_str()), _ => None })
    }
    pub fn pending_operation(&self) -> bool {
        self.events.iter().any(|e| match e { SessionEvent::OperationIntent {operation_id,..} => self.outcome(operation_id).is_none(), _ => false })
    }
    fn attempt_state(&self, attempt:&str)->&str {
        if self.after(attempt).is_some() {return "captured";}
        // Once accepted is known, a delayed Unknown event cannot downgrade it.
        if self.accepted_turn(attempt).is_some() {return "accepted";}
        if self.events.iter().any(|e|matches!(e,SessionEvent::Rejected{attempt_id} if attempt_id==attempt)) {return "rejected";}
        if self.events.iter().any(|e|matches!(e,SessionEvent::Unknown{attempt_id} if attempt_id==attempt)) {return "unknown";}
        "prepared"
    }
    pub fn latest_rejected(&self, attempt:&str)->bool { self.attempt_state(attempt)=="rejected" }
    pub fn latest_unknown(&self, attempt:&str)->bool { self.attempt_state(attempt)=="unknown" }
    pub fn last_capture(&self)->Option<(&str,&str)> {
        self.events.iter().rev().find_map(|e|match e { SessionEvent::Captured {attempt_id,after} if !self.events.iter().any(|x| matches!(x,SessionEvent::UndoApplied{attempt_id:a,..} if a==attempt_id))=>Some((attempt_id.as_str(),after.as_str())), _=>None })
    }
    pub fn current_dir(&self)->Option<&str> {
        self.events.iter().rev().find_map(|e|match e {SessionEvent::Captured{after,..}=>Some(after.as_str()),SessionEvent::UndoApplied{before,..}=>Some(before.as_str()),SessionEvent::OperationResult{operation_id,outcome} if outcome=="applied"=>self.applied_operation(operation_id,"undo"),_=>None})
    }
    fn applied_operation(&self, operation_id:&str, purpose:&str)->Option<&str> {
        self.events.iter().rev().find_map(|e|match e {SessionEvent::OperationIntent{operation_id:id,purpose:p,desired,..} if id==operation_id && p==purpose=>Some(desired.as_str()),_=>None})
    }
    pub fn latest_adopted(&self)->Option<&str> {
        self.events.iter().rev().find_map(|e|match e {SessionEvent::Adopted{after}=>Some(after.as_str()),SessionEvent::OperationResult{operation_id,outcome} if outcome=="applied"=>self.applied_operation(operation_id,"adopt"),_=>None})
    }
    pub fn baseline(&self)->VisualResult<Snapshot> {
        match self.latest_adopted() {Some(dir)=>load_snapshot(&self.folder.join(dir),&self.scope()?),None=>self.initial()}
    }
    pub fn orphan_snapshots(&self)->VisualResult<Vec<String>> {
        let mut expected=std::collections::BTreeSet::new();
        for e in &self.events {match e {SessionEvent::Prepared{before,..}=>{expected.insert(before.as_str());},SessionEvent::Captured{after,..}=>{expected.insert(after.as_str());},_=>()}}
        let mut orphan=Vec::new();
        for entry in fs::read_dir(&self.folder)? {
            let entry=entry?;
            let name=entry.file_name().into_string().map_err(|_|VisualError::Conflict("invalid snapshot folder name"))?;
            if name.starts_with("tx-") && entry.file_type()?.is_dir() && !expected.contains(name.as_str()) {orphan.push(name);}
        }
        Ok(orphan)
    }    pub fn revision(&self)->u64 { self.events.iter().filter(|e|matches!(e,SessionEvent::Captured{..}|SessionEvent::Adopted{..}|SessionEvent::UndoApplied{..})).count() as u64 }
    pub fn view(&self)->VisualResult<SessionView> {
        let scope=self.scope()?;
        let baseline=self.baseline()?;
        let last=self.last_capture();
        let mut phase=match self.latest() {
            Some((attempt,_,_))=>match self.attempt_state(attempt) {
                "prepared"=>"preparedUnresolved",
                "unknown"=>"acceptanceUnknown",
                "accepted"=>"runningOrReconciling",
                _ if last.is_some()=>"changes",
                _=>"editable",
            },
            None=>"editable",
        };
        let mut blockers=Vec::new();
        let orphan=self.orphan_snapshots()?;
        if !orphan.is_empty() {phase="recoveryRequired"; blockers.push(format!("unreferenced immutable snapshot: {}",orphan.join(", ")));}
        if self.pending_operation(){blockers.push("A file operation has unresolved durable intent; inspect its journal".into());}
        if matches!(phase,"preparedUnresolved"|"acceptanceUnknown"|"runningOrReconciling"){blockers.push(format!("session is {phase}; no file write is allowed"));}
        let current=match self.current_dir() {Some(dir)=>load_snapshot(&self.folder.join(dir),&scope)?,None=>self.initial()?};
        let mut changes=Vec::new();
        for file in &self.meta.scope {
            let b=baseline.state(&file.path); let e=current.state(&file.path);
            let change=if b==e {None}else{Some(match (b,e){(None,Some(_))=>ChangeKind::Add,(Some(_),None)=>ChangeKind::Delete,_=>ChangeKind::Change})};
            changes.push(FileDecision{path:file.path.clone(),change,conflict:None});
        }
        if matches!(phase,"changes") && changes.iter().all(|f|f.change.is_none()) {phase="editable";}
        if self.events.iter().rev().find_map(|e|match e {SessionEvent::OperationResult{outcome,..}=>Some(outcome.as_str()),SessionEvent::Adopted{..}|SessionEvent::UndoApplied{..}=>Some("applied"),_=>None}).is_some_and(|x|x=="partial" || x=="conflict") {phase="partial"; blockers.push("file operation is partial or conflicted; inspect journal and current files".into());}
        let last_operation=self.events.iter().rev().find_map(|e|match e {SessionEvent::OperationIntent{operation_id,purpose,..}=>Some(LastOperationView{kind:purpose.clone(),id:operation_id.clone(),outcome:self.outcome(operation_id).unwrap_or("unknown").into(),message:None}),_=>None});
        Ok(SessionView{session_id:self.meta.session_id.clone(),chat:self.meta.chat.clone(),source_root:self.meta.source_root.clone(),draft_root:self.meta.draft_root.clone(),html_path:self.meta.html_path.clone(),scope:self.meta.scope.clone(),phase:phase.into(),revision:self.revision(),changes,blockers,can_adopt:false,can_undo:false,last_operation})
    }
}
fn durable_file(path: &Path, bytes: &[u8]) -> VisualResult<()> {
    let mut f=OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?; f.sync_all()?; Ok(())
}
pub fn append(folder: &Path, event: &SessionEvent) -> VisualResult<()> {
    let mut f=OpenOptions::new().append(true).create(true).open(folder.join("events.jsonl"))?;
    serde_json::to_writer(&mut f,event)?; f.write_all(b"\n")?; f.sync_all()?; Ok(())
}
pub fn save_pending_binding(folder:&Path,binding:&PendingBinding)->VisualResult<()> {
    durable_file(&folder.join("binding.json"),&serde_json::to_vec_pretty(binding)?)
}pub fn save_meta(folder: &Path, meta: &SessionMeta) -> VisualResult<()> {
    durable_file(&folder.join("session.json"), &serde_json::to_vec_pretty(meta)?)
}
pub fn load(root: &Path, id: &str) -> VisualResult<LoadedSession> {
    validate_id(id)?;
    let folder=root.join(id);
    let meta:SessionMeta=serde_json::from_slice(&fs::read(folder.join("session.json"))?)?;
    if meta.version != 1 || meta.session_id != id { return Err(VisualError::Conflict("visual session metadata mismatch")); }
    let source_path=super::preview::normal_drive_path(Path::new(&meta.source_root)).map_err(|_|VisualError::Conflict("invalid source root"))?;
    let source=ProjectRoot::open(&source_path)?;
    let storage=ProjectRoot::open(root)?;
    if super::scope::root_contains(source.path(),storage.path()) || super::scope::root_contains(storage.path(),source.path()) {return Err(VisualError::Conflict("visual source overlaps storage"));}
    let draft_path=super::preview::normal_drive_path(Path::new(&meta.draft_root)).map_err(|_|VisualError::Conflict("invalid draft root"))?;
    let draft=ProjectRoot::open(&draft_path)?;
    let expected_draft=ProjectRoot::open(&folder.join("draft"))?;
    if draft.path()!=expected_draft.path() || super::scope::root_contains(source.path(),draft.path()) || super::scope::root_contains(draft.path(),source.path()) {return Err(VisualError::Conflict("visual session root binding changed"));}
    let scope=FileScope::new(meta.scope.clone())?;
    if !meta.scope.iter().any(|f|f.path==meta.html_path && f.access==FileAccess::Edit) {return Err(VisualError::Conflict("visual HTML is outside editable scope"));}
    let initial=load_snapshot(&folder.join("initial"), &scope)?;
    let saved:SnapshotManifest=serde_json::from_slice(&fs::read(folder.join("manifest.json"))?)?;
    if initial.manifest() != saved { return Err(VisualError::Conflict("visual initial bytes changed")); }
    let mut events=Vec::new();
    for line in BufReader::new(File::open(folder.join("events.jsonl"))?).lines() {
        let line=line?;
        let event=serde_json::from_str::<SessionEvent>(&line)?;
        match &event {
            SessionEvent::Prepared{before,..}=>validate_id(before)?,
            SessionEvent::Captured{after,..}|SessionEvent::Adopted{after}=>validate_id(after)?,
            SessionEvent::UndoApplied{before,..}=>validate_id(before)?,
            SessionEvent::OperationIntent{operation_id,purpose,desired,..}=>{validate_id(operation_id)?; validate_id(desired)?; if purpose!="adopt" && purpose!="undo" {return Err(VisualError::Conflict("invalid operation purpose"));}},
            SessionEvent::OperationResult{operation_id,..}=>validate_id(operation_id)?,
            _=>(),
        }
        events.push(event);
    }
    Ok(LoadedSession {meta,events,folder})
}
/// Read only the durable chat binding, independently of source/draft availability.
/// A durable binding identifies pending threads even without a reported cwd.
/// An unbound pending folder blocks unknown chats until manual recovery.
pub fn binding_id(root:&Path, chat:&ChatKey)->VisualResult<Option<String>> {
    if !root.exists(){return Ok(None)}
    ProjectRoot::open(root)?;
    let mut unbound_pending=false;
    for entry in fs::read_dir(root)? {
        let entry=entry?;
        if !entry.file_type()?.is_dir(){return Err(VisualError::Conflict("unrecognized visual storage entry"));}
        let id=entry.file_name().into_string().map_err(|_|VisualError::Conflict("invalid session folder"))?;
        if id=="_journals" {continue}
        validate_id(&id)?;
        ProjectRoot::open(&entry.path())?;
        let binding_path=entry.path().join("binding.json");
        let binding=match fs::symlink_metadata(&binding_path) {
            Ok(m) if m.file_type().is_symlink()=>return Err(VisualError::Conflict("linked visual binding")),
            Ok(_)=>Some(serde_json::from_slice::<PendingBinding>(&fs::read(&binding_path)?)?),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            Err(e)=>return Err(e.into()),
        };
        if let Some(b)=&binding {if b.version!=1 || b.session_id!=id {return Err(VisualError::Conflict("visual pending binding mismatch"));}}
        let meta_path=entry.path().join("session.json");
        let meta=match fs::symlink_metadata(&meta_path) {
            Ok(m) if m.file_type().is_symlink()=>return Err(VisualError::Conflict("linked visual session metadata")),
            Ok(_)=>Some(serde_json::from_slice::<SessionMeta>(&fs::read(&meta_path)?)?),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            Err(e)=>return Err(e.into()),
        };
        if let Some(m)=&meta {if m.version!=1 || m.session_id!=id {return Err(VisualError::Conflict("visual binding metadata mismatch"));}}
        if let (Some(b),Some(m))=(&binding,&meta) {if b.chat!=m.chat || b.draft_root!=m.draft_root {return Err(VisualError::Conflict("visual binding changed during creation"));}}
        if meta.as_ref().is_some_and(|m|&m.chat==chat) || binding.as_ref().is_some_and(|b|&b.chat==chat) {return Ok(Some(id));}
        if meta.is_none() && binding.is_none() && entry.path().join("pending.json").exists() {unbound_pending=true;}
    }
    if unbound_pending {return Err(VisualError::Conflict("unbound pending visual chat; inspect incomplete creation before other sends"));}
    Ok(None)
}
pub fn list(root: &Path) -> VisualResult<Vec<LoadedSession>> {
    if !root.exists() { return Ok(Vec::new()); }
    let mut sessions=Vec::new();
    for e in fs::read_dir(root)? {
        let e=e?;
        if !e.file_type()?.is_dir() { return Err(VisualError::Conflict("unrecognized visual storage entry")); }
        let id=e.file_name().into_string().map_err(|_| VisualError::Conflict("invalid session folder name"))?;
        if id=="_journals" { ProjectRoot::open(&e.path())?; continue; }
        sessions.push(load(root,&id)?);
    }
    Ok(sessions)
}
pub fn validate_id(id:&str)->VisualResult<()> {
    if id.is_empty() || id.len()>80 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b==b'-' || b==b'_') { return Err(VisualError::InvalidPath(id.into())); }
    Ok(())
}
pub fn save_snapshot(dir:&Path, snapshot:&Snapshot)->VisualResult<()> {
    fs::create_dir(dir)?;

    for file in snapshot.selected() {
        let rel=super::RelativePath::parse(&file.path)?;
        if let Some(bytes)=snapshot.bytes(&file.path) {
            let path=rel.join(dir);

            if let Some(parent)=path.parent() { fs::create_dir_all(parent)?; }
            durable_file(&path,bytes)?;
        }
    }
    durable_file(&dir.join("manifest.json"),&serde_json::to_vec_pretty(&snapshot.manifest())?)?;
    Ok(())
}
pub fn load_snapshot(dir:&Path, scope:&FileScope)->VisualResult<Snapshot> {
    let root=ProjectRoot::open(dir)?;
    let snapshot=Snapshot::capture(&root,scope)?;
    let saved:SnapshotManifest=serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
    if snapshot.manifest()!=saved { return Err(VisualError::Conflict("snapshot bytes do not match manifest")); }
    Ok(snapshot)
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastOperationView { pub kind:String, pub id:String, pub outcome:String, pub message:Option<String> }
#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::backend::model::{BackendKind, ExternalId};
    use std::sync::atomic::{AtomicU64,Ordering};
    static NEXT:AtomicU64=AtomicU64::new(0);
    #[test]
    fn journal_sibling_is_not_loaded_as_a_session_after_a_file_operation() {
        let base=std::env::current_dir().unwrap().join("target").join(format!("visual-session-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst)));
        let source=base.join("source"); let store=base.join("store");
        fs::create_dir_all(&source).unwrap(); fs::create_dir_all(&store).unwrap();
        fs::write(source.join("index.html"),b"<h1>test</h1>").unwrap();
        let project=ProjectRoot::open(&source).unwrap();
        let scope=FileScope::new(vec![SelectedFile{path:"index.html".into(),access:FileAccess::Edit}]).unwrap();
        let draft=super::super::create_isolated_draft(&project,scope.clone(),&store,"session-1").unwrap();
        fs::write(draft.storage_path().join("initial").join("manifest.json"),serde_json::to_vec(&draft.initial().manifest()).unwrap()).unwrap();
        let meta=SessionMeta{version:1,session_id:"session-1".into(),chat:ChatKey{backend:BackendKind::Codex,id:ExternalId("thread-1".into())},source_root:project.path().to_string_lossy().into_owned(),draft_root:draft.root().path().to_string_lossy().into_owned(),html_path:"index.html".into(),scope:scope.selected()};
        save_meta(draft.storage_path(),&meta).unwrap();
        File::create(draft.storage_path().join("events.jsonl")).unwrap();
        fs::create_dir_all(store.join("_journals").join("session-1").join("adopt-1")).unwrap();
        assert_eq!(list(&store).unwrap().len(),1);
        drop(draft); drop(project);
        fs::remove_dir_all(base).unwrap();
    }
    struct Case { base: PathBuf, root: PathBuf, id: String }
    impl Drop for Case { fn drop(&mut self) {let _=fs::remove_dir_all(&self.base);} }
    impl Case {
        fn new()->Self {
            let base=std::env::current_dir().unwrap().join("target").join(format!("visual-event-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst)));
            let source=base.join("source"); let root=base.join("store");
            fs::create_dir_all(&source).unwrap(); fs::create_dir_all(&root).unwrap();
            fs::write(source.join("index.html"),b"<h1>old</h1>").unwrap();
            let project=ProjectRoot::open(&source).unwrap();
            let scope=FileScope::new(vec![SelectedFile{path:"index.html".into(),access:FileAccess::Edit}]).unwrap();
            let id="session-1".to_string();
            let draft=super::super::create_isolated_draft(&project,scope.clone(),&root,&id).unwrap();
            fs::write(draft.storage_path().join("initial").join("manifest.json"),serde_json::to_vec(&draft.initial().manifest()).unwrap()).unwrap();
            let meta=SessionMeta{version:1,session_id:id.clone(),chat:ChatKey{backend:BackendKind::Codex,id:ExternalId("thread-1".into())},source_root:project.path().to_string_lossy().into_owned(),draft_root:draft.root().path().to_string_lossy().into_owned(),html_path:"index.html".into(),scope:scope.selected()};
            save_meta(draft.storage_path(),&meta).unwrap();
            File::create(draft.storage_path().join("events.jsonl")).unwrap();
            Self{base,root,id}
        }
        fn loaded(&self)->LoadedSession{load(&self.root,&self.id).unwrap()}
        fn folder(&self)->PathBuf{self.root.join(&self.id)}
        fn captured_edit(&self) {
            let before=self.loaded().initial().unwrap();
            save_snapshot(&self.folder().join("b1"),&before).unwrap();
            append(&self.folder(),&SessionEvent::Prepared{attempt_id:"a1".into(),client_message_id:"m1".into(),before:"b1".into()}).unwrap();
            append(&self.folder(),&SessionEvent::Accepted{attempt_id:"a1".into(),turn_id:"t1".into()}).unwrap();
            fs::write(self.folder().join("draft").join("index.html"),b"<h1>new</h1>").unwrap();
            let draft=ProjectRoot::open(&self.folder().join("draft")).unwrap();
            let after=Snapshot::capture(&draft,&self.loaded().scope().unwrap()).unwrap();
            save_snapshot(&self.folder().join("e1"),&after).unwrap();
            append(&self.folder(),&SessionEvent::Captured{attempt_id:"a1".into(),after:"e1".into()}).unwrap();
        }
    }
    #[test]
    fn accepted_attempt_cannot_be_downgraded_by_late_unknown() {
        let c=Case::new();
        append(&c.folder(),&SessionEvent::Prepared{attempt_id:"a1".into(),client_message_id:"m1".into(),before:"b1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::Unknown{attempt_id:"a1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::Accepted{attempt_id:"a1".into(),turn_id:"t1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::Unknown{attempt_id:"a1".into()}).unwrap();
        let s=c.loaded();
        assert!(!s.latest_unknown("a1"));
        assert_eq!(s.view().unwrap().phase,"runningOrReconciling");
    }
    #[test]
    fn rejected_next_attempt_preserves_last_captured_edit() {
        let c=Case::new(); c.captured_edit();
        let s=c.loaded(); let snapshot=load_snapshot(&c.folder().join("e1"),&s.scope().unwrap()).unwrap();
        save_snapshot(&c.folder().join("b2"),&snapshot).unwrap();
        append(&c.folder(),&SessionEvent::Prepared{attempt_id:"a2".into(),client_message_id:"m2".into(),before:"b2".into()}).unwrap();
        append(&c.folder(),&SessionEvent::Rejected{attempt_id:"a2".into()}).unwrap();
        let s=c.loaded();
        assert_eq!(s.last_capture(),Some(("a1","e1")));
        let v=s.view().unwrap();
        assert_eq!(v.phase,"changes");
        assert!(v.changes[0].change.is_some());
    }
    #[test]
    fn applied_adoption_reconstructs_baseline_after_restart() {
        let c=Case::new(); c.captured_edit();
        append(&c.folder(),&SessionEvent::OperationIntent{operation_id:"op1".into(),purpose:"adopt".into(),revision:1,desired:"e1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::OperationResult{operation_id:"op1".into(),outcome:"applied".into()}).unwrap();
        let s=c.loaded();
        assert_eq!(s.latest_adopted(),Some("e1"));
        assert!(!s.pending_operation());
        assert!(s.view().unwrap().changes.iter().all(|f|f.change.is_none()));
    }
    #[test]
    fn prewrite_rejection_settles_while_partial_remains_blocked() {
        let c=Case::new(); c.captured_edit();
        append(&c.folder(),&SessionEvent::OperationIntent{operation_id:"op1".into(),purpose:"adopt".into(),revision:1,desired:"e1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::OperationResult{operation_id:"op1".into(),outcome:"rejected".into()}).unwrap();
        let s=c.loaded(); assert!(!s.pending_operation());
        assert_eq!(s.view().unwrap().last_operation.unwrap().outcome,"rejected");
        append(&c.folder(),&SessionEvent::OperationIntent{operation_id:"op2".into(),purpose:"adopt".into(),revision:1,desired:"e1".into()}).unwrap();
        append(&c.folder(),&SessionEvent::OperationResult{operation_id:"op2".into(),outcome:"partial".into()}).unwrap();
        let s=c.loaded(); let v=s.view().unwrap();
        assert_eq!(v.phase,"partial");
        assert!(!v.blockers.is_empty());
    }
    #[test]
    fn binding_survives_unrelated_pending_folder_and_missing_source() {
        let c=Case::new();
        fs::create_dir(c.root.join("pending-2")).unwrap();
        fs::write(c.root.join("pending-2").join("pending.json"),b"pending").unwrap();
        let chat=c.loaded().meta.chat;
        assert_eq!(binding_id(&c.root,&chat).unwrap(),Some(c.id.clone()));
        fs::remove_dir_all(c.base.join("source")).unwrap();
        assert!(load(&c.root,&c.id).is_err());
        assert_eq!(binding_id(&c.root,&chat).unwrap(),Some(c.id.clone()));
    }
    #[test]
    fn incomplete_binding_is_found_without_wire_cwd_or_session_metadata() {
        let c=Case::new();
        let chat=c.loaded().meta.chat;
        let binding=PendingBinding{version:1,session_id:c.id.clone(),chat:chat.clone(),draft_root:c.folder().join("draft").to_string_lossy().into_owned()};
        save_pending_binding(&c.folder(),&binding).unwrap();
        fs::remove_file(c.folder().join("session.json")).unwrap();
        assert_eq!(binding_id(&c.root,&chat).unwrap(),Some(c.id.clone()));
        assert!(load(&c.root,&c.id).is_err());
    }
    #[test]
    fn unbound_pending_creation_fails_closed_for_unknown_chat() {
        let c=Case::new();
        let pending=c.root.join("pending-2");
        fs::create_dir(&pending).unwrap();
        fs::write(pending.join("pending.json"),b"prepared-before-thread-start").unwrap();
        let other=ChatKey{backend:BackendKind::Codex,id:ExternalId("other-thread".into())};
        assert!(binding_id(&c.root,&other).is_err());
    }
}