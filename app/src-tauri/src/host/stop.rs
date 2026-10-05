//! 中断後の停止照合（要件§3.4・§4.3）の純粋ロジック。時間はすべて引数で受け取る。
//!
//! - 中断要求の受付と停止確認は別。停止の確認は証拠（turn終端など）だけで行う。
//! - 要求から10秒経っても証拠がなければ `Unconfirmed`（停止未確認の案内）。これは停止・失敗の確定ではない。
//! - 時間の経過だけで `TurnEndConfirmed` 等へ進めない。

use crate::backend::model::*;

/// 中断要求から停止未確認の案内を出すまでの時間（ミリ秒）。
pub const STOP_CONFIRM_DEADLINE_MS: i64 = 10_000;

/// 証拠から要約を機械的に導出する。確認済みの証拠があれば経過時間より優先する。
pub fn derive_summary(ownership: Ownership, ev: &StopEvidence, now: UnixMillis) -> StopSummary {
    if ownership == Ownership::Unknown {
        return StopSummary::OwnershipUnknown;
    }
    if ev.os_gone_confirmed_at.is_some() {
        return StopSummary::OsGoneConfirmed;
    }
    if ev.managed_exec_ended_at.is_some() {
        return StopSummary::ManagedExecEndConfirmed;
    }
    if ev.turn_end_confirmed.is_some() {
        return StopSummary::TurnEndConfirmed;
    }
    match ev.interrupt_requested_at {
        None => StopSummary::NotRequested,
        Some(t) if now.0 - t.0 >= STOP_CONFIRM_DEADLINE_MS => StopSummary::Unconfirmed,
        Some(_) => StopSummary::InterruptRequested,
    }
}

/// 全対象の要約を再計算する。変化があれば true。
pub fn refresh(record: &mut StopRecord, now: UnixMillis) -> bool {
    let mut changed = false;
    for t in &mut record.targets {
        let s = derive_summary(t.ownership, &t.evidence, now);
        if s != t.summary {
            t.summary = s;
            changed = true;
        }
    }
    changed
}

/// turn終端の観測を該当対象へ記録する。変化があれば true。
pub fn apply_turn_end(record: &mut StopRecord, turn: &TurnKey, end: TurnEnd, at: UnixMillis, now: UnixMillis) -> bool {
    let mut changed = false;
    for t in &mut record.targets {
        if matches!(&t.target, StopTargetRef::Turn { turn: k } if k == turn) && t.evidence.turn_end_confirmed.is_none() {
            t.evidence.turn_end_confirmed = Some(TurnEndConfirmation { end, at });
            changed = true;
        }
    }
    refresh(record, now) || changed
}

/// 中断対象1件の初期状態。`requested_at` は要求を送った（受付または応答なしの）時刻。送っていなければ None。
pub fn new_target(turn: TurnKey, requested_at: Option<UnixMillis>, now: UnixMillis) -> StopTarget {
    let evidence = StopEvidence { interrupt_requested_at: requested_at, ..Default::default() };
    let summary = derive_summary(Ownership::Confirmed, &evidence, now);
    StopTarget { target: StopTargetRef::Turn { turn }, ownership: Ownership::Confirmed, evidence, summary }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(a: &str, t: &str) -> TurnKey {
        TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId(a.into()) }, turn_id: ExternalId(t.into()) }
    }

    fn rec(targets: Vec<StopTarget>) -> StopRecord {
        StopRecord {
            id: LocalId("s".into()),
            chat: ChatKey { backend: BackendKind::Codex, id: ExternalId("c".into()) },
            started_at: UnixMillis(0),
            targets,
        }
    }

    #[test]
    fn summary_follows_deadline_and_evidence() {
        let t0 = UnixMillis(1_000);
        let ev = StopEvidence { interrupt_requested_at: Some(t0), ..Default::default() };
        assert_eq!(derive_summary(Ownership::Confirmed, &ev, UnixMillis(1_000)), StopSummary::InterruptRequested);
        assert_eq!(derive_summary(Ownership::Confirmed, &ev, UnixMillis(10_999)), StopSummary::InterruptRequested);
        assert_eq!(derive_summary(Ownership::Confirmed, &ev, UnixMillis(11_000)), StopSummary::Unconfirmed);
        // 未要求は時間が経っても Unconfirmed にならない。
        assert_eq!(derive_summary(Ownership::Confirmed, &StopEvidence::default(), UnixMillis(99_999)), StopSummary::NotRequested);
        // 所有不明は常に OwnershipUnknown。
        assert_eq!(derive_summary(Ownership::Unknown, &ev, UnixMillis(1_000)), StopSummary::OwnershipUnknown);
    }

    #[test]
    fn turn_end_confirms_and_late_end_after_deadline_still_confirms() {
        let mut r = rec(vec![new_target(tk("a", "t1"), Some(UnixMillis(0)), UnixMillis(0)), new_target(tk("b", "t1"), Some(UnixMillis(0)), UnixMillis(0))]);
        assert!(refresh(&mut r, UnixMillis(10_000)));
        assert!(r.targets.iter().all(|t| t.summary == StopSummary::Unconfirmed));
        assert!(apply_turn_end(&mut r, &tk("a", "t1"), TurnEnd::Interrupted, UnixMillis(12_000), UnixMillis(12_000)));
        assert_eq!(r.targets[0].summary, StopSummary::TurnEndConfirmed);
        assert_eq!(r.targets[1].summary, StopSummary::Unconfirmed);
        // 別のturnの終端は無関係。
        assert!(!apply_turn_end(&mut r, &tk("b", "t0"), TurnEnd::Completed, UnixMillis(12_000), UnixMillis(12_000)));
        assert!(r.blocks_deletion());
    }

    #[test]
    fn refresh_without_change_reports_false() {
        let mut r = rec(vec![new_target(tk("a", "t1"), Some(UnixMillis(0)), UnixMillis(0))]);
        assert!(!refresh(&mut r, UnixMillis(5_000)));
    }
}
