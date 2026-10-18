//! 組織実行の画面向けの写し（P-ORG-7a）。エンジンの状態・ホストの通知・役割定義を `backend::org` のビュー型に直す純粋な変換。
//!
//! - 状態は2軸（業務状態・確認状態）を別々に、文字（`label`）とアイコン名（`icon`）で運ぶ。色には頼らない。
//! - 取れていない値は `None`／空にして、0や空文字で代用しない。
//! - 通知の本文は定型文だけ。会話の内容・ツール引数は入れない。


use std::collections::{BTreeSet, HashMap};

use org_engine::graph::{ActivationBound, CompiledGraph, Condition, EdgeDef, LoopBody, NodeId, NodeKind as GNode, OnExhausted, SlotId, WorkspaceKind};
use org_engine::{
    BlockedReason as EBlocked, ConfirmReason, ConfirmState, Effect, FailReason, HaltReason, LoadedRole, OrgNotice, ReconcileCause, SandboxKind, TaskId,
    TaskStatus, UsageRow,
};
use serde_json::Value;

use super::driver::{GraphSnapshot, OrgSnapshot, TaskSnapshot};
use super::graphs::{start_blocker, GraphReport};
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
                EBlocked::RoundLimit => (OrgReason::RoundLimit, "止まっています（やり直しの上限）"),
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
        HaltReason::DefinitionUnavailable => (OrgHaltKind::DefinitionUnavailable, "開始時のグラフ定義の写しを使えないため、このワークフローは進められません（再開できません）", "alert-triangle", false),
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
        OrgHostNotice::SlotWriteFailed { slot } => (
            K::SlotWriteFailed,
            Attention,
            None,
            format!("途中の出力（スロット「{slot}」）を保存できないため、以後は何も送りません。AgentDock を再起動してください"),
        ),
        OrgHostNotice::DefinitionNote { message, attention } => (K::DefinitionNote, if *attention { Attention } else { Info }, None, message.clone()),
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
    /// グラフの判定・gate の表示に使うスロットの値（`graph_slot_needs` が挙げた分を読んだもの。読めなかった版は入れない）。
    pub slots: &'a HashMap<(SlotId, u32), Value>,
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
        graph: snap.graph.as_ref().map(|gs| graph_view(gs, &snap.tasks, &tasks, input.slots)),
        tasks,
        plan_approval,
        sandbox_notice,
        trust: trust.flatten(),
        notices: shown.into_iter().skip(skip).collect(),
        dispatching: input.dispatching,
        readonly: false,
        archived_at: None,
        deletion: None,
    }
}

// ───────────────────────────── グラフ（流れ）の表示 ─────────────────────────────

/// 判定の履歴として読む版の数（新しい方から）。
const VERDICT_HISTORY: u32 = 12;
/// gate が見せるスロットの内容の長さの上限（文字）。
const GATE_SHOW_CHARS: usize = 4000;
/// 判定として出す文字列の長さの上限（文字）。
const VERDICT_CHARS: usize = 80;
/// 1 turn あたりの土台のトークン数（目安。見積りの表示だけに使う）。
pub const TOKENS_PER_TURN_HINT: u32 = 17_000;

/// 工程が書くスロット（agent の出力、または gate の自分のID）。
fn output_slot(g: &CompiledGraph, node: &NodeId) -> Option<SlotId> {
    match &g.node(node)?.kind {
        GNode::Agent { output, .. } => Some(output.clone()),
        GNode::Gate { .. } => Some(SlotId::new(node.0.clone())),
        _ => None,
    }
}

/// 工程の「判定」＝出力スロットのうち、辺の条件が見る項目。見る辺がなければ None（gate は選択 `choice`）。
fn verdict_path(g: &CompiledGraph, node: &NodeId) -> Option<(SlotId, String)> {
    let slot = output_slot(g, node)?;
    let from_edge = g.outgoing(node).find_map(|e| e.when.as_ref().filter(|w| w.slot == slot).map(|w| w.path.clone()));
    from_edge.or_else(|| matches!(g.node(node)?.kind, GNode::Gate { .. }).then(|| "choice".to_string())).map(|p| (slot, p))
}

fn at_path<'v>(root: &'v Value, path: &str) -> Option<&'v Value> {
    path.split('.').try_fold(root, |cur, part| cur.as_object()?.get(part))
}

fn short(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => short(s, VERDICT_CHARS),
        other => short(&other.to_string(), VERDICT_CHARS),
    }
}

/// スロットの `version` の値から判定を読む。版がなければ「まだ出力がない」、あっても値や項目が読めなければ「未取得」。
fn verdict_of(slot: &SlotId, path: &str, version: Option<u32>, slots: &HashMap<(SlotId, u32), Value>) -> OrgVerdict {
    let Some(version) = version else { return OrgVerdict::Pending };
    match slots.get(&(slot.clone(), version)).and_then(|v| at_path(v, path)) {
        Some(v) => OrgVerdict::Value { text: value_text(v) },
        None => OrgVerdict::NotObtained,
    }
}

/// 判定値の日本語訳（画面のため）。未知の値は元のまま。画面側（`org.ts` の `verdictJa`）と同じ対応。
fn verdict_ja(v: &str) -> &str {
    match v {
        "rework" => "やり直し",
        "pass" => "合格",
        "replan" => "計画からやり直し",
        other => other,
    }
}

/// 辺の条件の文言。項目名 `verdict` は「判定」、判定値は訳す（それ以外の項目名・値はそのまま）。
fn condition_label(c: Option<&Condition>) -> String {
    let Some(c) = c else { return "常に".to_string() };
    let show = |v: &Value| match v {
        Value::String(s) => verdict_ja(s).to_string(),
        other => other.to_string(),
    };
    let path = if c.path == "verdict" { "判定" } else { c.path.as_str() };
    if let Some(x) = &c.eq {
        format!("{} = {}", path, show(x))
    } else if let Some(x) = &c.ne {
        format!("{} ≠ {}", path, show(x))
    } else if let Some(xs) = &c.in_ {
        format!("{} が {} のいずれか", path, xs.iter().map(show).collect::<Vec<_>>().join("・"))
    } else {
        format!("{}（条件が不完全）", path)
    }
}

/// 差し戻しの辺の（使った回数, 利用者が足した回数）。
fn feedback_counts(gs: &GraphSnapshot, edge: &org_engine::graph::EdgeId) -> (u32, u32) {
    gs.feedback.get(edge).copied().unwrap_or((0, 0))
}

/// 周回の上限に達して止まっている戻り先の工程の辺（`node` が戻り先で、使った回数が許容回数に届いているもの）。
pub fn round_limit_of<'g>(gs: &'g GraphSnapshot, node: &NodeId) -> Option<(&'g LoopBody, u32, u32)> {
    gs.graph.loops.iter().find_map(|l| {
        if &l.target != node {
            return None;
        }
        let (used, extra) = feedback_counts(gs, &l.edge);
        let allowed = l.max_rounds + extra;
        (used >= allowed).then_some((l, used, allowed))
    })
}

/// gate の判断が受け付けられるか（工程が gate で、選択肢にあり、理由が必須なら書かれている）。エンジンも同じ確認をする。
pub fn gate_check(g: &CompiledGraph, node: &NodeId, choice: &str, note: Option<&str>) -> Result<(), String> {
    let Some(GNode::Gate { choices, .. }) = g.node(node).map(|d| &d.kind) else {
        return Err("判断を求めている工程ではありません".to_string());
    };
    let Some(c) = choices.iter().find(|c| c.id == choice) else {
        return Err("選択肢にない判断です".to_string());
    };
    if c.needs_note && note.is_none_or(|n| n.trim().is_empty()) {
        return Err("この選択には理由が必要です".to_string());
    }
    Ok(())
}

/// 前向きの辺での深さ（入口が0）。周回のある定義でも止まるように、ノード数で打ち切る。
fn depth_of(g: &CompiledGraph, node: &NodeId) -> usize {
    let mut d = 0;
    let mut cur = node;
    while let Some(Some(p)) = g.parent.get(cur) {
        d += 1;
        cur = p;
        if d > g.def.nodes.len() {
            break;
        }
    }
    d
}

/// 判定や gate の表示のためにホストが読むスロットの版（重複なし）。画面の更新のたびに読むので、版の数は小さく抑える。
pub fn graph_slot_needs(gs: &GraphSnapshot) -> Vec<(SlotId, u32)> {
    let g = &gs.graph;
    let mut need: BTreeSet<(SlotId, u32)> = BTreeSet::new();
    let latest = |slot: &SlotId, need: &mut BTreeSet<(SlotId, u32)>| {
        if let Some(v) = gs.slot_versions.get(slot) {
            need.insert((slot.clone(), *v));
        }
    };
    for n in &g.def.nodes {
        if let Some((slot, _)) = verdict_path(g, &n.id) {
            latest(&slot, &mut need);
        }
        if let GNode::Gate { shows, .. } = &n.kind {
            for s in shows {
                latest(s, &mut need);
            }
        }
    }
    for l in &g.loops {
        let Some(c) = g.def.edges.iter().find(|e| e.id() == l.edge).and_then(|e| e.when.as_ref()) else { continue };
        if let Some(top) = gs.slot_versions.get(&c.slot) {
            for v in top.saturating_sub(VERDICT_HISTORY - 1).max(1)..=*top {
                need.insert((c.slot.clone(), v));
            }
        }
    }
    need.into_iter().collect()
}

fn edge_view(gs: &GraphSnapshot, e: &EdgeDef, tasks: &[TaskSnapshot]) -> OrgGraphEdgeView {
    let label = condition_label(e.when.as_ref());
    match &e.feedback {
        Some(fb) => {
            let (used, extra) = feedback_counts(gs, &e.id());
            OrgGraphEdgeView { from: e.from.0.clone(), to: e.to.0.clone(), kind: OrgEdgeKind::Feedback, label, used, allowed: Some(fb.max_rounds + extra) }
        }
        None => {
            // 通った辺＝行き先の工程のタスクが、元の工程のタスクを依存にして作られている。
            let task_of = |n: &NodeId| tasks.iter().find(|t| t.node.as_ref() == Some(n));
            let taken = match (task_of(&e.from), task_of(&e.to)) {
                (Some(f), Some(t)) => t.deps.contains(&f.id),
                _ => false,
            };
            OrgGraphEdgeView { from: e.from.0.clone(), to: e.to.0.clone(), kind: OrgEdgeKind::Forward, label, used: u32::from(taken), allowed: None }
        }
    }
}

/// グラフで動くワークフローの流れの写し。ノードは先へ進む順（深さ、同じなら定義の順）。
pub fn graph_view(gs: &GraphSnapshot, tasks: &[TaskSnapshot], task_views: &[OrgTaskView], slots: &HashMap<(SlotId, u32), Value>) -> OrgGraphView {
    let g = &gs.graph;
    let mut order: Vec<usize> = (0..g.def.nodes.len()).collect();
    order.sort_by_key(|i| (depth_of(g, &g.def.nodes[*i].id), *i));
    let nodes = order
        .into_iter()
        .map(|i| {
            let n = &g.def.nodes[i];
            let task = tasks.iter().find(|t| t.node.as_ref() == Some(&n.id));
            let tv = task.and_then(|t| task_views.iter().find(|v| v.id == t.id.0));
            let (kind, role_id) = match &n.kind {
                GNode::Agent { role, .. } => (OrgNodeKind::Agent, Some(role.0.clone())),
                GNode::Gate { .. } => (OrgNodeKind::Gate, None),
                GNode::Router => (OrgNodeKind::Router, None),
                GNode::PlannedTasks { .. } => (OrgNodeKind::PlannedTasks, None),
            };
            // 周回の中の工程は、いちばん内側の周回の「第N周／最大M周」。
            let inner = g.loops.iter().filter(|l| l.members.contains(&n.id)).min_by_key(|l| l.members.len());
            let (round, max_round) = match (task, inner) {
                (Some(_), Some(l)) => {
                    let (used, extra) = feedback_counts(gs, &l.edge);
                    (Some(used + 1), Some(l.max_rounds + extra + 1))
                }
                _ => (None, None),
            };
            let last_verdict = verdict_path(g, &n.id).map(|(slot, path)| verdict_of(&slot, &path, gs.slot_versions.get(&slot).copied(), slots));
            let confirmed = tv.is_some_and(|v| v.confirm.state == OrgConfirmCode::Confirmed);
            let phase_reason = tv.map(|v| (v.status.phase, v.status.reason));
            let gate = match &n.kind {
                GNode::Gate { prompt, choices, shows } => Some(OrgGateView {
                    prompt: prompt.clone(),
                    choices: choices.iter().map(|c| OrgGateChoiceView { id: c.id.clone(), label: c.label.clone(), needs_note: c.needs_note }).collect(),
                    shows: shows
                        .iter()
                        .map(|s| {
                            let text = gs.slot_versions.get(s).and_then(|v| slots.get(&(s.clone(), *v))).map(|v| serde_json::to_string_pretty(v).unwrap_or_default());
                            let truncated = text.as_ref().is_some_and(|t| t.chars().count() > GATE_SHOW_CHARS);
                            OrgGateShowView { slot: s.0.clone(), text: text.map(|t| short(&t, GATE_SHOW_CHARS)), truncated }
                        })
                        .collect(),
                    awaiting: confirmed && phase_reason == Some((OrgTaskPhase::Blocked, Some(OrgReason::AwaitingApproval))),
                }),
                _ => None,
            };
            let round_limit = if phase_reason.is_some_and(|(p, r)| p == OrgTaskPhase::Blocked && r == Some(OrgReason::RoundLimit)) {
                round_limit_of(gs, &n.id).map(|(l, used, allowed)| OrgRoundLimitView { edge: l.edge.0.clone(), used, allowed, awaiting: confirmed })
            } else {
                None
            };
            OrgGraphNodeView {
                id: n.id.0.clone(),
                label: n.label.clone().unwrap_or_else(|| n.id.0.clone()),
                kind,
                role_id,
                task: task.map(|t| t.id.0.clone()),
                status: tv.map(|v| v.status.clone()),
                confirm: tv.map(|v| v.confirm.clone()),
                round,
                max_round,
                last_verdict,
                gate,
                round_limit,
            }
        })
        .collect();
    let edges = g.def.edges.iter().map(|e| edge_view(gs, e, tasks)).collect();
    let rounds = g
        .loops
        .iter()
        .filter_map(|l| {
            let e = g.def.edges.iter().find(|e| e.id() == l.edge)?;
            let fb = e.feedback.as_ref()?;
            let (used, extra) = feedback_counts(gs, &l.edge);
            let history = match e.when.as_ref() {
                Some(c) => {
                    let top = gs.slot_versions.get(&c.slot).copied().unwrap_or(0);
                    (top.saturating_sub(VERDICT_HISTORY - 1).max(1)..=top)
                        .map(|v| OrgRoundEntryView { index: v, verdict: verdict_of(&c.slot, &c.path, Some(v), slots) })
                        .collect()
                }
                None => Vec::new(),
            };
            let on_exhausted = match fb.on_exhausted {
                OnExhausted::Fail => OrgOnExhausted::Fail,
                OnExhausted::Ask => OrgOnExhausted::Ask,
            };
            Some(OrgRoundView { edge: l.edge.0.clone(), from: l.source.0.clone(), to: l.target.0.clone(), used, max_rounds: fb.max_rounds, extra, on_exhausted, history })
        })
        .collect();
    OrgGraphView { graph_id: g.def.id.clone(), name: g.def.display_name.clone(), nodes, edges, rounds }
}

// ───────────────────────────── グラフ定義（テンプレート）の一覧 ─────────────────────────────

/// 見積り。トークンは最大の turn 数 × 1 turn あたりの土台（目安）。
pub fn estimate_view(b: &ActivationBound) -> OrgEstimateView {
    OrgEstimateView {
        activations: b.activations,
        turns_typical: b.turns_typical,
        turns_max: b.turns_max,
        tokens_max: b.turns_max.saturating_mul(TOKENS_PER_TURN_HINT),
    }
}

fn template_view(g: &CompiledGraph) -> OrgGraphTemplateView {
    let blocker = start_blocker(g);
    OrgGraphTemplateView {
        id: g.def.id.clone(),
        name: g.def.display_name.clone(),
        description: g.def.description.clone(),
        workspace: match g.def.workspace {
            WorkspaceKind::Git => OrgWorkspaceKind::Git,
            WorkspaceKind::Folder => OrgWorkspaceKind::Folder,
        },
        roles: g.roles_used().into_iter().map(|r| r.0).collect(),
        file: format!("{}.json", g.def.id),
        selectable: blocker.is_none(),
        reasons: blocker.into_iter().map(str::to_string).collect(),
        estimate: estimate_view(&g.bound),
        suggested_max_turns: g.def.suggested_max_turns,
    }
}

pub fn graphs_view(report: Option<&GraphReport>) -> OrgGraphsView {
    let limits = org_engine::Limits::default();
    let (loaded, templates, problems, created) = match report {
        None => (false, Vec::new(), Vec::new(), Vec::new()),
        Some(r) => (
            true,
            r.graphs.iter().map(template_view).collect(),
            r.problems.iter().map(|p| OrgGraphProblemView { file: p.file.clone(), reasons: p.reasons.clone() }).collect(),
            r.created.clone(),
        ),
    };
    OrgGraphsView {
        loaded,
        templates,
        problems,
        created,
        default_max_turns: limits.max_turns,
        max_turns_limit: super::START_MAX_TURNS,
        tokens_per_turn_hint: TOKENS_PER_TURN_HINT,
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
            graph: None,
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
            node: None,
        }
    }

    fn entry(serial: u64, n: OrgHostNotice) -> NoticeEntry {
        NoticeEntry { serial, notice: n }
    }

    fn view_of(s: &OrgSnapshot, notices: &[NoticeEntry], rows: &[UsageRow]) -> OrgWorkflowView {
        workflow_view(&ViewInput { snapshot: s, notices, usage_rows: rows, slots: &HashMap::new(), dispatching: false })
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
        assert!(v.loaded && v.roles.len() == 5);
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

    // ───────────── グラフの表示 ─────────────

    const GATE_GRAPH: &str = r#"{
      "schemaVersion": 1, "id": "gated", "displayName": "g", "workspace": "git", "entry": "draft",
      "slots": { "spec": { "schema": "spec" } },
      "nodes": [
        { "id": "draft", "kind": "agent", "role": "planner", "output": "spec" },
        { "id": "confirm", "kind": "gate", "prompt": "OK?", "shows": ["spec"], "choices": [
            { "id": "ok", "label": "ok" }, { "id": "fix", "label": "fix", "needsNote": true } ] },
        { "id": "end", "kind": "router" }
      ],
      "edges": [
        { "from": "draft", "to": "confirm" },
        { "from": "confirm", "to": "end", "when": { "slot": "confirm", "path": "choice", "eq": "ok" } },
        { "from": "confirm", "to": "draft", "when": { "slot": "confirm", "path": "choice", "eq": "fix" },
          "feedback": { "maxRounds": 2, "onExhausted": "ask" } }
      ]
    }"#;

    fn compile(text: &str, id: &str) -> CompiledGraph {
        let def = org_engine::graph::parse_graph(text).unwrap();
        let roles = ["planner", "worker", "verifier", "reviewer"]
            .iter()
            .map(|r| {
                let workplace = match *r {
                    "worker" => org_engine::Workplace::OwnWorktree,
                    "verifier" | "reviewer" => org_engine::Workplace::DetachedAtCommit,
                    _ => org_engine::Workplace::SharedReadOnly,
                };
                (org_engine::RoleId::new(*r), org_engine::graph::RoleInfo { writes: *r == "worker", workplace })
            })
            .collect();
        org_engine::graph::validate(&def, &roles, &org_engine::outschema::builtin_schemas(), id, &org_engine::Limits::default()).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn quality() -> CompiledGraph {
        compile(org_engine::graph::template_json(org_engine::graph::TEMPLATE_QUALITY_LOOP).unwrap(), "quality-loop")
    }

    fn gtask(id: &str, node: &str, parent: Option<&str>, status: TaskStatus, confirm: ConfirmState) -> TaskSnapshot {
        let mut t = task(id, &parent.into_iter().collect::<Vec<_>>(), status, confirm);
        t.node = Some(NodeId::new(node));
        t.effects.clear();
        t
    }

    fn gsnap(g: CompiledGraph, feedback: &[(&str, (u32, u32))], versions: &[(&str, u32)], tasks: Vec<TaskSnapshot>) -> OrgSnapshot {
        let mut s = snap(tasks, None);
        s.graph = Some(GraphSnapshot {
            graph: g,
            feedback: feedback.iter().map(|(e, c)| (org_engine::graph::EdgeId::new(*e), *c)).collect(),
            slot_versions: versions.iter().map(|(sl, v)| (SlotId::new(*sl), *v)).collect(),
        });
        s
    }

    fn slot_map(items: &[(&str, u32, Value)]) -> HashMap<(SlotId, u32), Value> {
        items.iter().map(|(s, v, val)| ((SlotId::new(*s), *v), val.clone())).collect()
    }

    #[test]
    fn the_estimate_is_computed_from_the_definition_and_the_hint() {
        let g = quality();
        let e = estimate_view(&g.bound);
        assert_eq!((e.activations, e.turns_typical, e.turns_max), (g.bound.activations, g.bound.turns_typical, g.bound.turns_max));
        assert_eq!(e.turns_max, e.activations * 2, "worst case: one format fix for each turn");
        assert_eq!(e.tokens_max, e.turns_max * TOKENS_PER_TURN_HINT);
        assert_eq!(TOKENS_PER_TURN_HINT, 17_000);
    }

    #[test]
    fn the_templates_list_says_what_cannot_be_chosen_and_why() {
        let mut report = GraphReport::default();
        report.graphs.push(quality());
        report.graphs.push(compile(org_engine::graph::template_json(org_engine::graph::TEMPLATE_STANDARD).unwrap(), "standard"));
        report.problems.push(super::super::graphs::GraphProblem { file: "bad.json".into(), reasons: vec!["V4: x".into()] });
        let v = graphs_view(Some(&report));
        assert!(v.loaded && v.templates.len() == 2 && v.problems.len() == 1);
        assert!(v.templates.iter().all(|t| t.selectable && t.reasons.is_empty()), "both built-in templates can start");
        assert_eq!(v.templates[0].roles, vec!["planner".to_string(), "verifier".to_string(), "worker".to_string()]);
        assert_eq!((v.default_max_turns, v.max_turns_limit), (30, 200));
        // not loaded is different from loaded and empty
        let none = graphs_view(None);
        assert!(!none.loaded && none.templates.is_empty());
        // a folder template cannot be started yet: shown, but not selectable, with the reason
        let mut folder = quality();
        folder.def.workspace = WorkspaceKind::Folder;
        let t = template_view(&folder);
        assert!(!t.selectable && t.reasons.len() == 1);
    }

    #[test]
    fn the_graph_view_shows_rounds_verdicts_and_untaken_branches() {
        let tasks = vec![
            gtask("plan-1", "plan", None, TaskStatus::Completed, ConfirmState::Confirmed),
            gtask("build-1", "build", Some("plan-1"), TaskStatus::Running, ConfirmState::Confirmed),
            gtask("verify-1", "verify", Some("build-1"), TaskStatus::Reviewing, ConfirmState::Confirmed),
        ];
        let s = gsnap(quality(), &[("verify->build", (1, 0))], &[("spec", 1), ("report", 2), ("verdict", 2)], tasks);
        let slots = slot_map(&[("verdict", 1, serde_json::json!({ "verdict": "rework" })), ("verdict", 2, serde_json::json!({ "verdict": "pass" }))]);
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &slots, dispatching: false });
        let g = v.graph.expect("graph workflow carries its flow");
        assert_eq!(g.graph_id, "quality-loop");
        assert_eq!(g.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), vec!["plan", "build", "verify"], "forward order");
        let build = &g.nodes[1];
        assert_eq!((build.round, build.max_round), (Some(2), Some(4)), "second round of at most four passes");
        assert_eq!(g.nodes[2].last_verdict, Some(OrgVerdict::Value { text: "pass".into() }));
        assert_eq!(g.nodes[1].last_verdict, None, "build has no verdict");
        let fb = g.edges.iter().find(|e| e.from == "verify" && e.to == "build").unwrap();
        assert_eq!((fb.kind, fb.used, fb.allowed), (OrgEdgeKind::Feedback, 1, Some(3)));
        let replan = g.edges.iter().find(|e| e.to == "plan").unwrap();
        assert_eq!((replan.used, replan.allowed), (0, Some(1)), "an untaken branch stays at 0");
        let fwd = g.edges.iter().find(|e| e.from == "plan" && e.to == "build").unwrap();
        assert_eq!((fwd.kind, fwd.used, fwd.allowed), (OrgEdgeKind::Forward, 1, None));
        let r = g.rounds.iter().find(|r| r.to == "build").unwrap();
        assert_eq!((r.used, r.max_rounds, r.extra), (1, 3, 0));
        assert_eq!(r.history.iter().map(|h| (h.index, h.verdict.clone())).collect::<Vec<_>>(), vec![
            (1, OrgVerdict::Value { text: "rework".into() }),
            (2, OrgVerdict::Value { text: "pass".into() }),
        ]);
    }

    #[test]
    fn an_unreadable_slot_is_not_obtained_and_a_missing_one_is_pending() {
        let tasks = vec![
            gtask("plan-1", "plan", None, TaskStatus::Completed, ConfirmState::Confirmed),
            gtask("build-1", "build", Some("plan-1"), TaskStatus::Completed, ConfirmState::Confirmed),
            gtask("verify-1", "verify", Some("build-1"), TaskStatus::Running, ConfirmState::Confirmed),
        ];
        // verdict has a version but its file could not be read; nothing was written for the others
        let s = gsnap(quality(), &[], &[("verdict", 1)], tasks.clone());
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &HashMap::new(), dispatching: false });
        let g = v.graph.unwrap();
        assert_eq!(g.nodes[2].last_verdict, Some(OrgVerdict::NotObtained), "read failure is shown as not obtained, never as an empty value");
        assert_eq!(g.rounds[0].history, vec![OrgRoundEntryView { index: 1, verdict: OrgVerdict::NotObtained }]);
        let s = gsnap(quality(), &[], &[], tasks);
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &HashMap::new(), dispatching: false });
        let g = v.graph.unwrap();
        assert_eq!(g.nodes[2].last_verdict, Some(OrgVerdict::Pending));
        assert!(g.rounds[0].history.is_empty());
    }

    #[test]
    fn slots_to_read_are_the_latest_verdicts_gate_shows_and_a_bounded_history() {
        let s = gsnap(quality(), &[], &[("spec", 1), ("report", 4), ("verdict", 30)], vec![]);
        let need = graph_slot_needs(s.graph.as_ref().unwrap());
        assert!(need.iter().all(|(slot, _)| slot.0 == "verdict"), "only the verdict is read: {need:?}");
        assert_eq!(need.len(), 12, "a bounded history, newest 12 versions");
        assert_eq!(need.iter().map(|(_, v)| *v).max(), Some(30));
        let s = gsnap(compile(GATE_GRAPH, "gated"), &[], &[("spec", 3), ("confirm", 1)], vec![]);
        let need = graph_slot_needs(s.graph.as_ref().unwrap());
        assert!(need.contains(&(SlotId::new("confirm"), 1)) && need.contains(&(SlotId::new("spec"), 3)), "gate decision and the slot it shows");
    }

    #[test]
    fn the_gate_card_is_actionable_only_while_it_waits_for_the_user_and_is_confirmed() {
        use super::super::ipc::check_gate_decision;
        let wait = TaskStatus::Blocked(EBlocked::AwaitingApproval);
        let mk = |confirm| vec![gtask("draft-1", "draft", None, TaskStatus::Completed, ConfirmState::Confirmed), gtask("confirm-1", "confirm", Some("draft-1"), wait, confirm)];
        let slots = slot_map(&[("spec", 1, serde_json::json!({ "goal": "x" }))]);
        let s = gsnap(compile(GATE_GRAPH, "gated"), &[], &[("spec", 1)], mk(ConfirmState::Confirmed));
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &slots, dispatching: false });
        let gate = v.graph.as_ref().unwrap().nodes.iter().find(|n| n.id == "confirm").unwrap().gate.clone().unwrap();
        assert!(gate.awaiting && gate.choices.len() == 2 && gate.choices[1].needs_note);
        assert!(gate.shows[0].text.as_deref().is_some_and(|t| t.contains("goal")));
        // unconfirmed (e.g. needs reconciling): shown, but not actionable
        let s2 = gsnap(compile(GATE_GRAPH, "gated"), &[], &[("spec", 1)], mk(ConfirmState::NeedsReconcile(ReconcileCause::Restart)));
        let v = workflow_view(&ViewInput { snapshot: &s2, notices: &[], usage_rows: &[], slots: &slots, dispatching: false });
        assert!(!v.graph.unwrap().nodes.iter().find(|n| n.id == "confirm").unwrap().gate.as_ref().unwrap().awaiting);
        // not obtained slot content stays None
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &HashMap::new(), dispatching: false });
        assert_eq!(v.graph.unwrap().nodes.iter().find(|n| n.id == "confirm").unwrap().gate.as_ref().unwrap().shows[0].text, None);

        // the host-side check mirrors what the engine accepts
        let gs = s.graph.as_ref().unwrap();
        let t = &s.tasks[1];
        assert!(check_gate_decision(gs, t, "ok", None).is_ok());
        assert!(check_gate_decision(gs, t, "fix", None).is_err(), "a required reason");
        assert!(check_gate_decision(gs, t, "fix", Some("   ")).is_err(), "blank is not a reason");
        assert!(check_gate_decision(gs, t, "fix", Some("shorter")).is_ok());
        assert!(check_gate_decision(gs, t, "nope", None).is_err());
        assert!(check_gate_decision(gs, &s.tasks[0], "ok", None).is_err(), "not a gate");
        let gs2 = s2.graph.as_ref().unwrap();
        assert!(check_gate_decision(gs2, &s2.tasks[1], "ok", None).is_err(), "unconfirmed gate is not decided");
    }

    #[test]
    fn one_more_round_is_offered_only_for_the_target_stopped_at_the_limit() {
        use super::super::ipc::check_extra_round;
        let stopped = TaskStatus::Blocked(EBlocked::RoundLimit);
        let mk = |confirm| {
            vec![
                gtask("plan-1", "plan", None, TaskStatus::Completed, ConfirmState::Confirmed),
                gtask("build-1", "build", Some("plan-1"), stopped, confirm),
                gtask("verify-1", "verify", Some("build-1"), TaskStatus::Reviewing, ConfirmState::Confirmed),
            ]
        };
        let s = gsnap(quality(), &[("verify->build", (3, 0))], &[], mk(ConfirmState::Confirmed));
        let v = workflow_view(&ViewInput { snapshot: &s, notices: &[], usage_rows: &[], slots: &HashMap::new(), dispatching: false });
        let g = v.graph.unwrap();
        let build = g.nodes.iter().find(|n| n.id == "build").unwrap();
        let limit = build.round_limit.clone().expect("the stopped target carries the limit card");
        assert_eq!((limit.edge.as_str(), limit.used, limit.allowed, limit.awaiting), ("verify->build", 3, 3, true));
        assert!(g.nodes.iter().filter(|n| n.id != "build").all(|n| n.round_limit.is_none()));
        assert_eq!(build.status.as_ref().unwrap().reason, Some(OrgReason::RoundLimit));
        let gs = s.graph.as_ref().unwrap();
        assert!(check_extra_round(gs, &s.tasks[1]).is_ok());
        assert!(check_extra_round(gs, &s.tasks[0]).is_err(), "a completed task");
        assert!(check_extra_round(gs, &s.tasks[2]).is_err(), "the source of the loop is not the card");
        // a user-added round raises the allowance, and with rounds left there is no card
        let s_more = gsnap(quality(), &[("verify->build", (3, 1))], &[], mk(ConfirmState::Confirmed));
        assert!(check_extra_round(s_more.graph.as_ref().unwrap(), &s_more.tasks[1]).is_err());
        // an unconfirmed stop is shown but cannot be acted on
        let s_unc = gsnap(quality(), &[("verify->build", (3, 0))], &[], mk(ConfirmState::NeedsReconcile(ReconcileCause::Wake)));
        assert!(check_extra_round(s_unc.graph.as_ref().unwrap(), &s_unc.tasks[1]).is_err());
        // "accept this result" is offered under the same conditions
        use super::super::ipc::check_accept_round;
        assert!(check_accept_round(gs, &s.tasks[1]).is_ok());
        assert!(check_accept_round(gs, &s.tasks[0]).is_err());
        assert!(check_accept_round(gs, &s.tasks[2]).is_err());
        assert!(check_accept_round(s_more.graph.as_ref().unwrap(), &s_more.tasks[1]).is_err());
        assert!(check_accept_round(s_unc.graph.as_ref().unwrap(), &s_unc.tasks[1]).is_err());
    }

    #[test]
    fn an_unavailable_definition_halt_cannot_be_resumed() {
        let h = halt_view(HaltReason::DefinitionUnavailable);
        assert_eq!(h.kind, OrgHaltKind::DefinitionUnavailable);
        assert!(!h.resumable);
    }

    #[test]
    fn definition_notices_have_their_own_kinds() {
        let a = notice_view(&entry(1, OrgHostNotice::DefinitionNote { message: "m".into(), attention: true })).unwrap();
        assert_eq!((a.kind, a.severity), (OrgNoticeKind::DefinitionNote, OrgSeverity::Attention));
        let b = notice_view(&entry(2, OrgHostNotice::SlotWriteFailed { slot: "report".into() })).unwrap();
        assert_eq!((b.kind, b.severity), (OrgNoticeKind::SlotWriteFailed, OrgSeverity::Attention));
        assert_eq!(status_view(TaskStatus::Blocked(EBlocked::RoundLimit)).reason, Some(OrgReason::RoundLimit));
    }
}
