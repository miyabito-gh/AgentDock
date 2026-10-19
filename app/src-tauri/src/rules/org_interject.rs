//! 組織実行の割り込み指示の経路の判定（DESIGN_ORG_R2 2.1）。純粋関数で、何も送らない。
//!
//! 経路は3つ。実行中の turn に今すぐ届ける（`turn/steer`）／次にエンジンが送る turn に添える／今は送れない。
//! 割り込みで新しい turn は始まらない。確実に届いたと言える条件（受理済みの turn が実行中で、チャットが live）を満たすときだけ
//! `Steer` にし、それ以外は「次の送信に添える」か断る。

use org_engine::{ConfirmState, TaskStatus, TurnRef};

/// 経路の判定の材料（エンジンの状態の写しと、ホストが見ているチャットの様子）。
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub status: TaskStatus,
    pub confirm: ConfirmState,
    /// 「手動に切り替える」で組織から外れたチャット。
    pub released: bool,
    /// 手動の結果の取り込み中。
    pub adopting: bool,
    /// 受理済みの turn（最新の送信が受理されたもの）。
    pub accepted_turn: Option<TurnRef>,
    /// 最新の送信が受理（または照合で受理）で決着している。
    pub accepted_settled: bool,
    /// 最新の送信に何らかの決着がある（なければ受理待ち）。
    pub has_settle: bool,
    /// 受理済みの turn の終端を観測している。
    pub turn_ended: bool,
    pub live: bool,
    /// チャットで実行中の turn（ホストの観測）。
    pub active_turn: Option<TurnRef>,
    /// バックエンドが実行中の turn への追加指示に対応している。
    pub steer_supported: bool,
    /// 前の割り込みの結果がまだ決まっていない。
    pub open_interject: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterjectRoute {
    /// 実行中の turn へ今すぐ（宛先の turn を指定して1回）。
    Steer { turn: TurnRef },
    /// 次にエンジンがそのタスクへ送る turn に添える。
    NextTurn,
    Refuse(String),
}

pub const LABEL_STEER: &str = "実行中の turn に今すぐ届けます";
pub const LABEL_NEXT_TURN: &str = "次にこのタスクへ送るとき（差し戻し・再送など）に添えます。このタスクにもう送信がなければ届きません";

pub fn interject_route(i: &RouteInput) -> InterjectRoute {
    use InterjectRoute::*;
    if matches!(i.status, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled) {
        return Refuse("終わったタスクです。ワークフローへの追加指示を使ってください".into());
    }
    if i.released || i.status == TaskStatus::Blocked(org_engine::BlockedReason::Manual) {
        return Refuse("手動に切り替え済みです。チャットに直接書けます".into());
    }
    if i.adopting {
        return Refuse("結果の取り込み中です".into());
    }
    // 送信を始めたが受理の結果をまだ受けていない: 宛先の turn が決まらず、送る側も決まらない。
    if i.status == TaskStatus::Running && !i.has_settle {
        return Refuse("送信の受理を確認中です。少し待ってから送ってください".into());
    }
    let steerable = i.status == TaskStatus::Running
        && i.confirm == ConfirmState::Confirmed
        && i.accepted_settled
        && !i.turn_ended
        && i.live
        && i.steer_supported
        && i.accepted_turn.is_some()
        && i.accepted_turn == i.active_turn;
    if steerable {
        if i.open_interject {
            return Refuse("前の割り込みの結果を待っています".into());
        }
        return Steer { turn: i.accepted_turn.clone().unwrap() };
    }
    NextTurn
}

pub fn route_label(r: &InterjectRoute) -> &str {
    match r {
        InterjectRoute::Steer { .. } => LABEL_STEER,
        InterjectRoute::NextTurn => LABEL_NEXT_TURN,
        InterjectRoute::Refuse(why) => why,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{BlockedReason, FailReason, ReconcileCause};

    fn running() -> RouteInput {
        RouteInput {
            status: TaskStatus::Running,
            confirm: ConfirmState::Confirmed,
            released: false,
            adopting: false,
            accepted_turn: Some(TurnRef::new("t1")),
            accepted_settled: true,
            has_settle: true,
            turn_ended: false,
            live: true,
            active_turn: Some(TurnRef::new("t1")),
            steer_supported: true,
            open_interject: false,
        }
    }

    #[test]
    fn a_running_confirmed_live_turn_is_steered() {
        assert_eq!(interject_route(&running()), InterjectRoute::Steer { turn: TurnRef::new("t1") });
    }

    #[test]
    fn anything_less_certain_waits_for_the_next_turn_instead_of_steering() {
        let cases: Vec<Box<dyn Fn(&mut RouteInput)>> = vec![
            Box::new(|i| i.live = false),
            Box::new(|i| i.steer_supported = false),
            Box::new(|i| i.turn_ended = true),
            Box::new(|i| i.active_turn = Some(TurnRef::new("other"))),
            Box::new(|i| i.active_turn = None),
            Box::new(|i| i.confirm = ConfirmState::NeedsReconcile(ReconcileCause::Wake)),
            Box::new(|i| i.confirm = ConfirmState::StopUnconfirmed),
            Box::new(|i| i.accepted_settled = false),
        ];
        for (n, f) in cases.iter().enumerate() {
            let mut i = running();
            f(&mut i);
            assert_eq!(interject_route(&i), InterjectRoute::NextTurn, "case {n}");
        }
    }

    #[test]
    fn waiting_for_acceptance_is_refused() {
        let mut i = running();
        i.has_settle = false;
        i.accepted_turn = None;
        i.accepted_settled = false;
        assert!(matches!(interject_route(&i), InterjectRoute::Refuse(m) if m.contains("受理を確認中")));
    }

    #[test]
    fn tasks_that_are_not_running_take_the_note_to_their_next_turn() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::Ready,
            TaskStatus::Rework,
            TaskStatus::Reviewing,
            TaskStatus::Blocked(BlockedReason::NeedsAttention),
            TaskStatus::Blocked(BlockedReason::ReworkLimit),
            TaskStatus::Blocked(BlockedReason::UsageLimit),
        ] {
            let mut i = running();
            i.status = status;
            assert_eq!(interject_route(&i), InterjectRoute::NextTurn, "{status:?}");
        }
    }

    #[test]
    fn finished_manual_and_adopting_tasks_are_refused() {
        for status in [TaskStatus::Completed, TaskStatus::Cancelled, TaskStatus::Failed(FailReason::ExecutionFailed)] {
            let mut i = running();
            i.status = status;
            assert!(matches!(interject_route(&i), InterjectRoute::Refuse(m) if m.contains("終わったタスク")), "{status:?}");
        }
        let mut i = running();
        i.status = TaskStatus::Blocked(BlockedReason::Manual);
        assert!(matches!(interject_route(&i), InterjectRoute::Refuse(m) if m.contains("手動に切り替え")));
        let mut i = running();
        i.released = true;
        assert!(matches!(interject_route(&i), InterjectRoute::Refuse(m) if m.contains("手動に切り替え")));
        let mut i = running();
        i.adopting = true;
        assert!(matches!(interject_route(&i), InterjectRoute::Refuse(m) if m.contains("取り込み")));
    }

    #[test]
    fn a_second_interject_waits_for_the_first() {
        let mut i = running();
        i.open_interject = true;
        assert!(matches!(interject_route(&i), InterjectRoute::Refuse(_)));
        assert_eq!(route_label(&InterjectRoute::Steer { turn: TurnRef::new("t") }), LABEL_STEER);
    }
}
