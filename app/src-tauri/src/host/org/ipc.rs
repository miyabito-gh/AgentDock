//! 組織実行のIPC入口（P-ORG-7a）。Tauriコマンド（`commands.rs`）が呼ぶ `Host` のメソッド。
//!
//! 守ること:
//! - **閲覧・選択だけでは何も送らない**。`org_view`／`org_roles_view`／`org_recover_candidates` は読取りだけで、司令taskへ何も渡さない。
//! - 状態を動かすメソッド（承認・再計画・取消・手動完了・turn上限・チャットの結び付け）は `&UserConfirmed` を要する。作るのはコマンド層だけ。
//!   承認・再計画は送信権を作らない（送れるかは、開始・再開の操作で作った送信権の有無で決まる）。
//! - 画面が見ていたワークフローと違うものへの操作は受けない（`org_check_workflow`）。
//! - 承認要求への応答・resume・中断を行わない。再送の入力はエンジンにない（受理されなかった送信は「再開」で新しい試行として送る）。

use std::collections::HashMap;
use std::sync::Arc;

use org_engine::graph::SlotId;
use org_engine::{OrgInput, OrgStore, TaskId, TaskStatus, WorkflowId};
use tokio::sync::mpsc::UnboundedSender;

use super::driver::{DriverMsg, GraphSnapshot, TaskSnapshot};
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

/// 利用者が書いた理由。空白だけなら書かれていないものとして扱う（None）。
fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
}

/// 理由の長さを検査して整える。エンジンが渡す上限（`NOTE_MAX_CHARS`）を超えるものは、黙って切らずに拒否する。
pub fn checked_note(note: Option<String>) -> Result<Option<String>, IpcError> {
    let note = clean_note(note);
    if note.as_deref().is_some_and(|n| n.chars().count() > org_engine::prompt::NOTE_MAX_CHARS) {
        return Err(err(IpcErrorCode::InvalidArgs, format!("理由は{}文字までにしてください", org_engine::prompt::NOTE_MAX_CHARS)));
    }
    Ok(note)
}

/// gate の判断を受け付けられるか。工程が gate で、確認済みで判断を待っていて、選択肢にあり、理由が必須なら書かれていること。
/// エンジンも同じ確認をする（ここは、受け付けられないものを入力に変えないための事前確認）。
pub fn check_gate_decision(gs: &GraphSnapshot, task: &TaskSnapshot, choice: &str, note: Option<&str>) -> Result<(), String> {
    let Some(node) = task.node.as_ref() else { return Err("グラフの工程ではありません".to_string()) };
    let waiting = task.status == TaskStatus::Blocked(org_engine::BlockedReason::AwaitingApproval) && task.confirm == org_engine::ConfirmState::Confirmed;
    if !waiting {
        return Err("この工程は、判断を待っていません".to_string());
    }
    view::gate_check(&gs.graph, node, choice, note)
}

/// 「もう1周」を受け付けられるか。戻り先の工程が周回の上限で止まっていて、確認済みであること。
pub fn check_extra_round(gs: &GraphSnapshot, task: &TaskSnapshot) -> Result<(), String> {
    let Some(node) = task.node.as_ref() else { return Err("グラフの工程ではありません".to_string()) };
    let waiting = task.status == TaskStatus::Blocked(org_engine::BlockedReason::RoundLimit) && task.confirm == org_engine::ConfirmState::Confirmed;
    if !waiting || view::round_limit_of(gs, node).is_none() {
        return Err("周回の上限に達して止まっている工程ではありません".to_string());
    }
    Ok(())
}

/// 「この結果で受け入れる」を受け付けられるか。条件は「もう1周」と同じ（周回の上限で止まっている戻り先の工程、確認済み）。
pub fn check_accept_round(gs: &GraphSnapshot, task: &TaskSnapshot) -> Result<(), String> {
    check_extra_round(gs, task)
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
        let root = self.persist.store().map(|s| s.root().to_path_buf());
        let snapshot = self.org_snapshot();
        // 過去のワークフロー・アーカイブしたワークフロー（保存データの読取りだけ。何も書かず、司令taskへも何も渡さない）。
        let (history, mut archived, deletion_overflow) = root
            .as_deref()
            .map(|r| {
                super::history::read_lists(
                    r,
                    snapshot.as_ref().map(|s| &s.workflow),
                    super::history::HISTORY_KEEP,
                    super::history::ARCHIVED_KEEP,
                    &mut self.org_rt.history_cache.lock().unwrap(),
                )
            })
            .unwrap_or_default();
        let Some(snapshot) = snapshot else { return OrgSnapshotView { workflow: None, stall_minutes, history, archived, deletion_overflow: deletion_overflow as u32 } };
        let notices = self.org_rt.notice_entries();
        let usage_rows = self
            .persist
            .store()
            .map(|s| s.root().to_path_buf())
            .and_then(|root| FileOrgStore::new(workflow_dir(&root, &snapshot.workflow.0)).read_usage().ok())
            .unwrap_or_default();
        let dispatching = self.org_rt.grants.is_active(&snapshot.workflow);
        // グラフの判定・gate の表示に使うスロットの値（保存ファイルの読取りだけ。読めなかった版は入れず、画面で「未取得」になる）。
        let slots: HashMap<(SlotId, u32), serde_json::Value> = match (&snapshot.graph, root.as_deref()) {
            (Some(gs), Some(root)) => {
                let store = FileOrgStore::new(workflow_dir(root, &snapshot.workflow.0));
                view::graph_slot_needs(gs).into_iter().filter_map(|(s, v)| store.read_slot(&s, v).ok().flatten().map(|val| ((s, v), val))).collect()
            }
            _ => HashMap::new(),
        };
        let mut workflow = view::workflow_view(&ViewInput { snapshot: &snapshot, notices: &notices, usage_rows: &usage_rows, slots: &slots, dispatching });
        self.org_fill_live_fields(&mut workflow, &snapshot);
        // 動いている（または直前まで動いていた）ワークフローのアーカイブの印・削除の進みも載せる。
        if let Some(dir) = root.as_deref().map(|r| workflow_dir(r, &snapshot.workflow.0)) {
            workflow.archived_at = crate::store::org::read_archived_at(&dir);
            workflow.deletion = super::history::deletion_view_of(&dir);
            workflow.artifact_hint = super::artifacts::hint_for(&dir, &workflow.tasks);
        }
        if workflow.archived_at.is_some() {
            archived.insert(0, workflow.clone());
        }
        OrgSnapshotView { workflow: Some(workflow), stall_minutes, history, archived, deletion_overflow: deletion_overflow as u32 }
    }

    /// 直近に読んだ役割定義（読込み前は `loaded=false`）。
    pub fn org_roles_view(&self) -> OrgRolesView {
        view::roles_view(self.org_roles_report().as_ref())
    }

    /// 直近に読んだグラフ定義（読込み前は `loaded=false`）。読取りだけ。
    pub fn org_graphs_view(&self) -> OrgGraphsView {
        view::graphs_view(self.org_graphs_report().as_ref())
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
        self.org_start_checked_with(args, None, confirmed).await
    }

    /// `org_start_checked` の本体。`origin` は依頼整理を経た開始のときだけ付く（`org_intake_start`）。
    pub(in crate::host) async fn org_start_checked_with(
        self: &Arc<Self>,
        args: StartOrgArgs,
        origin: Option<crate::backend::intake::IntakeOrigin>,
        confirmed: UserConfirmed,
    ) -> Result<StartOrgResult, IpcError> {
        let parallelism = match (args.max_writers, args.max_total) {
            (None, None) => None,
            (w, t) => {
                let d = org_engine::Limits::default();
                let (w, t) = (w.unwrap_or(d.max_writers), t.unwrap_or(d.max_total));
                d.with_parallelism(w, t).map_err(|_| err(IpcErrorCode::InvalidArgs, "並列数は 1〜4 で指定してください"))?;
                Some((w, t))
            }
        };
        let graph = args.graph_id.map(|g| g.trim().to_string()).filter(|g| !g.is_empty());
        let wf = self
            .org_start(OrgStartArgs { request: args.request, repo: args.repo, parallelism, graph, max_turns: args.max_turns, max_retries: args.max_retries, origin }, confirmed)
            .await?;
        Ok(StartOrgResult { workflow_id: wf.0, intake_warning: None })
    }

    /// 再開（必要なチャットだけを resume し、新しい送信権を作る）。画面が見ていたワークフローと同じときだけ。
    /// `retry_tasks` は、利用者がチェックしたタスク（上限などで止まったもの）。再開の後に、それぞれ1回だけ新しい試行として送る
    /// （自動では送らない。この操作が送信の指示）。送れないタスクが含まれていれば、何もせずに拒否する。
    pub async fn org_resume_checked(self: &Arc<Self>, args: OrgResumeArgs, confirmed: UserConfirmed) -> Result<OrgResumeResult, IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        if self.org_rt.view.lock().unwrap().as_ref().is_some_and(|v| v.halt == Some(org_engine::HaltReason::DefinitionUnavailable)) {
            return Err(err(IpcErrorCode::InvalidArgs, "開始時のグラフ定義の写しを使えないため、このワークフローは再開できません"));
        }
        let mut retries = Vec::new();
        for task in &args.retry_tasks {
            let t = self.org_task_of_view(task)?;
            if let Err(why) = &t.retry {
                return Err(err(IpcErrorCode::InvalidArgs, format!("{task}: {}", why.text())));
            }
            if !retries.contains(&t.id) {
                retries.push(t.id);
            }
        }
        let report: OrgResumeReport = self.org_resume(confirmed).await?;
        for task in retries {
            let _ = tx.send(DriverMsg::Input(OrgInput::UserRetryTask { task }));
        }
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
    /// 理由（任意）は Planner に利用者の要望として渡る。
    pub fn org_replan(&self, args: OrgReplanArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        self.require_plan_pending()?;
        let note = checked_note(args.note)?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserReplan { note }));
        Ok(())
    }

    /// 状態の写しのグラフと、指定したタスク（グラフで動くワークフローのときだけ）。
    fn org_graph_task(&self, task: &str) -> Result<(GraphSnapshot, TaskSnapshot), IpcError> {
        let view = self.org_rt.view.lock().unwrap();
        let Some(v) = view.as_ref() else { return Err(err(IpcErrorCode::NotFound, "動いている組織実行がありません")) };
        let Some(gs) = v.graph.clone() else { return Err(err(IpcErrorCode::InvalidArgs, "このワークフローはグラフで動いていません")) };
        let t = v.tasks.iter().find(|t| t.id.0 == task).cloned().ok_or_else(|| err(IpcErrorCode::NotFound, format!("タスク {task} が見つかりません")))?;
        Ok((gs, t))
    }

    /// gate の判断。判断を待っている gate だけ（エンジンへ渡すのは、受け付けられる入力だけ）。
    pub fn org_gate_decide(&self, args: OrgGateDecideArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let (gs, t) = self.org_graph_task(&args.task)?;
        let note = checked_note(args.note)?;
        check_gate_decision(&gs, &t, &args.choice, note.as_deref()).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserGate { task: t.id, choice: args.choice, note }));
        Ok(())
    }

    /// 周回の上限で止まっている工程の「理由を書いてもう1周」。その辺の許容回数を1つだけ増やす。
    pub fn org_extra_round(&self, args: OrgExtraRoundArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let (gs, t) = self.org_graph_task(&args.task)?;
        check_extra_round(&gs, &t).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        let note = checked_note(args.note)?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserExtraRound { task: t.id, note }));
        Ok(())
    }

    /// 周回の上限で止まっている工程の「この結果で受け入れる」。周回を閉じるだけで、何も送らない。
    pub fn org_accept_round(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let (gs, t) = self.org_graph_task(&args.task)?;
        check_accept_round(&gs, &t).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        let _ = tx.send(DriverMsg::Input(OrgInput::UserAcceptRound { task: t.id }));
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

    /// 止まったタスクを、利用者の操作でやり直す（新しい試行。自動の再送ではない）。送っていないことが確かなものだけ
    /// （エンジンも同じ条件で確かめ、受理不明のものは受けない）。送信するには、動いている送信権（なければ「再開」）が要る。
    pub fn org_retry_task(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        if let Err(why) = &t.retry {
            let message = match t.status {
                TaskStatus::Blocked(org_engine::BlockedReason::UsageLimit | org_engine::BlockedReason::TurnFailed | org_engine::BlockedReason::ManualStop) => why.text(),
                _ => "このタスクは、送っていないことを確認できないため、やり直せません".to_string(),
            };
            return Err(err(IpcErrorCode::InvalidArgs, message));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserRetryTask { task: t.id }));
        Ok(())
    }

    /// 止まったタスクを、利用者の操作で失敗として確定する（これだけが止まったタスクを `Failed` にする）。確認状態が確定しているものだけ。
    pub fn org_fail_task(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let stopped = matches!(
            t.status,
            TaskStatus::Blocked(
                org_engine::BlockedReason::UsageLimit | org_engine::BlockedReason::TurnFailed | org_engine::BlockedReason::ManualStop | org_engine::BlockedReason::NeedsAttention | org_engine::BlockedReason::ReworkLimit | org_engine::BlockedReason::CheckFailed
            )
        );
        if !stopped || t.confirm != org_engine::ConfirmState::Confirmed {
            return Err(err(IpcErrorCode::InvalidArgs, "止まっていて、状態を確認できているタスクだけを失敗として確定できます"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserFailTask { task: t.id }));
        Ok(())
    }

    /// 手動に切り替えたチャットの最新 turn を読む（取込みの確認用。履歴の読取りだけで、何も送らない）。
    pub async fn org_manual_result(self: &Arc<Self>, args: OrgTaskArgs) -> Result<OrgManualResultView, IpcError> {
        self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let chat = self.org_adoptable_chat(&t)?;
        let (turns, running) = self.org_read_manual_turns(&chat).await?;
        Ok(view::manual_result_view(&turns, running, t.manual_format.clone()))
    }

    /// 取込みの対象にできるチャット（手動に切り替えたタスクで、状態が確定していて、チャットがあるもの）。
    fn org_adoptable_chat(&self, t: &TaskSnapshot) -> Result<ChatKey, IpcError> {
        if t.status != TaskStatus::Blocked(org_engine::BlockedReason::Manual) || t.confirm != org_engine::ConfirmState::Confirmed {
            return Err(err(IpcErrorCode::InvalidArgs, "手動に切り替えたタスクだけが、結果を取り込めます"));
        }
        t.chat.as_ref().and_then(chat_key_of_ref).ok_or_else(|| err(IpcErrorCode::InvalidArgs, "このタスクにはチャットがありません"))
    }

    async fn org_read_manual_turns(&self, chat: &ChatKey) -> Result<(Vec<crate::backend::model::TurnRecord>, bool), IpcError> {
        let agent = crate::host::state::agent_key_of(chat);
        let history = self
            .read_history(agent.clone(), crate::backend::backend::ReadOptions { include_turns: true })
            .await
            .map_err(IpcError::from)?;
        let running = self.read(|d| d.running_turn.contains_key(&agent));
        Ok((history.turns, running))
    }

    /// 手動に切り替えたチャットの、利用者が確認した turn の最終回答を、タスクの結果として取り込む（通常の完了判定に進む）。
    /// 履歴を読み直し、その turn が最新で、完了で、全件読めて、最終回答があり、実行中の turn がないことを確かめる。
    /// エンジンはこのチャットへ何も送らない（読取りだけ）。
    pub async fn org_adopt_manual(self: &Arc<Self>, args: OrgAdoptManualArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let chat = self.org_adoptable_chat(&t)?;
        let (turns, running) = self.org_read_manual_turns(&chat).await?;
        let v = view::manual_result_view(&turns, running, None);
        if let Some(problem) = v.problem {
            return Err(err(IpcErrorCode::InvalidArgs, problem));
        }
        if v.turn_id.as_deref() != Some(args.turn_id.as_str()) {
            return Err(err(IpcErrorCode::InvalidArgs, "最新のturnが、確認したときから変わりました。確認し直してください"));
        }
        // 取り込みの前に、回答待ちの承認・質問の今の件数を知らせる（完了の判定は、待ちがなくなってから）。
        let open = self.read(|d| d.requests.iter().filter(|r| r.chat == chat && r.state == crate::backend::model::RequestState::Pending).count() as u32);
        let _ = tx.send(DriverMsg::Input(OrgInput::RequestPending { chat: chat_ref_of(&chat), open }));
        let _ = tx.send(DriverMsg::Input(OrgInput::UserAdoptManual { task: t.id, turn: crate::rules::org::turn_ref_of(&crate::backend::model::ExternalId(args.turn_id)) }));
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

    /// グラフ定義ファイルの絶対パス（開く操作の対象）。読み込んだテンプレートか問題として報告したファイル名だけを受ける。
    pub fn org_graph_file_path(&self, args: &OrgOpenGraphFileArgs) -> Result<std::path::PathBuf, IpcError> {
        if !view::is_plain_role_file_name(&args.file) {
            return Err(err(IpcErrorCode::InvalidArgs, "グラフ定義ファイルの名前が正しくありません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、グラフ定義ファイルを開けません"));
        };
        let path = super::graphs::graphs_dir(&root).join(&args.file);
        if !path.is_file() {
            return Err(err(IpcErrorCode::NotFound, format!("グラフ定義ファイルが見つかりません: {}", args.file)));
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

    /// 読み直したグラフ定義（動いているワークフローがあれば拒否）。
    pub fn org_reload_graphs_view(&self) -> Result<OrgGraphsView, IpcError> {
        let report = self.org_reload_graphs()?;
        Ok(view::graphs_view(Some(&report)))
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

// ───────────── 介入・進行タイムライン（DESIGN_ORG_R2） ─────────────

/// タイムラインで一度に返す行数の上限。
const TIMELINE_LIMIT_MAX: u32 = 500;

/// ワークフローIDとして使える文字だけか（保存フォルダ名に使うので、区切りや `..` を通さない）。
fn is_plain_workflow_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 割り込み・追加指示の本文を検査して整える。空・長すぎるものは拒否する（黙って切らない）。
pub fn checked_text(text: &str) -> Result<String, IpcError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(err(IpcErrorCode::InvalidArgs, "指示の内容を書いてください"));
    }
    if t.chars().count() > org_engine::prompt::NOTE_MAX_CHARS {
        return Err(err(IpcErrorCode::InvalidArgs, format!("指示は{}文字までにしてください", org_engine::prompt::NOTE_MAX_CHARS)));
    }
    Ok(t.to_string())
}

impl Host {
    /// 割り込み指示の経路（読取りだけ）。実行中の turn・live・能力・前の割り込みの有無から、純粋関数で決める。
    pub(in crate::host) fn org_interject_route(&self, t: &TaskSnapshot) -> crate::rules::org_interject::InterjectRoute {
        use crate::rules::org_interject::{interject_route, RouteInput};
        let chat = t.chat.as_ref().and_then(chat_key_of_ref);
        let (live, active_turn) = match &chat {
            Some(c) => self.read(|d| {
                (
                    d.root_view(c).map(|v| v.freshness) == Some(crate::backend::model::Freshness::Live),
                    d.running_turn.get(&crate::host::state::agent_key_of(c)).map(crate::rules::org::turn_ref_of),
                )
            }),
            None => (false, None),
        };
        let released = chat.as_ref().is_some_and(|c| self.org_rt.grants.is_released(c));
        let ex = &t.extras;
        interject_route(&RouteInput {
            status: t.status,
            confirm: t.confirm,
            released,
            adopting: t.adopting,
            accepted_turn: ex.accepted_turn.clone(),
            accepted_settled: ex.accepted_settled,
            has_settle: ex.has_settle,
            turn_ended: ex.turn_ended,
            live,
            active_turn,
            steer_supported: crate::backend::backend::AiBackend::capabilities(self.backend.as_ref()).steer == crate::backend::model::Support::Supported,
            open_interject: ex.open_interject,
        })
    }

    /// 動いているワークフローの画面の写しに、時刻と割り込みの経路を載せる（記録とホストの観測が要る項目）。
    pub(in crate::host) fn org_fill_live_fields(&self, workflow: &mut OrgWorkflowView, snapshot: &super::driver::OrgSnapshot) {
        let tl = self.org_rt.timeline.lock().unwrap();
        let buf = tl.as_ref().filter(|b| b.workflow == snapshot.workflow);
        for (tv, ts) in workflow.tasks.iter_mut().zip(snapshot.tasks.iter()) {
            tv.since = buf.and_then(|b| b.since_of(&ts.id));
            let route = self.org_interject_route(ts);
            tv.interject = OrgInterjectRouteView {
                route: match &route {
                    crate::rules::org_interject::InterjectRoute::Steer { .. } => OrgInterjectRoute::Steer,
                    crate::rules::org_interject::InterjectRoute::NextTurn => OrgInterjectRoute::NextTurn,
                    crate::rules::org_interject::InterjectRoute::Refuse(_) => OrgInterjectRoute::Refused,
                },
                label: crate::rules::org_interject::route_label(&route).to_string(),
            };
        }
    }

    /// 進行タイムライン（読取りだけ）。動いているワークフローは保持している行、過去のものは保存された記録を読んで整える。
    pub fn org_timeline(&self, args: OrgTimelineArgs) -> Result<OrgTimelineView, IpcError> {
        let limit = args.limit.clamp(1, TIMELINE_LIMIT_MAX) as usize;
        if let Some(v) = self.org_rt.timeline.lock().unwrap().as_ref().filter(|b| b.workflow.0 == args.workflow_id).map(|b| b.view(args.after_seq, limit)) {
            return Ok(v);
        }
        if !is_plain_workflow_id(&args.workflow_id) {
            return Err(err(IpcErrorCode::InvalidArgs, "ワークフローの指定が正しくありません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、経過を読めません"));
        };
        let dir = workflow_dir(&root, &args.workflow_id);
        if !dir.is_dir() {
            return Err(err(IpcErrorCode::NotFound, "そのワークフローの記録が見つかりません"));
        }
        let read = crate::store::org::ReadOnlyOrgStore::new(dir).read_events().map_err(|e| err(IpcErrorCode::Io, format!("記録を読めません: {e}")))?;
        Ok(super::timeline::view_from_events(&read.events, args.after_seq, limit).0)
    }

    fn org_with_task(&self, workflow_id: &str, task: Option<&str>) -> Result<(UnboundedSender<DriverMsg>, Option<TaskSnapshot>), IpcError> {
        let (_, tx) = self.org_check_workflow(workflow_id)?;
        let t = task.map(|t| self.org_task_of_view(t)).transpose()?;
        Ok((tx, t))
    }

    /// 割り込み指示。経路を送る前に決める: 実行中の turn に届けられるときだけ `turn/steer`（1回）、
    /// そうでなければ「次の送信に添える」追加指示にする。送れないときは何もせず理由を返す。新しい turn は始めない。
    pub fn org_interject(&self, args: OrgInterjectArgs, _confirmed: &UserConfirmed) -> Result<OrgInterjectAck, IpcError> {
        use crate::rules::org_interject::{route_label, InterjectRoute};
        let text = checked_text(&args.text)?;
        let (tx, t) = self.org_with_task(&args.workflow_id, Some(&args.task))?;
        let t = t.expect("task requested");
        let route = self.org_interject_route(&t);
        let label = route_label(&route).to_string();
        match route {
            InterjectRoute::Refuse(why) => Err(err(IpcErrorCode::InvalidArgs, why)),
            InterjectRoute::NextTurn => {
                let _ = tx.send(DriverMsg::Input(OrgInput::UserAddNote { text, task: Some(t.id) }));
                Ok(OrgInterjectAck { route: OrgInterjectRoute::NextTurn, label })
            }
            InterjectRoute::Steer { turn } => {
                let _ = tx.send(DriverMsg::Input(OrgInput::UserInterject { task: t.id, text, fallback: args.fallback, active_turn: Some(turn) }));
                Ok(OrgInterjectAck { route: OrgInterjectRoute::Steer, label })
            }
        }
    }

    /// 届いたか確認できない（または照合で見つからなかった）割り込みを、次の送信に添えるか破棄するか。自動では何もしない。
    pub fn org_interject_fallback(&self, args: OrgInterjectFallbackArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let decidable = self.org_rt.view.lock().unwrap().as_ref().is_some_and(|v| {
            v.interjects.iter().any(|i| i.id.0 == args.id && matches!(i.outcome, Some(org_engine::InterjectOutcome::Unknown | org_engine::InterjectOutcome::ReconciledNotFound)))
        });
        if !decidable {
            return Err(err(IpcErrorCode::InvalidArgs, "この割り込みは、次の送信に添えるか破棄するかを選べる状態ではありません"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserInterjectFallback { id: org_engine::NoteId::new(args.id), add: args.add }));
        Ok(())
    }

    /// ワークフロー（またはタスク）への追加指示。実行中の turn には触らず、次にエンジンが送る turn に添える。
    pub fn org_add_note(&self, args: OrgAddNoteArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let text = checked_text(&args.text)?;
        let (tx, t) = self.org_with_task(&args.workflow_id, args.task.as_deref())?;
        let pending = self.org_rt.view.lock().unwrap().as_ref().map_or(0, |v| v.notes.iter().filter(|n| !n.note.withdrawn && !n.pending.is_empty()).count());
        if pending >= org_engine::MAX_PENDING_NOTES {
            return Err(err(IpcErrorCode::InvalidArgs, format!("届いていない追加指示が{}件あります。取り消してから追加してください", org_engine::MAX_PENDING_NOTES)));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserAddNote { text, task: t.map(|t| t.id) }));
        Ok(())
    }

    /// 追加指示を取り消す（以後の turn には添えない。すでに届いた分は戻せない）。
    pub fn org_withdraw_note(&self, args: OrgWithdrawNoteArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let ok = self.org_rt.view.lock().unwrap().as_ref().is_some_and(|v| v.notes.iter().any(|n| n.note.id.0 == args.id && !n.note.withdrawn));
        if !ok {
            return Err(err(IpcErrorCode::NotFound, "取り消せる追加指示が見つかりません"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserWithdrawNote { id: org_engine::NoteId::new(args.id) }));
        Ok(())
    }

    /// スキップ（計画の作業・検証タスクだけ。確認画面を経た利用者の操作）。検証役も一緒に飛ばす。
    pub fn org_skip_task(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        match &t.extras.skip {
            Some(Ok(_)) => {}
            Some(Err(why)) => return Err(err(IpcErrorCode::InvalidArgs, why.clone())),
            None => return Err(err(IpcErrorCode::InvalidArgs, "このタスクはスキップできません（計画の作業・検証タスクだけです）")),
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserSkipTask { task: t.id }));
        Ok(())
    }

    /// 差し戻し（利用者の指摘つき）。検証待ちでは指摘が必須、差し戻し上限で止まっているときは任意。
    pub fn org_send_back(&self, args: OrgSendBackArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let note = checked_note(args.note)?;
        match &t.extras.send_back {
            Some(Ok(())) => {}
            Some(Err(why)) => return Err(err(IpcErrorCode::InvalidArgs, why.clone())),
            None => return Err(err(IpcErrorCode::InvalidArgs, "このタスクは差し戻せません（計画の作業タスクだけです）")),
        }
        if note.is_none() && !matches!(t.status, TaskStatus::Blocked(org_engine::BlockedReason::ReworkLimit | org_engine::BlockedReason::CheckFailed)) {
            return Err(err(IpcErrorCode::InvalidArgs, "差し戻しの指摘を書いてください"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserSendBack { task: t.id, note }));
        Ok(())
    }

    /// 差し戻し上限で止まった作業タスクを、このまま受け入れる（何も送らない。取込みは利用者が行う）。
    pub fn org_accept_as_is(&self, args: OrgTaskArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        if !t.extras.can_accept_as_is {
            return Err(err(IpcErrorCode::InvalidArgs, "差し戻し上限、または検証のコマンドの失敗で止まっている作業タスクだけ、このまま受け入れられます"));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserAcceptAsIs { task: t.id }));
        Ok(())
    }

    /// 保留（まだ送っていないタスクをスケジューラが送らないようにする）／保留を解除。
    pub fn org_hold_task(&self, args: OrgHoldArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let (_, tx) = self.org_check_workflow(&args.workflow_id)?;
        let t = self.org_task_of_view(&args.task)?;
        let ok = if args.held { t.extras.can_hold } else { t.extras.held };
        if !ok {
            return Err(err(IpcErrorCode::InvalidArgs, if args.held { "まだ送っていない計画のタスクだけ保留にできます" } else { "保留中のタスクではありません" }));
        }
        let _ = tx.send(DriverMsg::Input(OrgInput::UserHoldTask { task: t.id, held: args.held }));
        Ok(())
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
            retryable: false,
            never_sent: false,
            retry: Err(org_engine::RetryRefusal::NotBlocked),
            retry_count: 0,
            adopting: false,
            manual_format: None,
            node: None,
            extras: Default::default(),
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
            graph: None,
            notes: Vec::new(),
            interjects: Vec::new(),
            notes_after_plan: 0,
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
    fn a_too_long_reason_is_refused_not_cut_silently() {
        let max = org_engine::prompt::NOTE_MAX_CHARS;
        assert_eq!(checked_note(Some("  ".into())).unwrap(), None);
        assert_eq!(checked_note(Some(" ok ".into())).unwrap().as_deref(), Some("ok"));
        assert!(checked_note(Some("あ".repeat(max))).is_ok());
        assert_eq!(checked_note(Some("あ".repeat(max + 1))).unwrap_err().code, IpcErrorCode::InvalidArgs);
    }

    #[test]
    fn resume_is_refused_when_the_definition_copy_is_unusable() {
        let (host, mut rx) = host_with_workflow(Some(HaltReason::DefinitionUnavailable), vec![task("a", TaskStatus::Pending)]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let c = UserConfirmed::from_user_command();
        let e = rt.block_on(host.org_resume_checked(OrgResumeArgs { workflow_id: "wf-1".into(), retry_tasks: Vec::new() }, c)).unwrap_err();
        assert_eq!(e.code, IpcErrorCode::InvalidArgs);
        assert!(drain(&mut rx).is_empty());
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
    fn retry_reaches_the_engine_only_for_a_task_known_not_to_have_been_sent() {
        let c = UserConfirmed::from_user_command();
        let mut sure = task("a", TaskStatus::Blocked(EBlocked::NeedsAttention));
        sure.retry = Ok(org_engine::RetryKind::Unsent);
        let unsure = task("b", TaskStatus::Blocked(EBlocked::NeedsAttention));
        let (host, mut rx) = host_with_workflow(None, vec![sure, unsure]);
        let args = |t: &str| OrgTaskArgs { workflow_id: "wf-1".into(), task: t.into() };
        assert_eq!(host.org_retry_task(args("b"), &c).unwrap_err().code, IpcErrorCode::InvalidArgs);
        assert_eq!(host.org_retry_task(OrgTaskArgs { workflow_id: "other".into(), task: "a".into() }, &c).unwrap_err().code, IpcErrorCode::NotFound);
        assert!(drain(&mut rx).is_empty());
        host.org_retry_task(args("a"), &c).unwrap();
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("UserRetryTask"));
        // 送信権は作らない。
        assert!(!host.org_rt.grants.is_active(&WorkflowId::new("wf-1")));
    }

    #[test]
    fn a_new_attempt_needs_a_retry_the_engine_would_accept_and_failing_is_only_for_stopped_confirmed_tasks() {
        let c = UserConfirmed::from_user_command();
        let mut limit = task("a", TaskStatus::Blocked(EBlocked::UsageLimit));
        limit.retry = Ok(org_engine::RetryKind::NewAttempt { reason: org_engine::RetryReason::UsageLimit });
        let mut spent = task("b", TaskStatus::Blocked(EBlocked::TurnFailed));
        spent.retry = Err(org_engine::RetryRefusal::RetryLimit { used: 3, max: 3 });
        let mut unsure = task("c", TaskStatus::Blocked(EBlocked::TurnFailed));
        unsure.confirm = ConfirmState::StopUnconfirmed;
        unsure.retry = Err(org_engine::RetryRefusal::SendUnknown);
        let running = task("d", TaskStatus::Running);
        let (host, mut rx) = host_with_workflow(None, vec![limit, spent, unsure, running]);
        let args = |t: &str| OrgTaskArgs { workflow_id: "wf-1".into(), task: t.into() };
        // refused retries say why (the limit), and send nothing
        let e = host.org_retry_task(args("b"), &c).unwrap_err();
        assert!(e.message.contains("3/3"), "{}", e.message);
        assert!(host.org_fail_task(args("c"), &c).is_err() && host.org_fail_task(args("d"), &c).is_err());
        assert!(drain(&mut rx).is_empty());
        host.org_retry_task(args("a"), &c).unwrap();
        host.org_fail_task(args("b"), &c).unwrap();
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 2);
        assert!(sent[0].contains("UserRetryTask") && sent[1].contains("UserFailTask"));
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
