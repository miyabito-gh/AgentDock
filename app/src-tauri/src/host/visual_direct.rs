//! 直接編集方式の Host 側。元のファイルを直接編集させ、ターンごとに戻す。保存と一致検査は `visual_edit::direct`。
use super::{err, visual::{discover, discover_markdown}, Host};
use crate::backend::{ipc::{IpcError, IpcErrorCode}, model::ChatKey};
use crate::visual_edit::{self as ve, direct::{self, DirectError, DirectStore, TurnView}, FileAccess, FileScope, ProjectRoot, RelativePath};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant}};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectTargetArgs { pub root_path:String, pub html_path:String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectBeginArgs { pub root_path:String, pub html_path:String, #[serde(default)] pub chat:Option<ChatKey> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectEndArgs { pub root_path:String, pub html_path:String, pub turn_id:String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectUndoArgs { pub root_path:String, pub html_path:String, pub turn_id:String, #[serde(default)] pub force:bool, #[serde(default)] pub chat:Option<ChatKey> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectHintArgs { pub root_path:String, pub html_path:String, #[serde(default)] pub selection:Vec<ve::source_hint::HintSelection> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectDiffArgs { pub root_path:String, pub html_path:String, pub turn_id:String, pub path:String, #[serde(default)] pub side:Option<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectScopeFile { pub path:String, pub access:FileAccess }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectOpened { pub root_path:String, pub html_path:String, pub scope:Vec<DirectScopeFile>, pub turns:Vec<TurnView> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectDiff { pub path:String, pub before:Option<String>, pub after:Option<String>, pub truncated:bool, pub binary:bool }

fn direct_err(e:DirectError)->IpcError { err(IpcErrorCode::Blocked,e.to_string()) }
fn ve_err(e:ve::VisualError)->IpcError { direct_err(e.into()) }
fn internal_err(e:impl std::fmt::Display)->IpcError { err(IpcErrorCode::Blocked,format!("内部エラーで処理を完了できませんでした（{e}）。もう一度お試しください。")) }
fn now_ms()->u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as u64).unwrap_or(0) }
/// スコープ探索の結果を短時間だけ覚える（stamp が数秒ごとに呼ばれても毎回フォルダを走査しない）。open/begin/end/undo/redo で無効にする。
const SCOPE_CACHE_TTL:Duration=Duration::from_secs(5);
type ScopeCache=Mutex<HashMap<(String,String),(Instant,FileScope)>>;
fn scope_cache()->&'static ScopeCache { static CACHE:OnceLock<ScopeCache>=OnceLock::new(); CACHE.get_or_init(Default::default) }
fn invalidate_scope(root_path:&str,html_path:&str) { if let Ok(mut m)=scope_cache().lock() {m.remove(&(root_path.to_string(),html_path.to_string()));} }
/// 検証済みの編集対象（ルートを開き、HTML の相対パスと保存先を得る）。
struct Target { root:ProjectRoot, html:RelativePath, store:DirectStore }
impl Target {
    fn open(host:&Host,root_path:&str,html_path:&str)->Result<Self,IpcError> {
        let root=ProjectRoot::open(Path::new(root_path)).map_err(|e|err(IpcErrorCode::Blocked,format!("編集するフォルダを開けません。フォルダの場所を確認してください。（{}）",direct::jp_visual(&e))))?;
        let html=ve::preview::relative_entry(root.path(),html_path).map_err(|e|err(IpcErrorCode::Blocked,format!("選んだ HTML ファイルのパスが正しくありません。フォルダの中の HTML を選び直してください。（{e}）")))?;
        let dir:PathBuf=host.app_data_dir.join("visual-editor").join("direct").join(direct::store_key(root.path(),&html));
        Ok(Self{root,html,store:DirectStore::new(dir)})
    }
    /// Markdown は文書＋参照画像、それ以外（HTML）は従来の探索。
    fn scope(&self)->Result<FileScope,IpcError> {
        let md=self.html.as_str().rsplit('.').next().is_some_and(|e|e.eq_ignore_ascii_case("md")||e.eq_ignore_ascii_case("markdown"));
        (if md {discover_markdown(&self.root,&self.html)} else {discover(&self.root,&self.html)}).map_err(ve_err)
    }
}
fn display(bytes:Option<Vec<u8>>)->(Option<String>,bool,bool) {
    const LIMIT:usize=256*1024;
    let Some(bytes)=bytes else {return (None,false,false)};
    if bytes.contains(&0) {return (None,false,true)}
    let Ok(text)=std::str::from_utf8(&bytes) else {return (None,false,true)};
    let mut end=text.len().min(LIMIT);
    while !text.is_char_boundary(end) {end-=1;}
    (Some(text[..end].to_string()),end<text.len(),false)
}
impl Host {
    /// ファイル操作は同期処理なので、tokio のワーカーを塞がないよう専用スレッドで行う。ターンの記録は一つのロックで直列化する。
    async fn direct_run<T:Send+'static>(self:&Arc<Self>,args:(String,String),work:impl FnOnce(Target)->Result<T,IpcError>+Send+'static)->Result<T,IpcError> {
        let _guard=self.visual_rt.lock.lock().await;
        invalidate_scope(&args.0,&args.1);
        let host=self.clone();
        tokio::task::spawn_blocking(move||work(Target::open(&host,&args.0,&args.1)?)).await.map_err(internal_err)?
    }
    /// 会話が実行中か(送信・ターンが未解決のものも含む)。停止を確認できない(未接続・未観測・再起動直後)だけなら実行中と断定せず false。削除済みの会話も false。
    async fn direct_chat_running(self:&Arc<Self>,chat:&ChatKey)->bool {
        use crate::backend::model::AgentState;
        if self.read(|d|d.chat(chat).is_none()) {return false;}
        if self.send_unsettled(chat) || self.subagent_turn_pending(chat) {return true;}
        let gate=self.queue_gate_input(chat).await;
        let active=|f:&crate::rules::queue::AgentFact|matches!(f.state,AgentState::Running|AgentState::Waiting|AgentState::Initializing);
        gate.root.as_ref().is_some_and(active) || gate.descendants.iter().any(active) || gate.manual_turn_pending || gate.unresolved_attempt.is_some()
    }
    /// 記録されたターンの会話(open のもの、指定ターンのもの、渡された会話)のうち、実行中のもの。
    async fn direct_running(self:&Arc<Self>,args:&(String,String),turn:Option<String>,extra:Option<ChatKey>)->Result<HashSet<ChatKey>,IpcError> {
        let host=self.clone();
        let a=args.clone();
        let mut chats=tokio::task::spawn_blocking(move||->Result<Vec<ChatKey>,IpcError> {
            Target::open(&host,&a.0,&a.1)?.store.chats_to_check(turn.as_deref()).map_err(direct_err)
        }).await.map_err(internal_err)??;
        chats.extend(extra);
        let mut running=HashSet::new();
        for chat in chats {if !running.contains(&chat) && self.direct_chat_running(&chat).await {running.insert(chat);}}
        Ok(running)
    }
    /// 検証と復旧（`open` のまま残ったターンのうち、会話が実行中でないものを確定）。
    pub async fn visual_direct_open(self:&Arc<Self>,args:DirectTargetArgs)->Result<DirectOpened,IpcError> {
        let key=(args.root_path,args.html_path);
        let running=self.direct_running(&key,None,None).await?;
        self.direct_run(key,move|t|{
            let scope=t.scope()?;
            t.store.recover_open(&t.root,Some(&scope),&|c|running.contains(c)).map_err(direct_err)?;
            Ok(DirectOpened{
                root_path:crate::visual_edit::preview::normal_drive_path(t.root.path()).map_err(|e|err(IpcErrorCode::Blocked,format!("編集するフォルダのパスを扱えません（{e}）。")))?.to_string_lossy().into_owned(),
                html_path:t.html.as_str().into(),
                scope:scope.files().map(|(p,a)|DirectScopeFile{path:p.as_str().into(),access:a}).collect(),
                turns:t.store.turns().map_err(direct_err)?,
            })
        }).await
    }
    /// 依頼の直前のスナップショット。失敗したらエラー（呼び出し側は送らない）。`chat` はターンに記録する。
    pub async fn visual_direct_turn_begin(self:&Arc<Self>,args:DirectBeginArgs)->Result<TurnView,IpcError> {
        let key=(args.root_path,args.html_path);
        let running=self.direct_running(&key,None,None).await?;
        let chat=args.chat;
        self.direct_run(key,move|t|{
            let scope=t.scope()?;
            t.store.begin(&t.root,&scope,now_ms(),chat.as_ref(),&|c|running.contains(c)).map_err(direct_err)
        }).await
    }
    pub async fn visual_direct_turn_end(self:&Arc<Self>,args:DirectEndArgs)->Result<Option<TurnView>,IpcError> {
        self.direct_run((args.root_path,args.html_path),move|t|{
            // 終了時にスコープを探索できなくても、開始時の対象で確定する。
            let scope=t.scope().ok();
            t.store.end(&t.root,&args.turn_id,scope.as_ref()).map_err(direct_err)
        }).await
    }
    pub async fn visual_direct_turns(self:&Arc<Self>,args:DirectTargetArgs)->Result<Vec<TurnView>,IpcError> {
        self.direct_run((args.root_path,args.html_path),|t|t.store.turns().map_err(direct_err)).await
    }
    /// 戻す・やり直す。対象ターンに記録された会話(旧ターンは渡された会話)が実行中なら `running:` で拒否する。停止を確認できないだけなら許可する。
    pub async fn visual_direct_undo(self:&Arc<Self>,args:DirectUndoArgs)->Result<Vec<TurnView>,IpcError> {
        let key=(args.root_path,args.html_path);
        let running=self.direct_running(&key,Some(args.turn_id.clone()),args.chat.clone()).await?;
        self.direct_run(key,move|t|{
            let scope=t.scope().ok();
            t.store.undo(&t.root,&args.turn_id,args.force,scope.as_ref(),args.chat.as_ref(),&|c|running.contains(c)).map_err(direct_err)
        }).await
    }
    pub async fn visual_direct_redo(self:&Arc<Self>,args:DirectUndoArgs)->Result<Vec<TurnView>,IpcError> {
        let key=(args.root_path,args.html_path);
        let running=self.direct_running(&key,Some(args.turn_id.clone()),args.chat.clone()).await?;
        self.direct_run(key,move|t|t.store.redo(&t.root,&args.turn_id,args.force,args.chat.as_ref(),&|c|running.contains(c)).map_err(direct_err)).await
    }
    /// スコープの編集対象ファイルの更新印（読み取りのみ）。スコープは5秒だけ覚える。探索に失敗したら空文字ではなくエラーを返す。
    pub async fn visual_direct_stamp(self:&Arc<Self>,args:DirectTargetArgs)->Result<String,IpcError> {
        let host=self.clone();
        tokio::task::spawn_blocking(move||{
            let t=Target::open(&host,&args.root_path,&args.html_path)?;
            let key=(args.root_path,args.html_path);
            let cached=scope_cache().lock().ok().and_then(|m|m.get(&key).filter(|(at,_)|at.elapsed()<SCOPE_CACHE_TTL).map(|(_,s)|s.clone()));
            let scope=match cached {
                Some(s)=>s,
                None=>{
                    let s=t.scope()?;
                    if let Ok(mut m)=scope_cache().lock() {m.insert(key,(Instant::now(),s.clone()));}
                    s
                }
            };
            Ok(direct::stamp(&t.root,&scope))
        }).await.map_err(internal_err)?
    }
    /// 選択した要素に対応するソースの抜粋（目安）。元のルートの編集対象 HTML だけを読む。該当なし・読めない場合は空文字。
    pub async fn visual_direct_source_hint(self:&Arc<Self>,args:DirectHintArgs)->Result<String,IpcError> {
        if args.selection.len()>12 {return Err(err(IpcErrorCode::Blocked,"選択した要素が多すぎます（最大12件）。"));}
        let host=self.clone();
        tokio::task::spawn_blocking(move||{
            let t=Target::open(&host,&args.root_path,&args.html_path)?;
            let scope=t.scope()?;
            let Some(source)=ve::source_hint::load_edit_source(&t.root,&scope,t.html.as_str()).map_err(ve_err)? else {return Ok(String::new())};
            Ok(ve::source_hint::build_hint(&source,&args.selection))
        }).await.map_err(internal_err)?
    }
    pub async fn visual_direct_diff(self:&Arc<Self>,args:DirectDiffArgs)->Result<DirectDiff,IpcError> {
        self.direct_run((args.root_path,args.html_path),move|t|{
            let sides=t.store.diff(&t.root,&args.turn_id,&args.path,args.side.as_deref()).map_err(direct_err)?;
            let (before,bt,bb)=display(sides.before);
            let (after,at,ab)=display(sides.after);
            Ok(DirectDiff{path:args.path,before,after,truncated:bt||at,binary:bb||ab})
        }).await
    }
}
