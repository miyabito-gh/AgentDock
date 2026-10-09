//! クラウド委任（P3-6、`app/DESIGN_P3.md` §1 #13）。CLI補助（`codex cloud`、experimental）で行い、App Serverに経路はない。
//!
//! - 委任: 依頼文・環境ID・ブランチを入力して明示確認した後だけ。送る前に `CloudTaskRecord`（`Submitting`）を `cloud-tasks.json` へ
//!   保存し、保存できなければ送らない。結果は出力の観測だけで分類する（`Submitted` / `Rejected` / `Unknown`）。受理不明は再送しない
//!   （一覧の取得＝読取りで照合する）。再起動時に `Submitting` のまま残っていたものも `Unknown` にする。
//! - 状態の取得はパネルを開いたときとユーザーの「更新」だけ（定期取得しない）。状態語は原文のまま表示し、AgentDockの状態（§4.1）へ写像しない。
//! - 差分は表示するだけ。取込み（apply）は作業フォルダのファイルを変えるため、そのフォルダで作業中のチャットがないことを確認し、
//!   明示確認の後だけ実行する。結果は終了コードと `git status` の前後の差で示す（成功を断定しない）。
//! - 環境IDはリポジトリごとに手入力で記憶する（`settings.cloudEnvByRepo`）。送ったときだけ更新する。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::persist::save_failure_message;
use super::worktree::git_run_err;
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::cloud::*;
use crate::backend::ipc::*;
use crate::backend::local::SaveScope;
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::gitops::worktree as gw;
use crate::gitops::GitOp;
use crate::rules::diff::{parse_status_v2, StatusEntry};
use crate::rules::baseline::norm_path;
use crate::store::records::{CloudTasksFile, SCHEMA_VERSION};

/// 依頼文の先頭として保存する文字数。
const PROMPT_HEAD_CHARS: usize = 200;

/// 委任・取込みの直列化（同時に1件）。
#[derive(Default)]
pub struct CloudRuntime {
    lock: tokio::sync::Mutex<()>,
}

/// 送信結果から記録の状態を作る（純粋）。
pub fn outcome_state(outcome: CloudSubmitOutcome) -> CloudTaskState {
    match outcome {
        CloudSubmitOutcome::Submitted { task_id } => CloudTaskState::Submitted { task_id },
        CloudSubmitOutcome::Rejected { message } => CloudTaskState::Rejected { message },
        CloudSubmitOutcome::Unknown { message } => CloudTaskState::Unknown { message },
    }
}

/// 送る前に断られた（接続なし・非対応など）ときの状態。受理不明（時間切れ）だけ `Unknown`、それ以外は送られていない。
pub fn failure_state(e: &BackendError) -> CloudTaskState {
    match e {
        BackendError::OutcomeUnknown { message } => CloudTaskState::Unknown { message: message.clone() },
        BackendError::NotConnected => CloudTaskState::Rejected { message: "Codexに接続していないため、送っていません".into() },
        BackendError::Unsupported { .. } => CloudTaskState::Rejected { message: "このCodexには cloud のサブコマンドがないため、送っていません".into() },
        other => CloudTaskState::Rejected { message: format!("送っていません（{other}）") },
    }
}

/// 保存から戻した記録。`Submitting` のまま残っていたものは、異常終了で結果を確認できていない（`Unknown`。再送しない）。
pub fn restored_tasks(tasks: Vec<CloudTaskRecord>) -> Vec<CloudTaskRecord> {
    tasks
        .into_iter()
        .map(|mut t| {
            if t.state == CloudTaskState::Submitting {
                t.state = CloudTaskState::Unknown { message: "送信の途中でAgentDockが終了したため、送られたか分かりません。「更新」で一覧と照合してください（再送はしません）".into() };
            }
            t
        })
        .collect()
}

/// 2つのパスが同じ場所か、一方がもう一方の内側か（区切りの境界で比べる）。
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (norm_path(a).trim_end_matches('\\').to_string(), norm_path(b).trim_end_matches('\\').to_string());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let inside = |outer: &str, inner: &str| inner == outer || inner.strip_prefix(outer).is_some_and(|r| r.starts_with('\\'));
    inside(&a, &b) || inside(&b, &a)
}

/// 取込みの前後で、状態が変わった（前になかった）パス。
pub fn status_delta(before: &[StatusEntry], after: &[StatusEntry]) -> Vec<String> {
    let mut v: Vec<String> = after.iter().filter(|e| !before.contains(e)).map(|e| e.path.clone()).collect();
    v.sort();
    v.dedup();
    v
}

impl Host {
    async fn save_cloud_tasks(self: &Arc<Self>) -> Result<(), String> {
        let Some(store) = self.persist.store().cloned() else { return Ok(()) };
        let host = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            let _g = host.persist.io_guard();
            let tasks = host.read(|d| d.cloud_tasks.clone());
            store.save_cloud_tasks(&CloudTasksFile { schema_version: SCHEMA_VERSION, tasks })
        })
        .await;
        match res {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(save_failure_message(&e)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn put_cloud_task(&self, rec: CloudTaskRecord) {
        self.mutate(|d| {
            match d.cloud_tasks.iter_mut().find(|r| r.id == rec.id) {
                Some(r) => *r = rec.clone(),
                None => d.cloud_tasks.push(rec.clone()),
            }
            ((), vec![HostEvent::CloudTaskUpdated { record: rec }])
        });
    }

    fn drop_cloud_task(&self, id: &LocalId) {
        self.mutate(|d| {
            d.cloud_tasks.retain(|r| &r.id != id);
            ((), Vec::new())
        });
    }

    /// 作業フォルダのリポジトリのルート（Gitがない・リポジトリでなければ None）。読取りのみ。
    async fn cloud_repo_root(&self, folder: &str) -> Option<String> {
        let folder = folder.trim();
        if folder.is_empty() || !Path::new(folder).is_dir() {
            return None;
        }
        gw::repo_root(&self.git_for_worktree(), Path::new(folder)).await.ok().flatten()
    }

    /// 作業フォルダに対する委任の既定値（記憶した環境ID・現在のブランチ）。読取りのみ。
    pub async fn cloud_env_hint(self: &Arc<Self>, args: CloudEnvArgs) -> Result<CloudEnvHint, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        let Some(root) = self.cloud_repo_root(&args.folder).await else { return Ok(CloudEnvHint { repo_key: None, env_id: None, branch: None }) };
        let git = self.git_for_worktree();
        let branch = git
            .run(Path::new(&root), &GitOp::CurrentBranch)
            .await
            .ok()
            .filter(|o| o.success())
            .map(|o| o.stdout_text().trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD");
        let env_id = self.read(|d| d.settings.cloud_env_by_repo.get(&root).cloned());
        Ok(CloudEnvHint { repo_key: Some(root), env_id, branch })
    }

    /// 委任を送る（確認画面の後のユーザー操作だけ）。送る前に記録を保存し、保存できなければ送らない。再送しない。
    pub async fn submit_cloud_task(self: &Arc<Self>, args: CloudSubmitArgs, confirmed: &UserConfirmed) -> Result<CloudTaskRecord, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        self.precheck_space()?;
        let prompt = args.prompt.trim().to_string();
        let env_id = args.env_id.trim().to_string();
        let branch = args.branch.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if prompt.is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼文が必要です"));
        }
        if env_id.is_empty() || env_id.starts_with('-') || env_id.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(err(IpcErrorCode::InvalidArgs, "環境IDが正しくありません（空白を含まず、空でない値）"));
        }
        let folder = args.folder.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let repo_key = match &folder {
            Some(f) => self.cloud_repo_root(f).await,
            None => None,
        };
        let _op = self.cloud_rt.lock.lock().await;
        let rec = CloudTaskRecord {
            id: self.local_id("cloud"),
            state: CloudTaskState::Submitting,
            since: now_ms(),
            origin_chat: args.origin_chat,
            env_id: env_id.clone(),
            branch: branch.clone(),
            prompt_head: prompt.chars().take(PROMPT_HEAD_CHARS).collect(),
            folder,
        };
        // 送る前に保存する（保存できなければ送らない）。
        self.put_cloud_task(rec.clone());
        if let Err(message) = self.save_cloud_tasks().await {
            self.drop_cloud_task(&rec.id);
            return Err(err(IpcErrorCode::Io, format!("実行前の記録を保存できなかったため、送っていません（{message}）")));
        }
        if let Some(key) = repo_key {
            // 環境IDの記憶は送ったときだけ更新する。
            self.mutate(|d| {
                d.settings.cloud_env_by_repo.insert(key, env_id.clone());
                let settings = d.settings.clone();
                ((), vec![HostEvent::SettingsUpdated { settings }])
            });
            self.schedule_save(SaveScope::AppSettings, Duration::ZERO);
        }
        let state = match self.backend.cloud_submit(CloudSubmit { prompt, env_id, branch }, confirmed).await {
            Ok(outcome) => outcome_state(outcome),
            Err(e) => failure_state(&e),
        };
        let done = CloudTaskRecord { state, ..rec };
        self.put_cloud_task(done.clone());
        if let Err(message) = self.save_cloud_tasks().await {
            self.warn(format!("クラウド委任の結果を保存できませんでした（{message}）。再起動後は「送信の途中」として扱われ、再送はされません"));
        }
        Ok(done)
    }

    /// クラウド側のタスク一覧（読取り。パネルを開いたときとユーザーの「更新」だけ）。
    pub async fn list_cloud_tasks(self: &Arc<Self>) -> Result<CloudTaskList, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        Ok(self.backend.cloud_list().await?)
    }

    /// タスク1件の状態（読取り。原文の先頭を表示する）。
    pub async fn cloud_task_status(self: &Arc<Self>, args: CloudTaskIdArgs) -> Result<CloudCommandOutput, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        let id = checked_task_id(&args.task_id)?;
        Ok(self.backend.cloud_status(id).await?)
    }

    /// タスクの差分（読取り。表示するだけでローカルは変えない）。
    pub async fn cloud_task_diff(self: &Arc<Self>, args: CloudTaskIdArgs) -> Result<CloudCommandOutput, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        let id = checked_task_id(&args.task_id)?;
        Ok(self.backend.cloud_diff(id).await?)
    }

    /// 取込みを止める条件。取込み先のフォルダ（と重なる作業フォルダ）で、作業中・停止未確認・送信受理不明のチャットがある。
    fn cloud_apply_blocker(&self, folders: &[&str]) -> Option<IpcError> {
        let overlapping = self.read(|d| d.chats.iter().filter(|c| c.cwd.value().is_some_and(|w| folders.iter().any(|f| paths_overlap(w, f)))).map(|c| c.key.clone()).collect::<Vec<_>>());
        let busy = self.read(|d| {
            overlapping.iter().any(|k| {
                d.agents.iter().any(|v| &v.agent.chat == k && d.running_turn.contains_key(&v.agent.key)) || d.open_stop(k).is_some() || d.locals.get(k).is_some_and(|l| !l.pending_ops.is_empty())
            })
        });
        if busy {
            return Some(blocked(BlockedReason::ChatBusy, "取込み先の作業フォルダで作業中（または停止未確認）のチャットがあるため、取り込めません。完了または停止の確認後に実行してください"));
        }
        if overlapping.iter().any(|k| self.unknown_attempt(k).is_some()) {
            return Some(blocked(BlockedReason::ChatBusy, "取込み先の作業フォルダのチャットに、送信の受理が未確認のものがあるため、取り込めません"));
        }
        None
    }

    /// 取込み（`cloud apply`）。作業フォルダのファイルを変える操作で、確認画面の後のユーザー操作だけが呼ぶ。
    /// 取込み先はGitのリポジトリのルート。前後の `git status` の差と終了コードを返す（成功を断定しない）。
    pub async fn apply_cloud_task(self: &Arc<Self>, args: CloudApplyArgs, confirmed: &UserConfirmed) -> Result<CloudApplyResult, IpcError> {
        self.require_op(ParityOp::CloudDelegation)?;
        let id = checked_task_id(&args.task_id)?;
        let folder = args.folder.trim().to_string();
        if folder.is_empty() || !Path::new(&folder).is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, format!("取込み先のフォルダが見つかりません: {folder}")));
        }
        let _op = self.cloud_rt.lock.lock().await;
        let git = self.git_for_worktree();
        let root = match gw::repo_root(&git, Path::new(&folder)).await.map_err(git_run_err)? {
            Some(r) => r,
            None => return Err(blocked(BlockedReason::NotARepository, "取込み先はGitのリポジトリではないため、取り込めません（変更を照合できません）")),
        };
        // 実行中は、重なる作業フォルダのチャットのキューを保留する（先に印を付けてから判定する）。
        let _folder_op = self.begin_folder_op(vec![folder.clone(), root.clone()]);
        if let Some(e) = self.cloud_apply_blocker(&[folder.as_str(), root.as_str()]) {
            return Err(e);
        }
        let root_path = PathBuf::from(&root);
        let status_of = |out: Result<crate::gitops::GitOutput, crate::gitops::GitError>| -> Option<Vec<StatusEntry>> {
            out.ok().filter(|o| o.success()).map(|o| parse_status_v2(&String::from_utf8_lossy(&o.stdout)))
        };
        // 実行前の状態を読めなければ、結果を照合できないので取り込まない。
        let Some(before) = status_of(git.run(&root_path, &GitOp::Status).await) else {
            return Err(err(IpcErrorCode::Io, "取込み前のGitの状態を読めなかったため、取り込んでいません"));
        };
        let out = self.backend.cloud_apply(id, root.clone(), confirmed).await?;
        let after = status_of(git.run(&root_path, &GitOp::Status).await);
        let changed_paths = match after {
            Some(after) => Known::direct(status_delta(&before, &after)),
            None => Known::NotFetched,
        };
        Ok(CloudApplyResult { status: out.status, output_head: out.text, changed_paths, folder: root })
    }
}

/// タスクIDの確認（空・空白・先頭`-`は受け付けない。形の細部はバックエンドが確認する）。
fn checked_task_id(id: &str) -> Result<String, IpcError> {
    let id = id.trim();
    if id.is_empty() || id.starts_with('-') || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(err(IpcErrorCode::InvalidArgs, "タスクIDが正しくありません"));
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(state: CloudTaskState) -> CloudTaskRecord {
        CloudTaskRecord { id: LocalId("c1".into()), state, since: UnixMillis(1), origin_chat: None, env_id: "e".into(), branch: None, prompt_head: "p".into(), folder: None }
    }

    #[test]
    fn submit_outcomes_map_to_task_states_without_resend_semantics() {
        assert_eq!(outcome_state(CloudSubmitOutcome::Submitted { task_id: "t".into() }), CloudTaskState::Submitted { task_id: "t".into() });
        assert!(matches!(outcome_state(CloudSubmitOutcome::Rejected { message: "x".into() }), CloudTaskState::Rejected { .. }));
        assert!(matches!(outcome_state(CloudSubmitOutcome::Unknown { message: "x".into() }), CloudTaskState::Unknown { .. }));
        // 送る前に断られたものは Rejected、受理不明（時間切れ）だけ Unknown。
        assert!(matches!(failure_state(&BackendError::NotConnected), CloudTaskState::Rejected { .. }));
        assert!(matches!(failure_state(&BackendError::Unsupported { capability: "c".into() }), CloudTaskState::Rejected { .. }));
        assert!(matches!(failure_state(&BackendError::OutcomeUnknown { message: "t".into() }), CloudTaskState::Unknown { .. }));
    }

    #[test]
    fn submitting_left_after_restart_becomes_unknown_and_others_are_kept() {
        let out = restored_tasks(vec![rec(CloudTaskState::Submitting), rec(CloudTaskState::Submitted { task_id: "t".into() }), rec(CloudTaskState::Rejected { message: "m".into() })]);
        assert!(matches!(out[0].state, CloudTaskState::Unknown { .. }));
        assert_eq!(out[1].state, CloudTaskState::Submitted { task_id: "t".into() });
        assert!(matches!(out[2].state, CloudTaskState::Rejected { .. }));
    }

    #[test]
    fn overlap_is_decided_on_path_boundaries() {
        assert!(paths_overlap("C:/repo", "c:\\repo"));
        assert!(paths_overlap("C:\\repo", "C:\\repo\\sub"));
        assert!(paths_overlap("C:\\repo\\sub", "C:\\repo"));
        assert!(!paths_overlap("C:\\repo", "C:\\repo2"));
        assert!(!paths_overlap("", "C:\\repo"));
    }

    #[test]
    fn status_delta_lists_only_paths_that_changed_state() {
        let e = |p: &str, y: char| StatusEntry { path: p.into(), old_path: None, x: '.', y, untracked: false, unmerged: false };
        let before = vec![e("a.rs", 'M'), e("b.rs", 'M')];
        let after = vec![e("a.rs", 'M'), e("b.rs", 'D'), e("c.rs", 'A')];
        assert_eq!(status_delta(&before, &after), vec!["b.rs".to_string(), "c.rs".to_string()]);
        assert!(status_delta(&after, &after).is_empty());
    }

    #[test]
    fn task_id_check_rejects_option_like_and_blank() {
        assert!(checked_task_id(" task_1 ").is_ok());
        assert!(checked_task_id("").is_err());
        assert!(checked_task_id("-x").is_err());
        assert!(checked_task_id("a b").is_err());
    }
}
