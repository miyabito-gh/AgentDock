use super::{err, Host, SendAuth};
use crate::backend::{backend::{PermissionPreset, SendMode, SendRequest, UserConfirmed}, ipc::{IpcError, IpcErrorCode, StartChatArgs}, model::{BackendKind, ChatKey, ModelChoice, SendAttempt, SendState}};
use crate::visual_edit::{self as ve, session::{self, LoadedSession, RestoreInfo, SessionEvent, SessionMeta, SessionView}, FileAccess, FileScope, ProjectRoot, RelativePath, SelectedFile, Snapshot};
use serde::Deserialize;
use std::{fs, path::{Path, PathBuf}, sync::Arc};

#[derive(Default)]
pub(super) struct VisualRuntime { pub lock: tokio::sync::Mutex<()> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualCreateArgs { pub root_path:String, pub html_path:String, pub model:Option<ModelChoice>, #[serde(default)] pub chat:Option<ChatKey>, #[serde(default)] pub mode:Option<String> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualSessionArgs { pub session_id:String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualReleaseArgs { pub chat:ChatKey }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualWriteArgs { pub session_id:String, pub revision:u64 }
/// 保存領域の中の、リンクでないフォルダだけを削除する（保存領域そのものや外は削除しない）。
fn remove_storage_dir(storage:&Path,dir:&Path)->Result<(),String> {
    let meta=match fs::symlink_metadata(dir) {Ok(m)=>m,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()),Err(e)=>return Err(e.to_string())};
    if meta.file_type().is_symlink() || !meta.is_dir() {return Err("not a plain directory".into());}
    let root=fs::canonicalize(storage).map_err(|e|e.to_string())?;
    let target=fs::canonicalize(dir).map_err(|e|e.to_string())?;
    if target==root || !target.starts_with(&root) {return Err("outside the visual storage".into());}
    fs::remove_dir_all(dir).map_err(|e|e.to_string())
}
/// visual_create の途中で失敗（早期return）したとき、作りかけの保存先を削除する。完了時は dir を None にして無効にする。
struct CreateCleanup { storage:PathBuf, dir:Option<PathBuf> }
impl Drop for CreateCleanup {
    fn drop(&mut self) {
        if let Some(dir)=self.dir.take() {
            if let Err(e)=remove_storage_dir(&self.storage,&dir) {crate::diag::log("visual-create",&format!("could not clean up an incomplete edit copy: {e}"));}
        }
    }
}
/// 編集の記録フォルダを、途中で止まっても「紐付いているように見える」状態を残さない順に消す。
fn remove_session_folder(storage:&Path,folder:&Path)->Result<(),String> {
    let mut dirs=Vec::new(); let mut files=Vec::new();
    for e in fs::read_dir(folder).map_err(|x|x.to_string())? {
        let e=e.map_err(|x|x.to_string())?;
        let name=e.file_name().to_string_lossy().into_owned();
        if e.file_type().map_err(|x|x.to_string())?.is_dir() {dirs.push(e.path());} else {files.push(name);}
    }
    for d in dirs {remove_storage_dir(storage,&d)?;}
    let keep=["session.json","events.jsonl"];
    for f in files.iter().filter(|f|!keep.contains(&f.as_str())) {fs::remove_file(folder.join(f)).map_err(|x|x.to_string())?;}
    for f in keep {if folder.join(f).exists() {fs::remove_file(folder.join(f)).map_err(|x|x.to_string())?;}}
    fs::remove_dir(folder).map_err(|x|x.to_string())
}
fn visual_err(e:impl std::fmt::Display)->IpcError { err(IpcErrorCode::Blocked,format!("Visual Editor: {e}")) }
/// OperationIntent を追記する前（元のフォルダにもコピーにも何も書いていない）に止めたときのエラー。UIが「反映していません」と出せるよう、先頭に印を付ける。
const NOTHING_WRITTEN:&str="nothing written: ";
fn nw_err(e:impl std::fmt::Display)->IpcError { err(IpcErrorCode::Blocked,format!("{NOTHING_WRITTEN}Visual Editor: {e}")) }
fn nw_ipc(mut e:IpcError)->IpcError { if !e.message.starts_with(NOTHING_WRITTEN) {e.message=format!("{NOTHING_WRITTEN}{}",e.message);} e }
fn path_string(path:&Path)->Result<String,IpcError> { Ok(ve::preview::normal_drive_path(path).map_err(visual_err)?.to_string_lossy().into_owned()) }
fn known_access(ext:&str)->Option<FileAccess> {
    match ext { "html"|"htm"|"css"|"js"|"mjs"|"svg"=>Some(FileAccess::Edit), "png"|"jpg"|"jpeg"|"gif"|"webp"|"avif"|"ico"|"woff"|"woff2"|"ttf"|"otf"|"json"=>Some(FileAccess::ReadOnly), _=>None }
}
fn discover(root:&ProjectRoot, html:&RelativePath)->ve::VisualResult<FileScope> {
    let mut chosen=Vec::new();
    let mut stack=vec![root.path().to_path_buf()];
    while let Some(dir)=stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry=entry?;
            let relative=entry.path().strip_prefix(root.path()).map_err(|_| ve::VisualError::Conflict("scope path escaped root"))?.to_string_lossy().replace('\\',"/");
            let Ok(rel)=RelativePath::parse(&relative) else { continue };
            let metadata=fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() { return Err(ve::VisualError::Conflict("link in discovered scope")); }
            if metadata.is_dir() { ProjectRoot::from_verified_path(&entry.path())?; stack.push(entry.path()); continue; }
            if !metadata.is_file() { continue; }
            let ext=rel.as_str().rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if let Some(access)=known_access(&ext) {
                chosen.push(SelectedFile {path:relative,access});
                if chosen.len()>ve::MAX_FILES { return Err(ve::VisualError::Limit("discovered file count")); }
            }
        }
    }
    if !chosen.iter().any(|f| f.path==html.as_str() && f.access==FileAccess::Edit) { return Err(ve::VisualError::Conflict("selected HTML is not an editable file")); }
    FileScope::new(chosen)
}
impl Host {
    fn visual_root(&self)->PathBuf { self.app_data_dir.join("visual-editor") }
    fn visual_cwd(&self,chat:&ChatKey)->bool {
        let root=path_string(&self.visual_root()).unwrap_or_else(|_|self.visual_root().to_string_lossy().into_owned());
        self.read(|d|d.chat(chat).and_then(|c|c.cwd.value()).cloned()).is_some_and(|cwd| {
            let cwd=crate::rules::work_folders::normalize_path(&cwd).unwrap_or(cwd);
            let root=crate::rules::work_folders::normalize_path(&root).unwrap_or(root);
            crate::rules::work_folders::is_same_or_under(&cwd,&root)
        })
    }
    /// 編集を終えたが、元の作業場所へ戻す送信がまだの会話か（作業場所の表示はまだコピーを指す）。この間の引き継ぎ・分岐・レビュー・作業フォルダ変更は止める。
    pub fn visual_restore_pending(&self, chat:&ChatKey)->Result<bool,IpcError> {
        if !self.visual_cwd(chat) {return Ok(false);}
        Ok(session::restore_for(&self.visual_root(),chat).map_err(visual_err)?.is_some())
    }
    pub fn visual_bound(&self, chat:&ChatKey)->Result<bool,IpcError> {
        if session::binding_id(&self.visual_root(),chat).map_err(visual_err)?.is_some() {return Ok(true);}
        // 作業場所がコピー配下でも、既存の会話での編集を終えた会話は編集中ではない（元へ戻す送信で作業場所を直す）。
        if self.visual_cwd(chat) {return Ok(session::restore_for(&self.visual_root(),chat).map_err(visual_err)?.is_none());}
        Ok(false)
    }
    /// 会話中のリンクの基準にする作業フォルダ（先頭が現在の作業フォルダ）。編集を終えて、元の作業場所へ戻す送信がまだの会話は、
    /// 作業フォルダがまだ編集用コピーを指しているため、戻し先（元の作業フォルダ）も内側として扱う。読めないときは現在の作業フォルダだけ。
    pub(super) fn chat_link_roots(&self,chat:&ChatKey)->Vec<String> {
        let mut roots:Vec<String>=self.read(|d|d.chats.iter().find(|c|&c.key==chat).and_then(|c|c.cwd.value().cloned())).into_iter().collect();
        if let Ok(Some((_,info)))=session::restore_for(&self.visual_root(),chat) {roots.extend(info.folders);}
        roots
    }
    /// 反映を止めた会話の表示（会話名と、重なっているフォルダ）。
    fn overlap_detail(&self,chat:&ChatKey,folders:&[String])->String {
        let path=self.chat_operation_folders(chat).into_iter().find(|p|folders.iter().any(|f|super::cloud::paths_overlap(p,f))).unwrap_or_default();
        if path.is_empty() {self.chat_title(chat)} else {format!("{} @ {}",self.chat_title(chat),path)}
    }
    /// 一覧から外れている（アーカイブ・一覧から外した・削除保留）会話か。
    fn visual_chat_hidden(&self,chat:&ChatKey)->bool {
        self.read(|d|d.locals.get(chat).is_some_and(|l|!matches!(l.visibility,crate::backend::local::ListVisibility::Visible) || l.delete_pending.is_some()))
    }
    /// 停止を確認できなかった会話（確認できたものは含めない）。一覧から外れた会話は、動いている兆しがなければ確認済みとして扱う。
    async fn visual_unconfirmed_overlaps(self:&Arc<Self>,chats:&[ChatKey])->Vec<ChatKey> {
        let mut out=Vec::new();
        for chat in chats {
            if self.visual_chat_stopped(chat).await {continue;}
            if self.visual_chat_hidden(chat) {
                let gate=self.queue_gate_input(chat).await;
                let active=gate.root.as_ref().is_some_and(|r|matches!(r.state,crate::backend::model::AgentState::Running|crate::backend::model::AgentState::Waiting|crate::backend::model::AgentState::Initializing))
                    || gate.descendants.iter().any(|f|matches!(f.state,crate::backend::model::AgentState::Running|crate::backend::model::AgentState::Waiting|crate::backend::model::AgentState::Initializing))
                    || gate.open_stop.is_some() || gate.unresolved_attempt.is_some() || gate.unresolved_op.is_some() || gate.manual_turn_pending;
                if !active {continue;}
            }
            out.push(chat.clone());
        }
        out
    }
    fn visual_loaded(&self,id:&str)->Result<LoadedSession,IpcError> { session::load(&self.visual_root(),id).map_err(visual_err) }
    /// 既存の会話で続ける・分岐して続ける場合の、対象の会話と戻し先。専用チャットを新規に作る場合は None。
    /// 会話が止まっていると確認できるか。ライブで終了を観測済みのturnは通常の経路では履歴を読まないため、ここで読取り専用に履歴の終端を確認する。
    async fn visual_chat_stopped(self:&Arc<Self>,chat:&ChatKey)->bool {
        let mut gate=self.queue_gate_input(chat).await;
        if !Self::visual_overlap_stopped(&gate) && gate.root.as_ref().is_some_and(|r|r.latest_end.is_some() && !r.terminal_explicit) {
            if self.read_terminal_fact(crate::host::state::agent_key_of(chat)).await {gate=self.queue_gate_input(chat).await;}
        }
        Self::visual_overlap_stopped(&gate)
    }
    async fn visual_existing_target(self:&Arc<Self>,args:&VisualCreateArgs,source:&ProjectRoot,confirmed:&UserConfirmed)->Result<Option<(ChatKey,RestoreInfo,Option<String>)>,IpcError> {
        let mode=args.mode.as_deref().unwrap_or("new");
        if mode=="new" {return Ok(None);}
        if mode!="current" && mode!="fork" {return Err(visual_err("unknown start mode"));}
        let Some(chat)=args.chat.clone() else {return Err(visual_err("an existing chat is required for this start mode"));};
        if chat.backend!=BackendKind::Codex {return Err(visual_err("only Codex chats can be edited in place"));}
        self.check_not_org_owned(&chat)?;
        if self.read(|d|d.chat(&chat).is_none()) {return Err(visual_err("chat is not listed"));}
        if self.visual_bound(&chat)? {return Err(visual_err("this chat is already used by an edit session"));}
        // 前の編集を終えたあと、元の作業場所へ戻す送信がまだの会話は、前の戻し先を引き継いで新しい編集を始められる。
        let pending=session::restore_for(&self.visual_root(),&chat).map_err(visual_err)?;
        if self.send_unsettled(&chat) || self.subagent_turn_pending(&chat) {return Err(visual_err("this chat has a send or turn that is not settled"));}
        if !self.visual_chat_stopped(&chat).await {return Err(visual_err("this chat is running, or its stop is not confirmed"));}
        let (restore,superseded)=match pending {
            // 作業場所はまだコピーを指しているので、いまの値ではなく、前の編集が記録した元の場所を戻し先にする。
            Some((old_id,info))=>(info,Some(old_id)),
            None=>{
                let cwd=self.read(|d|d.chat(&chat).and_then(|c|c.cwd.value().cloned())).ok_or_else(||visual_err("the chat's work folder is unknown"))?;
                let folders=self.work_folders_for_send(&chat,true).map(|f|f.folders).filter(|f|!f.is_empty()).unwrap_or_else(||vec![cwd.clone()]);
                (RestoreInfo{folders},None)
            }
        };
        let _=source;
        if mode=="current" {return Ok(Some((chat,restore,superseded)));}
        let forked=self.fork_chat(crate::backend::ipc::ForkChatArgs{chat:chat.clone(),through_turn:None},confirmed).await?;
        let Some(new_chat)=forked.chat else {return Err(visual_err("the chat could not be forked (not accepted or not confirmed); nothing was changed"));};
        Ok(Some((new_chat.key,restore,None)))
    }
    pub async fn visual_create(self:&Arc<Self>,args:VisualCreateArgs,confirmed:&UserConfirmed)->Result<SessionView,IpcError> {
        // 送信ロックは visual ロックより先に取る（通常の送信と同じ順序）。
        let _send_guard=match (&args.chat,args.mode.as_deref()) {(Some(c),Some("current"|"fork"))=>Some(self.queue_rt.send_lock(c).lock_owned().await),_=>None};
        let _guard=self.visual_rt.lock.lock().await;
        let source=ProjectRoot::open(Path::new(&args.root_path)).map_err(visual_err)?;
        let html=ve::preview::relative_entry(source.path(),&args.html_path).map_err(visual_err)?;
        let scope=discover(&source,&html).map_err(visual_err)?;
        let existing=self.visual_existing_target(&args,&source,confirmed).await?;
        let storage_root=self.visual_root(); fs::create_dir_all(&storage_root).map_err(visual_err)?;
        let id=self.local_id("visual").0;
        session::validate_id(&id).map_err(visual_err)?;
        // 作成の途中で失敗したら、作りかけの保存先（pending.json だけの殻は全会話の送信を止める）を片付ける。draft（フォルダを掴む）より後に落ちるよう、先に宣言する。
        let mut cleanup=CreateCleanup{storage:storage_root.clone(),dir:None};
        let draft=ve::create_isolated_draft(&source,scope.clone(),&storage_root,&id).map_err(visual_err)?;
        cleanup.dir=Some(draft.storage_path().to_path_buf());
        let initial_manifest=serde_json::to_vec_pretty(&draft.initial().manifest()).map_err(visual_err)?;
        let initial_manifest_path=draft.storage_path().join("initial").join("manifest.json");
        let mut manifest_file=fs::OpenOptions::new().write(true).create_new(true).open(initial_manifest_path).map_err(visual_err)?;
        use std::io::Write;
        manifest_file.write_all(&initial_manifest).map_err(visual_err)?; manifest_file.sync_all().map_err(visual_err)?;
        let draft_root=path_string(draft.root().path())?;
        let pending=serde_json::json!({"version":1,"sessionId":id,"draftRoot":draft_root});
        let mut pending_file=fs::OpenOptions::new().write(true).create_new(true).open(draft.storage_path().join("pending.json")).map_err(visual_err)?;
        serde_json::to_writer(&mut pending_file,&pending).map_err(visual_err)?; pending_file.sync_all().map_err(visual_err)?;
        let (chat_key,restore,superseded)=match existing {
            Some((key,restore,superseded))=>(key,Some(restore),superseded),
            None=>{
                let started=match self.start_chat_in(StartChatArgs { backend:BackendKind::Codex,cwd:None,model:args.model,permission:Some(PermissionPreset::WorkspaceWriteOnRequest),first_message:None,worktree:None,extra_folders:Vec::new(),confirmed_strong:Vec::new() },Some(draft_root.clone()),confirmed).await {
                    Ok(started)=>started,
                    // 会話を作れなかったときの作りかけのコピーは、cleanup が片付ける。
                    Err(e)=>return Err(e),
                };
                (started.chat.key,None,None)
            }
        };
        let binding=session::PendingBinding{version:1,session_id:id.clone(),chat:chat_key.clone(),draft_root:draft_root.clone()};
        session::save_pending_binding(draft.storage_path(),&binding).map_err(visual_err)?;
        let meta=SessionMeta {version:1,session_id:id,chat:chat_key,source_root:path_string(source.path())?,draft_root,html_path:html.as_str().into(),scope:scope.selected(),mode:restore.as_ref().map(|_|"current".to_string()),restore};
        session::save_meta(draft.storage_path(),&meta).map_err(visual_err)?;
        fs::OpenOptions::new().write(true).create_new(true).open(draft.storage_path().join("events.jsonl")).and_then(|f|f.sync_all()).map_err(visual_err)?;
        cleanup.dir=None;
        // 戻し先は新しい編集が引き継いだので、前の編集の戻し待ちは置き換え済みとして閉じる。
        if let Some(old)=superseded {if let Ok(s)=self.visual_loaded(&old) {if s.restore_pending() {let _=session::append(&s.folder,&SessionEvent::Restored);}}}
        self.visual_loaded(&meta.session_id)?.view().map_err(visual_err)
    }
    /// 既存の会話での編集を終える。以後その会話の送信は、元の作業場所へ戻して行う。専用チャットの編集は終えるものがない。
    pub async fn visual_end(self:&Arc<Self>,session_id:&str)->Result<SessionView,IpcError> {
        let initial=self.visual_loaded(session_id)?;
        let chat=initial.meta.chat.clone();
        let _send_guard=self.queue_rt.send_lock(&chat).lock_owned().await;
        let _guard=self.visual_rt.lock.lock().await;
        let mut s=self.visual_loaded(session_id)?;
        if !s.is_current_mode() {return Err(visual_err("only an edit on an existing chat can be ended"));}
        if s.ended() {return s.view().map_err(visual_err);}
        self.visual_refresh(&mut s).await?;
        let view=s.view().map_err(visual_err)?;
        if matches!(view.phase.as_str(),"preparedUnresolved"|"acceptanceUnknown"|"runningOrReconciling"|"partial"|"recoveryRequired") || s.pending_operation() {
            return Err(visual_err("the edit is still running or unresolved; wait until it stops and its changes are confirmed"));
        }
        if self.send_unsettled(&chat) || self.subagent_turn_pending(&chat) {return Err(visual_err("this chat has a send or turn that is not settled"));}
        session::append(&s.folder,&SessionEvent::Ended).map_err(visual_err)?;
        self.visual_loaded(session_id)?.view().map_err(visual_err)
    }
    /// 編集用コピーに紐付いたままの既存の会話を、利用者の確認のうえで切り離す（終えられない・コピーが無い・記録が壊れたときの出口）。
    /// コピーも元のフォルダも削除・変更しない。会話は「終えた」扱いになり、次の通常の送信で元の作業場所へ戻る。
    /// AIの停止は確認しない（実行中のAIがコピーへ書く可能性は残る）。専用会話は戻し先がないので対象外。
    pub async fn visual_release(self:&Arc<Self>,chat:&ChatKey)->Result<bool,IpcError> {
        let _send_guard=self.queue_rt.send_lock(chat).lock_owned().await;
        let _guard=self.visual_rt.lock.lock().await;
        let Some(id)=session::binding_id(&self.visual_root(),chat).map_err(visual_err)? else {return Ok(false)};
        session::validate_id(&id).map_err(visual_err)?;
        let folder=self.visual_root().join(&id);
        let meta:SessionMeta=fs::read(folder.join("session.json")).ok().and_then(|b|serde_json::from_slice(&b).ok()).ok_or_else(||visual_err("the edit record is unreadable"))?;
        // 元への反映・取り消しが途中のものは、状態が見えなくなるので切り離さない（コピーが読めない場合は、守るものが無いので続ける）。
        // 一部だけ書いた(partial)編集も切り離さない（原本が一部書き換わっていて、戻すための journal を失わせないため）。何も書いていない衝突(conflict)は partial にしないので、ここでは止まらない。
        if let Ok(s)=self.visual_loaded(&id) {
            let _=session::remove_orphan_snapshots(&s);
            if s.pending_operation() || matches!(s.view().map(|v|v.phase).unwrap_or_default().as_str(),"recoveryRequired"|"partial") {return Err(visual_err("the edit has an unfinished apply or undo; resolve it before releasing"));}
        }
        if meta.mode.as_deref()!=Some("current") || meta.restore.is_none() {return Err(visual_err("only an edit on an existing chat can be released"));}
        session::append(&folder,&SessionEvent::Ended).map_err(visual_err)?;
        Ok(true)
    }
    /// 古い編集の自動整理（利用者の許可、2026-11-03）。対象は `session::cleanup_candidates`（反映済みで終えた編集と、破棄した編集の殻。既定は14日経過後）。
    /// 途中で失敗しても、紐付けの確認が壊れない順に消す（先に「破棄」を記録し、中身→識別ファイル→記録の順）。失敗した分は残して次回に回す。
    pub async fn visual_cleanup(self:&Arc<Self>,min_age_days:u64)->Result<usize,IpcError> {
        let _guard=self.visual_rt.lock.lock().await;
        let root=self.visual_root();
        let ids=session::cleanup_candidates(&root,std::time::Duration::from_secs(min_age_days.saturating_mul(86_400))).map_err(visual_err)?;
        let mut removed=0usize;
        for id in ids {
            let folder=root.join(&id);
            if !fs::read_to_string(folder.join("events.jsonl")).map(|t|t.contains("\"kind\":\"discarded\"")).unwrap_or(false) {
                if session::append(&folder,&SessionEvent::Discarded).is_err() {continue;}
            }
            let _=remove_storage_dir(&root,&root.join("_journals").join(&id));
            if remove_session_folder(&root,&folder).is_ok() {removed+=1;}
        }
        Ok(removed)
    }
    /// 編集を破棄する。コピーのファイルを削除し、反映前のバックアップ（元の内容）は `keep_backup` のとき残す。
    /// 反映していない編集は元に影響しないので、すべて削除する。元のフォルダには触れない。
    pub async fn visual_discard(self:&Arc<Self>,session_id:&str,keep_backup:bool)->Result<(),IpcError> {
        let initial=self.visual_loaded(session_id)?;
        let chat=initial.meta.chat.clone();
        let _send_guard=self.queue_rt.send_lock(&chat).lock_owned().await;
        let _guard=self.visual_rt.lock.lock().await;
        let mut s=self.visual_loaded(session_id)?;
        if s.is_current_mode() && !s.ended() {return Err(visual_err("end the edit first, then discard it"));}
        self.visual_refresh(&mut s).await?;
        let view=s.view().map_err(visual_err)?;
        if matches!(view.phase.as_str(),"preparedUnresolved"|"acceptanceUnknown"|"runningOrReconciling"|"partial"|"recoveryRequired") || s.pending_operation() {
            return Err(visual_err("the edit is still running or unresolved; wait until it stops and its changes are confirmed"));
        }
        if self.send_unsettled(&chat) || self.subagent_turn_pending(&chat) {return Err(visual_err("this chat has a send or turn that is not settled"));}
        let keep=keep_backup && s.latest_adopted().is_some();
        let folder=s.folder.clone();
        let storage=self.visual_root();
        // 先に記録する（以後は一覧・紐付けから外れる）。削除が途中で失敗しても、元のファイルには影響しない。
        session::append(&folder,&SessionEvent::Discarded).map_err(visual_err)?;
        let mut failed=Vec::new();
        let mut targets=Vec::new();
        for entry in fs::read_dir(&folder).map_err(visual_err)? {
            let entry=entry.map_err(visual_err)?;
            if !entry.file_type().map_err(visual_err)?.is_dir() {continue;}
            let name=entry.file_name().to_string_lossy().into_owned();
            let remove=name=="draft" || (!keep && name=="initial") || (name.starts_with("tx-") && (!keep || name.ends_with("-after")));
            if remove {targets.push(entry.path());}
        }
        if !keep {targets.push(storage.join("_journals").join(session_id));}
        for dir in targets {
            if let Err(e)=remove_storage_dir(&storage,&dir) {failed.push(format!("{}: {e}",dir.display()));}
        }
        if failed.is_empty() {Ok(())} else {Err(visual_err(format!("the edit was discarded, but some files could not be deleted: {}",failed.join("; "))))}
    }
    pub async fn visual_list(self:&Arc<Self>)->Result<Vec<SessionView>,IpcError> {
        let _guard=self.visual_rt.lock.lock().await;
        session::list(&self.visual_root()).map_err(visual_err)?.iter().map(|s|s.view().map_err(visual_err)).collect()
    }
    fn visual_gate_stopped(&self,gate:&crate::rules::queue::GateInput,turn:&str)->Result<(),IpcError> {
        if !gate.connected || !gate.descendant_scan_complete || gate.open_stop.is_some() || gate.unresolved_attempt.is_some() || gate.unresolved_op.is_some() || gate.manual_turn_pending || gate.delete_pending || gate.quitting { return Err(visual_err("stop, scan, send, or operation is unresolved")); }
        let root=gate.root.as_ref().ok_or_else(||visual_err("root state unavailable"))?;
        if root.latest_turn.as_ref().map(|t|t.0.as_str())!=Some(turn) || root.latest_end.is_none() || !root.terminal_explicit { return Err(visual_err("accepted root turn has no authoritative terminal")); }
        if gate.descendants.iter().any(|f| f.latest_end.is_none() || !f.terminal_explicit || matches!(f.state,crate::backend::model::AgentState::Running|crate::backend::model::AgentState::Waiting|crate::backend::model::AgentState::Initializing)) { return Err(visual_err("descendant stop is unconfirmed")); }
        Ok(())
    }
    fn visual_overlap_stopped(gate:&crate::rules::queue::GateInput)->bool {
        if !gate.connected || !gate.descendant_scan_complete || gate.open_stop.is_some() || gate.unresolved_attempt.is_some() || gate.unresolved_op.is_some() || gate.manual_turn_pending || gate.delete_pending || gate.quitting {return false;}
        let Some(root)=gate.root.as_ref() else {return false};
        let terminal=|f:&crate::rules::queue::AgentFact| {
            if matches!(f.state,crate::backend::model::AgentState::Running|crate::backend::model::AgentState::Waiting|crate::backend::model::AgentState::Initializing) {return false;}
            match &f.latest_turn {
                Some(_)=>f.terminal_explicit && f.latest_end.is_some(),
                None=>f.state==crate::backend::model::AgentState::Idle && f.freshness==crate::backend::model::Freshness::Live,
            }
        };
        terminal(root) && gate.descendants.iter().all(&terminal)
    }    async fn visual_refresh(self:&Arc<Self>,s:&mut LoadedSession)->Result<(),IpcError> {
        // スナップショット作成の途中で止まった残り（参照されない tx-*）は、作り直せるので先に片付ける（残ると回復待ちで固定される）。
        match session::remove_orphan_snapshots(s) {
            Ok(n) if n>0=>crate::diag::log("visual-session",&format!("removed {n} unreferenced snapshot folder(s) left by an interrupted save")),
            Ok(_)=>(),
            Err(e)=>crate::diag::log("visual-session",&format!("could not remove unreferenced snapshots: {e}")),
        }
        let Some((attempt,_,_))=s.latest() else {return Ok(())};
        let attempt=attempt.to_string();
        if s.after(&attempt).is_some() || s.latest_rejected(&attempt) {return Ok(())}
        let Some(turn)=s.accepted_turn(&attempt).map(str::to_string) else {return Ok(())};
        let mut gate=self.queue_gate_input(&s.meta.chat).await;
        // ライブで終了を観測済みのturnは通常の経路では履歴を読まないため、ここで読取り専用に一度だけ確認し、履歴での終端確認を揃える。
        if self.visual_gate_stopped(&gate,&turn).is_err() && gate.root.as_ref().is_some_and(|r|r.latest_end.is_some() && !r.terminal_explicit && r.latest_turn.as_ref().map(|t|t.0.as_str())==Some(turn.as_str())) {
            if self.read_terminal_fact(crate::host::state::agent_key_of(&s.meta.chat)).await { gate=self.queue_gate_input(&s.meta.chat).await; }
        }
        if let Err(e)=self.visual_gate_stopped(&gate,&turn) {
            static LAST_LOG:std::sync::atomic::AtomicI64=std::sync::atomic::AtomicI64::new(0);
            let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0);
            if now-LAST_LOG.load(std::sync::atomic::Ordering::Relaxed)>=10_000 {
                LAST_LOG.store(now,std::sync::atomic::Ordering::Relaxed);
                let root=gate.root.as_ref().map(|r|format!("state={:?} fresh={:?} turn={:?} end={} explicit={}",r.state,r.freshness,r.latest_turn.as_ref().map(|t|t.0.clone()),r.latest_end.is_some(),r.terminal_explicit));
                crate::diag::log("visual-gate",&format!("not stopped: {} accepted_turn={turn} connected={} scan_complete={} open_stop={} unresolved_attempt={} unresolved_op={} manual_pending={} delete={} quitting={} descendants={} root=[{}]",e.message,gate.connected,gate.descendant_scan_complete,gate.open_stop.is_some(),gate.unresolved_attempt.is_some(),gate.unresolved_op.is_some(),gate.manual_turn_pending,gate.delete_pending,gate.quitting,gate.descendants.len(),root.unwrap_or_default()));
            }
            return Ok(());
        }
        let root=ProjectRoot::open(Path::new(&s.meta.draft_root)).map_err(visual_err)?;
        ve::audit_draft_scope(&root,&s.scope().map_err(visual_err)?).map_err(visual_err)?;
        let after=Snapshot::capture(&root,&s.scope().map_err(visual_err)?).map_err(visual_err)?;
        let directory=format!("tx-{attempt}-after");
        session::save_snapshot(&s.folder.join(&directory),&after).map_err(visual_err)?;
        session::append(&s.folder,&SessionEvent::Captured{attempt_id:attempt,after:directory}).map_err(visual_err)?;
        *s=self.visual_loaded(&s.meta.session_id)?;
        Ok(())
    }
    pub async fn visual_status(self:&Arc<Self>,id:&str)->Result<SessionView,IpcError> {
        let _guard=self.visual_rt.lock.lock().await;
        let mut s=self.visual_loaded(id)?;
        let capture_issue=self.visual_refresh(&mut s).await.err();
        let mut view=s.view().map_err(visual_err)?;
        if let Some(e)=capture_issue {view.phase="recoveryRequired".into(); view.blockers.push(format!("after snapshot was not safely captured: {}",e.message)); return Ok(view);}
        if let Some(op)=view.last_operation.as_mut() {
            if op.outcome!="applied" {
                let journal=self.visual_root().join("_journals").join(&s.meta.session_id).join(&op.id);
                if journal.is_dir() {
                    let target=if op.kind=="adopt" || op.kind=="revert" {&s.meta.source_root} else {&s.meta.draft_root};
                    match ProjectRoot::open(Path::new(target)).and_then(|root|ve::apply::reconcile(&root,&journal)) {
                        Ok(files)=>{let details=files.iter().map(|f|format!("{}: {:?}",f.path,f.state)).collect::<Vec<_>>().join(", "); op.message=Some(format!("Read-only journal inspection: {details}"));},
                        Err(e)=>{op.message=Some(format!("Journal inspection unavailable: {e}")); view.blockers.push("journal recovery requires manual inspection".into());},
                    }
                } else {op.message=Some("No writer journal exists: this operation wrote nothing to the files".into());}
            }
        }
        if view.phase=="changes" && view.changes.iter().any(|f|f.change.is_some()) && !s.pending_operation() {
            if let Some((attempt,_))=s.last_capture() {
                if let Some(turn)=s.accepted_turn(attempt) {
                    let gate=self.queue_gate_input(&s.meta.chat).await;
                    if self.visual_gate_stopped(&gate,turn).is_ok() && !self.send_unsettled(&s.meta.chat) && !self.subagent_turn_pending(&s.meta.chat) {
                        let scope=s.scope().map_err(visual_err)?;
                        let draft=ProjectRoot::open(Path::new(&s.meta.draft_root)).map_err(visual_err)?;
                        if ve::audit_draft_scope(&draft,&scope).is_ok() {
                            let expected=session::load_snapshot(&s.folder.join(s.current_dir().unwrap()),&scope).map_err(visual_err)?;
                            if Snapshot::capture(&draft,&scope).map_err(visual_err)?==expected {
                                let source=ProjectRoot::open(Path::new(&s.meta.source_root)).map_err(visual_err)?;
                                let current=Snapshot::capture(&source,&scope).map_err(visual_err)?;
                                let baseline=s.baseline().map_err(visual_err)?;
                                let review=ve::review_adoption(&baseline,&expected,&current,ve::ReviewEvidence{stopped:true,draft_scope_audited:true}).map_err(visual_err)?;
                                view.can_adopt=!review.has_conflicts();
                                for (file,decision) in view.changes.iter_mut().zip(review.files) {file.conflict=decision.conflict;}
                                if !view.can_adopt {view.blockers.push("source changed or read-only dependency changed".into());}
                                if s.latest_adopted()!=s.last_capture().map(|(_,dir)|dir) {
                                    let before=s.events.iter().rev().find_map(|e|match e {SessionEvent::Prepared{attempt_id,before,..} if attempt_id==attempt=>Some(before.as_str()),_=>None});
                                    if let Some(before)=before {
                                        let b=session::load_snapshot(&s.folder.join(before),&scope).map_err(visual_err)?;
                                        let undo=ve::review_latest_undo(&b,&expected,&expected,true,false,ve::ReviewEvidence{stopped:true,draft_scope_audited:true}).map_err(visual_err)?;
                                        view.can_undo=!undo.has_conflicts();
                                    }
                                }
                            } else {view.blockers.push("draft changed after captured revision".into());}
                        } else {view.blockers.push("draft contains files outside reviewed scope".into());}
                    }
                }
            }
        }
        // 反映済みなら、反映前へ戻せるかを調べる（元のフォルダが反映後のままで、AIの停止・送信の確定が済んでいるとき）。
        if !s.pending_operation() && matches!(view.phase.as_str(),"editable"|"changes") {
            if let (Some(adopted_dir),Some((attempt,_))) = (s.latest_adopted(),s.last_capture()) {
                if let Some(turn)=s.accepted_turn(attempt) {
                    let gate=self.queue_gate_input(&s.meta.chat).await;
                    if self.visual_gate_stopped(&gate,turn).is_ok() && !self.send_unsettled(&s.meta.chat) && !self.subagent_turn_pending(&s.meta.chat) {
                        // 読めないときは「戻せない」とするだけで、状態確認そのものは失敗させない。
                        let same=(|| -> Result<bool,String> {
                            let scope=s.scope().map_err(|e|e.to_string())?;
                            let adopted=session::load_snapshot(&s.folder.join(adopted_dir),&scope).map_err(|e|e.to_string())?;
                            let source=ProjectRoot::open(Path::new(&s.meta.source_root)).map_err(|e|e.to_string())?;
                            Ok(Snapshot::capture(&source,&scope).map_err(|e|e.to_string())?==adopted)
                        })();
                        view.can_revert=matches!(same,Ok(true));
                        if !matches!(same,Ok(true)) {view.blockers.push("the original files changed after adoption; restoring is unavailable".into());}
                    }
                }
            }
        }
        Ok(view)
    }
    pub(super) async fn visual_before_send(self:&Arc<Self>,request:&mut SendRequest,attempt:&SendAttempt,auth:&SendAuth<'_>)->Result<Option<String>,IpcError> {
        let _guard=self.visual_rt.lock.lock().await;
        let binding=session::binding_id(&self.visual_root(),&request.chat).map_err(visual_err)?;
        let Some(id)=binding else {
            // 既存の会話で編集を終えた直後の最初の送信は、作業場所を元へ戻して行う（Codexのturn上書きは以後のturnにも及ぶため）。
            // この会話は作業場所がまだコピーを指していることがあるので、「紐付けが不完全」の検査より先に見る。
            if matches!(request.mode,SendMode::NewTurn) {
                if let Some((sid,info))=session::restore_for(&self.visual_root(),&request.chat).map_err(visual_err)? {
                    request.cwd=None;
                    request.work_folders=Some(crate::backend::model::WorkFolders{folders:info.folders});
                    return Ok(Some(sid));
                }
            }
            if self.visual_cwd(&request.chat) {return Err(visual_err("incomplete visual session binding; send blocked: この会話の作業場所が、編集用コピーのままです（編集が破棄されたか、記録が見つかりません）。送信しても元のフォルダへは自動で戻せません。作業フォルダを選び直すか、新しい会話で続けてください。"));}
            return Ok(None)
        };
        let mut s=self.visual_loaded(&id)?;
        if !matches!(auth,SendAuth::User) {return Err(visual_err("visual chat accepts user sends only"));}
        // 既存の会話で続ける場合: 編集中の新しいturnは、作業場所と書込み範囲を必ず編集用コピーだけにし、権限も書込み可の既定に固定する。
        if s.is_current_mode() && matches!(request.mode,SendMode::NewTurn) {
            request.cwd=None;
            request.work_folders=Some(crate::backend::model::WorkFolders{folders:vec![s.meta.draft_root.clone()]});
            request.permission=Some(PermissionPreset::WorkspaceWriteOnRequest);
        }
        if matches!(request.permission,Some(PermissionPreset::FullAccess)) {return Err(visual_err("full access is unavailable for an isolated visual chat"));}
        if let Some(cwd)=&request.cwd { if !crate::rules::work_folders::is_same_or_under(&s.meta.draft_root,cwd) || !crate::rules::work_folders::is_same_or_under(cwd,&s.meta.draft_root) { return Err(visual_err("visual chat cwd must remain the isolated copy")); } }
        if request.work_folders.as_ref().is_some_and(|f| f.folders.len()!=1 || !crate::rules::work_folders::is_same_or_under(&s.meta.draft_root,&f.folders[0]) || !crate::rules::work_folders::is_same_or_under(&f.folders[0],&s.meta.draft_root)) {return Err(visual_err("visual chat cannot receive extra work folders"));}
        if let SendMode::Steer{turn}= &request.mode {
            let Some((active,_,_))=s.latest() else {return Err(visual_err("no active visual transaction for steer"));};
            if s.accepted_turn(active)!=Some(turn.turn_id.0.as_str()) || s.after(active).is_some() {return Err(visual_err("steer turn does not match active visual transaction"));}
            return Ok(Some(s.meta.session_id));
        }
        self.visual_refresh(&mut s).await?;
        if let Some((previous,_,_))=s.latest() { if s.after(previous).is_none() && !s.latest_rejected(previous) { return Err(visual_err("previous visual transaction is unresolved")); } }
        if s.pending_operation() || matches!(s.view().map_err(visual_err)?.phase.as_str(),"partial"|"recoveryRequired") {return Err(visual_err("visual file operation is unresolved or partial"));}
        let scope=s.scope().map_err(visual_err)?;
        let root=ProjectRoot::open(Path::new(&s.meta.draft_root)).map_err(visual_err)?;
        ve::audit_draft_scope(&root,&scope).map_err(visual_err)?;
        let before=Snapshot::capture(&root,&scope).map_err(visual_err)?;
        let name=format!("tx-{}-before",attempt.attempt_id.0);
        session::save_snapshot(&s.folder.join(&name),&before).map_err(visual_err)?;
        session::append(&s.folder,&SessionEvent::Prepared{attempt_id:attempt.attempt_id.0.clone(),client_message_id:request.client_message_id.clone(),before:name}).map_err(visual_err)?;
        Ok(Some(s.meta.session_id))
    }
    pub(super) async fn visual_reconciled(&self,chat:&ChatKey,attempt:&SendAttempt) {
        let _guard=self.visual_rt.lock.lock().await;
        let Ok(binding)=session::binding_id(&self.visual_root(),chat) else {return};
        let Some(id)=binding else {
            // 終了後の最初の送信の受理を、あとの照合で確認できた場合。
            if matches!(attempt.state,SendState::Accepted{..}) {
                if let Ok(Some((sid,_)))=session::restore_for(&self.visual_root(),chat) {
                    if let Ok(s)=self.visual_loaded(&sid) {if s.restore_pending() {let _=session::append(&s.folder,&SessionEvent::Restored);}}
                    else {let _=session::mark_restored_if_pending(&self.visual_root(),&sid);}
                }
            }
            return;
        };
        let Ok(s)=self.visual_loaded(&id) else {return};
        let event=match &attempt.state {
            SendState::Accepted{turn}=>Some(SessionEvent::Accepted{attempt_id:attempt.attempt_id.0.clone(),turn_id:turn.turn_id.0.clone()}),
            SendState::NotFoundAfterReconcile=>Some(SessionEvent::Rejected{attempt_id:attempt.attempt_id.0.clone()}),
            _=>None,
        };
        if let Some(e)=event {
            let id=&attempt.attempt_id.0;
            if s.events.iter().any(|x|matches!(x,SessionEvent::Prepared{attempt_id,..} if attempt_id==id)) && s.accepted_turn(id).is_none() && !s.latest_rejected(id) {
                let _=session::append(&s.folder,&e);
            }
        }
    }
    /// 終了後の最初の送信が受理されたら、戻し済みの印を付ける。作業場所を元へ直したあとに呼ぶ（呼び出し側の順序が前提）。
    pub(super) async fn visual_mark_restored(&self,id:Option<&str>,attempt:&SendAttempt) {
        let _guard=self.visual_rt.lock.lock().await;
        let Some(id)=id else {return};
        if !matches!(attempt.state,SendState::Accepted{..}) {return;}
        match self.visual_loaded(id) {
            Ok(s)=>{if s.restore_pending() {let _=session::append(&s.folder,&SessionEvent::Restored);}},
            // 破棄済みなどで読み込めない編集でも、印だけは付ける（付けないと、以後の送信で作業フォルダが元へ上書きされ続ける）。
            Err(_)=>{let _=session::mark_restored_if_pending(&self.visual_root(),id);},
        }
    }
    pub(super) async fn visual_after_send(&self,id:Option<&str>,attempt:&SendAttempt) {
        let _guard=self.visual_rt.lock.lock().await;
        let Some(id)=id else {return};
        // 戻し済みの印は visual_mark_restored（作業場所を直したあと）で付けるので、ここでは何もしない。
        let Ok(s)=self.visual_loaded(id) else {return};
        if s.restore_pending() {return;}
        let event=match &attempt.state {
            SendState::Accepted{turn}=>Some(SessionEvent::Accepted{attempt_id:attempt.attempt_id.0.clone(),turn_id:turn.turn_id.0.clone()}),
            SendState::Rejected{..}=>Some(SessionEvent::Rejected{attempt_id:attempt.attempt_id.0.clone()}),
            SendState::AcceptanceUnknown{..}=>Some(SessionEvent::Unknown{attempt_id:attempt.attempt_id.0.clone()}),
            _=>None,
        };
        if let Some(e)=event {
            let id=&attempt.attempt_id.0;
            if s.events.iter().any(|x|matches!(x,SessionEvent::Prepared{attempt_id,..} if attempt_id==id)) {
                let settled=s.accepted_turn(id).is_some() || s.latest_rejected(id);
                if !settled {let _=session::append(&s.folder,&e);}
            }
        }
    }
}
impl Host {
    async fn visual_write(self:&Arc<Self>,args:VisualWriteArgs,adopt:bool,revert:bool)->Result<SessionView,IpcError> {
        // All send locks precede the visual lock, matching normal send dispatch.
        // Recheck the overlap set after taking the folder operation guard.
        let initial=self.visual_loaded(&args.session_id).map_err(nw_ipc)?;
        let folders=vec![initial.meta.source_root.clone(),initial.meta.draft_root.clone()];
        let _folder_op=self.begin_folder_op(folders.clone());
        let overlapping=self.chats_overlapping_folders(&folders);
        let mut send_guards=Vec::new();
        for chat in &overlapping {send_guards.push(self.queue_rt.send_lock(chat).lock_owned().await);}
        let _visual_guard=self.visual_rt.lock.lock().await;
        let mut s=self.visual_loaded(&args.session_id).map_err(nw_ipc)?;
        if s.meta.source_root!=initial.meta.source_root || s.meta.draft_root!=initial.meta.draft_root {return Err(nw_err("visual session root changed during operation setup"));}
        let overlap_now=self.chats_overlapping_folders(&folders);
        if overlap_now.iter().any(|chat| !overlapping.contains(chat)) {return Err(nw_err("a new overlapping chat appeared during operation setup"));}
        self.visual_refresh(&mut s).await.map_err(nw_ipc)?;
        if s.revision()!=args.revision {return Err(nw_err("reviewed revision expired: AIの編集が更新されました。変更を確認してから、もう一度押してください。"));}
        if s.pending_operation() || matches!(s.view().map_err(nw_err)?.phase.as_str(),"partial"|"recoveryRequired") {return Err(nw_err("previous file operation or snapshot is unresolved"));}
        if let Some((attempt,_,_))=s.latest() {
            if s.after(attempt).is_none() && !s.latest_rejected(attempt) {return Err(nw_err("latest send has no settled after snapshot"));}
        }
        let (attempt,after_dir)=s.last_capture().ok_or_else(||nw_err("there is no captured edit to review"))?;
        let attempt=attempt.to_string();
        let after_dir=after_dir.to_string();
        let turn=s.accepted_turn(&attempt).ok_or_else(||nw_err("captured edit has no accepted turn"))?.to_string();

        let gate=self.queue_gate_input(&s.meta.chat).await;
        self.visual_gate_stopped(&gate,&turn).map_err(nw_ipc)?;
        if self.send_unsettled(&s.meta.chat) || self.subagent_turn_pending(&s.meta.chat) {return Err(nw_err("visual chat send or turn is unsettled"));}
        // 重なる会話を、1件ずつ止まらず全部まとめて調べる（押すたびに別の会話で止まらないように）。止めるときは、会話名と重なっているフォルダを全件返す。
        let mut unsettled=Vec::new();
        let mut to_check=Vec::new();
        for chat in &overlapping {
            if chat==&s.meta.chat {continue;}
            if self.send_unsettled(chat) || self.subagent_turn_pending(chat) {unsettled.push(self.overlap_detail(chat,&folders));} else {to_check.push(chat.clone());}
        }
        if !unsettled.is_empty() {return Err(nw_err(format!("overlapping chat has unsettled send or turn: {}",unsettled.join(" / "))));}
        // 一度も再開していない会話は、履歴の終端（読取りのみ）で停止を確認する。確認の走査は呼ぶたびに始まるので、短く待って確認できたものを外しながら数回調べる。
        let mut unconfirmed=self.visual_unconfirmed_overlaps(&to_check).await;
        for _ in 0..8 {
            if unconfirmed.is_empty() {break;}
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            unconfirmed=self.visual_unconfirmed_overlaps(&unconfirmed).await;
        }
        if !unconfirmed.is_empty() {
            let detail:Vec<String>=unconfirmed.iter().map(|c|self.overlap_detail(c,&folders)).collect();
            return Err(nw_err(format!("overlapping chat stop or descendant scan is unconfirmed: {}",detail.join(" / "))));
        }
        let scope=s.scope().map_err(nw_err)?;
        let draft=ProjectRoot::open(Path::new(&s.meta.draft_root)).map_err(nw_err)?;
        ve::audit_draft_scope(&draft,&scope).map_err(nw_err)?;
        let draft_current=Snapshot::capture(&draft,&scope).map_err(nw_err)?;
        let current_dir=s.current_dir().ok_or_else(||nw_err("current snapshot unavailable"))?;
        let current_snapshot=session::load_snapshot(&s.folder.join(current_dir),&scope).map_err(nw_err)?;
        if draft_current!=current_snapshot {return Err(nw_err("copy changed after the reviewed revision"));}
        let (target,expected,desired,purpose)=if revert {
            // 反映した内容を、反映前の状態へ戻す。元のフォルダが反映後のままのときだけ（反映後に元を変えていたら、その変更を上書きしないため戻さない）。
            let adopted_dir=s.latest_adopted().ok_or_else(||nw_err("nothing was adopted"))?.to_string();
            let adopted=session::load_snapshot(&s.folder.join(&adopted_dir),&scope).map_err(nw_err)?;
            let before_adopt=s.baseline_before_latest_adoption().map_err(nw_err)?;
            let source=ProjectRoot::open(Path::new(&s.meta.source_root)).map_err(nw_err)?;
            let source_current=Snapshot::capture(&source,&scope).map_err(nw_err)?;
            if source_current!=adopted {return Err(nw_err("the original files changed after adoption; restoring would overwrite those changes"));}
            (source,adopted,before_adopt,ve::apply::WritePurpose::RevertAdoption)
        } else if adopt {
            let baseline=s.baseline().map_err(nw_err)?;
            let source=ProjectRoot::open(Path::new(&s.meta.source_root)).map_err(nw_err)?;
            let source_current=Snapshot::capture(&source,&scope).map_err(nw_err)?;
            let review=ve::review_adoption(&baseline,&current_snapshot,&source_current,ve::ReviewEvidence{stopped:true,draft_scope_audited:true}).map_err(nw_err)?;
            if review.has_conflicts(){return Err(nw_err(format!("adoption conflict: {review:?}")));}
            (source,baseline,current_snapshot,ve::apply::WritePurpose::AdoptSource)
        } else {
            if s.latest_adopted()==Some(after_dir.as_str()) {return Err(nw_err("latest edit was already adopted"));}
            let before_dir=s.latest().filter(|(a,_,_)| *a==attempt).map(|(_,_,b)|b.to_string()).or_else(||s.events.iter().rev().find_map(|e|match e {SessionEvent::Prepared{attempt_id,before,..} if attempt_id==&attempt=>Some(before.clone()),_=>None})).ok_or_else(||nw_err("before snapshot missing"))?;
            let before=session::load_snapshot(&s.folder.join(&before_dir),&scope).map_err(nw_err)?;
            let review=ve::review_latest_undo(&before,&current_snapshot,&draft_current,true,false,ve::ReviewEvidence{stopped:true,draft_scope_audited:true}).map_err(nw_err)?;
            if review.has_conflicts(){return Err(nw_err(format!("undo conflict: {review:?}")));}
            (draft,current_snapshot,before,ve::apply::WritePurpose::UndoLatestDraft)
        };
        let journal_parent=self.visual_root().join("_journals").join(&s.meta.session_id);
        fs::create_dir_all(&journal_parent).map_err(nw_err)?;
        ProjectRoot::open(&journal_parent).map_err(nw_err)?;
        let purpose_name=if revert {"revert"} else if adopt {"adopt"} else {"undo"};
        let op_id=self.local_id(purpose_name).0;
        session::append(&s.folder,&SessionEvent::OperationIntent{operation_id:op_id.clone(),purpose:purpose_name.into(),revision:args.revision,desired:if revert {s.dir_before_latest_adoption()} else if adopt {current_dir.to_string()} else {s.events.iter().rev().find_map(|e|match e {SessionEvent::Prepared{attempt_id,before,..} if attempt_id==&attempt=>Some(before.clone()),_=>None}).ok_or_else(||nw_err("before snapshot missing"))?}}).map_err(nw_err)?;
        let mut permit=ve::apply::HostWritePermit::after_host_checks();
        let outcome=match ve::apply::apply_checked_classified(&mut permit,&target,&scope,&expected,&desired,&journal_parent,&op_id,purpose) {
            ve::apply::CheckedAttempt::PrewriteRejected(e)=>{
                // The writer only returns Err before creating its journal or mutating a target.
                // Once a journal exists, it returns Partial and keeps the intent blocked.
                session::append(&s.folder,&SessionEvent::OperationResult{operation_id:op_id,outcome:"rejected".into()}).map_err(visual_err)?;
                return Err(nw_err(e));
            }
            // 比較で食い違いが見つかった場合: writer は journal を作る前に返すので、何も書いていない。拒否と同じく確定させ、会話を「一部だけ書いた」状態にしない。
            ve::apply::CheckedAttempt::Journaled(ve::apply::WriteOutcome::Conflict{paths})=>{
                session::append(&s.folder,&SessionEvent::OperationResult{operation_id:op_id,outcome:"rejected".into()}).map_err(visual_err)?;
                return Err(nw_err(format!("files changed since the review ({}); reload and review again",paths.join(", "))));
            }
            ve::apply::CheckedAttempt::Journaled(outcome)=>outcome,
        };
        let label=match &outcome {ve::apply::WriteOutcome::Applied{..}=>"applied",ve::apply::WriteOutcome::Partial{..}=>"partial",ve::apply::WriteOutcome::Conflict{..}=>"conflict"};
        session::append(&s.folder,&SessionEvent::OperationResult{operation_id:op_id,outcome:label.into()}).map_err(visual_err)?;
        if label=="applied" {
            if revert {}
            else if adopt {session::append(&s.folder,&SessionEvent::Adopted{after:current_dir.into()}).map_err(visual_err)?;}
            else {
                let before_dir=s.events.iter().rev().find_map(|e|match e {SessionEvent::Prepared{attempt_id,before,..} if attempt_id==&attempt=>Some(before.clone()),_=>None}).ok_or_else(||visual_err("before snapshot missing"))?;
                session::append(&s.folder,&SessionEvent::UndoApplied{attempt_id:attempt,before:before_dir}).map_err(visual_err)?;
            }
        }
        s=self.visual_loaded(&s.meta.session_id)?;
        let mut view=s.view().map_err(visual_err)?;
        if label!="applied" {view.blockers.push(format!("file operation {label}; inspect the durable journal and current files"));}
        Ok(view)
    }
    pub async fn visual_adopt(self:&Arc<Self>,args:VisualWriteArgs)->Result<SessionView,IpcError> {self.visual_write(args,true,false).await}
    pub async fn visual_undo(self:&Arc<Self>,args:VisualWriteArgs)->Result<SessionView,IpcError> {self.visual_write(args,false,false).await}
    pub async fn visual_revert(self:&Arc<Self>,args:VisualWriteArgs)->Result<SessionView,IpcError> {self.visual_write(args,false,true).await}
}
impl Host {
    pub async fn visual_diff(self:&Arc<Self>,args:VisualDiffArgs)->Result<VisualDiffView,IpcError> {
        const LIMIT:usize=256*1024;
        fn display(bytes:Option<&[u8]>)->(Option<String>,bool,bool) {
            let Some(bytes)=bytes else {return (None,false,false)};
            if bytes.contains(&0) {return (None,false,true)}
            let Ok(text)=std::str::from_utf8(bytes) else {return (None,false,true)};
            let mut end=text.len().min(LIMIT);
            while !text.is_char_boundary(end) {end-=1;}
            (Some(text[..end].to_string()),end<text.len(),false)
        }
        let _guard=self.visual_rt.lock.lock().await;
        let mut s=self.visual_loaded(&args.session_id)?;
        // refresh が Captured を追記して revision を進めても、利用者が見ていた版(更新前)と一致していれば、読み取り専用の差分は現在の版で返す（書き込みは別途 revision を要求する）。
        let seen=s.revision();
        self.visual_refresh(&mut s).await?;
        if args.revision!=seen && args.revision!=s.revision() {return Err(visual_err("reviewed revision expired: AIの編集が更新されました。変更を確認してから、もう一度押してください。"));}
        if !s.meta.scope.iter().any(|f|f.path==args.path) {return Err(visual_err("file is outside reviewed visual scope"));}
        let baseline=s.baseline().map_err(visual_err)?;
        let current=match s.current_dir(){Some(dir)=>session::load_snapshot(&s.folder.join(dir),&s.scope().map_err(visual_err)?).map_err(visual_err)?,None=>s.initial().map_err(visual_err)?};
        let (before,bt,bb)=display(baseline.bytes(&args.path));
        let (after,at,ab)=display(current.bytes(&args.path));
        Ok(VisualDiffView{path:args.path,before,after,truncated:bt||at,binary:bb||ab})
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualDiffArgs { pub session_id:String, pub path:String, pub revision:u64 }
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualDiffView { pub path:String, pub before:Option<String>, pub after:Option<String>, pub truncated:bool, pub binary:bool }
#[cfg(test)]
mod visual_gate_tests {
    use super::*;
    use crate::backend::model::{AgentKey,AgentState,ExternalId,Freshness,TurnEnd,UnixMillis};
    use crate::rules::queue::{AgentFact,GateInput};
    fn root()->AgentFact {AgentFact{agent:AgentKey{backend:BackendKind::Codex,id:ExternalId("root".into())},state:AgentState::Idle,freshness:Freshness::Live,latest_turn:None,latest_end:None,terminal_explicit:false,observed_at:UnixMillis(1)}}
    fn gate()->GateInput {GateInput{connected:true,root:Some(root()),descendants:Vec::new(),descendant_scan_complete:true,open_stop:None,unresolved_attempt:None,unresolved_op:None,delete_pending:false,quitting:false,manual_turn_pending:false,user_confirmed_unknown:false}}
    #[test]
    fn overlapping_chat_requires_complete_scan_and_explicit_terminal_evidence() {
        let mut g=gate(); assert!(Host::visual_overlap_stopped(&g));
        g.descendant_scan_complete=false; assert!(!Host::visual_overlap_stopped(&g));
        g.descendant_scan_complete=true;
        g.root.as_mut().unwrap().latest_turn=Some(ExternalId("turn".into()));
        assert!(!Host::visual_overlap_stopped(&g));
        let r=g.root.as_mut().unwrap(); r.latest_end=Some((TurnEnd::Completed,UnixMillis(2))); r.terminal_explicit=true;
        assert!(Host::visual_overlap_stopped(&g));
        let mut child=root(); child.agent.id=ExternalId("child".into()); child.latest_turn=Some(ExternalId("child-turn".into()));
        g.descendants.push(child); assert!(!Host::visual_overlap_stopped(&g));
        g.descendants[0].latest_end=Some((TurnEnd::Interrupted,UnixMillis(3))); g.descendants[0].terminal_explicit=true;
        assert!(Host::visual_overlap_stopped(&g));
        g.open_stop=Some(crate::backend::model::LocalId("stop".into())); assert!(!Host::visual_overlap_stopped(&g));
    }
}#[cfg(test)]
mod discover_tests {
    use super::*;
    #[test]
    fn discovers_files_in_subdirectories_of_a_canonical_root() {
        let base=std::env::temp_dir().join(format!("agentdock-discover-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("assets")).unwrap();
        fs::write(base.join("index.html"),"<p>x</p>").unwrap();
        fs::write(base.join("assets").join("mark.svg"),"<svg/>").unwrap();
        let root=ProjectRoot::open(&base).unwrap();
        let html=RelativePath::parse("index.html").unwrap();
        let scope=discover(&root,&html).expect("subdirectory must not be rejected");
        let paths:Vec<_>=scope.selected().iter().map(|f|f.path.clone()).collect();
        let _=fs::remove_dir_all(&base);
        assert!(paths.contains(&"index.html".to_string()) && paths.contains(&"assets/mark.svg".to_string()), "{paths:?}");
        assert!(scope.selected().iter().all(|f| f.access==FileAccess::Edit), "svg and html must be editable");
    }
}
#[cfg(test)]
mod pin_save_tests {
    use super::*;
    #[test]
    fn pinned_root_blocks_directory_swap_but_not_saves_in_ancestors() {
        let base=std::env::temp_dir().join(format!("agentdock-pin-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        let child=base.join("a").join("b"); fs::create_dir_all(&child).unwrap();
        let state=base.join("a").join("state.json");
        crate::store::atomic::write_atomic(&state,b"1").unwrap();
        let pins=ProjectRoot::open(&child).unwrap();
        let ancestor=crate::store::atomic::write_atomic(&state,b"2");
        let inside=crate::store::atomic::write_atomic(&child.join("x.json"),b"2");
        let swap=fs::rename(&child,base.join("a").join("moved"));
        drop(pins);
        let _=fs::remove_dir_all(&base);
        assert!(ancestor.is_ok(), "save in pinned ancestor failed: {ancestor:?}");
        assert!(inside.is_ok(), "save inside pinned dir failed: {inside:?}");
        assert!(swap.is_err(), "pinned directory must not be renamable");
    }
}
#[cfg(test)]
mod discard_tests {
    use super::*;
    #[test]
    fn remove_storage_dir_only_deletes_plain_directories_inside_the_storage() {
        let base=std::env::temp_dir().join(format!("agentdock-discard-{}",std::process::id()));
        let _=fs::remove_dir_all(&base);
        let storage=base.join("store"); let draft=storage.join("s1").join("draft"); let outside=base.join("outside");
        fs::create_dir_all(draft.join("assets")).unwrap(); fs::write(draft.join("assets").join("a.txt"),"x").unwrap();
        fs::create_dir_all(&outside).unwrap(); fs::write(outside.join("keep.txt"),"x").unwrap();
        assert!(remove_storage_dir(&storage,&draft).is_ok() && !draft.exists());
        assert!(remove_storage_dir(&storage,&draft).is_ok(), "an already removed directory is not an error");
        assert!(remove_storage_dir(&storage,&outside).is_err() && outside.join("keep.txt").exists());
        assert!(remove_storage_dir(&storage,&storage).is_err() && storage.exists());
        let _=fs::remove_dir_all(&base);
    }
}
