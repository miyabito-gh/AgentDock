//! 組織実行のIPC入口（P-ORG-7a）。Tauriコマンド（`commands.rs`）が呼ぶ `Host` のメソッド。
//!
//! 守ること:
//! - **閲覧・選択だけでは何も送らない**。`org_view`／`org_roles_view`／`org_recover_candidates` は読取りだけで、司令taskへ何も渡さない。
//! - 状態を動かすメソッド（承認・再計画・取消・手動完了・turn上限・チャットの結び付け）は `&UserConfirmed` を要する。作るのはコマンド層だけ。
//!   承認・再計画は送信権を作らない（送れるかは、開始・再開の操作で作った送信権の有無で決まる）。
//! - 画面が見ていたワークフローと違うものへの操作は受けない（`org_check_workflow`）。
//! - 承認要求への応答・resume・中断を行わない。再送の入力はエンジンにない（受理されなかった送信は「再開」で新しい試行として送る）。

use std::sync::Arc;

use org_engine::{OrgInput, OrgStore, TaskId, TaskStatus, WorkflowId};
use tokio::sync::mpsc::UnboundedSender;

use super::driver::DriverMsg;
use super::ports::RuntimeRead;
use super::view::{self, ViewInput, STALL_MINUTES_MAX, STALL_MINUTES_MIN};
use super::{OrgResumeReport, OrgStartArgs};
use crate::backend::backend::UserConfirmed;
use crate::backend::ipc::{IpcError, IpcErrorCode};
use crate::backend::local::SaveScope;
use crate::backend::model::{ChatKey, SaveState};
use crate::backend::org::*;
use crate::host::{err, Host};
use crate::rules::org::{chat_key_of_ref, chat_ref_of};
use crate::store::org::{workflow_dir, FileOrgStore};
use crate::store::records::OrgBinding;

/// 停滞時間の指定を検査する（分）。
pub fn validate_stall_minutes(minutes: u32) -> Result<u32, IpcError> {
    if (STALL_MINUTES_MIN..=STALL_MINUTES_MAX).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(err(IpcErrorCode::InvalidArgs, format!("停滞とみなす時間は {STALL_MINUTES_MIN}〜{STALL_MINUTES_MAX} 分で指定してください")))
    }
}

/// 候補のチャットを選べるか。AgentDockが作った（hosted）チャットで、組織の記録がまだないものだけ。
pub fn classify_candidate(local: Option<(bool, bool)>) -> (bool, Option<String>) {
    match local {
        None => (false, Some("AgentDock の記録がないチャットです（結び付けられません）".to_string())),
        Some((false, _)) => (false, Some("AgentDock が作ったチャットではありません".to_string())),
        Some((true, true)) => (false, Some("すでに別の組織タスクに結び付いています".to_string())),
        Some((true, false)) => (true, None),
    }
}

impl Host {
    // ───────────── 読取り（何も送らない） ─────────────

    /// 現在の組織実行の写し。読取りだけで、司令taskへ何も渡さない。
    pub fn org_view(&self) -> OrgSnapshotView {
        let stall_minutes = self.read(|d| d.settings.org_stall_minutes);
        let Some(snapshot) = self.org_snapshot() else { return OrgSnapshotView { workflow: None, stall_minutes } };
        let notices = self.org_rt.notice_entries();
        let usage_rows = self
            .persist
            .store()
            .map(|s| s.root().to_path_buf())
            .and_then(|root| FileOrgStore::new(workflow_dir(&root, &snapshot.workflow.0)).read_usage().ok())
            .unwrap_or_default();
        let dispatching = self.org_rt.grants.is_active(&snapshot.workflow);
        let workflow = view::workflow_view(&ViewInput { snapshot: &snapshot, notices: &notices, usage_rows: &usage_rows, dispatching });
        OrgSnapshotView { workflow: Some(workflow), stall_minutes }
    }

    /// 直近に読んだ役割定義（読込み前は `loaded=false`）。
    pub fn org_roles_view(&self) -> OrgRolesView {
        view::roles_view(self.org_roles_report().as_ref())
    }

    /// 画面が見ていたワークフローが、動いているものと同じか確かめる。
    pub fn org_check_workflow(&self, workflow_id: &str) -> Result<(WorkflowId, UnboundedSender<DriverMsg>), IpcError> {
        let guard = self.org_rt.driver.lock().unwrap();
        let Some(h) = guard.as_ref() else { return Err(err(IpcErrorCode::NotFound, "動いている組織実行がありません")) };
        if h.workflow.0 != workflow_id {
            return Err(err(IpcErrorCode::NotFound, "別の組織実行です（画面を更新してください）"));
        }
        Ok((h.workflow.clone(), h.tx.clone()))
    }

    fn org_task_of_view(&self, task: &str) -> Result<super::driver::TaskSnapshot, IpcError> {
        let view = self.org_rt.view.lock().unwrap();
        view.as_ref()
            .and_then(|v| v.tasks.iter().find(|t| t.id.0 == task).cloned())
            .ok_or_else(|| err(IpcErrorCode::NotFound, format!("タスク {task} が見つかりません")))
    }

    // ───────────── 利用者の操作（コマンド層が `UserConfirmed` を作る） ─────────────

    /// 開始の引数を検査して `org_start` へ渡す。並列数の範囲外は拒否する。
    pub async fn org_start_checked(self: &Arc<Self>, args: StartOrgArgs, confirmed: UserConfirmed) -> Result<StartOrgResult, IpcError> {
        let parallelism = match (args.max_writers, args.max_total) {
            (None, None) => None,
            (w, t) => {
                let d = org_engine::Limits::default();
                let (w, t) = (w.unwrap_or(d.max_writers), t.unwrap_or(d.max_total));
                d.with_parallelism(w, t).map_err(|_| err(IpcErrorCode::InvalidArgs, "並列数は 1〜4 で指定してください"))?;
                Some((w, t))
            }
        };
        let wf = self.org_start(OrgStartArgs { request: args.request, repo: args.repo, parallelism }, confirmed).await?;
        Ok(StartOrgResult { workflow_id: wf.0 })
    }

    /// 再開（必要なチャットだけを resume し、新しい送信権を作る）。画面が見ていたワークフローと同じときだけ。
    pub async fn org_resume_checked(self: &Arc<Self>, args: OrgWorkflowArgs, confirmed: UserConfirmed) -> Result<OrgResumeResult, IpcError> {
        self.org_check_workflow(&args.workflow_id)?;
        let report: OrgResumeReport = self.org_resume(confirmed).await?;
        Ok(OrgResumeResult {
            resumed: report.resumed,
            failed: report.failed.into_iter().map(|(chat, message)| OrgResumeFailure { chat, message }).collect(),
        })
    }

    /// 一時停止（送信権を失効させるだけ。実行中の turn は止めない）。
    pub fn org_pause_checked(&self, args: OrgWorkflowArgs) -> Result<(), IpcError> {
        self.org_check_workflow(&args.workflow_id)?;
        self.org_pause();
        Ok(())
    }

    /// 計画を承認する。計画の承認待ちのときだけ。送信権は作らない（なければ「再開」が要る）。
    pub fn org_approve_plan(&self, args: OrgWorkflowArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        self.require_plan_pending()?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserApprovePlan));
        Ok(())
    }

    /// 差し戻して再計画する。計画の承認待ちのときだけ（エンジンが、作業が始まっていれば拒否する）。
    pub fn org_replan(&self, args: OrgWorkflowArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        self.require_plan_pending()?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserReplan));
        Ok(())
    }

    fn require_plan_pending(&self) -> Result<(), IpcError> {
        let pending = self.org_rt.view.lock().unwrap().as_ref().is_some_and(|v| v.halt == Some(org_engine::HaltReason::AwaitingPlanApproval));
        if pending {
            Ok(())
        } else {
            Err(err(IpcErrorCode::InvalidArgs, "計画の承認待ちではありません"))
        }
    }

    /// 取消し。利用者の操作だけが `cancelled` にする。実行中だったものは Codex 側の停止を確認できたとは扱わない（通知で案内）。
    pub fn org_cancel(&self, args: OrgCancelArgs, _confirmed: &UserConfirmed) -> Result<Vec<String>, IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let targets: Vec<TaskId> = match &args.scope {
            OrgCancelScope::Task { task } => {
                let t = self.org_task_of_view(task)?;
                if matches!(t.status, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled) {
                    return Err(err(IpcErrorCode::InvalidArgs, format!("タスク {task} はすでに終わっています")));
                }
                vec![t.id]
            }
            OrgCancelScope::Workflow => self
                .org_rt
                .view
                .lock()
                .unwrap()
                .as_ref()
                .map(|v| v.tasks.iter().filter(|t| !matches!(t.status, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)).map(|t| t.id.clone()).collect())
                .unwrap_or_default(),
        };
        for t in &targets {
            let _ = tx.send(DriverMsg::Input(OrgInput::UserCancelTask { task: t.clone() }));
        }
        Ok(targets.into_iter().map(|t| t.0).collect())
    }

    /// 手動に切り替えたタスクを、利用者が完了にする。
    pub fn org_accept_manual(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        if t.status != TaskStatus::Blocked(org_engine::BlockedReason::Manual) || t.confirm != org_engine::ConfirmState::Confirmed {
            return Err(err(IpcErrorCode::InvalidArgs, "手動に切り替えたタスクだけを完了にできます"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserAcceptManual { task: t.id }));
        Ok(())
    }

    /// turn 上限を上げる（今の値より大きいときだけ）。
    pub fn org_raise_turn_limit(&self, args: OrgRaiseTurnLimitArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let current = self.org_rt.view.lock().unwrap().as_ref().map(|v| v.limits.max_turns);
        if current.is_some_and(|c| args.to <= c) {
            return Err(err(IpcErrorCode::InvalidArgs, "turn 上限は今の値より大きくしてください"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserRaiseTurnLimit { to: args.to }));
        Ok(())
    }

    // ───────────── 作成結果が不明なタスクのチャット選択 ─────────────

    /// 結び付け先の候補（作業フォルダが同じチャット。読取りだけ。**自動では結び付けない**）。
    pub async fn org_recover_candidates(self: &Arc<Self>, args: OrgTaskArgs) -> Result<Vec<OrgChatCandidate>, IpcError> {
        self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let Some(cwd) = t.chat_cwd.clone().filter(|_| t.chat.is_none()) else {
            return Err(err(IpcErrorCode::InvalidArgs, "このタスクは、チャットの作成結果が不明な状態ではありません"));
        };
        let keys = self.list_by_cwd(&cwd).await.map_err(IpcError::from)?;
        Ok(keys.into_iter().map(|chat| self.candidate_of(chat)).collect())
    }

    fn candidate_of(&self, chat: ChatKey) -> OrgChatCandidate {
        let (local, name) = self.read(|d| {
            (
                d.locals.get(&chat).map(|l| (l.hosted, l.org.is_some())),
                d.chat(&chat).and_then(|c| c.name.value().cloned()),
            )
        });
        let (selectable, why_not) = classify_candidate(local);
        OrgChatCandidate { chat, name, selectable, why_not }
    }

    /// 利用者が選んだチャットを、作成結果が不明だったタスクに結び付ける。所有の記録を保存できたときだけ結び付ける。
    /// 候補は選んだ時点で読み直して確かめる。送信・resume はしない。
    pub async fn org_choose_chat(self: &Arc<Self>, args: OrgChooseChatArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (workflow, tx) = self.org_check_workflow(&args.workflow_id)?;
        let candidates = self.org_recover_candidates(OrgTaskArgs { workflow_id: args.workflow_id.clone(), task: args.task.clone() }).await?;
        let Some(c) = candidates.iter().find(|c| c.chat == args.chat) else {
            return Err(err(IpcErrorCode::InvalidArgs, "候補にないチャットです"));
        };
        if !c.selectable {
            return Err(err(IpcErrorCode::InvalidArgs, c.why_not.clone().unwrap_or_else(|| "このチャットは選べません".to_string())));
        }
        let t = self.org_task_of_view(&args.task)?;
        let binding = OrgBinding { workflow_id: workflow.0.clone(), task_id: t.id.0.clone(), role_id: t.role.clone(), released_at: None };
        let chat = args.chat.clone();
        self.update_local(&chat, true, std::time::Duration::ZERO, |f| f.org = Some(binding.clone()));
        let saved = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await;
        if !saved.is_some_and(|s| matches!(s.state, SaveState::Saved { .. })) {
            self.update_local(&chat, true, std::time::Duration::ZERO, |f| f.org = None);
            return Err(err(IpcErrorCode::Io, "所有の記録を保存できなかったため、結び付けていません"));
        }
        self.org_rt.observer.lock().unwrap().bind(chat.clone(), workflow, t.id.clone());
        let _ = tx.send(DriverMsg::Input(OrgInput::ChatRecovered { task: t.id, chat: chat_ref_of(&chat) }));
        Ok(())
    }

    // ───────────── 停滞時間の設定 ─────────────

    /// 停滞とみなす時間（分）を保存して、動いている司令taskへ反映する。通知の条件だけで、送信・状態は変えない。
    pub fn org_set_stall_minutes(self: &Arc<Self>, args: OrgSetStallArgs) -> Result<u32, IpcError> {
        let minutes = validate_stall_minutes(args.minutes)?;
        self.update_settings(|s| s.org_stall_minutes = minutes)?;
        self.org_apply_stall_setting();
        Ok(minutes)
    }

    /// 保存済みの停滞時間を司令taskへ渡す（開始・復元・設定の保存のとき）。
    pub(in crate::host) fn org_apply_stall_setting(&self) {
        let minutes = self.read(|d| d.settings.org_stall_minutes).clamp(STALL_MINUTES_MIN, STALL_MINUTES_MAX);
        self.org_rt.send_all(vec![DriverMsg::SetStallThreshold(i64::from(minutes) * 60_000)]);
    }

    /// 役割ファイルの絶対パス（開く操作の対象）。読み込んだ役割か問題として報告したファイル名だけを受ける。
    pub fn org_role_file_path(&self, args: &OrgOpenRoleFileArgs) -> Result<std::path::PathBuf, IpcError> {
        if !view::is_plain_role_file_name(&args.file) {
            return Err(err(IpcErrorCode::InvalidArgs, "役割ファイルの名前が正しくありません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、役割ファイルを開けません"));
        };
        let path = super::roles::roles_dir(&root).join(&args.file);
        if !path.is_file() {
            return Err(err(IpcErrorCode::NotFound, format!("役割ファイルが見つかりません: {}", args.file)));
        }
        Ok(path)
    }

    /// 役割の有効化（権限を広げる定義）。確認画面に出した指紋が今の権限と一致するときだけ受ける。
    pub fn org_enable_role_checked(&self, args: OrgEnableRoleArgs, confirmed: UserConfirmed) -> Result<OrgRolesView, IpcError> {
        let current = self
            .org_roles_report()
            .and_then(|r| r.roles.iter().find(|x| x.def.id == args.role_id).map(|x| org_engine::permission_fingerprint(&x.def)));
        match current {
            None => return Err(err(IpcErrorCode::NotFound, format!("役割「{}」は読み込まれていません", args.role_id))),
            Some(fp) if fp != args.fingerprint => {
                return Err(err(IpcErrorCode::Blocked, "確認した後に役割の権限が変わりました。もう一度、内容を確認してください"));
            }
            Some(_) => {}
        }
        let report = self.org_enable_role(&args.role_id, confirmed)?;
        Ok(view::roles_view(Some(&report)))
    }

    /// 読み直した役割定義（動いているワークフローがあれば拒否）。
    pub fn org_reload_roles_view(&self) -> Result<OrgRolesView, IpcError> {
        let report = self.org_reload_roles()?;
        Ok(view::roles_view(Some(&report)))
    }

    /// 手動に切り替える（保存できたときだけ解放する）。
    pub async fn org_release_chat_checked(self: &Arc<Self>, chat: &ChatKey, confirmed: &UserConfirmed) -> Result<(), IpcError> {
        self.org_release_chat(chat, confirmed).await
    }

    #[allow(dead_code)]
    fn org_key_of(task: &super::driver::TaskSnapshot) -> Option<ChatKey> {
        task.chat.as_ref().and_then(chat_key_of_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{BlockedReason as EBlocked, ConfirmState, HaltReason, Limits};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    use super::super::driver::{OrgSnapshot, TaskSnapshot};
    use super::super::DriverHandle;

    fn task(id: &str, status: TaskStatus) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId::new(id),
            role: "worker".into(),
            status,
            confirm: ConfirmState::Confirmed,
            chat: None,
            title: String::new(),
            deps: Vec::new(),
            effects: Vec::new(),
            attempts: 1,
            commit: None,
            chat_cwd: None,
        }
    }

    fn host_with_workflow(halt: Option<HaltReason>, tasks: Vec<TaskSnapshot>) -> (Arc<Host>, UnboundedReceiver<DriverMsg>) {
        let host = Arc::new(Host::new(std::env::temp_dir()));
        let (tx, rx) = unbounded_channel();
        *host.org_rt.driver.lock().unwrap() = Some(DriverHandle { workflow: WorkflowId::new("wf-1"), tx });
        *host.org_rt.view.lock().unwrap() = Some(OrgSnapshot {
            workflow: WorkflowId::new("wf-1"),
            title: None,
            halt,
            tasks,
            request: "r".into(),
            repo: String::new(),
            limits: Limits::default(),
            turns_used: 0,
            auto_replans_used: 0,
        });
        (host, rx)
    }

    fn wf(id: &str) -> OrgWorkflowArgs {
        OrgWorkflowArgs { workflow_id: id.into() }
    }

    fn drain(rx: &mut UnboundedReceiver<DriverMsg>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(match m {
                DriverMsg::Input(i) => format!("{i:?}"),
                DriverMsg::SetStallThreshold(ms) => format!("stall:{ms}"),
                _ => "other".to_string(),
            });
        }
        out
    }

    #[test]
    fn viewing_and_selecting_send_nothing_to_the_engine() {
        let (host, mut rx) = host_with_workflow(Some(HaltReason::AwaitingPlanApproval), vec![task("a", TaskStatus::Pending)]);
        let v = host.org_view();
        assert_eq!(v.workflow.as_ref().map(|w| w.workflow_id.as_str()), Some("wf-1"));
        assert!(!v.workflow.unwrap().dispatching, "viewing does not create a send grant");
        let _ = host.org_roles_view();
        let _ = host.org_check_workflow("wf-1").unwrap();
        assert!(drain(&mut rx).is_empty(), "read-only commands must not feed the driver");
    }

    #[test]
    fn nothing_is_shown_when_no_workflow_exists() {
        let host = Arc::new(Host::new(std::env::temp_dir()));
        let v = host.org_view();
        assert!(v.workflow.is_none());
        assert_eq!(v.stall_minutes, 30);
        assert!(!host.org_roles_view().loaded);
        assert_eq!(host.org_check_workflow("wf-1").unwrap_err().code, IpcErrorCode::NotFound);
    }

    #[test]
    fn approving_the_plan_requires_a_pending_plan_and_the_same_workflow() {
        let c = UserConfirmed::from_user_command();
        let (host, mut rx) = host_with_workflow(Some(HaltReason::User), vec![task("a", TaskStatus::Pending)]);
        assert_eq!(host.org_approve_plan(wf("wf-1"), &c).unwrap_err().code, IpcErrorCode::InvalidArgs, "not waiting for a plan");
        assert!(drain(&mut rx).is_empty());

        let (host, mut rx) = host_with_workflow(Some(HaltReason::AwaitingPlanApproval), vec![task("a", TaskStatus::Pending)]);
        assert_eq!(host.org_approve_plan(wf("other"), &c).unwrap_err().code, IpcErrorCode::NotFound, "stale screen");
        assert!(drain(&mut rx).is_empty());
        host.org_approve_plan(wf("wf-1"), &c).unwrap();
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("UserApprovePlan"));
        // 承認は送信権を作らない。
        assert!(!host.org_rt.grants.is_active(&WorkflowId::new("wf-1")));
    }

    #[test]
    fn cancel_is_explicit_and_skips_finished_tasks() {
        let c = UserConfirmed::from_user_command();
        let (host, mut rx) = host_with_workflow(None, vec![task("a", TaskStatus::Running), task("b", TaskStatus::Completed), task("c", TaskStatus::Pending)]);
        assert!(host.org_cancel(OrgCancelArgs { workflow_id: "wf-1".into(), scope: OrgCancelScope::Task { task: "b".into() } }, &c).is_err());
        assert_eq!(host.org_cancel(OrgCancelArgs { workflow_id: "wf-1".into(), scope: OrgCancelScope::Task { task: "zz".into() } }, &c).unwrap_err().code, IpcErrorCode::NotFound);
        assert!(drain(&mut rx).is_empty());
        let done = host.org_cancel(OrgCancelArgs { workflow_id: "wf-1".into(), scope: OrgCancelScope::Workflow }, &c).unwrap();
        assert_eq!(done, vec!["a".to_string(), "c".to_string()]);
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|s| s.contains("UserCancelTask")));
    }

    #[test]
    fn manual_accept_and_turn_limit_are_checked_before_reaching_the_engine() {
        let c = UserConfirmed::from_user_command();
        let (host, mut rx) = host_with_workflow(None, vec![task("a", TaskStatus::Running), task("m", TaskStatus::Blocked(EBlocked::Manual))]);
        assert!(host.org_accept_manual(OrgTaskArgs { workflow_id: "wf-1".into(), task: "a".into() }, &c).is_err());
        assert!(host.org_raise_turn_limit(OrgRaiseTurnLimitArgs { workflow_id: "wf-1".into(), to: 30 }, &c).is_err(), "not larger than now");
        assert!(drain(&mut rx).is_empty());
        host.org_accept_manual(OrgTaskArgs { workflow_id: "wf-1".into(), task: "m".into() }, &c).unwrap();
        host.org_raise_turn_limit(OrgRaiseTurnLimitArgs { workflow_id: "wf-1".into(), to: 50 }, &c).unwrap();
        let sent = drain(&mut rx);
        assert!(sent[0].contains("UserAcceptManual") && sent[1].contains("UserRaiseTurnLimit"));
    }

    #[test]
    fn pausing_revokes_and_does_not_send() {
        let (host, mut rx) = host_with_workflow(None, vec![task("a", TaskStatus::Running)]);
        assert_eq!(host.org_pause_checked(wf("other")).unwrap_err().code, IpcErrorCode::NotFound);
        assert!(drain(&mut rx).is_empty());
        host.org_pause_checked(wf("wf-1")).unwrap();
        let sent = drain(&mut rx);
        assert!(sent.iter().any(|s| s == "other"), "revoke goes to the driver");
        assert!(sent.iter().any(|s| s.contains("UserPause")));
    }

    #[test]
    fn stall_setting_is_range_checked_and_reaches_the_driver() {
        assert!(validate_stall_minutes(0).is_err() && validate_stall_minutes(1441).is_err());
        assert_eq!(validate_stall_minutes(45).unwrap(), 45);
        let (host, mut rx) = host_with_workflow(None, vec![]);
        host.mutate(|d| {
            d.settings.org_stall_minutes = 45;
            ((), vec![])
        });
        host.org_apply_stall_setting();
        assert_eq!(drain(&mut rx), vec![format!("stall:{}", 45 * 60_000)]);
    }

    #[test]
    fn only_agentdock_chats_without_an_org_record_can_be_chosen() {
        assert!(classify_candidate(Some((true, false))).0);
        for (l, _) in [(None, ()), (Some((false, false)), ()), (Some((true, true)), ())] {
            let (ok, why) = classify_candidate(l);
            assert!(!ok && why.is_some());
        }
    }

    #[test]
    fn role_files_outside_the_roles_folder_are_refused() {
        let host = Arc::new(Host::new(std::env::temp_dir()));
        for bad in ["../x.json", "a\\b.json", "worker"] {
            assert_eq!(host.org_role_file_path(&OrgOpenRoleFileArgs { file: bad.into() }).unwrap_err().code, IpcErrorCode::InvalidArgs);
        }
    }
}
