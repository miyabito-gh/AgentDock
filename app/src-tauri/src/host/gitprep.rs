//! Gitの準備とアプリからのプッシュ（DESIGN_ORG_INTAKE §2・§3）のホスト側。Tauriコマンド（`commands.rs`）が呼ぶ。
//!
//! 守ること:
//! - **閲覧・プレビューでは何も書かない・送らない**（`git_repo_status`／`git_prep_preview`／`git_push_preview`／`git_push_verify`）。
//! - 準備・プッシュ・途中の準備の取り除きは `&UserConfirmed`（コマンド層が作る確認ダイアログ後の操作）を要する。自動では呼ばない。
//! - 同じフォルダへの準備・プッシュは同時に1本だけ。再送しない（結果が分からなければ「確認できません」）。
//! - 実行の記録は `git-ops.jsonl`（種類・フォルダ・コミット・除外・結果）。URLの資格情報は含めない。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{blocked, err, now_ms, Host};
use crate::backend::backend::UserConfirmed;
use crate::backend::git_ops::*;
use crate::backend::ipc::{BlockedReason, IpcError, IpcErrorCode};
use crate::gitops::{prep, push};
use crate::rules::git_prep::folder_verdict;
use crate::rules::work_folders::{check_folder, os_probe, FolderEnv};

/// 実行中のフォルダ（二重実行の防止）。
#[derive(Default)]
pub struct GitPrepRuntime {
    busy: Mutex<HashSet<String>>,
}

/// 実行中の印。手放すと外れる。
pub(in crate::host) struct BusyGuard<'a> {
    rt: &'a GitPrepRuntime,
    key: String,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.rt.busy.lock().unwrap().remove(&self.key);
    }
}

impl GitPrepRuntime {
    /// 同じフォルダのGit操作（準備・プッシュ・組織実行の取り込みと戻す）を1本に絞る印を取る。取れなければ実行中。
    pub(in crate::host) fn begin(&self, folder: &str) -> Option<BusyGuard<'_>> {
        let key = folder.to_lowercase();
        let inserted = self.busy.lock().unwrap().insert(key.clone());
        inserted.then(|| BusyGuard { rt: self, key })
    }
}

/// 記録ファイル名（保存領域の直下）。
pub const GIT_OPS_LOG: &str = "git-ops.jsonl";

pub(in crate::host) fn busy_error() -> IpcError {
    blocked(BlockedReason::ChatBusy, "このフォルダのGit操作が実行中です。終わってからもう一度操作してください")
}

impl Host {
    /// フォルダの指定を検査する（空でなく、実在するフォルダ）。
    fn git_folder(&self, folder: &str) -> Result<PathBuf, IpcError> {
        let f = folder.trim();
        if f.is_empty() || !Path::new(f).is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, format!("フォルダが見つかりません: {folder}")));
        }
        Ok(PathBuf::from(f))
    }

    /// 一時のgitdirを作る場所（AgentDockの保存領域内）。
    fn git_tmp_base(&self) -> Result<PathBuf, IpcError> {
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、Gitの準備を始められません（記録を残せません）"));
        };
        Ok(root.join("tmp"))
    }

    /// フォルダの場所を理由にした断り（ドライブ直下・ユーザーフォルダ直下・Windows・Program Files・ネットワークなど）。
    async fn git_prep_folder_refusal(&self, folder: &str) -> Option<String> {
        let f = folder.to_string();
        let verdict = tokio::task::spawn_blocking(move || folder_verdict(&check_folder(&f, &[], &FolderEnv::from_os(), &os_probe))).await;
        match verdict {
            Ok(v) => v,
            Err(_) => Some("フォルダを確認できませんでした".to_string()),
        }
    }

    /// 実行の記録（失敗しても操作は止めない）。
    pub(in crate::host) fn log_git_op(&self, record: serde_json::Value) {
        use std::io::Write;
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else { return };
        let mut obj = match record {
            serde_json::Value::Object(o) => o,
            _ => return,
        };
        obj.insert("at".to_string(), serde_json::json!(now_ms().0));
        let line = serde_json::Value::Object(obj).to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(root.join(GIT_OPS_LOG)) {
            let _ = writeln!(f, "{line}");
        }
    }

    // ───────────── 読取り ─────────────

    /// 作業フォルダのGitの状態（読取りのみ）。
    pub async fn git_repo_status(&self, args: GitFolderArgs) -> Result<RepoStatusView, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        Ok(prep::repo_status(&self.git_for_worktree(), &folder).await)
    }

    /// 組織実行を始める前の確認。コミットのあるリポジトリ（上位のリポジトリの一部も可）であること。満たさなければ、準備ダイアログへ誘導できる理由で止める。
    pub(in crate::host) async fn precheck_repo_ready(&self, folder: &str) -> Result<(), IpcError> {
        let status = prep::repo_status(&self.git_for_worktree(), Path::new(folder)).await;
        repo_ready_error(status)
    }

    /// 準備のプレビュー（読取りのみ。一時のgitdirはAgentDockの領域に作って消す）。
    pub async fn git_prep_preview(&self, args: GitPrepArgs) -> Result<RepoPrepPreview, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        let tmp = self.git_tmp_base()?;
        let refusal = self.git_prep_folder_refusal(&args.folder).await;
        prep::preview(&self.git_for_worktree(), &folder, &tmp, refusal, args.apply_excludes).await.map_err(|m| err(IpcErrorCode::InvalidArgs, m))
    }

    /// プッシュ前の確認（読取りのみ。ネットワークは使わない）。
    pub async fn git_push_preview(&self, args: PushPreviewArgs) -> Result<PushPreview, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        Ok(push::preview(&self.git_for_worktree(), &folder, args.remote.as_deref()).await)
    }

    /// 結果が分からなかったプッシュの確認（`ls-remote`。読取りのみ）。
    pub async fn git_push_verify(&self, args: GitPushVerifyArgs) -> Result<PushVerify, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        Ok(push::verify(&self.git_for_worktree(), &folder, &args.remote, &args.branch, &args.expected_sha).await)
    }

    // ───────────── 利用者の操作（確認ダイアログの後） ─────────────

    /// 準備を実行する。`.git` だけを作る。再検査（状態・指紋）が違えば何もしない。
    pub async fn git_prepare_repo(&self, args: GitPrepareArgs, _confirmed: &UserConfirmed) -> Result<RepoPrepResult, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        let tmp = self.git_tmp_base()?;
        let Some(_guard) = self.gitprep_rt.begin(&args.folder) else { return Err(busy_error()) };
        let refusal = self.git_prep_folder_refusal(&args.folder).await;
        let spec = prep::PrepareSpec {
            folder: &folder,
            tmp_base: &tmp,
            expected_token: &args.preview_token,
            apply_excludes: args.apply_excludes,
            folder_refusal: refusal,
            prep_id: self.local_id("prep").0,
            now_ms: now_ms().0,
        };
        let outcome = prep::prepare(&self.git_for_worktree(), spec).await;
        self.log_git_op(serde_json::json!({
            "kind": "prepare",
            "folder": args.folder,
            "applyExcludes": args.apply_excludes,
            "outcome": &outcome,
        }));
        Ok(RepoPrepResult { outcome })
    }

    /// 前回の途中の準備（印があり、コミットがない `.git`）を取り除く。
    pub async fn git_prep_remove_interrupted(&self, args: GitFolderArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let folder = self.git_folder(&args.folder)?;
        let Some(_guard) = self.gitprep_rt.begin(&args.folder) else { return Err(busy_error()) };
        let result = prep::remove_interrupted(&self.git_for_worktree(), &folder).await;
        self.log_git_op(serde_json::json!({ "kind": "removeInterrupted", "folder": args.folder, "ok": result.is_ok(), "error": result.as_ref().err() }));
        result.map_err(|m| err(IpcErrorCode::Rejected, m))
    }

    /// プッシュを実行する。確認時のコミット（`expectedSha`）だけを送る。結果が分からなければ `Unknown`（成功にしない）。
    pub async fn git_push(&self, args: GitPushArgs, _confirmed: &UserConfirmed) -> Result<PushResult, IpcError> {
        let folder = self.git_folder(&args.folder)?;
        let Some(_guard) = self.gitprep_rt.begin(&args.folder) else { return Err(busy_error()) };
        let result = push::push(&self.git_for_worktree(), &folder, &args.remote, &args.branch, &args.expected_sha, args.set_upstream).await;
        self.log_git_op(serde_json::json!({
            "kind": "push",
            "folder": args.folder,
            "remote": args.remote,
            "branch": args.branch,
            "sha": args.expected_sha,
            "setUpstream": args.set_upstream,
            "result": result.as_ref().ok(),
            "refused": result.as_ref().err(),
        }));
        result.map_err(|m| err(IpcErrorCode::Rejected, m))
    }
}

/// 組織実行を始められるGitの状態か。始められなければ理由（準備が要るものは `RepoNotPrepared`）。
pub fn repo_ready_error(status: RepoStatus) -> Result<(), IpcError> {
    match status {
        RepoStatus::Ready { .. } | RepoStatus::Nested { .. } => Ok(()),
        RepoStatus::NotRepo => Err(blocked(
            BlockedReason::RepoNotPrepared { repo: RepoStatus::NotRepo },
            "このフォルダはGitのリポジトリではありません。Workerが専用worktreeで作業するには、先に「Gitを準備」でリポジトリを作り、最初のコミットを作る必要があります",
        )),
        s @ RepoStatus::NoCommit { .. } => Err(blocked(
            BlockedReason::RepoNotPrepared { repo: s },
            "このフォルダにはまだコミットがありません。Workerが専用worktreeで作業するには、先に最初のコミットを作る必要があります（「Gitを準備」から作れます）",
        )),
        s @ RepoStatus::PrepInterrupted { .. } => Err(blocked(
            BlockedReason::RepoNotPrepared { repo: s },
            "前回のGitの準備が途中で止まっています。「Gitを準備」から、続きを行うか取り除いてください",
        )),
        RepoStatus::OwnerMismatch => Err(err(
            IpcErrorCode::InvalidArgs,
            "Gitがこのフォルダの所有者の違いを理由に拒否したため、リポジトリを確認できません。Gitの safe.directory を確認してください",
        )),
        RepoStatus::Unavailable { message } => Err(err(IpcErrorCode::InvalidArgs, format!("Gitでリポジトリを確認できません: {message}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_repository_with_a_commit_may_start_and_preparable_states_point_to_the_prep_dialog() {
        assert!(repo_ready_error(RepoStatus::Ready { root: "C:/r".into(), branch: "main".into() }).is_ok());
        assert!(repo_ready_error(RepoStatus::Nested { root: "C:/r".into() }).is_ok());
        for s in [RepoStatus::NotRepo, RepoStatus::NoCommit { root: "C:/r".into() }, RepoStatus::PrepInterrupted { root: "C:/r".into() }] {
            let e = repo_ready_error(s.clone()).unwrap_err();
            assert_eq!(e.code, IpcErrorCode::Blocked);
            assert_eq!(e.blocked, Some(BlockedReason::RepoNotPrepared { repo: s }));
        }
        assert_eq!(repo_ready_error(RepoStatus::OwnerMismatch).unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert!(repo_ready_error(RepoStatus::Unavailable { message: "x".into() }).unwrap_err().message.contains("x"));
    }

    #[test]
    fn the_same_folder_cannot_run_two_git_operations_at_once() {
        let rt = GitPrepRuntime::default();
        let first = rt.begin("C:\\Work\\A").expect("first");
        assert!(rt.begin("c:\\work\\a").is_none(), "case-insensitive on Windows");
        assert!(rt.begin("C:\\Work\\B").is_some());
        drop(first);
        assert!(rt.begin("C:\\Work\\A").is_some());
    }
}
