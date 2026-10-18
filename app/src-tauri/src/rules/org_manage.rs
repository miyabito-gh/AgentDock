//! 組織ワークフローの削除・アーカイブの可否と、削除の手順（DESIGN_ORG_DELETE §1〜§3）。純粋ロジック。
//!
//! 守ること:
//! - 通信断・停止未確認・受理不明（確認状態が `Confirmed` でない）のタスクがあるものは、削除もアーカイブもしない。
//! - 削除のために中断はしない。停止を確認できない役割チャットがあれば断る（削除保留は作らない）。
//! - 手順は「役割チャットを1件ずつ → 記録フォルダ」の順。チャットが全部済むまでフォルダ削除へ進まない。

use org_engine::{ConfirmState, TaskStatus};
use serde::{Deserialize, Serialize};

use crate::backend::local::{DeletePending, DeletePendingReason};
use crate::backend::model::{ChatKey, UnixMillis};
use crate::backend::org::OrgManageVerdict;
use crate::rules::manage::{delete_decision, ChatWork, DeleteDecision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManageKind {
    Archive,
    Delete,
}

/// タスク1件の判定材料。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskFact {
    pub status: TaskStatus,
    pub confirm: ConfirmState,
}

/// 削除する役割チャット1件の判定材料。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatFact {
    pub chat: ChatKey,
    /// 理由の文に出す名前（名前がなければ識別子）。
    pub label: String,
    pub work: ChatWork,
    pub pending: Option<DeletePending>,
    /// 保存に失敗している内容がある。
    pub save_blocked: bool,
    /// 引き継ぎ中・作業領域の共用などで削除できない理由。
    pub in_use: Option<String>,
}

pub struct VerdictInput<'a> {
    /// `None`＝タスクの状態を読めない（削除を始めた後の続きだけ。最初の判定では必ず渡す）。
    pub tasks: Option<&'a [TaskFact]>,
    /// 司令taskがこのワークフローで動いている。
    pub driver_on_this: bool,
    pub plan_pending: bool,
    pub connected: bool,
    /// 役割チャットの会話も削除する（false なら手動に切り替えて残すだけ）。
    pub delete_chats: bool,
    pub chats: &'a [ChatFact],
}

fn terminal(s: &TaskStatus) -> bool {
    matches!(s, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)
}

/// このチャットが削除できない理由。削除してよければ None。
pub fn chat_block_reason(c: &ChatFact) -> Option<String> {
    if c.save_blocked {
        return Some(format!("「{}」の保存に失敗している内容があります。保存を再試行してから削除してください", c.label));
    }
    if let Some(m) = &c.in_use {
        return Some(format!("「{}」: {m}", c.label));
    }
    match delete_decision(&c.work, c.pending.as_ref()) {
        DeleteDecision::Proceed => None,
        DeleteDecision::StopFirst => Some(format!("「{}」の作業が終わっていません", c.label)),
        DeleteDecision::Pend(DeletePendingReason::OwnershipUnknown) => {
            Some(format!("「{}」は送信の受理を確認できていません。確認できるまで削除できません", c.label))
        }
        DeleteDecision::Pend(_) => Some(format!("「{}」は停止を確認できていません。停止を確認してから削除してください", c.label)),
    }
}

/// 削除・アーカイブの可否。拒否のときは理由を並べる（空にはならない）。
pub fn verdict(kind: ManageKind, input: &VerdictInput<'_>) -> OrgManageVerdict {
    let mut reasons: Vec<String> = Vec::new();
    if input.driver_on_this {
        reasons.push("このワークフローはまだ動いています".to_string());
    }
    if let Some(tasks) = input.tasks {
        if tasks.is_empty() {
            if input.plan_pending {
                reasons.push("計画の承認待ちです。先に承認または取り消してください".to_string());
            }
        } else {
            let open = tasks.iter().filter(|t| !terminal(&t.status)).count();
            if open > 0 {
                reasons.push(format!("終わっていないワークフローです（終わっていないタスクが{open}件あります）"));
            }
            let unconfirmed = tasks.iter().filter(|t| t.confirm != ConfirmState::Confirmed).count();
            if unconfirmed > 0 {
                reasons.push(format!("実行中・停止未確認・通信断のタスクがあります（{unconfirmed}件）。状態を確認できるまで、まだ扱えません"));
            }
        }
    }
    if kind == ManageKind::Delete && input.delete_chats {
        if !input.connected && !input.chats.is_empty() {
            reasons.push("Codex に接続していません（役割チャットの履歴の削除に必要です）".to_string());
        }
        reasons.extend(input.chats.iter().filter_map(chat_block_reason));
    }
    if reasons.is_empty() {
        OrgManageVerdict::Allowed
    } else {
        OrgManageVerdict::Blocked { reasons }
    }
}

/// 所有の記録（`chat.json.org`）の要約。
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub chat: ChatKey,
    pub workflow_id: String,
    /// 「手動に切り替える」済み。
    pub released: bool,
}

/// 削除の対象。タスクの `chat` 参照と chat.json の所有記録の和集合（作成結果が失われたチャットも含む）。
/// 返すのは（消す＝未解放, 残す＝手動に切り替え済み）。重複なし。別のワークフローの記録のチャットは含めない。
pub fn delete_targets(workflow_id: &str, task_chats: &[ChatKey], bindings: &[Binding]) -> (Vec<ChatKey>, Vec<ChatKey>) {
    let mut del: Vec<ChatKey> = Vec::new();
    let mut kept: Vec<ChatKey> = Vec::new();
    let from_records = bindings.iter().filter(|b| b.workflow_id == workflow_id).map(|b| &b.chat);
    for chat in task_chats.iter().chain(from_records) {
        if del.contains(chat) || kept.contains(chat) {
            continue;
        }
        match bindings.iter().find(|b| &b.chat == chat) {
            Some(b) if b.workflow_id != workflow_id => {}
            Some(b) if b.released => kept.push(chat.clone()),
            _ => del.push(chat.clone()),
        }
    }
    (del, kept)
}

/// 削除の進みの記録（`org\<wf>\delete.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRecord {
    pub requested_at: UnixMillis,
    /// false なら役割チャットは消さずに手動へ切り替える。
    pub delete_chats: bool,
    pub chats: Vec<ChatKey>,
    /// 済んだ段の名前（`step_label`）。
    #[serde(default)]
    pub done: Vec<String>,
    /// 直近の実行で失敗した段（「名前: 理由」）。
    #[serde(default)]
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Chat(ChatKey),
    /// 記録フォルダ（slots → graph・roles → usage → events → workflow → フォルダ）。
    Files,
}

pub const FILES_LABEL: &str = "記録フォルダ";

pub fn step_label(s: &Step) -> String {
    match s {
        Step::Chat(c) => format!("役割チャット {}", c.id.0),
        Step::Files => FILES_LABEL.to_string(),
    }
}

/// これから実行する段。済み（`done`）は飛ばし、チャットが全部済むまでフォルダの段は出さない。
pub fn next_steps(rec: &DeleteRecord) -> Vec<Step> {
    let pending: Vec<Step> = rec.chats.iter().map(|c| Step::Chat(c.clone())).filter(|s| !rec.done.contains(&step_label(s))).collect();
    if !pending.is_empty() {
        return pending;
    }
    if rec.done.iter().any(|d| d == FILES_LABEL) {
        Vec::new()
    } else {
        vec![Step::Files]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{BackendKind, ExternalId};
    use org_engine::{BlockedReason, ReconcileCause};

    fn chat(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn work(c: &ChatKey) -> ChatWork {
        ChatWork {
            chat: c.clone(),
            has_unfinished: false,
            stop_unconfirmed: None,
            ownership_unknown: false,
            queue_pending: false,
            queue_in_flight: false,
            unresolved_send: false,
            first_send_in_flight: false,
        }
    }
    fn fact(id: &str) -> ChatFact {
        let c = chat(id);
        ChatFact { chat: c.clone(), label: id.into(), work: work(&c), pending: None, save_blocked: false, in_use: None }
    }
    fn t(status: TaskStatus) -> TaskFact {
        TaskFact { status, confirm: ConfirmState::Confirmed }
    }
    fn input<'a>(tasks: &'a [TaskFact], chats: &'a [ChatFact]) -> VerdictInput<'a> {
        VerdictInput { tasks: Some(tasks), driver_on_this: false, plan_pending: false, connected: true, delete_chats: true, chats }
    }
    fn blocked(v: &OrgManageVerdict) -> bool {
        matches!(v, OrgManageVerdict::Blocked { reasons } if !reasons.is_empty())
    }

    #[test]
    fn all_terminal_and_confirmed_is_allowed_for_both() {
        let tasks = [t(TaskStatus::Completed), t(TaskStatus::Cancelled), t(TaskStatus::Failed(org_engine::FailReason::ExecutionFailed))];
        let chats = [fact("a")];
        for k in [ManageKind::Archive, ManageKind::Delete] {
            assert_eq!(verdict(k, &input(&tasks, &chats)), OrgManageVerdict::Allowed);
        }
    }

    #[test]
    fn any_unfinished_task_blocks_both() {
        for open in [TaskStatus::Running, TaskStatus::Blocked(BlockedReason::Manual), TaskStatus::Reviewing, TaskStatus::Pending] {
            let tasks = [t(TaskStatus::Completed), t(open)];
            for k in [ManageKind::Archive, ManageKind::Delete] {
                assert!(blocked(&verdict(k, &input(&tasks, &[]))));
            }
        }
    }

    #[test]
    fn unconfirmed_tasks_block_both_even_when_terminal() {
        for confirm in [ConfirmState::NeedsReconcile(ReconcileCause::Restart), ConfirmState::SendUnknown, ConfirmState::StopUnconfirmed, ConfirmState::InterruptRequested] {
            let tasks = [TaskFact { status: TaskStatus::Completed, confirm }];
            for k in [ManageKind::Archive, ManageKind::Delete] {
                assert!(blocked(&verdict(k, &input(&tasks, &[]))), "{confirm:?}");
            }
        }
    }

    #[test]
    fn a_running_driver_blocks_and_an_empty_plan_pending_blocks() {
        let tasks = [t(TaskStatus::Completed)];
        let mut i = input(&tasks, &[]);
        i.driver_on_this = true;
        assert!(blocked(&verdict(ManageKind::Archive, &i)));
        let empty: [TaskFact; 0] = [];
        let mut i = input(&empty, &[]);
        assert_eq!(verdict(ManageKind::Archive, &i), OrgManageVerdict::Allowed);
        i.plan_pending = true;
        assert!(blocked(&verdict(ManageKind::Archive, &i)));
    }

    #[test]
    fn chat_conditions_and_connection_matter_only_for_delete() {
        let tasks = [t(TaskStatus::Completed)];
        let mut running = fact("a");
        running.work.has_unfinished = true;
        let mut stop = fact("b");
        stop.work.stop_unconfirmed = Some(crate::backend::model::LocalId("s".into()));
        let mut unresolved = fact("c");
        unresolved.work.unresolved_send = true;
        let mut saving = fact("d");
        saving.save_blocked = true;
        for c in [running, stop, unresolved, saving] {
            let chats = [c];
            assert!(blocked(&verdict(ManageKind::Delete, &input(&tasks, &chats))));
            assert_eq!(verdict(ManageKind::Archive, &input(&tasks, &chats)), OrgManageVerdict::Allowed);
        }
        // not connected: delete (with chats to delete) is refused, archive is not
        let chats = [fact("a")];
        let mut i = input(&tasks, &chats);
        i.connected = false;
        assert!(blocked(&verdict(ManageKind::Delete, &i)));
        assert_eq!(verdict(ManageKind::Archive, &i), OrgManageVerdict::Allowed);
        // keeping the chats (release only) needs neither the connection nor their idleness
        i.delete_chats = false;
        assert_eq!(verdict(ManageKind::Delete, &i), OrgManageVerdict::Allowed);
    }

    fn b(id: &str, wf: &str, released: bool) -> Binding {
        Binding { chat: chat(id), workflow_id: wf.into(), released }
    }

    #[test]
    fn targets_are_the_union_without_duplicates_and_without_other_workflows() {
        // a: task reference only, b: ownership record only, c: both, d: released, e: another workflow's chat
        let tasks = [chat("a"), chat("c"), chat("d"), chat("e")];
        let bindings = [b("b", "wf-1", false), b("c", "wf-1", false), b("d", "wf-1", true), b("e", "wf-2", false), b("f", "wf-2", false)];
        let (del, kept) = delete_targets("wf-1", &tasks, &bindings);
        assert_eq!(del, vec![chat("a"), chat("c"), chat("b")]);
        assert_eq!(kept, vec![chat("d")]);
    }

    fn rec(chats: &[&str], done: &[&str]) -> DeleteRecord {
        DeleteRecord {
            requested_at: UnixMillis(1),
            delete_chats: true,
            chats: chats.iter().map(|c| chat(c)).collect(),
            done: done.iter().map(|c| step_label(&Step::Chat(chat(c)))).collect(),
            failed: vec!["x".into()],
        }
    }

    #[test]
    fn next_steps_skip_done_chats_and_wait_for_all_chats_before_the_files() {
        assert_eq!(next_steps(&rec(&["a", "b"], &[])), vec![Step::Chat(chat("a")), Step::Chat(chat("b"))]);
        assert_eq!(next_steps(&rec(&["a", "b"], &["a"])), vec![Step::Chat(chat("b"))]);
        assert_eq!(next_steps(&rec(&["a", "b"], &["a", "b"])), vec![Step::Files]);
        assert_eq!(next_steps(&rec(&[], &[])), vec![Step::Files]);
        let mut done_all = rec(&["a"], &["a"]);
        done_all.done.push(FILES_LABEL.into());
        assert!(next_steps(&done_all).is_empty());
        // the files step is never offered while a chat is still pending, even if "files" was recorded
        let mut odd = rec(&["a"], &[]);
        odd.done.push(FILES_LABEL.into());
        assert_eq!(next_steps(&odd), vec![Step::Chat(chat("a"))]);
    }
}
