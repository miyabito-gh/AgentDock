//! Parent chat orchestration uses the ordinary host. Native child creation stays with Codex.
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
use serde::{Deserialize, Serialize};
use super::{Host, err, now_ms, state::agent_key_of};
use crate::backend::{backend::UserConfirmed, ipc::*, model::*, subagents::*};
use crate::backend::org_artifacts::OrgImportOutcome;
use crate::gitops::{import as gi, artifacts as ga};
use crate::rules::queue::{GateInput, AgentFact};
use crate::store::atomic::{append_line, write_atomic};

struct Token { run: String, source: String, prepared: gi::Prepared, at: i64 }
#[derive(Default)]
pub struct SubagentRuntime {
    operation: tokio::sync::Mutex<()>,
    io: Mutex<()>,
    tokens: Mutex<HashMap<String, Token>>,
    observing: AtomicBool,
    scans: Mutex<HashMap<AgentKey, (bool, Vec<AgentKey>)>>,
}
const TTL: i64 = 10 * 60 * 1000;

fn failure(e: impl std::fmt::Display) -> IpcError { err(IpcErrorCode::Io, format!("新方式の記録を確認・保存できません: {e}")) }
fn same_path(a: &str, b: &str) -> bool { match (std::fs::canonicalize(a), std::fs::canonicalize(b)) { (Ok(a), Ok(b)) => a == b, _ => false } }
fn valid_id(id: &str) -> bool { id.starts_with("run-") && id.len() < 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') }
fn run_path(root: &Path, id: &str) -> Result<PathBuf, IpcError> {
    if !valid_id(id) { return Err(err(IpcErrorCode::InvalidArgs, "依頼IDが正しくありません")); }
    no_links(root)?;
    let dir = root.join(id);
    no_links(&dir)?;
    no_links(&dir.join("run.json"))?;
    Ok(dir.join("run.json"))
}
fn no_links(path: &Path) -> Result<(), IpcError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return Err(failure("リンク・ジャンクションの記録には書き込みません")); } }
        if meta.file_type().is_symlink() { return Err(failure("リンクの記録には書き込みません")); }
    }
    Ok(())
}
fn read_run(root: &Path, id: &str) -> Result<SubagentRun, IpcError> {
    let r: SubagentRun = serde_json::from_slice(&std::fs::read(run_path(root, id)?).map_err(failure)?).map_err(failure)?;
    if r.schema_version != 1 || r.id != id { return Err(failure("未対応の記録形式、またはIDの不一致です。上書きしません")); }
    Ok(r)
}
fn save_run(root: &Path, run: &SubagentRun) -> Result<(), IpcError> {
    let path = run_path(root, &run.id)?;
    if path.exists() { read_run(root, &run.id)?; }
    let bytes = serde_json::to_vec_pretty(run).map_err(failure)?;
    if crate::store::atomic::free_space(root).is_ok_and(|n| n < bytes.len() as u64 + crate::store::FREE_SPACE_MARGIN_BYTES) { return Err(failure("保存先の空き容量が不足しています")); }
    write_atomic(&path, &bytes).map_err(failure)
}
fn read_runs(root: &Path) -> Result<Vec<SubagentRun>, IpcError> {
    let entries = match std::fs::read_dir(root) { Ok(e) => e, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]), Err(e) => return Err(failure(e)) };
    let mut out = Vec::new();
    for e in entries { let e = e.map_err(failure)?; let id = e.file_name().to_string_lossy().into_owned(); if valid_id(&id) { out.push(read_run(root, &id)?); } }
    out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Ok(out)
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Ledger { Begin { record: SubagentImportRecord }, End { id: String, outcome: OrgImportOutcome } }
fn ledger_path(root: &Path, id: &str) -> Result<PathBuf, IpcError> { let p = run_path(root, id)?.with_file_name("imports.jsonl"); no_links(&p)?; Ok(p) }
fn append(root: &Path, id: &str, row: &Ledger) -> Result<(), IpcError> {
    append_line(&ledger_path(root, id)?, &serde_json::to_string(row).map_err(failure)?).map_err(failure)
}
fn imports(root: &Path, id: &str) -> Result<Vec<SubagentImportRecord>, IpcError> {
    let text = match std::fs::read_to_string(ledger_path(root, id)?) { Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]), Err(e) => return Err(failure(e)) };
    let mut records: Vec<SubagentImportRecord> = vec![];
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        match serde_json::from_str(line).map_err(failure)? {
            Ledger::Begin { record } => {
                if record.outcome.is_some() || records.iter().any(|r| r.id == record.id) { return Err(failure("取り込み台帳の開始記録が重複・不正です")); }
                records.push(record);
            },
            Ledger::End { id, outcome } => {
                let r = records.iter_mut().find(|r| r.id == id).ok_or_else(|| failure("取り込み台帳に対応する開始記録がありません"))?;
                if r.outcome.as_ref().is_some_and(|o| !matches!(o, OrgImportOutcome::Unknown { .. })) { return Err(failure("確定済み取り込み台帳の終了が重複しています")); }
                r.outcome = Some(outcome);
            }
        }
    }
    Ok(records)
}
fn terminal(f: &AgentFact) -> bool {
    f.latest_turn.is_some() && matches!(f.latest_end, Some((TurnEnd::Completed, _))) &&
        (f.freshness == Freshness::Live || (f.freshness == Freshness::HistoryOnly && f.terminal_explicit)) &&
        !matches!(f.state, AgentState::Running | AgentState::Waiting | AgentState::Initializing | AgentState::Failed | AgentState::Interrupted) && (f.state != AgentState::Unknown || f.terminal_explicit)
}
/// Stricter than queue dispatch: a completed parent never relaxes an unknown child.
fn gate_blockers(run: &SubagentRun, gate: &GateInput, pending_queue: bool) -> Vec<String> {
    let mut b = Vec::new();
    if run.phase != "ready" { b.push(format!("開始工程が未確認です（{}）。自動で再開・再送しません", run.phase)); }
    if !gate.connected { b.push("バックエンド未接続です".into()); }
    if !gate.descendant_scan_complete { b.push("子孫の走査が完了していません".into()); }
    if gate.root.as_ref().is_none_or(|f| !terminal(f)) { b.push("親の最新turnの完了・鮮度を確認できません".into()); }
    if gate.descendants.iter().any(|f| !terminal(f)) { b.push("作業中・失敗・中断・状態不明、または終端未確認の子孫があります".into()); }
    if run.observed_children.iter().any(|k| !gate.descendants.iter().any(|f| &f.agent == k)) { b.push("以前観測した子孫の状態がまだ復元されていません".into()); }
    if gate.open_stop.is_some() || gate.unresolved_attempt.is_some() || gate.unresolved_op.is_some() || gate.manual_turn_pending { b.push("停止・送信の受理・操作の結果が未確認です".into()); }
    if gate.delete_pending || gate.quitting || pending_queue { b.push("削除・終了処理、または未送信のキューがあります".into()); }
    b
}

pub fn parent_prompt(request: &str, worktree: &str, base: &str) -> String {
    format!("AgentDockの親Codex + nativeサブエージェント依頼です。あなたが親として計画・委任・差し戻し・最終報告を担当してください。Codexが提供するnativeサブエージェント機能を使い、適切な独立作業を実際に委任してください。AgentDockの旧組織ロール実行ではありません。native機能が利用できない場合はその制約を明示し、使ったと装わないでください。\n作業場所: {worktree}\n開始コミット: {base}\n調査は並列可、編集担当は同時に1体、検証は編集停止後に行ってください。これらは親への指示でありアプリによる排他制御・子別sandbox保証ではありません。子へ作業場所と編集範囲を明示し、子作成で専用worktreeが自動作成されると仮定しないでください。適用されるAGENTS.mdと利用者のモデル制約を子にも伝えてください。\n通常の追加指示はこの親会話に届きます。承認・質問は通常のホスト経路を使ってください。元の作業フォルダへ取り込み・push・破壊的な操作を勝手に行わないでください。成果を確認可能にするため、適切ならこの専用worktreeで変更をコミットしてください。コミットできない理由や未コミットの残りは報告してください。最終報告には変更・検証結果・制約・残作業を含め、子孫が停止済みか確認してください。親の申告だけでアプリの停止・検証成功・取り込み成功が確定することはありません。\n\n利用者の依頼:\n{request}")
}

impl Host {
    fn subagent_root(&self) -> Result<PathBuf, IpcError> {
        self.persist.store().map(|s| s.root().join("subagent-runs")).ok_or_else(|| failure("保存先が利用できません"))
    }
    async fn subagent_io<R: Send + 'static>(self: &Arc<Self>, f: impl FnOnce(&Host, &Path) -> Result<R, IpcError> + Send + 'static) -> Result<R, IpcError> {
        let host = self.clone();
        tokio::task::spawn_blocking(move || { let _g = host.subagent_rt.io.lock().unwrap(); let root = host.subagent_root()?; f(&host, &root) }).await.map_err(failure)?
    }
    async fn subagent_save(self: &Arc<Self>, run: &SubagentRun) -> Result<(), IpcError> {
        let mut run = run.clone();
        self.subagent_io(move |_, root| {
            if run_path(root, &run.id)?.exists() {
                let latest = read_run(root, &run.id)?;
                for c in latest.observed_children { if !run.observed_children.contains(&c) { run.observed_children.push(c); } }
            }
            save_run(root, &run)
        }).await
    }
    async fn subagent_read(self: &Arc<Self>, id: String) -> Result<SubagentRun, IpcError> { self.subagent_io(move |_, root| read_run(root, &id)).await }
    fn recovery_candidates(&self, run: &SubagentRun) -> Vec<ChatKey> {
        let Some(wt) = &run.worktree else { return vec![] };
        self.read(|d| d.locals.iter().filter(|(c, l)| l.worktree.as_ref() == Some(&wt.id) && d.hosted.contains(*c)).map(|(c, _)| c.clone()).collect())
    }
    /// Keep all found IDs, including children not yet adopted/read due to scan limits.
    pub(super) fn subagent_note_scan(self: &Arc<Self>, root: &AgentKey, found: &[Agent], complete: bool) {
        let verified = complete && found.iter().all(|a| matches!(a.parent, ParentLink::Explicit { .. }));
        self.subagent_rt.scans.lock().unwrap().insert(root.clone(), (verified, found.iter().map(|a| a.key.clone()).collect()));
        self.subagent_observe();
    }
    fn subagent_scan_children(&self, parent: &ChatKey) -> Vec<AgentKey> {
        self.subagent_rt.scans.lock().unwrap().get(&agent_key_of(parent)).map(|(_, ids)| ids.clone()).unwrap_or_default()
    }
    /// A coalesced observation only records discovered IDs. It never resumes/sends/interrupts.
    pub(super) fn subagent_observe(self: &Arc<Self>) {
        if self.subagent_rt.observing.swap(true, Ordering::SeqCst) { return; }
        let host = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let copy = host.clone();
            let result = tokio::task::spawn_blocking(move || {
                let _g = copy.subagent_rt.io.lock().unwrap();
                let root = copy.subagent_root()?;
                for mut r in read_runs(&root)? {
                    let Some(parent) = r.parent.as_ref() else { continue };
                    let children = copy.read(|d| d.agents.iter().filter(|a| &a.agent.chat == parent && a.agent.key != agent_key_of(parent)).map(|a| a.agent.key.clone()).collect::<Vec<_>>());
                    let old = r.observed_children.len();
                    for child in children.into_iter().chain(copy.subagent_scan_children(parent)) { if !r.observed_children.contains(&child) { r.observed_children.push(child); } }
                    if old != r.observed_children.len() { save_run(&root, &r)?; }
                }
                Ok::<(), IpcError>(())
            }).await;
            host.subagent_rt.observing.store(false, Ordering::SeqCst);
            if let Ok(Err(e)) = result { host.warn(e.message); }
        });
    }
    pub async fn subagent_start(self: &Arc<Self>, args: SubagentStartArgs, confirmed: &UserConfirmed) -> Result<SubagentRunView, IpcError> {
        let _op = self.subagent_rt.operation.try_lock().map_err(|_| err(IpcErrorCode::Blocked, "新方式の開始処理が実行中です"))?;
        if args.request.trim().is_empty() { return Err(err(IpcErrorCode::InvalidArgs, "依頼を入力してください")); }
        self.precheck_repo_ready(&args.repo).await?;
        let canonical = std::fs::canonicalize(&args.repo).map_err(failure)?;
        let actual = crate::gitops::worktree::repo_root(&self.git_for_worktree(), &canonical).await.map_err(failure)?.ok_or_else(|| failure("Gitのルートを確認できません"))?;
        if std::fs::canonicalize(&actual).map_err(failure)? != canonical { return Err(err(IpcErrorCode::InvalidArgs, "リポジトリのルートフォルダを選んでください")); }
        let args = SubagentStartArgs { repo: canonical.to_string_lossy().into_owned(), ..args };
        let mut r = SubagentRun { schema_version: 1, id: self.local_id("run").0, title: super::derive_chat_name(&args.request).unwrap_or_default(), created_at: now_ms(), repo: args.repo.clone(), worktree: None, parent: None, first_attempt: None, phase: "creatingWorktree".into(), detail: None, observed_children: vec![] };
        self.subagent_save(&r).await?;
        let rec = match self.create_worktree(crate::backend::worktree::CreateWorktreeArgs { repo_root: args.repo, branch: None, base: None, chat_name: Some(r.title.clone()) }, confirmed).await {
            Ok(rec) => rec,
            Err(e) => { r.phase = "startUnknown".into(); r.detail = Some(e.message); self.subagent_save(&r).await?; return self.subagent_view(r).await; }
        };
        r.worktree = Some(rec.clone());
        r.phase = if matches!(rec.state, WorktreeState::Ready) { "creatingParent" } else { "startUnknown" }.into();
        self.subagent_save(&r).await?;
        if r.phase != "creatingParent" { return self.subagent_view(r).await; }
        let parent = match self.start_chat(StartChatArgs { backend: BackendKind::Codex, cwd: Some(rec.path.clone()), model: args.model, permission: None, first_message: None, worktree: Some(rec.id.clone()), extra_folders: vec![], confirmed_strong: vec![] }, confirmed).await {
            Ok(p) => p,
            Err(e) => { r.phase = "startUnknown".into(); r.detail = Some(e.message); self.subagent_save(&r).await?; return self.subagent_view(r).await; }
        };
        r.parent = Some(parent.chat.key.clone());
        r.phase = "firstSendPending".into();
        self.subagent_save(&r).await?; // durable parent before a model turn can start
        let prompt = parent_prompt(&args.request, &rec.path, &rec.base_commit);
        match self.send_message(SendMessageArgs { chat: parent.chat.key, text: prompt, intent: SendIntent::NewTurn, attachments: vec![], steer_entry: None }, confirmed).await {
            Ok(a) => { r.first_attempt = Some(a.attempt_id); r.phase = "ready".into(); }
            Err(e) => { r.phase = "firstSendUnknown".into(); r.detail = Some(e.message); }
        }
        // Preserve children an event may already have added.
        self.subagent_save(&r).await?;
        self.subagent_view(r).await
    }
    async fn subagent_view(self: &Arc<Self>, run: SubagentRun) -> Result<SubagentRunView, IpcError> {
        let id = run.id;
        let run = self.subagent_io(move |host, root| {
            let mut run = read_run(root, &id)?;
            let children = host.read(|d| d.agents.iter().filter(|a| Some(&a.agent.chat) == run.parent.as_ref() && Some(a.agent.key.clone()) != run.parent.as_ref().map(agent_key_of)).map(|a| a.agent.key.clone()).collect::<Vec<_>>());
            let old = run.observed_children.len();
            let scanned = run.parent.as_ref().map(|p| host.subagent_scan_children(p)).unwrap_or_default();
            for c in children.into_iter().chain(scanned) { if !run.observed_children.contains(&c) { run.observed_children.push(c); } }
            if old != run.observed_children.len() { save_run(root, &run)?; }
            Ok(run)
        }).await?;
        let (agents, pending_queue) = self.read(|d| (d.agents.iter().filter(|a| Some(&a.agent.chat) == run.parent.as_ref()).cloned().collect::<Vec<_>>(), run.parent.as_ref().and_then(|c| d.queues.get(c)).is_some_and(|q| crate::rules::queue::has_pending(&q.queue) || q.queue.awaiting.is_some())));
        let mut blockers = if let Some(chat) = &run.parent { gate_blockers(&run, &self.queue_gate_input(chat).await, pending_queue) } else { vec!["親チャットが未確認です。再試行せず、worktreeと通常履歴を確認してください".into()] };
        if let Some(chat) = &run.parent {
            if !self.subagent_rt.scans.lock().unwrap().get(&agent_key_of(chat)).is_some_and(|(complete, _)| *complete) { blockers.push("子孫の直接親を含む走査の照合が未完了です".into()); }
            if self.send_unsettled(chat) || self.subagent_turn_pending(chat) || self.read(|d| d.running_turn.keys().any(|k| d.agents.iter().any(|a| &a.agent.chat == chat && &a.agent.key == k))) { blockers.push("送信の確定または実行中turnの終端を待っています".into()); }
        }
        let id = run.id.clone();
        let records = self.subagent_io(move |_, root| imports(root, &id)).await?;
        if records.iter().any(|r| r.outcome.is_none() || matches!(r.outcome, Some(OrgImportOutcome::Unknown { .. } | OrgImportOutcome::Partial { .. }))) { blockers.push("取り込み結果が未確認です。「HEADを照合」で確認してください".into()); }
        let recovery_candidates = self.recovery_candidates(&run);
        Ok(SubagentRunView { run, agents, blockers, imports: records, recovery_candidates })
    }
    pub async fn subagent_runs(self: &Arc<Self>) -> Result<Vec<SubagentRunView>, IpcError> {
        let runs = self.subagent_io(|_, root| read_runs(root)).await?;
        let mut out = vec![]; for r in runs { out.push(self.subagent_view(r).await?); } Ok(out)
    }
    pub async fn subagent_recover(self: &Arc<Self>, args: SubagentRecoverArgs, _confirmed: &UserConfirmed) -> Result<SubagentRunView, IpcError> {
        let _op = self.subagent_rt.operation.lock().await;
        let mut run = self.subagent_read(args.id).await?;
        if run.parent.as_ref().is_some_and(|p| p != &args.parent) || !self.recovery_candidates(&run).contains(&args.parent) { return Err(failure("この依頼のworktreeに結び付いた親候補ではありません")); }
        let history = self.open_chat(OpenChatArgs { chat: args.parent.clone() }).await?;
        let wt = run.worktree.as_ref().ok_or_else(|| failure("worktreeが未確認です"))?;
        if history.agent.key != agent_key_of(&args.parent) || !matches!(history.agent.parent, ParentLink::Root) || history.chat.as_ref().and_then(|c| c.cwd.value()).is_none_or(|p| !same_path(p, &wt.path)) { return Err(failure("履歴の親・作業場所が一致しません")); }
        run.parent = Some(args.parent); run.phase = "ready".into(); run.detail = Some("利用者が親履歴を照合しました。送信・再開・再試行は行っていません".into());
        self.subagent_save(&run).await?;
        self.subagent_view(run).await
    }
    async fn import_source(&self, run: &SubagentRun) -> Result<String, IpcError> {
        let wt = run.worktree.as_ref().ok_or_else(|| failure("worktreeの記録がありません"))?;
        if !matches!(wt.state, WorktreeState::Ready) { return Err(failure("worktreeが準備済みではありません")); }
        let managed = self.persist.store().ok_or_else(|| failure("保存先が未確認です"))?.worktrees_dir();
        super::changes::confine_to(Some(&managed.to_string_lossy()), &wt.path).map_err(failure)?;
        if !self.read(|d| d.worktrees.iter().any(|w| w.id == wt.id && w.path == wt.path && w.repo_root == wt.repo_root && w.base_commit == wt.base_commit && w.branch == wt.branch && matches!(w.state, WorktreeState::Ready))) { return Err(failure("worktreeの管理台帳が一致しません")); }
        if !same_path(&wt.repo_root, &run.repo) { return Err(failure("元リポジトリの記録が一致しません")); }
        let git = self.git_for_worktree(); let p = Path::new(&wt.path);
        let listed = crate::gitops::worktree::list(&git, Path::new(&run.repo)).await.map_err(|e| failure(e.message()))?;
        let entry = listed.iter().find(|w| same_path(&w.path, &wt.path)).ok_or_else(|| failure("Gitのworktree一覧に成果の場所がありません"))?;
        if entry.bare || entry.detached || entry.prunable || entry.locked || entry.branch.as_deref() != Some(wt.branch.as_str()) { return Err(failure("成果worktreeの状態・ブランチが一致しません")); }
        // Compare both Git's own root and its absolute administrative directory.
        let actual_root = crate::gitops::worktree::repo_root(&git, p).await.map_err(failure)?.ok_or_else(|| failure("成果のGitルートが不明です"))?;
        if !same_path(&actual_root, &wt.path) { return Err(failure("成果worktreeのGitルートが一致しません")); }
        let facts = gi::repo_facts(&git, p).await;
        if facts.in_progress.is_some() || facts.branch.as_deref() != Some(wt.branch.as_str()) { return Err(failure("成果worktreeで別のGit操作が進行中です")); }
        let source_gitdir = git.run(p, &crate::gitops::GitOp::AbsoluteGitDir).await.map_err(failure)?;
        let admin = std::fs::canonicalize(source_gitdir.stdout_text().trim()).map_err(failure)?;
        let common = admin.parent().and_then(Path::parent).ok_or_else(|| failure("Git管理ディレクトリを確認できません"))?;
        let target_gitdir = git.run(Path::new(&run.repo), &crate::gitops::GitOp::AbsoluteGitDir).await.map_err(failure)?;
        let target_admin = std::fs::canonicalize(target_gitdir.stdout_text().trim()).map_err(failure)?;
        let target_common = if target_admin.parent().is_some_and(|p| p.file_name().is_some_and(|n| n == "worktrees")) { target_admin.parent().unwrap().parent().unwrap() } else { &target_admin };
        if common != target_common { return Err(failure("成果と取り込み先が同じGitリポジトリではありません")); }
        let head = gi::head_now(&git, p).await.ok_or_else(|| failure("成果のHEADを読めません"))?;
        if !gi::dirty_paths(&git, p).await.map_err(|e| failure(e.message()))?.is_empty() { return Err(failure("専用worktreeに未コミットの変更があります。親へコミットを依頼するか、手動で確認してください")); }
        if entry.head.as_deref() != Some(head.as_str()) { return Err(failure("成果HEADが照合中に変わりました")); }
        if head == wt.base_commit || ga::is_ancestor(&git, p, &wt.base_commit, &head).await != Some(true) { return Err(failure("開始コミットからの成果コミットを確認できません")); }
        Ok(head)
    }
    pub async fn subagent_import_preview(self: &Arc<Self>, args: SubagentRunArgs) -> Result<SubagentImportPreview, IpcError> {
        let run = self.subagent_read(args.id.clone()).await?;
        let view = self.subagent_view(run).await?;
        let mut out = SubagentImportPreview { token: None, source_head: None, target_head: None, result_head: None, files: vec![], blockers: view.blockers };
        if !out.blockers.is_empty() { return Ok(out); }
        let source = match self.import_source(&view.run).await { Ok(s) => s, Err(e) => { out.blockers.push(e.message); return Ok(out); } };
        out.source_head = Some(source.clone());
        let git = self.git_for_worktree(); let repo = Path::new(&view.run.repo);
        let facts = gi::repo_facts(&git, repo).await;
        out.target_head = facts.head.clone();
        if facts.branch.as_deref().is_none_or(|s| s == "HEAD" || s.starts_with("agentdock/")) || facts.in_progress.is_some() || facts.git_supported != Some(true) { out.blockers.push("取り込み先のブランチ・Gitの対応・途中の操作を確認してください".into()); return Ok(out); }
        let prepared = match gi::prepare(&git, repo, &facts, &view.run.title, &[gi::Candidate { task: view.run.id.clone(), title: view.run.title.clone(), commit: source.clone() }]).await.map_err(|e| failure(e.message()))? {
            gi::PrepareOutcome::Ready(p) => p,
            gi::PrepareOutcome::Conflict { files } => { out.blockers.push(format!("衝突があります: {}", files.join("、"))); return Ok(out); }
            gi::PrepareOutcome::NoChange => { out.blockers.push("取り込む差分はありません".into()); return Ok(out); }
            gi::PrepareOutcome::Failed(m) => { out.blockers.push(m); return Ok(out); }
        };
        let check = gi::check_worktree(&git, repo, &prepared.files).await.map_err(|e| failure(e.message()))?;
        if !check.overlap.is_empty() || !check.locked.is_empty() { out.blockers.push(format!("未コミットの変更との重なり・ファイルロックがあります: {}", check.overlap.iter().chain(check.locked.iter()).cloned().collect::<Vec<_>>().join("、"))); return Ok(out); }
        if self.import_source(&view.run).await? != source { return Err(failure("成果HEADがプレビュー中に変わりました")); }
        out.files = prepared.files.clone(); out.result_head = Some(prepared.result.clone());
        let token = self.local_id("subagent-import").0;
        { let mut tokens = self.subagent_rt.tokens.lock().unwrap(); tokens.retain(|_, t| now_ms().0 - t.at <= TTL); tokens.insert(token.clone(), Token { run: view.run.id, source, prepared, at: now_ms().0 }); }
        out.token = Some(token); Ok(out)
    }
    pub async fn subagent_import(self: &Arc<Self>, args: SubagentImportArgs, _confirmed: &UserConfirmed) -> Result<OrgImportOutcome, IpcError> {
        let _op = self.subagent_rt.operation.try_lock().map_err(|_| failure("別の新方式の操作が実行中です"))?;
        let token = self.subagent_rt.tokens.lock().unwrap().remove(&args.token).filter(|t| t.run == args.id && now_ms().0 - t.at <= TTL).ok_or_else(|| failure("確認が期限切れか使用済みです。もう一度プレビューしてください"))?;
        let run = self.subagent_read(args.id.clone()).await?;
        let _chat = run.parent.as_ref().ok_or_else(|| failure("親が未確認です"))?;
        let Some(_git) = self.gitprep_rt.begin(&run.repo) else { return Err(super::gitprep::busy_error()) };
        let _folder = self.begin_folder_op(vec![run.repo.clone(), run.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_default()]);
        // Freeze target-folder dispatch while checking every overlapping chat. Never wait for
        // chat locks here: a sender that started first makes this import refuse and retryable.
        let target_chats = self.chats_overlapping_folders(std::slice::from_ref(&run.repo));
        let mut send_guards = Vec::with_capacity(target_chats.len());
        for target in &target_chats {
            let lock = self.queue_rt.send_lock(target);
            match lock.try_lock_owned() {
                Ok(guard) => send_guards.push(guard),
                Err(_) => return Err(failure("取り込み先でチャットの送信処理が進行中です")),
            }
        }
        let target_busy = self.read(|d| target_chats.iter().any(|target| {
            d.open_stop(target).is_some()
                || d.locals.get(target).is_some_and(|local| !local.pending_ops.is_empty())
                || d.agents.iter().any(|agent| &agent.agent.chat == target && d.running_turn.contains_key(&agent.agent.key))
        }));
        if target_busy || target_chats.iter().any(|target| self.send_unsettled(target) || self.subagent_turn_pending(target)) {
            return Err(failure("取り込み先の別チャットに未確認の送信・操作・停止、または実行中turnがあります"));
        }
        let view = self.subagent_view(run).await?;
        if !view.blockers.is_empty() { return Err(failure(view.blockers.join("\n"))); }
        if self.import_source(&view.run).await? != token.source { return Err(failure("成果のHEADが確認時から変わりました。もう一度プレビューしてください")); }
        let target_active = self.read(|d| d.agents.iter().any(|a| d.running_turn.contains_key(&a.agent.key) && d.chats.iter().find(|c| c.key == a.agent.chat).and_then(|c| c.cwd.value()).is_some_and(|p| same_path(p, &view.run.repo))));
        if target_active { return Err(failure("取り込み先で別チャットが作業中です")); }
        let root = self.subagent_root()?;
        let record = SubagentImportRecord { id: self.local_id("import").0, at: now_ms(), before: token.prepared.head.clone(), result: token.prepared.result.clone(), source: token.source, outcome: None };
        let outcome = gi::execute(&self.git_for_worktree(), Path::new(&view.run.repo), &token.prepared, || {
            let _g = self.subagent_rt.io.lock().unwrap(); append(&root, &args.id, &Ledger::Begin { record: record.clone() }).map_err(|e| e.message)
        }).await;
        match outcome {
            gi::ExecOutcome::Refused(m) | gi::ExecOutcome::BeginFailed(m) => Err(failure(m)),
            gi::ExecOutcome::Done(outcome) => { let id = args.id; let result = outcome.clone(); self.subagent_io(move |_, root| append(root, &id, &Ledger::End { id: record.id, outcome: result })).await.map_err(|e| failure(format!("実行後の台帳保存を確認できません。再試行せずHEADを照合してください: {}", e.message)))?; Ok(outcome) }
        }
    }
    /// Explicit read-only reconciliation; no retry, reset, resume, or status inferred from a parent message.
    pub async fn subagent_verify_import(self: &Arc<Self>, args: SubagentRunArgs) -> Result<Vec<SubagentImportRecord>, IpcError> {
        let _op = self.subagent_rt.operation.lock().await;
        let run = self.subagent_read(args.id.clone()).await?;
        let id = args.id.clone();
        let records = self.subagent_io(move |_, root| imports(root, &id)).await?;
        let head = gi::head_now(&self.git_for_worktree(), Path::new(&run.repo)).await;
        for r in records.iter().filter(|r| r.outcome.is_none() || matches!(r.outcome, Some(OrgImportOutcome::Unknown { .. }))) {
            // Only the exact prepared result proves application. The old HEAD alone cannot prove that no file was touched.
            let outcome = if head.as_deref() == Some(r.result.as_str()) { OrgImportOutcome::Applied { head: r.result.clone() } } else { OrgImportOutcome::Unknown { head_now: head.clone().map(|value| Known::direct(value)).unwrap_or(Known::NotFetched) } };
            let id = args.id.clone(); let record_id = r.id.clone(); self.subagent_io(move |_, root| append(root, &id, &Ledger::End { id: record_id, outcome })).await?;
        }
        let id = args.id; self.subagent_io(move |_, root| imports(root, &id)).await
    }
}

#[cfg(test)]
#[path = "subagents_tests.rs"]
mod tests;
