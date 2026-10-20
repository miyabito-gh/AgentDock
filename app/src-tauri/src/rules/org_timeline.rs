//! 組織実行の進行タイムライン（DESIGN_ORG_R2 1）。`events.jsonl` の記録を人間向けの行に整える純粋な畳み込み。
//!
//! 1行ずつ `push` する。連続した計画のタスク追加は1行にまとめる（直前の行の置き換え）。重複する行・内部だけの記録
//! （送信の回数・チャットの作成・差し戻しの指摘の保存など）は出さない。取れない値は推測で埋めず、その行を出さない。
//! タスクごとの「今の業務状態に入った時刻」`since`（経過時間の元）と確認状態も、この畳み込みが持つ。

use std::collections::{HashMap, HashSet};

use org_engine::{
    BlockedReason, Cause, ConfirmState, FailReason, HaltReason, InterjectOutcome, NoteId, NoteScope, OrgEvent, OrgEventKind, ReconcileCause, SettleOutcome,
    TaskId, TaskStatus, TurnEndKind, TurnKind,
};

use crate::backend::model::UnixMillis;
use crate::backend::org::{OrgActor, OrgActorKind, OrgTimelineEntry, OrgTone};

/// 1つの文の長さの上限（文字）。
const TEXT_MAX: usize = 120;
/// 引用（依頼・指示の本文）の長さ。
const QUOTE_MAX: usize = 80;

/// `push` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldChange {
    /// 出さない記録。
    None,
    Add(OrgTimelineEntry),
    /// 直前の行を置き換える（連続した計画のタスク追加のまとめ）。
    ReplaceLast(OrgTimelineEntry),
}

#[derive(Debug, Clone, Default)]
struct Meta {
    role: String,
}

/// 連続したタスク追加の途中経過。
#[derive(Debug, Clone)]
struct AddRun {
    first_id: String,
    first_title: String,
    count: u32,
    seq: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineFold {
    tasks: HashMap<TaskId, Meta>,
    /// 今の業務状態に入った時刻。
    since: HashMap<TaskId, (i64, TaskStatus)>,
    confirm: HashMap<TaskId, ConfirmState>,
    rework: HashMap<TaskId, u32>,
    retries: HashMap<TaskId, u32>,
    skipped: HashSet<TaskId>,
    interjects: HashMap<NoteId, TaskId>,
    max_retries: u32,
    halt: Option<HaltReason>,
    run: Option<AddRun>,
    /// 利用者の操作の行がすでに説明した「再開（Ready）」を、もう一度出さないための印。
    quiet_ready: Option<TaskId>,
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut it = one.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

fn role_label(role: &str) -> String {
    match role {
        "planner" => "Planner".into(),
        "worker" => "Worker".into(),
        "reviewer" => "Reviewer".into(),
        "gate" => "ゲート".into(),
        "router" => "ルーター".into(),
        other => other.to_string(),
    }
}

fn blocked_label(r: BlockedReason) -> &'static str {
    match r {
        BlockedReason::AwaitingApproval => "承認待ち",
        BlockedReason::Manual => "手動に切替",
        BlockedReason::ManualStop => "中断",
        BlockedReason::Limit => "上限",
        BlockedReason::NeedsAttention => "要対応",
        BlockedReason::UsageLimit => "利用上限",
        BlockedReason::TurnFailed => "turnが失敗",
        BlockedReason::RoundLimit => "やり直しの上限",
        BlockedReason::ReworkLimit => "差し戻しの上限",
    }
}

fn fail_label(r: FailReason) -> &'static str {
    match r {
        FailReason::ExecutionFailed => "実行に失敗",
        FailReason::InvalidArtifact => "成果物が不正",
        FailReason::LimitReached => "上限に到達",
    }
}

fn halt_label(h: HaltReason) -> &'static str {
    match h {
        HaltReason::Restart => "再起動のため",
        HaltReason::Shutdown => "終了の手順のため",
        HaltReason::User => "一時停止",
        HaltReason::NeedsResume => "送る先のチャットが live でないため",
        HaltReason::SandboxUnhealthy => "sandbox を起動できないため",
        HaltReason::AwaitingPlanApproval => "計画の承認待ち",
        HaltReason::DefinitionUnavailable => "定義の写しを使えないため",
        HaltReason::UsageLimit => "利用上限のため",
    }
}

fn turn_kind_label(k: TurnKind) -> &'static str {
    match k {
        TurnKind::Plan => "計画",
        TurnKind::Task => "作業",
        TurnKind::Review => "検証",
        TurnKind::FormatFix => "形式修正",
        TurnKind::Rework => "差し戻し",
        TurnKind::Replan => "再計画",
        TurnKind::Retry => "再送",
    }
}

fn user() -> OrgActor {
    OrgActor { kind: OrgActorKind::User, label: "あなた".into() }
}
fn engine() -> OrgActor {
    OrgActor { kind: OrgActorKind::Engine, label: "AgentDock".into() }
}
fn codex() -> OrgActor {
    OrgActor { kind: OrgActorKind::Codex, label: "Codex".into() }
}

impl TimelineFold {
    pub fn new() -> Self {
        Self::default()
    }

    /// そのタスクが今の業務状態に入った時刻（記録から辿れなければ None）。
    pub fn since_of(&self, task: &TaskId) -> Option<UnixMillis> {
        self.since.get(task).map(|(at, _)| UnixMillis(*at))
    }

    fn role_actor(&self, task: &TaskId) -> OrgActor {
        let role = self.tasks.get(task).map(|m| m.role.as_str()).unwrap_or("");
        if role.is_empty() {
            engine()
        } else {
            OrgActor { kind: OrgActorKind::Role, label: role_label(role) }
        }
    }

    fn row(ev: &OrgEvent, task: Option<&TaskId>, actor: OrgActor, tone: OrgTone, text: String) -> OrgTimelineEntry {
        OrgTimelineEntry {
            seq: ev.seq,
            seq_to: ev.seq,
            at: UnixMillis(ev.at_ms),
            task: task.map(|t| t.0.clone()),
            actor,
            tone,
            text: clip(&text, TEXT_MAX),
            count: 1,
        }
    }

    /// 記録を1件畳み込む。
    pub fn push(&mut self, ev: &OrgEvent) -> FoldChange {
        // 連続するタスク追加以外の記録が来たら、まとめは閉じる。
        if !matches!(ev.kind, OrgEventKind::TaskAdded { .. }) {
            self.run = None;
        }
        let quiet = self.quiet_ready.take();
        use OrgEventKind as K;
        let change = match &ev.kind {
            K::Created { limits, .. } => {
                self.max_retries = limits.max_retries;
                None
            }
            K::Requested { request, .. } => Some(Self::row(ev, None, user(), OrgTone::Info, format!("ワークフローを開始（依頼: {}）", clip(request, QUOTE_MAX)))),
            K::TaskAdded { task } => return self.task_added(ev, task),
            K::ActivationAdded { task, .. } => {
                self.tasks.insert(task.id.clone(), Meta { role: task.role.0.clone() });
                self.since.insert(task.id.clone(), (ev.at_ms, TaskStatus::Pending));
                None
            }
            K::StatusChanged { task, to, cause } => self.status_changed(ev, task, *to, *cause, quiet),
            K::ConfirmChanged { task, to } => self.confirm_changed(ev, task, *to),
            K::ReworkCounted { task } => {
                *self.rework.entry(task.clone()).or_default() += 1;
                None
            }
            K::LimitsChanged { limits } => {
                self.max_retries = limits.max_retries;
                Some(Self::row(ev, None, user(), OrgTone::Info, format!("turn 上限を {} に", limits.max_turns)))
            }
            K::Halted { reason } => self.halted(ev, *reason),
            K::Dispatched { task, kind, .. } => {
                Some(Self::row(ev, Some(task), self.role_actor(task), OrgTone::Info, format!("送信しました（{}）", turn_kind_label(*kind))))
            }
            K::DispatchSettled { task, outcome, .. } => match outcome {
                SettleOutcome::Rejected => Some(Self::row(ev, Some(task), engine(), OrgTone::Bad, "送信が断られました".into())),
                SettleOutcome::ReconciledAccepted { .. } => Some(Self::row(ev, Some(task), engine(), OrgTone::Good, "照合: 届いていました".into())),
                SettleOutcome::ReconciledNotFound => Some(Self::row(ev, Some(task), engine(), OrgTone::Warn, "照合: 届いていませんでした".into())),
                SettleOutcome::Accepted { .. } | SettleOutcome::Unknown => None,
            },
            K::TurnObserved { task, end, .. } => {
                let (tone, text) = match end {
                    TurnEndKind::Completed => (OrgTone::Info, "turn 完了（結果を確認中）"),
                    TurnEndKind::Failed { usage_limit: true } => (OrgTone::Warn, "利用上限で終了"),
                    TurnEndKind::Failed { usage_limit: false } | TurnEndKind::FailedUnclassified => (OrgTone::Bad, "turn が失敗"),
                    TurnEndKind::Interrupted => (OrgTone::Warn, "中断で終了"),
                };
                Some(Self::row(ev, Some(task), codex(), tone, text.into()))
            }
            K::Committed { task, commit } => Some(Self::row(ev, Some(task), engine(), OrgTone::Info, format!("コミット {}", commit.chars().take(7).collect::<String>()))),
            K::CommitEmpty { task } => Some(Self::row(ev, Some(task), engine(), OrgTone::Warn, "Workerが変更を書きませんでした（コミットなし）".into())),
            K::SlotWritten { task, slot, version, .. } => {
                Some(Self::row(ev, Some(task), self.role_actor(task), OrgTone::Info, format!("{} が {} を出力（第{version}版）", task.0, slot.0)))
            }
            K::EdgeTaken { from, to: Some(to), .. } => Some(Self::row(ev, Some(from), engine(), OrgTone::Info, format!("{} → {}", from.0, to.0))),
            K::EdgeTaken { .. } => None,
            K::FeedbackAllowanceRaised { by, .. } => Some(Self::row(ev, None, user(), OrgTone::Info, format!("やり直しを{by}回追加"))),
            K::GateDecided { task, choice, .. } => Some(Self::row(ev, Some(task), user(), OrgTone::Info, format!("あなたの判断: {choice}"))),
            K::UserNoteAdded { text, scope, .. } => {
                let (task, to) = match scope {
                    NoteScope::AllFollowing => (None, "全体へ"),
                    NoteScope::Task { task } => (Some(task.clone()), "このタスクへ"),
                    NoteScope::Node { .. } => (None, "このタスクへ"),
                };
                Some(Self::row(ev, task.as_ref(), user(), OrgTone::Info, format!("追加指示（{to}）: {}", clip(text, QUOTE_MAX))))
            }
            K::UserNoteWithdrawn { .. } => Some(Self::row(ev, None, user(), OrgTone::Info, "追加指示を取り消し".into())),
            K::InterjectRequested { id, task, text, .. } => {
                self.interjects.insert(id.clone(), task.clone());
                Some(Self::row(ev, Some(task), user(), OrgTone::Info, format!("割り込み指示: {}", clip(text, QUOTE_MAX))))
            }
            K::InterjectSettled { id, outcome } => {
                let task = self.interjects.get(id).cloned();
                let (tone, text) = match outcome {
                    InterjectOutcome::Delivered { note: None } => (OrgTone::Good, "割り込み指示が届きました".to_string()),
                    InterjectOutcome::Delivered { note: Some(n) } => (OrgTone::Good, format!("割り込み指示が届きました（{n}）")),
                    InterjectOutcome::NotDelivered { reason } => (OrgTone::Warn, format!("割り込み指示が届きませんでした（{}）", clip(reason, QUOTE_MAX))),
                    InterjectOutcome::Unknown => (OrgTone::Warn, "割り込み指示が届いたか確認できません".to_string()),
                    InterjectOutcome::ReconciledDelivered => (OrgTone::Good, "照合: 割り込み指示は届いていました".to_string()),
                    InterjectOutcome::ReconciledNotFound => (OrgTone::Warn, "照合: 割り込み指示は届いていませんでした".to_string()),
                    InterjectOutcome::FallbackNoted => (OrgTone::Info, "割り込み指示を次の送信に添えます".to_string()),
                    InterjectOutcome::Discarded => (OrgTone::Info, "割り込み指示を破棄しました".to_string()),
                };
                Some(Self::row(ev, task.as_ref(), engine(), tone, text))
            }
            K::RetryRequested { task } => {
                let n = self.retries.entry(task.clone()).or_default();
                *n += 1;
                let text = format!("新しい試行を指示（{}/{}）", *n, self.max_retries);
                self.quiet_ready = Some(task.clone());
                Some(Self::row(ev, Some(task), user(), OrgTone::Info, text))
            }
            K::TaskSkipped { task } => {
                self.skipped.insert(task.clone());
                Some(Self::row(ev, Some(task), user(), OrgTone::Info, "スキップ".into()))
            }
            K::UserReworkRequested { task } => {
                self.quiet_ready = Some(task.clone());
                Some(Self::row(ev, Some(task), user(), OrgTone::Info, "あなたが差し戻し".into()))
            }
            K::TaskHeld { task, held } => Some(Self::row(ev, Some(task), user(), OrgTone::Info, if *held { "保留".into() } else { "保留を解除".into() })),
            K::ManualTurnAdopted { task, .. } => Some(Self::row(ev, Some(task), user(), OrgTone::Info, "手動の結果を取り込み".into())),
            // 内部だけの記録（出さない）。
            K::TurnSpent { .. }
            | K::AutoReplanSpent
            | K::ChatRequested { .. }
            | K::ChatBound { .. }
            | K::ChatRequestAbandoned { .. }
            | K::GraphBound { .. }
            | K::FeedbackCounted { .. }
            | K::FindingsRecorded { .. } => None,
        };
        change.map_or(FoldChange::None, FoldChange::Add)
    }

    fn task_added(&mut self, ev: &OrgEvent, task: &org_engine::PlanTask) -> FoldChange {
        self.tasks.insert(task.id.clone(), Meta { role: task.role.0.clone() });
        self.since.insert(task.id.clone(), (ev.at_ms, TaskStatus::Pending));
        let planner = OrgActor { kind: OrgActorKind::Role, label: "Planner".into() };
        match self.run.as_mut() {
            Some(run) => {
                run.count += 1;
                let rest = run.count - 1;
                let text = format!("計画: {}{}ほか {rest}件を追加", run.first_id, if run.first_title.is_empty() { String::new() } else { format!("『{}』", clip(&run.first_title, 30)) });
                let mut e = Self::row(ev, None, planner, OrgTone::Info, text);
                e.seq = run.seq;
                e.count = run.count;
                FoldChange::ReplaceLast(e)
            }
            None => {
                self.run = Some(AddRun { first_id: task.id.0.clone(), first_title: task.title.clone(), count: 1, seq: ev.seq });
                let title = if task.title.is_empty() { String::new() } else { format!("『{}』", clip(&task.title, 30)) };
                FoldChange::Add(Self::row(ev, None, planner, OrgTone::Info, format!("計画: {}{title}を追加", task.id)))
            }
        }
    }

    fn status_changed(&mut self, ev: &OrgEvent, task: &TaskId, to: TaskStatus, cause: Cause, quiet: Option<TaskId>) -> Option<OrgTimelineEntry> {
        let changed = self.since.get(task).is_none_or(|(_, s)| *s != to);
        if changed {
            self.since.insert(task.clone(), (ev.at_ms, to));
        }
        let by_user = cause == Cause::User;
        let actor = if by_user { user() } else { engine() };
        let skipped = self.skipped.contains(task);
        let (tone, text): (OrgTone, String) = match to {
            // 進み具合の途中経過。ほかの行（送信・結果）が語るので出さない。
            TaskStatus::Pending | TaskStatus::Running => return None,
            TaskStatus::Ready if !by_user => return None,
            TaskStatus::Ready => {
                if quiet.as_ref() == Some(task) {
                    return None;
                }
                (OrgTone::Info, "あなたが再開".into())
            }
            TaskStatus::Completed => (OrgTone::Good, "完了".into()),
            TaskStatus::Blocked(r) => (if r == BlockedReason::AwaitingApproval { OrgTone::Wait } else { OrgTone::Warn }, format!("止まっています（{}）", blocked_label(r))),
            TaskStatus::Failed(r) => (OrgTone::Bad, format!("失敗（{}）", fail_label(r))),
            // スキップの行がすでに語っている。
            TaskStatus::Cancelled if skipped => return None,
            TaskStatus::Cancelled => (OrgTone::Info, if cause == Cause::StopConfirmed { "取消し（停止を確認）".into() } else { "取消し".into() }),
            TaskStatus::Reviewing => (OrgTone::Info, "検証待ちへ".into()),
            TaskStatus::Rework => {
                if by_user {
                    // 「あなたが差し戻し」の行が語っている。
                    return None;
                }
                (OrgTone::Warn, format!("差し戻し（{}回目）", self.rework.get(task).copied().unwrap_or(0)))
            }
        };
        Some(Self::row(ev, Some(task), actor, tone, text))
    }

    fn confirm_changed(&mut self, ev: &OrgEvent, task: &TaskId, to: ConfirmState) -> Option<OrgTimelineEntry> {
        let prev = self.confirm.insert(task.clone(), to).unwrap_or(ConfirmState::Confirmed);
        let (tone, text) = match to {
            ConfirmState::SendUnknown => (OrgTone::Warn, "受理を確認できません".to_string()),
            ConfirmState::NeedsReconcile(c) => {
                let why = match c {
                    ReconcileCause::Disconnect => "接続断",
                    ReconcileCause::Wake => "スリープ復帰",
                    ReconcileCause::Restart => "再起動",
                };
                (OrgTone::Wait, format!("{why}のため照合待ち"))
            }
            ConfirmState::StopUnconfirmed => (OrgTone::Warn, "停止を確認できません".to_string()),
            ConfirmState::Confirmed if prev != ConfirmState::Confirmed => (OrgTone::Good, "照合で確認".to_string()),
            ConfirmState::Confirmed | ConfirmState::InterruptRequested => return None,
        };
        Some(Self::row(ev, Some(task), engine(), tone, text))
    }

    fn halted(&mut self, ev: &OrgEvent, reason: Option<HaltReason>) -> Option<OrgTimelineEntry> {
        let prev = std::mem::replace(&mut self.halt, reason);
        match reason {
            Some(r) => {
                let tone = if r == HaltReason::AwaitingPlanApproval { OrgTone::Wait } else { OrgTone::Warn };
                let actor = if r == HaltReason::User { user() } else { engine() };
                Some(Self::row(ev, None, actor, tone, format!("全体を停止: {}", halt_label(r))))
            }
            None => {
                let text = if prev == Some(HaltReason::AwaitingPlanApproval) { "計画の承認待ちを解除" } else { "再開" };
                Some(Self::row(ev, None, user(), OrgTone::Info, text.into()))
            }
        }
    }
}

/// 利用者の操作の行、または止まった所・待っている所の行（絞り込み「あなたの操作と止まった所」）。
pub fn is_user_or_stopped(e: &OrgTimelineEntry) -> bool {
    e.actor.kind == OrgActorKind::User || matches!(e.tone, OrgTone::Warn | OrgTone::Bad | OrgTone::Wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{Limits, PlanTask, RoleId};

    fn ev(seq: u64, kind: OrgEventKind) -> OrgEvent {
        OrgEvent { schema_version: 2, seq, at_ms: 1_000 + seq as i64, kind }
    }
    fn plan(id: &str, role: &str, title: &str) -> PlanTask {
        PlanTask { id: TaskId::new(id), role: RoleId::new(role), deps: vec![], effects: vec![], title: title.into(), instructions: String::new() }
    }
    fn tid(s: &str) -> TaskId {
        TaskId::new(s)
    }
    fn texts(f: &mut TimelineFold, kinds: Vec<OrgEventKind>) -> Vec<String> {
        let mut out: Vec<(u64, String)> = Vec::new();
        for (i, k) in kinds.into_iter().enumerate() {
            match f.push(&ev(i as u64 + 1, k)) {
                FoldChange::Add(e) => out.push((e.seq, e.text)),
                FoldChange::ReplaceLast(e) => {
                    let last = out.last_mut().unwrap();
                    *last = (e.seq, e.text);
                }
                FoldChange::None => {}
            }
        }
        out.into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn consecutive_plan_tasks_fold_into_one_row_and_hidden_records_stay_hidden() {
        let mut f = TimelineFold::new();
        let t = texts(
            &mut f,
            vec![
                OrgEventKind::Created { workflow_id: org_engine::WorkflowId::new("w"), limits: Limits::default() },
                OrgEventKind::Requested { request: "do it".into(), repo: "r".into() },
                OrgEventKind::TaskAdded { task: plan("planner-1", "planner", "計画") },
                OrgEventKind::StatusChanged { task: tid("planner-1"), to: TaskStatus::Ready, cause: Cause::Engine },
                OrgEventKind::ChatRequested { task: tid("planner-1"), cwd: "c".into() },
                OrgEventKind::TurnSpent { kind: TurnKind::Plan },
                OrgEventKind::TaskAdded { task: plan("t1", "worker", "ログイン画面") },
                OrgEventKind::TaskAdded { task: plan("t2", "worker", "API") },
                OrgEventKind::TaskAdded { task: plan("t3", "reviewer", "検証") },
            ],
        );
        assert_eq!(t[0], "ワークフローを開始（依頼: do it）");
        assert_eq!(t[1], "計画: planner-1『計画』を追加");
        // the run is broken by the other records; a new run starts at t1
        assert_eq!(t[2], "計画: t1『ログイン画面』ほか 2件を追加");
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn engine_progress_rows_are_hidden_and_stops_and_results_are_shown() {
        let mut f = TimelineFold::new();
        let t = texts(
            &mut f,
            vec![
                OrgEventKind::TaskAdded { task: plan("w", "worker", "x") },
                OrgEventKind::StatusChanged { task: tid("w"), to: TaskStatus::Ready, cause: Cause::Engine },
                OrgEventKind::StatusChanged { task: tid("w"), to: TaskStatus::Running, cause: Cause::Engine },
                OrgEventKind::Dispatched { task: tid("w"), attempt: org_engine::AttemptRef::new("a"), client_message_id: "m".into(), kind: TurnKind::Task, notes_upto: None },
                OrgEventKind::DispatchSettled { task: tid("w"), attempt: org_engine::AttemptRef::new("a"), outcome: SettleOutcome::Accepted { turn: org_engine::TurnRef::new("t") } },
                OrgEventKind::TurnObserved { task: tid("w"), turn: org_engine::TurnRef::new("t"), end: TurnEndKind::Completed },
                OrgEventKind::Committed { task: tid("w"), commit: "abcdef0123".into() },
                OrgEventKind::StatusChanged { task: tid("w"), to: TaskStatus::Blocked(BlockedReason::ReworkLimit), cause: Cause::Engine },
            ],
        );
        assert_eq!(t[1], "送信しました（作業）");
        assert!(!t.iter().any(|x| x.contains("受理") || x.contains("実行待ち") || x.contains("実行中")));
        assert_eq!(&t[2..], ["turn 完了（結果を確認中）", "コミット abcdef0", "止まっています（差し戻しの上限）"]);
    }

    #[test]
    fn reconcile_confirmed_is_shown_only_after_an_unconfirmed_state() {
        let mut f = TimelineFold::new();
        let w = tid("w");
        let t = texts(
            &mut f,
            vec![
                OrgEventKind::TaskAdded { task: plan("w", "worker", "x") },
                OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::Confirmed },
                OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::NeedsReconcile(ReconcileCause::Wake) },
                OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::Confirmed },
                OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::SendUnknown },
                OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::StopUnconfirmed },
            ],
        );
        assert_eq!(&t[1..], ["スリープ復帰のため照合待ち", "照合で確認", "受理を確認できません", "停止を確認できません"]);
        assert_eq!(t.len(), 5, "the first Confirmed (nothing before it) is not shown");
    }

    #[test]
    fn interventions_are_shown_as_the_users_and_a_skip_is_not_repeated_as_a_cancel() {
        let mut f = TimelineFold::new();
        let t = texts(
            &mut f,
            vec![
                OrgEventKind::Created { workflow_id: org_engine::WorkflowId::new("w"), limits: Limits::default() },
                OrgEventKind::TaskAdded { task: plan("w", "worker", "x") },
                OrgEventKind::UserNoteAdded { id: NoteId::new("n1"), text: "use tabs".into(), scope: NoteScope::AllFollowing },
                OrgEventKind::InterjectRequested { id: NoteId::new("int-4"), task: tid("w"), turn: org_engine::TurnRef::new("t"), text: "README too".into(), fallback: true },
                OrgEventKind::InterjectSettled { id: NoteId::new("int-4"), outcome: InterjectOutcome::NotDelivered { reason: "rejected".into() } },
                OrgEventKind::TaskSkipped { task: tid("w") },
                OrgEventKind::StatusChanged { task: tid("w"), to: TaskStatus::Cancelled, cause: Cause::User },
                OrgEventKind::RetryRequested { task: tid("w") },
                OrgEventKind::StatusChanged { task: tid("w"), to: TaskStatus::Ready, cause: Cause::User },
            ],
        );
        assert_eq!(
            &t[1..],
            [
                "追加指示（全体へ）: use tabs",
                "割り込み指示: README too",
                "割り込み指示が届きませんでした（rejected）",
                "スキップ",
                "新しい試行を指示（1/3）",
            ]
        );
    }

    #[test]
    fn since_tracks_when_the_current_state_was_entered() {
        let mut f = TimelineFold::new();
        let w = tid("w");
        f.push(&ev(1, OrgEventKind::TaskAdded { task: plan("w", "worker", "x") }));
        assert_eq!(f.since_of(&w), Some(UnixMillis(1_001)));
        f.push(&ev(5, OrgEventKind::StatusChanged { task: w.clone(), to: TaskStatus::Ready, cause: Cause::Engine }));
        f.push(&ev(9, OrgEventKind::StatusChanged { task: w.clone(), to: TaskStatus::Running, cause: Cause::Engine }));
        assert_eq!(f.since_of(&w), Some(UnixMillis(1_009)));
        // a confirmation change does not restart the business-state clock
        f.push(&ev(12, OrgEventKind::ConfirmChanged { task: w.clone(), to: ConfirmState::SendUnknown }));
        assert_eq!(f.since_of(&w), Some(UnixMillis(1_009)));
        assert_eq!(f.since_of(&tid("unknown")), None);
    }

    #[test]
    fn halts_and_long_text_are_clipped() {
        let mut f = TimelineFold::new();
        let long = "あ".repeat(500);
        match f.push(&ev(1, OrgEventKind::Requested { request: long, repo: "r".into() })) {
            FoldChange::Add(e) => assert!(e.text.chars().count() <= 120 + 1, "{}", e.text.chars().count()),
            other => panic!("{other:?}"),
        }
        let r = f.push(&ev(2, OrgEventKind::Halted { reason: Some(HaltReason::AwaitingPlanApproval) }));
        assert!(matches!(r, FoldChange::Add(e) if e.text == "全体を停止: 計画の承認待ち" && e.tone == OrgTone::Wait));
        let r = f.push(&ev(3, OrgEventKind::Halted { reason: None }));
        assert!(matches!(r, FoldChange::Add(e) if e.text == "計画の承認待ちを解除"));
    }
}
