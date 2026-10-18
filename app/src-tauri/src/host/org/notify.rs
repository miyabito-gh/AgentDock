//! 組織実行の通知を OS 通知の集約器へつなぐ変換（P-ORG-7a）。判断は `rules::notify`、ここは種類の振り分けだけ。
//!
//! - 即時: 計画承認待ち、タスクの停止（止まった・上限・利用上限・sandbox・取消の停止未確認・送信の見送り・保存失敗・スロットの保存失敗）、停滞。
//! - 2秒集約（ワークフロー単位）: ワークフローの完了とタスクの失敗。
//! - 出さない: タスク完了・自動承認・子孫未確認・拒否の文言・設定照合・照合結果・定義の案内（画面の通知欄には出る）。
//! - 本文には会話内容・理由の文言を入れない（`rules::notify::render` が定型文だけで作る）。

use org_engine::{OrgNotice, TaskId, WorkflowId};

use super::ports::OrgHostNotice;
use crate::backend::model::ChatKey;
use crate::rules::notify::{NotifyInput, OrgAttention, OrgOutcome};

/// 通知に出すワークフロー名。依頼文は使わず、ID（`wf-<開始時刻ms>`）の末尾4桁から作る（例「ワークフロー #1234」）。
pub fn notice_name(workflow: &WorkflowId) -> String {
    let digits: String = workflow.0.chars().filter(char::is_ascii_digit).collect();
    let tail: String = digits.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    if tail.is_empty() {
        "ワークフロー".to_string()
    } else {
        format!("ワークフロー #{tail}")
    }
}

/// 通知1件を集約器への入力にする。`title` は `notice_name` の値（依頼文の抜粋を渡さない）。OS通知を出さないものは None。
/// `title` はワークフロー名、`task_chat` はタスクの役割チャット（通知を開いたときの宛先）。
pub fn notify_input_of(
    workflow: &WorkflowId,
    title: Option<&str>,
    serial: u64,
    task_chat: &dyn Fn(&TaskId) -> Option<ChatKey>,
    n: &OrgHostNotice,
) -> Option<NotifyInput> {
    let key = format!("org|{}|{serial}", workflow.0);
    let title_s = title.map(str::to_string);
    let attention = |what: OrgAttention, task: Option<&TaskId>| NotifyInput::OrgAttention {
        key: key.clone(),
        title: title_s.clone(),
        chat: task.and_then(task_chat),
        what,
    };
    let ended = |outcome: OrgOutcome| NotifyInput::OrgEnded { key: key.clone(), workflow: workflow.0.clone(), title: title_s.clone(), outcome };
    match n {
        OrgHostNotice::Engine(e) => match e {
            OrgNotice::PlanNeedsApproval { .. } => Some(attention(OrgAttention::PlanNeedsApproval, None)),
            OrgNotice::TaskBlocked { task, .. } | OrgNotice::UsageLimit { task } | OrgNotice::CancelUnconfirmed { task } => {
                Some(attention(OrgAttention::TaskStopped, Some(task)))
            }
            OrgNotice::TurnLimitReached | OrgNotice::SandboxUnhealthy { .. } => Some(attention(OrgAttention::TaskStopped, None)),
            OrgNotice::TaskFailed { .. } => Some(ended(OrgOutcome::TaskFailed)),
            OrgNotice::WorkflowCompleted => Some(ended(OrgOutcome::WorkflowCompleted)),
            OrgNotice::PlanAutoApproved { .. } | OrgNotice::TaskCompleted { .. } | OrgNotice::DescendantsUnconfirmed { .. } | OrgNotice::Refused { .. } => None,
        },
        OrgHostNotice::SendRefused { task, .. } => Some(attention(OrgAttention::TaskStopped, Some(task))),
        OrgHostNotice::PersistFailed | OrgHostNotice::SlotWriteFailed { .. } => Some(attention(OrgAttention::TaskStopped, None)),
        OrgHostNotice::Stalled { task, .. } => Some(attention(OrgAttention::Stalled, Some(task))),
        OrgHostNotice::Config(_) | OrgHostNotice::Reconciled { .. } | OrgHostNotice::DefinitionNote { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{BackendKind, ExternalId};
    use crate::rules::org::TrustCompare;
    use org_engine::FailReason;

    fn chat_of(_: &TaskId) -> Option<ChatKey> {
        Some(ChatKey { backend: BackendKind::Codex, id: ExternalId("c1".into()) })
    }

    fn input(n: OrgHostNotice) -> Option<NotifyInput> {
        notify_input_of(&WorkflowId::new("wf-1"), Some("依頼"), 7, &chat_of, &n)
    }

    #[test]
    fn plan_approval_and_stops_are_attention_and_completion_or_failure_are_ended() {
        let plan = input(OrgHostNotice::Engine(OrgNotice::PlanNeedsApproval { reasons: vec![] })).unwrap();
        assert!(matches!(plan, NotifyInput::OrgAttention { what: OrgAttention::PlanNeedsApproval, chat: None, .. }));
        let blocked = input(OrgHostNotice::Engine(OrgNotice::TaskBlocked { task: TaskId::new("t"), why: "secret reason".into() })).unwrap();
        assert!(matches!(&blocked, NotifyInput::OrgAttention { what: OrgAttention::TaskStopped, chat: Some(_), .. }));
        let stalled = input(OrgHostNotice::Stalled { task: TaskId::new("t"), quiet_minutes: 31 }).unwrap();
        assert!(matches!(stalled, NotifyInput::OrgAttention { what: OrgAttention::Stalled, .. }));
        let failed = input(OrgHostNotice::Engine(OrgNotice::TaskFailed { task: TaskId::new("t"), reason: FailReason::ExecutionFailed })).unwrap();
        assert!(matches!(failed, NotifyInput::OrgEnded { outcome: OrgOutcome::TaskFailed, .. }));
        let done = input(OrgHostNotice::Engine(OrgNotice::WorkflowCompleted)).unwrap();
        assert!(matches!(done, NotifyInput::OrgEnded { outcome: OrgOutcome::WorkflowCompleted, .. }));
    }

    #[test]
    fn the_notice_name_is_not_derived_from_the_request() {
        assert_eq!(notice_name(&WorkflowId::new("wf-1760000001234")), "ワークフロー #1234");
        assert_eq!(notice_name(&WorkflowId::new("wf-x")), "ワークフロー");
    }

    #[test]
    fn quiet_notices_do_not_reach_the_os_and_keys_differ_per_notice() {
        assert!(input(OrgHostNotice::Engine(OrgNotice::TaskCompleted { task: TaskId::new("t") })).is_none());
        assert!(input(OrgHostNotice::Config(TrustCompare::Unchanged)).is_none());
        assert!(input(OrgHostNotice::Engine(OrgNotice::Refused { reason: "x".into() })).is_none());
        let a = notify_input_of(&WorkflowId::new("wf-1"), None, 1, &chat_of, &OrgHostNotice::PersistFailed).unwrap();
        let b = notify_input_of(&WorkflowId::new("wf-1"), None, 2, &chat_of, &OrgHostNotice::PersistFailed).unwrap();
        assert_ne!(a, b);
    }
}
