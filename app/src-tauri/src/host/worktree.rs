//! Git worktree（P3-7、`app/DESIGN_P3.md` §1 #14）。AgentDock管理の操作で、バックエンドには依存しない。
//!
//! - worktreeを指定しない新規チャットは従来どおり通常の作業場所で動く。「分離」を選んだときだけ作る（チャット作成のたびには作らない）。
//! - 作成: `WorktreeRecord`（`Creating`）を `worktrees.json` に保存してから `git worktree add -b`。成功で `Ready`、失敗は `Failed`
//!   （作られた分は自動削除せず、表示して手動の削除を案内する）。保存できなければgitを実行しない。
//! - 削除: AgentDockが作った記録のあるものだけ。Gitの一覧と実在を照合し、使っているチャットに作業中・停止未確認・送信待ち・受理不明が
//!   あれば止める。`--force` なし。未コミット変更があれば止め、「変更を破棄して削除」は2段目の確認で `--force`。ブランチは残す
//!   （選んだときだけ `git branch -d`。`-D` は使わない）。判定は `rules::worktree`。
//! - 外部のworktreeは一覧に出すだけ（作業フォルダとして選べるが削除しない）。チャットの削除ではworktreeを消さない。
//! - 置き場所は専用領域の `worktrees\`。パスは字句と解決後の両方で配下であることを確認する（`..`・リンク越えは不可）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::changes::{confine_to, git_error_status};
use super::persist::save_failure_message;
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::UserConfirmed;
use crate::backend::changes::ListStatus;
use crate::backend::ipc::*;
use crate::backend::local::QueueEntryState;
use crate::backend::model::*;
use crate::backend::worktree::*;
use crate::gitops::worktree::{self as gw, WtError};
use crate::gitops::{Git, GitError, GitOp};
use crate::rules::revert::norm_path;
use crate::rules::worktree::{self as rw, ChatUse, Listed, RemoveBlock, RemoveInput, RemoveVerdict};
use crate::store::layout;
use crate::store::records::{WorktreesFile, SCHEMA_VERSION};

/// worktree操作の作業状態。作成・削除（git実行）は同時に1件だけ。
#[derive(Default)]
pub struct WorktreeRuntime {
    lock: tokio::sync::Mutex<()>,
}

fn git_run_err(e: GitError) -> IpcError {
    match e {
        GitError::Unavailable(_) => blocked(BlockedReason::GitUnavailable, "Gitを実行できません（未導入、または設定のGitの場所が違います）"),
        GitError::Timeout => err(IpcErrorCode::OutcomeUnknown, "Gitが時間内に終わらず、結果が分かりません。状態を確認してください"),
        GitError::InvalidArgument(m) => err(IpcErrorCode::InvalidArgs, format!("指定が正しくありません: {m}")),
        other => err(IpcErrorCode::Io, other.to_string()),
    }
}

fn wt_err(e: WtError) -> IpcError {
    match e {
        WtError::Run(g) => git_run_err(g),
        WtError::Exit { message } => err(IpcErrorCode::Rejected, message),
    }
}

/// 文字列から安定した種を作る（短いIDの元。暗号用途ではない）。
fn seed_of(text: &str) -> u64 {
    text.bytes().fold(14695981039346656037u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211))
}

/// 削除判定の材料（読取りの結果）。
struct Assessment {
    verdict: RemoveVerdict,
    using_chats: Vec<ChatKey>,
    dirty: Known<u32>,
    detail: Option<String>,
}

fn to_api_verdict(v: RemoveVerdict) -> WorktreeRemoveVerdict {
    match v {
        RemoveVerdict::Allowed => WorktreeRemoveVerdict::Allowed,
        RemoveVerdict::NeedsDiscard { dirty } => WorktreeRemoveVerdict::NeedsDiscard { dirty },
        RemoveVerdict::AlreadyGone => WorktreeRemoveVerdict::AlreadyGone,
        RemoveVerdict::Blocked(b) => WorktreeRemoveVerdict::Blocked {
            reason: match b {
                RemoveBlock::OutsideRoot => RemoveBlockReason::OutsideRoot,
                RemoveBlock::InUse => RemoveBlockReason::InUse,
                RemoveBlock::MainWorktree => RemoveBlockReason::MainWorktree,
                RemoveBlock::NotListed => RemoveBlockReason::NotListed,
                RemoveBlock::Unverified => RemoveBlockReason::Unverified,
            },
        },
    }
}

fn blocked_message(b: RemoveBlock, detail: Option<&str>) -> String {
    match b {
        RemoveBlock::OutsideRoot => "記録されたパスがAgentDockの置き場所の外（またはリンク先が外）のため、削除しません".into(),
        RemoveBlock::InUse => "このworktreeを使っているチャットに、作業中・停止未確認・送信待ち・受理不明があるため、削除できません".into(),
        RemoveBlock::MainWorktree => "リポジトリ本体の作業ツリーのため、削除しません".into(),
        RemoveBlock::NotListed => "フォルダは存在しますが、Gitのworktree一覧にありません。別の用途のフォルダかもしれないため、削除しません".into(),
        RemoveBlock::Unverified => format!("Gitの状態を確認できないため、削除しません{}", detail.map(|d| format!("（{d}）")).unwrap_or_default()),
    }
}

impl Host {
    fn git_for_worktree(&self) -> Git {
        let tool = self.read(|d| d.settings.tools.git.clone());
        Git::new(tool.as_deref())
    }

    /// worktreeの置き場所（専用領域の `worktrees\`）。
    fn worktree_root(&self) -> PathBuf {
        match self.persist.store() {
            Some(s) => s.worktrees_dir(),
            None => self.app_data_dir.join(layout::WORKTREES_DIR),
        }
    }

    /// 台帳を `worktrees.json` へ書く。書込みの直前に最新の台帳を読む（並行した保存で古い内容が後から書かれない）。
    async fn save_worktrees(self: &Arc<Self>) -> Result<(), String> {
        let Some(store) = self.persist.store().cloned() else { return Ok(()) };
        let host = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            let _g = host.persist.io_guard();
            let worktrees = host.read(|d| d.worktrees.clone());
            store.save_worktrees(&WorktreesFile { schema_version: SCHEMA_VERSION, worktrees })
        })
        .await;
        match res {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(save_failure_message(&e)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn put_worktree(&self, rec: WorktreeRecord) {
        self.mutate(|d| {
            match d.worktrees.iter_mut().find(|r| r.id == rec.id) {
                Some(r) => *r = rec.clone(),
                None => d.worktrees.push(rec.clone()),
            }
            ((), vec![HostEvent::WorktreeUpdated { id: rec.id.clone(), record: Some(rec) }])
        });
    }

    fn drop_worktree(&self, id: &LocalId) {
        self.mutate(|d| {
            d.worktrees.retain(|r| &r.id != id);
            ((), vec![HostEvent::WorktreeUpdated { id: id.clone(), record: None }])
        });
    }

    fn worktree_record(&self, id: &LocalId) -> Option<WorktreeRecord> {
        self.read(|d| d.worktrees.iter().find(|r| &r.id == id).cloned())
    }

    /// チャットに結び付いたworktreeの場所（削除確認の「worktreeは残る」表示用）。記録を外した後は None。
    pub(super) fn chat_worktree_path(&self, chat: &ChatKey) -> Option<String> {
        self.read(|d| {
            let id = d.locals.get(chat)?.worktree.as_ref()?;
            d.worktrees.iter().find(|r| &r.id == id).map(|r| r.path.clone())
        })
    }

    // ───────────── 読取り ─────────────

    /// フォルダのGit情報（読取りのみ）。リポジトリでない・Gitがない場合は理由つきで返す。取得できなかった値は未取得。
    pub async fn git_info(self: &Arc<Self>, args: GitInfoArgs) -> Result<GitInfo, IpcError> {
        let folder = args.folder.trim().to_string();
        let blank = |status: ListStatus| GitInfo { folder: folder.clone(), status, root: None, branch: None, head: None, dirty_count: Known::NotFetched, worktrees: vec![] };
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Ok(blank(ListStatus::NotSupported { message: "フォルダが見つかりません".into() }));
        }
        let git = self.git_for_worktree();
        let root = match gw::repo_root(&git, Path::new(&folder)).await {
            Err(e) => return Ok(blank(git_error_status(&e))),
            Ok(None) => return Ok(blank(ListStatus::NotSupported { message: "このフォルダはGitのリポジトリではありません".into() })),
            Ok(Some(r)) => r,
        };
        let rootp = PathBuf::from(&root);
        let one_line = |o: Result<crate::gitops::GitOutput, GitError>| o.ok().filter(|o| o.success()).map(|o| o.stdout_text().trim().to_string()).filter(|s| !s.is_empty());
        let branch = one_line(git.run(&rootp, &GitOp::CurrentBranch).await);
        let head = one_line(git.run(&rootp, &GitOp::Head).await);
        let dirty_count = gw::dirty_count(&git, &rootp).await.map_or(Known::NotFetched, Known::direct);
        let records = self.read(|d| d.worktrees.clone());
        let (status, worktrees) = match gw::list(&git, &rootp).await {
            Ok(list) => {
                let entries = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, w)| {
                        let origin = if i == 0 {
                            WorktreeOrigin::Main
                        } else {
                            match records.iter().find(|r| norm_path(&r.path) == norm_path(&w.path)) {
                                Some(r) => WorktreeOrigin::AgentDock { record: r.id.clone() },
                                None => WorktreeOrigin::External,
                            }
                        };
                        WorktreeEntry { path: w.path, branch: w.branch, head: w.head, origin, detached: w.detached, locked: w.locked, prunable: w.prunable }
                    })
                    .collect();
                (ListStatus::Ready, entries)
            }
            Err(e) => (ListStatus::NotFetched { message: format!("worktreeの一覧を取得できませんでした（{}）", e.message()) }, vec![]),
        };
        Ok(GitInfo { folder, status, root: Some(root), branch, head, dirty_count, worktrees })
    }

    // ───────────── 作成 ─────────────

    /// worktreeを作る（ユーザーの明示操作）。保存できなければgitを実行しない。戻り値の `state` が `Ready` のときだけ作成成功。
    pub async fn create_worktree(self: &Arc<Self>, args: CreateWorktreeArgs, _confirmed: &UserConfirmed) -> Result<WorktreeRecord, IpcError> {
        self.precheck_space()?;
        let folder = args.repo_root.trim().to_string();
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, format!("リポジトリのフォルダが見つかりません: {folder}")));
        }
        let _op = self.worktree_rt.lock.lock().await;
        let git = self.git_for_worktree();
        let top = match gw::repo_root(&git, Path::new(&folder)).await.map_err(git_run_err)? {
            Some(r) => r,
            None => return Err(blocked(BlockedReason::NotARepository, "このフォルダはGitのリポジトリではないため、worktreeを作れません")),
        };
        // 名前付けと記録は、メインの作業ツリー（一覧の先頭）を基準にする。
        let listed = gw::list(&git, Path::new(&top)).await.map_err(wt_err)?;
        let main_path = listed.first().map(|w| w.path.clone()).unwrap_or_else(|| top.clone());
        let rev = args.base.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("HEAD");
        let base_commit = gw::resolve_commit(&git, Path::new(&top), rev).await.map_err(|e| match e {
            WtError::Exit { message } => err(IpcErrorCode::InvalidArgs, message),
            other => wt_err(other),
        })?;

        let id = self.local_id("wt");
        let short = rw::short_id(seed_of(&id.0));
        let branch = match args.branch.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(b) => b.to_string(),
            None => rw::worktree_branch_name(args.chat_name.as_deref().unwrap_or("chat"), &short),
        };
        let root = self.worktree_root();
        let root_str = root.to_string_lossy().into_owned();
        let path = root.join(rw::worktree_folder_name(&rw::repo_name_of(&main_path), &short)).to_string_lossy().into_owned();
        // 実行前に、引数が許可の形か（ブランチ名など）を確認する。
        GitOp::WorktreeAdd { path: path.clone(), branch: branch.clone(), base: base_commit.clone() }.args().map_err(git_run_err)?;
        if !rw::is_under(&root_str, &path) {
            return Err(err(IpcErrorCode::InvalidArgs, "作成先が置き場所の外になるため、worktreeを作りません"));
        }
        if Path::new(&path).exists() {
            return Err(blocked(BlockedReason::AlreadyExists, format!("作成先がすでに存在します: {path}")));
        }
        std::fs::create_dir_all(&root).map_err(|e| err(IpcErrorCode::Io, format!("worktreeの置き場所を作成できません: {e}")))?;

        // 作る前に記録を保存する（保存できなければ実行しない。異常終了後は `Creating` のまま残り、一覧と実在を照合して扱う）。
        let mut rec = WorktreeRecord { id: id.clone(), repo_root: main_path, path: path.clone(), branch: branch.clone(), base_commit: base_commit.clone(), created_at: now_ms(), created_for_chat: None, state: WorktreeState::Creating };
        self.put_worktree(rec.clone());
        if let Err(message) = self.save_worktrees().await {
            self.drop_worktree(&id);
            return Err(err(IpcErrorCode::Io, format!("実行前の記録を保存できなかったため、worktreeは作っていません（{message}）")));
        }
        rec.state = match gw::add(&git, Path::new(&top), &path, &branch, &base_commit).await {
            Ok(()) => WorktreeState::Ready,
            Err(e) if e.is_timeout() => WorktreeState::Failed { message: "Gitが時間内に終わらず、作成できたか分かりません。一覧で確認し、不要なら手動で削除してください".into() },
            Err(e) => WorktreeState::Failed { message: format!("{}（作られた分は自動では削除しません。必要なら手動で削除してください）", e.message()) },
        };
        self.put_worktree(rec.clone());
        if let Err(message) = self.save_worktrees().await {
            self.warn(format!("worktreeの記録を保存できませんでした（{message}）。再起動後に状態が戻る場合があります"));
        }
        Ok(rec)
    }

    /// 新しいチャットに結び付ける worktree の確認（作成済みで、チャットの作業フォルダがその場所であること）。
    pub(super) fn check_worktree_binding(&self, worktree: &Option<LocalId>, cwd: Option<&str>) -> Result<(), IpcError> {
        let Some(id) = worktree else { return Ok(()) };
        let Some(rec) = self.worktree_record(id) else {
            return Err(err(IpcErrorCode::NotFound, "指定されたworktreeの記録がありません"));
        };
        if !matches!(rec.state, WorktreeState::Ready) {
            return Err(err(IpcErrorCode::InvalidArgs, "worktreeの作成が完了していないため、作業フォルダにできません"));
        }
        match cwd {
            Some(c) if norm_path(c).trim_end_matches('\\') == norm_path(&rec.path).trim_end_matches('\\') => Ok(()),
            _ => Err(err(IpcErrorCode::InvalidArgs, "作業フォルダがworktreeの場所と一致しません")),
        }
    }

    /// チャットを作れた後に、worktreeの記録へ「作った対象のチャット」を残す（保存できなくても作業は止めない）。
    pub(super) async fn bind_worktree_chat(self: &Arc<Self>, id: &LocalId, chat: &ChatKey) {
        let changed = self.mutate(|d| {
            let Some(r) = d.worktrees.iter_mut().find(|r| &r.id == id) else { return (None, vec![]) };
            if r.created_for_chat.is_some() {
                return (None, vec![]);
            }
            r.created_for_chat = Some(chat.clone());
            let rec = r.clone();
            (Some(rec.clone()), vec![HostEvent::WorktreeUpdated { id: rec.id.clone(), record: Some(rec) }])
        });
        if changed.is_some() {
            if let Err(message) = self.save_worktrees().await {
                self.warn(format!("worktreeの記録を保存できませんでした（{message}）"));
            }
        }
    }

    // ───────────── 削除 ─────────────

    /// worktreeを使っているチャットと、削除を止める作業があるか。
    fn chats_using_worktree(&self, rec: &WorktreeRecord) -> (Vec<ChatKey>, bool) {
        let mut uses: Vec<(ChatKey, ChatUse)> = self.read(|d| {
            let mut keys: HashSet<ChatKey> = d.chats.iter().map(|c| c.key.clone()).collect();
            keys.extend(d.locals.keys().cloned());
            keys.into_iter()
                .filter_map(|k| {
                    let bound = d.locals.get(&k).and_then(|l| l.worktree.as_ref()).map(|i| i.0.as_str());
                    let cwd = d.chat(&k).and_then(|c| c.cwd.value().cloned());
                    if !rw::chat_uses_worktree(bound, cwd.as_deref(), &rec.id.0, &rec.path) {
                        return None;
                    }
                    let u = ChatUse {
                        running: d.agents.iter().any(|v| v.agent.chat == k && d.running_turn.contains_key(&v.agent.key)),
                        stop_open: d.open_stop(&k).is_some(),
                        unknown_send: false,
                        pending_ops: d.locals.get(&k).is_some_and(|l| !l.pending_ops.is_empty()),
                        queue_pending: d.queues.get(&k).is_some_and(|q| {
                            q.queue.entries.iter().any(|e| {
                                matches!(e.state, QueueEntryState::Waiting | QueueEntryState::Sending { .. } | QueueEntryState::AcceptanceUnknown { .. } | QueueEntryState::NotAccepted { .. })
                            })
                        }),
                    };
                    Some((k, u))
                })
                .collect()
        });
        for (k, u) in uses.iter_mut() {
            u.unknown_send = self.unknown_attempt(k).is_some();
        }
        uses.sort_by(|a, b| a.0.id.0.cmp(&b.0.id.0));
        let busy = uses.iter().any(|(_, u)| u.blocks_removal());
        (uses.into_iter().map(|(k, _)| k).collect(), busy)
    }

    /// 削除の可否を、Gitの一覧・実在・使用中・未コミット変更から判定する（読取りのみ）。
    async fn assess_removal(self: &Arc<Self>, git: &Git, rec: &WorktreeRecord) -> Assessment {
        let root_str = self.worktree_root().to_string_lossy().into_owned();
        let under_root = rw::is_under(&root_str, &rec.path) && confine_to(Some(&root_str), &rec.path).is_ok();
        let (using_chats, in_use) = self.chats_using_worktree(rec);
        let path_exists = Path::new(&rec.path).exists();
        let mut detail: Option<String> = None;
        let mut dirty = Known::NotFetched;
        let listed = if !Path::new(&rec.repo_root).is_dir() {
            detail = Some("元のリポジトリのフォルダが見つかりません".into());
            Listed::Unknown
        } else {
            match gw::list(git, Path::new(&rec.repo_root)).await {
                Err(e) => {
                    detail = Some(e.message());
                    Listed::Unknown
                }
                Ok(list) => match list.iter().position(|w| norm_path(&w.path) == norm_path(&rec.path)) {
                    Some(i) => Listed::Yes { main: i == 0 },
                    None => Listed::No,
                },
            }
        };
        if matches!(listed, Listed::Yes { main: false }) && path_exists {
            match gw::dirty_count(git, Path::new(&rec.path)).await {
                Ok(n) => dirty = Known::direct(n),
                Err(e) => detail = Some(e.message()),
            }
        }
        let verdict = rw::remove_verdict(&RemoveInput { under_root, listed, path_exists, in_use, dirty: dirty.value().copied() });
        Assessment { verdict, using_chats, dirty, detail }
    }

    /// 削除確認に出す内容（読取りのみ）。記録のないもの（外部のworktree）は対象外。
    pub async fn preview_remove_worktree(self: &Arc<Self>, args: WorktreeIdArgs) -> Result<WorktreeRemovePreview, IpcError> {
        let Some(record) = self.worktree_record(&args.id) else {
            return Err(err(IpcErrorCode::NotFound, "AgentDockが作ったworktreeの記録がありません（外部のworktreeは削除できません）"));
        };
        let git = self.git_for_worktree();
        let a = self.assess_removal(&git, &record).await;
        Ok(WorktreeRemovePreview { record, verdict: to_api_verdict(a.verdict), using_chats: a.using_chats, dirty: a.dirty, detail: a.detail })
    }

    /// worktreeを削除する（ユーザーの確認後だけ）。`--force` は未コミット変更があり、`force` が指定されたときだけ。ブランチは残す
    /// （`delete_branch` のときだけ、マージ済みなら `git branch -d`）。
    pub async fn remove_worktree(self: &Arc<Self>, args: RemoveWorktreeArgs, _confirmed: &UserConfirmed) -> Result<WorktreeRemoveOutcome, IpcError> {
        let _op = self.worktree_rt.lock.lock().await;
        let Some(rec) = self.worktree_record(&args.id) else {
            return Err(err(IpcErrorCode::NotFound, "AgentDockが作ったworktreeの記録がありません（外部のworktreeは削除できません）"));
        };
        let git = self.git_for_worktree();
        // 実行直前にもう一度判定する（確認画面の後に状況が変わっていれば止める）。
        let a = self.assess_removal(&git, &rec).await;
        let discard = match a.verdict {
            RemoveVerdict::Blocked(RemoveBlock::InUse) => {
                return Err(blocked(BlockedReason::WorktreeInUse, blocked_message(RemoveBlock::InUse, None)));
            }
            RemoveVerdict::Blocked(b) => return Err(err(IpcErrorCode::Rejected, blocked_message(b, a.detail.as_deref()))),
            RemoveVerdict::NeedsDiscard { dirty } if !args.force => {
                return Err(blocked(BlockedReason::WorktreeDirty, format!("未コミットの変更が{dirty}件あるため、削除を止めました。変更を破棄してよいか、もう一度確認してください")));
            }
            RemoveVerdict::NeedsDiscard { .. } => true,
            RemoveVerdict::AlreadyGone => {
                self.drop_worktree(&rec.id);
                if let Err(message) = self.save_worktrees().await {
                    self.warn(format!("worktreeの記録を保存できませんでした（{message}）"));
                }
                return Ok(WorktreeRemoveOutcome::RecordDropped);
            }
            RemoveVerdict::Allowed => false,
        };
        if let Err(e) = gw::remove(&git, Path::new(&rec.repo_root), &rec.path, discard).await {
            return Err(if e.is_timeout() {
                err(IpcErrorCode::OutcomeUnknown, "Gitが時間内に終わらず、削除できたか分かりません。状態を確認してください")
            } else {
                wt_err(e)
            });
        }
        // 実体は消えた。ブランチは残す（選んだときだけ、マージ済みなら削除。`-D` は使わない）。
        let branch = if args.delete_branch {
            match gw::delete_branch_merged(&git, Path::new(&rec.repo_root), &rec.branch).await {
                Ok(()) => BranchOutcome::Deleted,
                Err(e) => BranchOutcome::NotDeleted { message: e.message() },
            }
        } else {
            BranchOutcome::Kept
        };
        self.drop_worktree(&rec.id);
        if let Err(message) = self.save_worktrees().await {
            self.warn(format!("worktreeの記録を保存できませんでした（{message}）。再起動後に記録が残る場合があります（実体がなければ記録だけが外れます）"));
        }
        Ok(WorktreeRemoveOutcome::Removed { branch })
    }
}
