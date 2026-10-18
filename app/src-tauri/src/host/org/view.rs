//! 組織実行の画面向けの写し（P-ORG-7a）。エンジンの状態・ホストの通知・役割定義を `backend::org` のビュー型に直す純粋な変換。
//!
//! - 状態は2軸（業務状態・確認状態）を別々に、文字（`label`）とアイコン名（`icon`）で運ぶ。色には頼らない。
//! - 取れていない値は `None`／空にして、0や空文字で代用しない。
//! - 通知の本文は定型文だけ。会話の内容・ツール引数は入れない。


use org_engine::{
    BlockedReason as EBlocked, ConfirmReason, ConfirmState, Effect, FailReason, HaltReason, LoadedRole, OrgNotice, ReconcileCause, SandboxKind, TaskId,
    TaskStatus, UsageRow,
};

use super::driver::{OrgSnapshot, TaskSnapshot};
use super::ports::{OrgHostNotice, WorktreeFinding};
use super::roles::RoleReport;
use super::NoticeEntry;
use crate::backend::model::TurnEnd;
use crate::backend::org::*;
use crate::rules::org::{chat_key_of_ref, TrustCompare};

/// 画面に出す通知の件数（新しいものを残す）。
const NOTICE_SHOWN: usize = 30;

fn finished(s: TaskStatus) -> bool {
    matches!(s, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)
}

pub fn status_view(s: TaskStatus) -> OrgStatusView {
    use OrgTaskPhase as P;
    let (phase, reason, label, icon): (P, Option<OrgReason>, &str, &str) = match s {
        TaskStatus::Pending => (P::Pending, None, "待機中", "hourglass"),
        TaskStatus::Ready => (P::Ready, None, "実行待ち", "circle"),
        TaskStatus::Running => (P::Running, None, "実行中", "play"),
        TaskStatus::Blocked(r) => {
            let (reason, label) = match r {
                EBlocked::AwaitingApproval => (OrgReason::AwaitingApproval, "止まっています（承認待ち）"),
                EBlocked::Manual => (OrgReason::Manual, "止まっています（手動に切替済み）"),
                EBlocked::ManualStop => (OrgReason::ManualStop, "止まっています（中断されました）"),
                EBlocked::Limit => (OrgReason::Limit, "止まっています（上限）"),
                EBlocked::NeedsAttention => (OrgReason::NeedsAttention, "止まっています（要対応）"),
                // 専用の理由はG-A5で追加する（IPCの型を変えないため、今は要対応と同じ種類で文言だけ分ける）。
                EBlocked::RoundLimit => (OrgReason::NeedsAttention, "止まっています（周回の上限）"),
            };
            (P::Blocked, Some(reason), label, "pause")
        }
        TaskStatus::Reviewing => (P::Reviewing, None, "レビュー中", "eye"),
        TaskStatus::Rework => (P::Rework, None, "差し戻し中", "rotate"),
        TaskStatus::Completed => (P::Completed, None, "完了", "check"),
        TaskStatus::Failed(r) => {
            let (reason, label) = match r {
                FailReason::ExecutionFailed => (OrgReason::ExecutionFailed, "失敗（実行に失敗）"),
                FailReason::InvalidArtifact => (OrgReason::InvalidArtifact, "失敗（成果物が不正）"),
                FailReason::LimitReached => (OrgReason::LimitReached, "失敗（上限に到達）"),
            };
            (P::Failed, Some(reason), label, "x")
        }
        TaskStatus::Cancelled => (P::Cancelled, None, "取り消し", "ban"),
    };
    OrgStatusView { phase, reason, label: label.to_string(), icon: icon.to_string() }
}

pub fn confirm_view(c: ConfirmState) -> OrgConfirmView {
    let cause = |c: ReconcileCause| match c {
        ReconcileCause::Disconnect => OrgReconcileCause::Disconnect,
        ReconcileCause::Wake => OrgReconcileCause::Wake,
        ReconcileCause::Restart => OrgReconcileCause::Restart,
    };
    let (state, cause_v, label, icon) = match c {
        ConfirmState::Confirmed => (OrgConfirmCode::Confirmed, None, "確認済み".to_string(), "shield-check"),
        ConfirmState::SendUnknown => (OrgConfirmCode::SendUnknown, None, "送信が受け付けられたか不明".to_string(), "help-circle"),
        ConfirmState::NeedsReconcile(why) => {
            let when = match why {
                ReconcileCause::Disconnect => "切断後",
                ReconcileCause::Wake => "スリープ復帰後",
                ReconcileCause::Restart => "再起動後",
            };
            (OrgConfirmCode::NeedsReconcile, Some(cause(why)), format!("要照合（{when}）"), "refresh")
        }
        ConfirmState::InterruptRequested => (OrgConfirmCode::InterruptRequested, None, "中断を要求済み（停止は未確認）".to_string(), "stop-circle"),
        ConfirmState::StopUnconfirmed => (OrgConfirmCode::StopUnconfirmed, None, "停止を確認できていません".to_string(), "alert-triangle"),
    };
    OrgConfirmView { state, cause: cause_v, label, icon: icon.to_string() }
}

pub fn halt_view(h: HaltReason) -> OrgHaltView {
    let (kind, label, icon, resumable) = match h {
        HaltReason::Restart => (OrgHaltKind::Restart, "再起動前の状態を復元しました。「再開」で続けます", "refresh", true),
        HaltReason::Shutdown => (OrgHaltKind::Shutdown, "終了の手順のため停止しています", "stop-circle", true),
        HaltReason::User => (OrgHaltKind::User, "一時停止中です", "pause", true),
        HaltReason::NeedsResume => (OrgHaltKind::NeedsResume, "送る先のチャットが live ではないため停止しています。「再開」で戻します", "pause", true),
        HaltReason::SandboxUnhealthy => (OrgHaltKind::SandboxUnhealthy, "Codex の sandbox を起動できないため停止しています", "alert-triangle", true),
        HaltReason::AwaitingPlanApproval => (OrgHaltKind::AwaitingPlanApproval, "計画の承認待ちです", "help-circle", false),
    };
    OrgHaltView { kind, label: label.to_string(), icon: icon.to_string(), resumable }
}

pub fn effect_label(e: Effect) -> &'static str {
    match e {
        Effect::UserTreeWrite => "利用者の作業ツリーへの書込み",
        Effect::PushMergeTag => "push・マージ・タグ",
        Effect::OutsideWorkdir => "作業フォルダ外の操作",
        Effect::NetworkOrInstall => "ネットワーク・インストール",
        Effect::SecretsOrAuth => "秘密情報・認証",
        Effect::DeletionPrimary => "削除が主目的の作業",
    }
}

fn effect_view(e: Effect) -> OrgEffect {
    match e {
        Effect::UserTreeWrite => OrgEffect::UserTreeWrite,
        Effect::PushMergeTag => OrgEffect::PushMergeTag,
        Effect::OutsideWorkdir => OrgEffect::OutsideWorkdir,
        Effect::NetworkOrInstall => OrgEffect::NetworkOrInstall,
        Effect::SecretsOrAuth => OrgEffect::SecretsOrAuth,
        Effect::DeletionPrimary => OrgEffect::DeletionPrimary,
    }
}

pub fn confirm_reason_view(r: &ConfirmReason) -> OrgConfirmReasonView {
    let (task, text) = match r {
        ConfirmReason::DeclaredEffect { task, effect } => (Some(task.0.clone()), format!("{}: 「{}」を宣言しています", task.0, effect_label(*effect))),
        ConfirmReason::DisabledRole { task, role } => {
            (Some(task.0.clone()), format!("{}: 有効にしていない役割「{}」を使います（権限を広げる定義が未確認です）", task.0, role.0))
        }
        ConfirmReason::ReplanScopeChanged => (None, "再計画で作業の範囲が変わりました".to_string()),
        ConfirmReason::AutoReplanLimitExceeded => (None, "自動で再計画できる回数の上限を超えました".to_string()),
        ConfirmReason::ParallelismExceeded { requested, max } => (None, format!("並列数 {requested} が上限 {max} を超えます")),
        ConfirmReason::TurnLimitExceeded { needed, remaining } => (None, format!("必要な最小 turn 数 {needed} が残り {remaining} を超えます")),
    };
    OrgConfirmReasonView { task, text }
}

fn turn_end_label(e: TurnEnd) -> &'static str {
    match e {
        TurnEnd::Completed => "完了",
        TurnEnd::Failed => "失敗",
        TurnEnd::Interrupted => "中断",
    }
}

pub fn worktree_view(w: WorktreeFinding) -> OrgWorktreeView {
    let (kind, entries, label) = match w {
        WorktreeFinding::NotOwned => (OrgWorktreeKind::NotOwned, None, "作業場所は AgentDock が作ったものではないため、状態を調べていません".to_string()),
        WorktreeFinding::Missing => (OrgWorktreeKind::Missing, None, "作業場所のフォルダが見つかりません".to_string()),
        WorktreeFinding::Clean => (OrgWorktreeKind::Clean, None, "作業場所に未コミットの変更はありません".to_string()),
        WorktreeFinding::Uncommitted { entries } => {
            (OrgWorktreeKind::Uncommitted, Some(entries), format!("作業場所に未コミットの変更が {entries} 件あります（破棄していません）"))
        }
        WorktreeFinding::Unreadable => (OrgWorktreeKind::Unreadable, None, "作業場所の状態を読めませんでした".to_string()),
    };
    OrgWorktreeView { kind, entries, label }
}

pub fn reconcile_view(task: &TaskId, turn_read: bool, turn_end: Option<TurnEnd>, worktree: WorktreeFinding) -> OrgReconcileView {
    let turn = if !turn_read {
        "履歴を読めなかったため、終端は未確認のままです".to_string()
    } else if let Some(e) = turn_end {
        format!("履歴で終端（{}）を確認しました", turn_end_label(e))
    } else {
        "履歴では終端を確認できていません".to_string()
    };
    let wt = worktree_view(worktree);
    OrgReconcileView { task: task.0.clone(), turn_read, turn_end, text: format!("{turn}。{}", wt.label), worktree: wt }
}

/// config.toml の前後照合の表示。`Unchanged` は None（出さない）。
pub fn trust_view(t: &TrustCompare) -> Option<OrgTrustView> {
    match t {
        TrustCompare::Unchanged => None,
        TrustCompare::CodexTrustAppended { projects } => Some(OrgTrustView {
            kind: OrgTrustKind::CodexTrustAppended,
            severity: OrgSeverity::Info,
            text: "Codex が作業フォルダを信頼済みとして config.toml に追記しました（AgentDock は変更していません）".to_string(),
            projects: projects.clone(),
        }),
        TrustCompare::Other { .. } => Some(OrgTrustView {
            kind: OrgTrustKind::Other,
            severity: OrgSeverity::Attention,
            text: "config.toml に想定外の変更があります（AgentDock は変更していません）".to_string(),
            projects: Vec::new(),
        }),
        TrustCompare::Unverified { reason } => {
            Some(OrgTrustView { kind: OrgTrustKind::Unverified, severity: OrgSeverity::Attention, text: reason.clone(), projects: Vec::new() })
        }
    }
}

fn fail_label(r: FailReason) -> &'static str {
    match r {
        FailReason::ExecutionFailed => "実行に失敗",
        FailReason::InvalidArtifact => "成果物が不正",
        FailReason::LimitReached => "上限に到達",
    }
}

/// 通知1件を画面の行にする。出さないもの（変更なしの照合）は None。
pub fn notice_view(e: &NoticeEntry) -> Option<OrgNoticeView> {
    use OrgNoticeKind as K;
    use OrgSeverity::{Attention, Info};
    let task_of = |t: &TaskId| Some(t.0.clone());
    let (kind, severity, task, text): (K, OrgSeverity, Option<String>, String) = match &e.notice {
        OrgHostNotice::Engine(n) => match n {
            OrgNotice::PlanNeedsApproval { .. } => (K::PlanNeedsApproval, Attention, None, "計画の承認が必要です".to_string()),
            OrgNotice::PlanAutoApproved { tasks } => (K::PlanAutoApproved, Info, None, format!("計画を自動で承認しました（{tasks}件）")),
            OrgNotice::TaskCompleted { task } => (K::TaskCompleted, Info, task_of(task), format!("{} が完了しました", task.0)),
            OrgNotice::TaskFailed { task, reason } => (K::TaskFailed, Attention, task_of(task), format!("{} が失敗しました（{}）", task.0, fail_label(*reason))),
            OrgNotice::TaskBlocked { task, why } => (K::TaskBlocked, Attention, task_of(task), format!("{} が止まりました: {why}", task.0)),
            OrgNotice::TurnLimitReached => (K::TurnLimitReached, Attention, None, "turn 上限に達しました。上限を上げると続けられます".to_string()),
            OrgNotice::UsageLimit { task } => (K::UsageLimit, Attention, task_of(task), format!("{}: 利用上限に達しました", task.0)),
            OrgNotice::SandboxUnhealthy { detail } => (K::SandboxUnhealthy, Attention, None, detail.clone()),
            OrgNotice::DescendantsUnconfirmed { task, count } => (
                K::DescendantsUnconfirmed,
                Info,
                task_of(task),
                format!("{}: 状態を確認できない子エージェントが {count} 件あります（不明のまま残しています）", task.0),
            ),
            OrgNotice::CancelUnconfirmed { task } => {
                (K::CancelUnconfirmed, Attention, task_of(task), format!("{} を取り消しました。Codex 側の停止は確認できていません", task.0))
            }
            OrgNotice::WorkflowCompleted => (K::WorkflowCompleted, Info, None, "ワークフローが完了しました。ブランチ・差分を確認して取り込んでください".to_string()),
            OrgNotice::Refused { reason } => (K::Refused, Attention, None, reason.clone()),
        },
        OrgHostNotice::Config(t) => {
            let v = trust_view(t)?;
            (K::Config, v.severity, None, v.text)
        }
        OrgHostNotice::SendRefused { task, why } => (K::SendRefused, Attention, task_of(task), format!("{}: 送信を見送りました（{why}）", task.0)),
        OrgHostNotice::PersistFailed => {
            (K::PersistFailed, Attention, None, "記録を追記できないため、以後は何も送りません。AgentDock を再起動してください".to_string())
        }
        OrgHostNotice::Reconciled { task, turn_read, turn_end, worktree } => {
            (K::Reconciled, Info, task_of(task), format!("{}: {}", task.0, reconcile_view(task, *turn_read, *turn_end, *worktree).text))
        }
        OrgHostNotice::Stalled { task, quiet_minutes } => (
            K::Stalled,
            Attention,
            task_of(task),
            format!("{}: {quiet_minutes}分間、新しい出力がありません（自動では止めません）", task.0),
        ),
    };
    Some(OrgNoticeView { serial: e.serial, kind, severity, task, text })
}

/// ビューの材料（読取りだけで集めたもの）。
pub struct ViewInput<'a> {
    pub snapshot: &'a OrgSnapshot,
    /// 古い順。
    pub notices: &'a [NoticeEntry],
    pub usage_rows: &'a [UsageRow],
    /// 送信権が今あるか。
    pub dispatching: bool,
}

fn task_view(t: &TaskSnapshot, snap: &OrgSnapshot, notices: &[NoticeEntry]) -> OrgTaskView {
    let waiting_on: Vec<String> = t
        .deps
        .iter()
        .filter(|d| snap.tasks.iter().find(|x| &x.id == *d).is_some_and(|x| x.status != TaskStatus::Completed))
        .map(|d| d.0.clone())
        .collect();
    // 直近の照合の結果と、停滞の通知（このタスクについての最新の通知が停滞なら立てる）。
    let mut reconcile = None;
    let mut last_for_task: Option<&OrgHostNotice> = None;
    for e in notices {
        match &e.notice {
            OrgHostNotice::Reconciled { task, turn_read, turn_end, worktree } if task == &t.id => {
                reconcile = Some(reconcile_view(task, *turn_read, *turn_end, *worktree));
                last_for_task = Some(&e.notice);
            }
            OrgHostNotice::Stalled { task, .. } if task == &t.id => last_for_task = Some(&e.notice),
            _ => {}
        }
    }
    let stall_notified = t.status == TaskStatus::Running && matches!(last_for_task, Some(OrgHostNotice::Stalled { .. }));
    OrgTaskView {
        id: t.id.0.clone(),
        role: t.role.clone(),
        title: t.title.clone(),
        deps: t.deps.iter().map(|d| d.0.clone()).collect(),
        waiting_on,
        status: status_view(t.status),
        confirm: confirm_view(t.confirm),
        attempts: t.attempts,
        max_attempts: 1 + snap.limits.max_rework,
        chat: t.chat.as_ref().and_then(chat_key_of_ref),
        commit: t.commit.clone(),
        effects: t.effects.iter().copied().map(effect_view).collect(),
        stall_notified,
        reconcile,
        can_cancel: !finished(t.status),
        can_accept_manual: t.status == TaskStatus::Blocked(EBlocked::Manual) && t.confirm == ConfirmState::Confirmed,
        needs_chat_choice: t.chat.is_none()
            && t.chat_cwd.is_some()
            && t.status == TaskStatus::Blocked(EBlocked::NeedsAttention)
            && t.confirm == ConfirmState::SendUnknown,
        can_retry: t.retryable,
        attention_note: attention_note(t, notices),
    }
}

/// 「要対応」で止まったタスクの理由の1行。通知（メモリ上）に残っていなければ、失われたことを明示する（空にしない）。
fn attention_note(t: &TaskSnapshot, notices: &[NoticeEntry]) -> Option<String> {
    if t.status != TaskStatus::Blocked(EBlocked::NeedsAttention) {
        return None;
    }
    let why = notices.iter().rev().find_map(|e| match &e.notice {
        OrgHostNotice::Engine(OrgNotice::TaskBlocked { task, why }) if task == &t.id => Some(why.clone()),
        _ => None,
    });
    Some(why.unwrap_or_else(|| "理由は再起動で失われました".to_string()))
}

pub fn usage_view(snap: &OrgSnapshot, rows: &[UsageRow]) -> OrgUsageView {
    let rows_without = rows.iter().filter(|r| r.last_total_tokens.is_none()).count() as u32;
    let known: u64 = rows.iter().filter_map(|r| r.last_total_tokens).sum();
    let with_tokens = rows.len() as u32 - rows_without;
    OrgUsageView {
        turns_used: snap.turns_used,
        turns_remaining: snap.limits.max_turns.saturating_sub(snap.turns_used),
        auto_replans_used: snap.auto_replans_used,
        known_tokens: (with_tokens > 0).then_some(known),
        rows: rows.len() as u32,
        rows_without_tokens: rows_without,
    }
}

pub fn workflow_view(input: &ViewInput<'_>) -> OrgWorkflowView {
    let snap = input.snapshot;
    let tasks: Vec<OrgTaskView> = snap.tasks.iter().map(|t| task_view(t, snap, input.notices)).collect();
    let mut halt = snap.halt.map(halt_view);
    // 再開してもチャットに送れず止まったままの利用者に、取れる操作を1行で示す。
    if let Some(h) = halt.as_mut().filter(|h| h.kind == OrgHaltKind::NeedsResume) {
        let unfinished = snap.tasks.iter().filter(|t| !finished(t.status));
        let hint = if snap.tasks.iter().any(|t| t.retryable) {
            "再開しても進まないタスクは「やり直す…」で新しいチャットから送り直せます（送っていないことが確かなものだけ）"
        } else if unfinished.count() > 0 {
            "再開しても進まないときは、送ったか確認できないため再送できません。「取り消す…」で全体を取り消せます"
        } else {
            ""
        };
        if !hint.is_empty() {
            h.label = format!("{}。{hint}", h.label);
        }
    }

    let plan_approval = (snap.halt == Some(HaltReason::AwaitingPlanApproval)).then(|| {
        let latest = input.notices.iter().rev().find_map(|e| match &e.notice {
            OrgHostNotice::Engine(OrgNotice::PlanNeedsApproval { reasons }) => Some(reasons),
            _ => None,
        });
        match latest {
            Some(r) => OrgPlanApprovalView { reasons: r.iter().map(confirm_reason_view).collect(), reasons_known: true },
            None => OrgPlanApprovalView { reasons: Vec::new(), reasons_known: false },
        }
    });
    let sandbox_notice = (snap.halt == Some(HaltReason::SandboxUnhealthy)).then(|| {
        input
            .notices
            .iter()
            .rev()
            .find_map(|e| match &e.notice {
                OrgHostNotice::Engine(OrgNotice::SandboxUnhealthy { detail }) => Some(detail.clone()),
                _ => None,
            })
            .unwrap_or_else(|| crate::rules::org::SANDBOX_UNHEALTHY_NOTICE.to_string())
    });
    let trust = input.notices.iter().rev().find_map(|e| match &e.notice {
        OrgHostNotice::Config(t) => Some(trust_view(t)),
        _ => None,
    });
    let shown: Vec<OrgNoticeView> = input.notices.iter().filter_map(notice_view).collect();
    let skip = shown.len().saturating_sub(NOTICE_SHOWN);
    OrgWorkflowView {
        workflow_id: snap.workflow.0.clone(),
        title: snap.title.clone(),
        request: snap.request.clone(),
        repo: snap.repo.clone(),
        halt,
        limits: OrgLimitsView {
            max_writers: snap.limits.max_writers,
            max_total: snap.limits.max_total,
            max_rework: snap.limits.max_rework,
            max_auto_replans: snap.limits.max_auto_replans,
            max_turns: snap.limits.max_turns,
            max_tasks: snap.limits.max_tasks,
            stagnation_minutes: snap.limits.stagnation_minutes,
        },
        usage: usage_view(snap, input.usage_rows),
        finished: !tasks.is_empty() && snap.tasks.iter().all(|t| finished(t.status)),
        tasks,
        plan_approval,
        sandbox_notice,
        trust: trust.flatten(),
        notices: shown.into_iter().skip(skip).collect(),
        dispatching: input.dispatching,
        readonly: false,
    }
}

// ───────────────────────────── 役割定義 ─────────────────────────────

/// 列挙値の名前（serde の名前。取れなければ Debug）。
fn enum_name<T: serde::Serialize + std::fmt::Debug>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|x| x.as_str().map(str::to_string)).unwrap_or_else(|| format!("{v:?}"))
}

fn role_view(r: &LoadedRole) -> OrgRoleView {
    OrgRoleView {
        id: r.def.id.clone(),
        display_name: r.def.display_name.clone(),
        file: format!("{}.json", r.def.id),
        enabled: r.enabled(),
        widened: r.widened.clone(),
        confirmed: r.confirmed,
        fingerprint: org_engine::permission_fingerprint(&r.def),
        sandbox: match r.def.sandbox {
            SandboxKind::ReadOnly => "readOnly".to_string(),
            SandboxKind::WorkspaceWrite => "workspaceWrite".to_string(),
        },
        approval: enum_name(&r.def.approval),
        workplace: enum_name(&r.def.workplace),
        allow_network: r.def.allow_network,
        extra_writable_dirs: r.def.extra_writable_dirs.clone(),
        model: r.def.model.clone(),
        effort: r.def.effort.clone(),
        max_concurrent: r.def.max_concurrent,
    }
}

pub fn roles_view(report: Option<&RoleReport>) -> OrgRolesView {
    match report {
        None => OrgRolesView { loaded: false, roles: Vec::new(), problems: Vec::new(), created: Vec::new() },
        Some(r) => OrgRolesView {
            loaded: true,
            roles: r.roles.iter().map(role_view).collect(),
            problems: r.problems.iter().map(|p| OrgRoleProblemView { file: p.file.clone(), reasons: p.reasons.clone(), aside: p.aside.clone() }).collect(),
            created: r.created.clone(),
        },
    }
}

/// 役割ファイル名として安全か（フォルダ内の `*.json` の名前だけ。区切り・親参照を受けない）。
pub fn is_plain_role_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && name.ends_with(".json")
}

/// 停滞時間（分）を許す範囲。
pub const STALL_MINUTES_MIN: u32 = 1;
pub const STALL_MINUTES_MAX: u32 = 1440;


#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{Limits, WorkflowId};
    use std::collections::HashSet;

    fn snap(tasks: Vec<TaskSnapshot>, halt: Option<HaltReason>) -> OrgSnapshot {
        OrgSnapshot {
            workflow: WorkflowId::new("wf-1"),
            title: Some("t".into()),
            halt,
            tasks,
            request: "r".into(),
            repo: "C:/repo".into(),
            limits: Limits::default(),
            turns_used: 4,
            auto_replans_used: 0,
        }
    }

    fn task(id: &str, deps: &[&str], status: TaskStatus, confirm: ConfirmState) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId::new(id),
            role: "worker".into(),
            status,
            confirm,
            chat: None,
            title: format!("title {id}"),
            deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
            effects: vec![Effect::PushMergeTag],
            attempts: 2,
            commit: None,
            chat_cwd: None,
            retryable: false,
            never_sent: false,
        }
    }

    fn entry(serial: u64, n: OrgHostNotice) -> NoticeEntry {
        NoticeEntry { serial, notice: n }
    }

    fn view_of(s: &OrgSnapshot, notices: &[NoticeEntry], rows: &[UsageRow]) -> OrgWorkflowView {
        workflow_view(&ViewInput { snapshot: s, notices, usage_rows: rows, dispatching: false })
    }

    #[test]
    fn both_axes_carry_text_and_icon_so_color_is_not_needed() {
        let all_status = [
            TaskStatus::Pending,
            TaskStatus::Ready,
            TaskStatus::Running,
            TaskStatus::Blocked(EBlocked::AwaitingApproval),
            TaskStatus::Blocked(EBlocked::Manual),
            TaskStatus::Blocked(EBlocked::ManualStop),
            TaskStatus::Blocked(EBlocked::Limit),
            TaskStatus::Blocked(EBlocked::NeedsAttention),
            TaskStatus::Reviewing,
            TaskStatus::Rework,
            TaskStatus::Completed,
            TaskStatus::Failed(FailReason::ExecutionFailed),
            TaskStatus::Failed(FailReason::InvalidArtifact),
            TaskStatus::Failed(FailReason::LimitReached),
            TaskStatus::Cancelled,
        ];
        let mut labels = HashSet::new();
        for s in all_status {
            let v = status_view(s);
            assert!(!v.label.is_empty() && !v.icon.is_empty());
            assert!(labels.insert(v.label), "every state has its own label");
        }
        let confirms = [
            ConfirmState::Confirmed,
            ConfirmState::SendUnknown,
            ConfirmState::NeedsReconcile(ReconcileCause::Disconnect),
            ConfirmState::NeedsReconcile(ReconcileCause::Wake),
            ConfirmState::NeedsReconcile(ReconcileCause::Restart),
            ConfirmState::InterruptRequested,
            ConfirmState::StopUnconfirmed,
        ];
        let mut labels = HashSet::new();
        for c in confirms {
            let v = confirm_view(c);
            assert!(!v.label.is_empty() && !v.icon.is_empty());
            assert!(labels.insert(v.label));
        }
        // 失敗・止まりの理由は種類で別の値になる。
        assert_eq!(status_view(TaskStatus::Failed(FailReason::LimitReached)).reason, Some(OrgReason::LimitReached));
        assert_eq!(status_view(TaskStatus::Running).reason, None);
    }

    #[test]
    fn task_view_derives_waiting_attempts_and_actions_without_inventing_values() {
        let s = snap(
            vec![
                task("a", &[], TaskStatus::Completed, ConfirmState::Confirmed),
                task("b", &[], TaskStatus::Running, ConfirmState::Confirmed),
                task("c", &["a", "b"], TaskStatus::Pending, ConfirmState::Confirmed),
                task("m", &[], TaskStatus::Blocked(EBlocked::Manual), ConfirmState::Confirmed),
            ],
            None,
        );
        let v = view_of(&s, &[], &[]);
        let c = v.tasks.iter().find(|t| t.id == "c").unwrap();
        assert_eq!(c.waiting_on, vec!["b".to_string()]);
        assert_eq!((c.attempts, c.max_attempts), (2, 3));
        assert!(c.can_cancel && !c.can_accept_manual);
        assert!(!v.tasks.iter().find(|t| t.id == "a").unwrap().can_cancel, "finished tasks cannot be cancelled");
        assert!(v.tasks.iter().find(|t| t.id == "m").unwrap().can_accept_manual);
        assert!(!v.finished);
        assert_eq!(v.usage.turns_remaining, 26);
        assert_eq!(v.usage.known_tokens, None, "no usage rows: not obtained, not 0");
    }

    #[test]
    fn unknown_chat_creation_asks_the_user_to_choose() {
        let mut t = task("p", &[], TaskStatus::Blocked(EBlocked::NeedsAttention), ConfirmState::SendUnknown);
        t.chat_cwd = Some("C:/repo".into());
        let v = view_of(&snap(vec![t.clone()], None), &[], &[]);
        assert!(v.tasks[0].needs_chat_choice);
        t.chat_cwd = None;
        assert!(!view_of(&snap(vec![t], None), &[], &[]).tasks[0].needs_chat_choice);
    }

    #[test]
    fn a_needs_attention_task_shows_its_reason_or_says_it_was_lost() {
        let blocked = task("w", &[], TaskStatus::Blocked(EBlocked::NeedsAttention), ConfirmState::Confirmed);
        let s = snap(vec![blocked, task("b", &[], TaskStatus::Running, ConfirmState::Confirmed)], None);
        let n = [entry(1, OrgHostNotice::Engine(OrgNotice::TaskBlocked { task: TaskId::new("w"), why: "作業用worktreeを作成できませんでした: fatal".into() }))];
        let v = view_of(&s, &n, &[]);
        assert_eq!(v.tasks[0].attention_note.as_deref(), Some("作業用worktreeを作成できませんでした: fatal"));
        assert_eq!(v.tasks[1].attention_note, None, "only needs-attention tasks carry a reason");
        let lost = view_of(&s, &[], &[]);
        assert_eq!(lost.tasks[0].attention_note.as_deref(), Some("理由は再起動で失われました"));
    }

    #[test]
    fn usage_counts_unknown_rows_separately() {
        let row = |t: Option<u64>| UsageRow {
            schema_version: 1,
            at_ms: 0,
            workflow_id: WorkflowId::new("wf-1"),
            task: None,
            kind: org_engine::TurnKind::Task,
            turns_used: 1,
            last_total_tokens: t,
            thread_total_tokens: None,
            model_context_window: None,
        };
        let s = snap(vec![], None);
        let u = usage_view(&s, &[row(Some(100)), row(None), row(Some(50))]);
        assert_eq!((u.known_tokens, u.rows, u.rows_without_tokens), (Some(150), 3, 1));
        let u = usage_view(&s, &[row(None)]);
        assert_eq!(u.known_tokens, None);
    }

    #[test]
    fn plan_approval_card_lists_the_reasons_and_is_honest_when_they_are_gone() {
        let s = snap(vec![task("a", &[], TaskStatus::Pending, ConfirmState::Confirmed)], Some(HaltReason::AwaitingPlanApproval));
        let notices = vec![entry(
            3,
            OrgHostNotice::Engine(OrgNotice::PlanNeedsApproval {
                reasons: vec![
                    ConfirmReason::DeclaredEffect { task: TaskId::new("a"), effect: Effect::PushMergeTag },
                    ConfirmReason::ParallelismExceeded { requested: 5, max: 3 },
                ],
            }),
        )];
        let v = view_of(&s, &notices, &[]);
        let p = v.plan_approval.unwrap();
        assert!(p.reasons_known && p.reasons.len() == 2);
        assert_eq!(p.reasons[0].task.as_deref(), Some("a"));
        assert!(!v.halt.unwrap().resumable, "a plan awaiting approval is cleared only by approving it");
        // 再起動後（通知が無い）は、理由が分からないことをそのまま示す。
        let p = view_of(&s, &[], &[]).plan_approval.unwrap();
        assert!(!p.reasons_known && p.reasons.is_empty());
        // 承認待ちでなければ出ない。
        assert!(view_of(&snap(vec![], None), &notices, &[]).plan_approval.is_none());
    }

    #[test]
    fn reconcile_trust_sandbox_and_stall_come_from_the_notices() {
        let s = snap(vec![task("w", &[], TaskStatus::Running, ConfirmState::NeedsReconcile(ReconcileCause::Restart))], Some(HaltReason::SandboxUnhealthy));
        let notices = vec![
            entry(1, OrgHostNotice::Reconciled { task: TaskId::new("w"), turn_read: true, turn_end: None, worktree: WorktreeFinding::Uncommitted { entries: 3 } }),
            entry(2, OrgHostNotice::Config(TrustCompare::CodexTrustAppended { projects: vec!["C:/repo".into()] })),
            entry(3, OrgHostNotice::Engine(OrgNotice::SandboxUnhealthy { detail: "sandbox down".into() })),
            entry(4, OrgHostNotice::Stalled { task: TaskId::new("w"), quiet_minutes: 31 }),
        ];
        let v = view_of(&s, &notices, &[]);
        let t = &v.tasks[0];
        let r = t.reconcile.as_ref().unwrap();
        assert!(r.turn_read && r.turn_end.is_none());
        assert_eq!((r.worktree.kind, r.worktree.entries), (OrgWorktreeKind::Uncommitted, Some(3)));
        assert!(r.text.contains("終端を確認できていません"), "a missing end is said, not turned into done: {}", r.text);
        assert!(t.stall_notified);
        assert_eq!(v.sandbox_notice.as_deref(), Some("sandbox down"));
        let trust = v.trust.unwrap();
        assert_eq!((trust.kind, trust.severity), (OrgTrustKind::CodexTrustAppended, OrgSeverity::Info));
        assert!(trust.text.contains("AgentDock は変更していません"));
        // 変更なしの照合は通知の行にも出さない。
        let n = vec![entry(1, OrgHostNotice::Config(TrustCompare::Unchanged))];
        let v = view_of(&snap(vec![], None), &n, &[]);
        assert!(v.trust.is_none() && v.notices.is_empty());
        // 想定外の変更は注意で出し、戻さない。
        let n = vec![entry(1, OrgHostNotice::Config(TrustCompare::Other { changed_lines: 2 }))];
        assert_eq!(view_of(&snap(vec![], None), &n, &[]).trust.unwrap().severity, OrgSeverity::Attention);
    }

    #[test]
    fn notice_text_never_carries_conversation_content() {
        let n = notice_view(&entry(1, OrgHostNotice::Engine(OrgNotice::TaskFailed { task: TaskId::new("t1"), reason: FailReason::InvalidArtifact }))).unwrap();
        assert_eq!((n.kind, n.severity), (OrgNoticeKind::TaskFailed, OrgSeverity::Attention));
        assert!(n.text.contains("t1") && n.text.contains("成果物が不正"));
        let s = notice_view(&entry(2, OrgHostNotice::PersistFailed)).unwrap();
        assert!(s.text.contains("何も送りません"));
    }

    #[test]
    fn roles_view_exposes_reasons_and_a_fingerprint_for_confirmation() {
        let none = roles_view(None);
        assert!(!none.loaded && none.roles.is_empty(), "not loaded is different from loaded and empty");
        let root = std::env::temp_dir().join(format!("agentdock-roleview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let report = super::super::roles::prepare_roles(&root, 1);
        let v = roles_view(Some(&report));
        assert!(v.loaded && v.roles.len() == 3);
        assert!(v.roles.iter().all(|r| r.enabled && r.widened.is_empty() && !r.fingerprint.is_empty() && r.file.ends_with(".json")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn role_file_names_cannot_escape_the_roles_folder() {
        assert!(is_plain_role_file_name("worker.json"));
        for bad in ["", "..", "../x.json", "a/b.json", "a\\b.json", "C:x.json", "worker", "worker.json.bak"] {
            assert!(!is_plain_role_file_name(bad), "{bad}");
        }
    }
}
