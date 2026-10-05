//! 削除・アーカイブ・終了・wakeの判断（P6・P7）。純粋ロジック。

use crate::backend::local::*;
use crate::backend::model::*;

/// チャットの作業状況の要約（判断材料）。ホストが状態から組み立てる。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatWork {
    pub chat: ChatKey,
    /// 親・子孫に running / waiting / initializing / unknown がある。
    pub has_unfinished: bool,
    /// 停止記録に停止未確認・所有不明がある。
    pub stop_unconfirmed: Option<LocalId>,
    pub ownership_unknown: bool,
    /// キューに Waiting / Sending / AcceptanceUnknown がある。
    pub queue_pending: bool,
    pub unresolved_send: bool,
}

/// Codexへ `archive` を送ってよいか（M43。作業完了・停止確認・キュー消化の後）。
pub fn archive_ready(work: &ChatWork) -> bool {
    let _ = work;
    todo!("P7")
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeleteDecision {
    /// すぐ削除してよい（作業なし・停止確認済み）。
    Proceed,
    /// 先に中断と停止照合が要る（ユーザーが「中断して削除」を選んだ後）。
    StopFirst,
    /// 停止未確認・所有不明のため保留する（再起動後も保持、送信禁止）。
    Pend(DeletePendingReason),
}

/// 削除の可否。`pending` が既にあり理由が `ReadyForUserRetry` 以外なら Pend のまま。
pub fn delete_decision(work: &ChatWork, pending: Option<&DeletePending>) -> DeleteDecision {
    let _ = (work, pending);
    todo!("P7")
}

/// 停止照合の更新後、削除保留の理由を更新する。停止を確認できても自動削除せず `ReadyForUserRetry` にするだけ。
pub fn refresh_delete_pending(pending: &mut DeletePending, work: &ChatWork) -> bool {
    let _ = (pending, work);
    todo!("P7")
}

/// 完全終了の確認対象（作業中・停止未確認・受理不明・キュー送信待ちのチャット）。
pub fn busy_for_quit(works: &[ChatWork]) -> Vec<ChatKey> {
    works
        .iter()
        .filter(|w| w.has_unfinished || w.stop_unconfirmed.is_some() || w.ownership_unknown || w.queue_pending || w.unresolved_send)
        .map(|w| w.chat.clone())
        .collect()
}

/// 停止が確認できた対象か。証拠（turn終端・管理実行の終了・OS実体の消滅）があるものだけ。中断要求の受付や時間経過では確認にしない。
fn target_confirmed(t: &StopTarget) -> bool {
    t.ownership == Ownership::Confirmed
        && matches!(t.summary, StopSummary::TurnEndConfirmed | StopSummary::ManagedExecEndConfirmed | StopSummary::OsGoneConfirmed)
}

/// 指定IDの停止記録のうち、確認できていない対象を持つもの。一覧に無いIDは確認できていないものとして残す。
fn unconfirmed_records(ids: &[LocalId], records: &[StopRecord]) -> Vec<LocalId> {
    ids.iter()
        .filter(|id| records.iter().find(|r| &r.id == *id).is_none_or(|r| !r.targets.iter().all(target_confirmed)))
        .cloned()
        .collect()
}

/// 停止未確認の案内を出すまでの時間（ミリ秒）。停止確認の期限（`host::stop`）と同じ10秒。
pub const QUIT_STOP_NOTICE_MS: i64 = 10_000;

/// 終了手順の次の段階。`now - started_at >= 10秒` かつ未確認が残れば `StopUnconfirmed`（停止確定ではない、D04）。
/// すべて確認できたら `Flushing`。保存失敗があれば `SaveFailed`。
///
/// - `Flushing` のまま返ったら、保存に失敗がなく終了してよい（ホストは書き出しの後にこの関数を呼び直す）。
/// - 一度 `StopUnconfirmed` になっても、後から確認できれば `Flushing` へ進む（時間切れで停止と判定しない）。
/// - `Idle`・`Confirming` は外部の操作でだけ動くので変えない。
pub fn next_quit_phase(phase: &QuitPhase, records: &[StopRecord], save_failed: &[SaveScope], now: UnixMillis) -> QuitPhase {
    match phase {
        QuitPhase::Stopping { records: ids, started_at } => {
            let open = unconfirmed_records(ids, records);
            if open.is_empty() {
                QuitPhase::Flushing
            } else if now.0 - started_at.0 >= QUIT_STOP_NOTICE_MS {
                QuitPhase::StopUnconfirmed { records: open }
            } else {
                phase.clone()
            }
        }
        QuitPhase::StopUnconfirmed { records: ids } => {
            let open = unconfirmed_records(ids, records);
            if open.is_empty() {
                QuitPhase::Flushing
            } else {
                QuitPhase::StopUnconfirmed { records: open }
            }
        }
        QuitPhase::Flushing | QuitPhase::SaveFailed { .. } => {
            if save_failed.is_empty() {
                QuitPhase::Flushing
            } else {
                QuitPhase::SaveFailed { failed: save_failed.to_vec() }
            }
        }
        QuitPhase::Idle | QuitPhase::Confirming { .. } => phase.clone(),
    }
}

/// sleepからの復帰で、live の鮮度を要照合へ落とす対象（状態は変えない）。live 以外はすでに照合が要る・履歴のみ・切断・非対応なので触らない。
pub fn agents_to_mark_on_resume(views: &[AgentView]) -> Vec<AgentKey> {
    views.iter().filter(|v| v.freshness == Freshness::Live).map(|v| v.agent.key.clone()).collect()
}

/// 壁時計の飛び（sleep検出の予備）。前回の刻みから `expected_ms` を大きく超えたら復帰とみなす。
pub const WAKE_GAP_THRESHOLD_MS: i64 = 30_000;
pub fn looks_like_resume(prev_tick: UnixMillis, now: UnixMillis, expected_ms: i64) -> bool {
    // 時計が戻った（負の差）は復帰ではない。
    now.0 - prev_tick.0 - expected_ms >= WAKE_GAP_THRESHOLD_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn ak(s: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn work(c: &str) -> ChatWork {
        ChatWork { chat: ck(c), has_unfinished: false, stop_unconfirmed: None, ownership_unknown: false, queue_pending: false, unresolved_send: false }
    }
    fn target(summary: StopSummary, ownership: Ownership) -> StopTarget {
        StopTarget {
            target: StopTargetRef::Turn { turn: TurnKey { agent: ak("a"), turn_id: ExternalId("t".into()) } },
            ownership,
            evidence: StopEvidence::default(),
            summary,
        }
    }
    fn rec(id: &str, targets: Vec<StopTarget>) -> StopRecord {
        StopRecord { id: LocalId(id.into()), chat: ck("c"), started_at: UnixMillis(0), targets }
    }
    fn lid(s: &str) -> LocalId {
        LocalId(s.into())
    }
    const T0: i64 = 100_000;

    #[test]
    fn busy_picks_every_kind_of_unfinished_work_and_keeps_order() {
        let idle = work("idle");
        let mut running = work("running");
        running.has_unfinished = true;
        let mut stopping = work("stopping");
        stopping.stop_unconfirmed = Some(lid("s1"));
        let mut owner = work("owner");
        owner.ownership_unknown = true;
        let mut queued = work("queued");
        queued.queue_pending = true;
        let mut unresolved = work("unresolved");
        unresolved.unresolved_send = true;
        let got = busy_for_quit(&[idle.clone(), running, stopping, owner, queued, unresolved, idle]);
        assert_eq!(got, vec![ck("running"), ck("stopping"), ck("owner"), ck("queued"), ck("unresolved")]);
        assert!(busy_for_quit(&[]).is_empty());
    }

    #[test]
    fn stopping_waits_then_shows_unconfirmed_at_ten_seconds_and_never_confirms_by_time() {
        let phase = QuitPhase::Stopping { records: vec![lid("r1")], started_at: UnixMillis(T0) };
        let open = vec![rec("r1", vec![target(StopSummary::InterruptRequested, Ownership::Confirmed)])];
        assert_eq!(next_quit_phase(&phase, &open, &[], UnixMillis(T0 + 9_999)), phase);
        assert_eq!(next_quit_phase(&phase, &open, &[], UnixMillis(T0 + 10_000)), QuitPhase::StopUnconfirmed { records: vec![lid("r1")] });
        // 時間が経っても、証拠がなければ Flushing にならない。
        assert!(matches!(next_quit_phase(&phase, &open, &[], UnixMillis(T0 + 3_600_000)), QuitPhase::StopUnconfirmed { .. }));
    }

    #[test]
    fn evidence_moves_on_to_flushing_even_after_the_notice() {
        let done = vec![rec("r1", vec![target(StopSummary::TurnEndConfirmed, Ownership::Confirmed), target(StopSummary::OsGoneConfirmed, Ownership::Confirmed)])];
        let stopping = QuitPhase::Stopping { records: vec![lid("r1")], started_at: UnixMillis(T0) };
        assert_eq!(next_quit_phase(&stopping, &done, &[], UnixMillis(T0 + 1)), QuitPhase::Flushing);
        let unconfirmed = QuitPhase::StopUnconfirmed { records: vec![lid("r1"), lid("r2")] };
        let mixed = vec![done[0].clone(), rec("r2", vec![target(StopSummary::Unconfirmed, Ownership::Confirmed)])];
        assert_eq!(next_quit_phase(&unconfirmed, &mixed, &[], UnixMillis(T0)), QuitPhase::StopUnconfirmed { records: vec![lid("r2")] });
        let all = vec![done[0].clone(), rec("r2", vec![target(StopSummary::ManagedExecEndConfirmed, Ownership::Confirmed)])];
        assert_eq!(next_quit_phase(&unconfirmed, &all, &[], UnixMillis(T0)), QuitPhase::Flushing);
    }

    #[test]
    fn unknown_ownership_and_missing_records_count_as_unconfirmed() {
        let stopping = QuitPhase::Stopping { records: vec![lid("r1"), lid("gone")], started_at: UnixMillis(T0) };
        let recs = vec![rec("r1", vec![target(StopSummary::TurnEndConfirmed, Ownership::Unknown)])];
        assert_eq!(
            next_quit_phase(&stopping, &recs, &[], UnixMillis(T0 + 10_000)),
            QuitPhase::StopUnconfirmed { records: vec![lid("r1"), lid("gone")] }
        );
        // 記録が空（対象なし）なら確認すべきものがない。
        let empty = QuitPhase::Stopping { records: vec![], started_at: UnixMillis(T0) };
        assert_eq!(next_quit_phase(&empty, &[], &[], UnixMillis(T0)), QuitPhase::Flushing);
    }

    #[test]
    fn save_failure_never_ends_as_a_normal_exit() {
        let failed = vec![SaveScope::AppSettings];
        assert_eq!(next_quit_phase(&QuitPhase::Flushing, &[], &failed, UnixMillis(T0)), QuitPhase::SaveFailed { failed: failed.clone() });
        // 再試行で直れば Flushing（終了してよい）に戻る。
        assert_eq!(next_quit_phase(&QuitPhase::SaveFailed { failed }, &[], &[], UnixMillis(T0)), QuitPhase::Flushing);
        assert_eq!(next_quit_phase(&QuitPhase::Flushing, &[], &[], UnixMillis(T0)), QuitPhase::Flushing);
    }

    #[test]
    fn idle_and_confirming_only_move_by_user_decision() {
        let c = QuitPhase::Confirming { busy: vec![ck("a")] };
        assert_eq!(next_quit_phase(&c, &[], &[SaveScope::AppSettings], UnixMillis(T0)), c);
        assert_eq!(next_quit_phase(&QuitPhase::Idle, &[], &[], UnixMillis(T0)), QuitPhase::Idle);
    }

    fn view(id: &str, f: Freshness) -> AgentView {
        AgentView {
            agent: Agent {
                key: ak(id),
                chat: ck("c"),
                parent: ParentLink::Root,
                forked_from: Known::NotFetched,
                display_name: Known::NotFetched,
                role: Known::NotFetched,
                assignment: Known::NotFetched,
                agent_path: Known::NotFetched,
                latest_turn: None,
            },
            status: AgentStatus {
                state: AgentState::Running,
                raw: RawState { label: "x".into() },
                scope: StateScope::Agent,
                turn: None,
                wait: None,
                evidence: Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(0) },
            },
            freshness: f,
            current_activity: None,
        }
    }

    #[test]
    fn only_live_agents_are_marked_for_reconcile_on_resume() {
        let views = vec![view("live", Freshness::Live), view("hist", Freshness::HistoryOnly), view("need", Freshness::NeedsReconcile), view("dis", Freshness::Disconnected), view("uns", Freshness::Unsupported)];
        assert_eq!(agents_to_mark_on_resume(&views), vec![ak("live")]);
    }

    #[test]
    fn wall_clock_jump_is_a_resume_but_normal_ticks_and_clock_rollback_are_not() {
        // 5秒刻みのタイマー。
        assert!(!looks_like_resume(UnixMillis(1_000), UnixMillis(6_100), 5_000));
        assert!(!looks_like_resume(UnixMillis(1_000), UnixMillis(35_999), 5_000));
        assert!(looks_like_resume(UnixMillis(1_000), UnixMillis(36_000), 5_000));
        assert!(looks_like_resume(UnixMillis(1_000), UnixMillis(3_600_000), 5_000));
        assert!(!looks_like_resume(UnixMillis(10_000), UnixMillis(1_000), 5_000));
    }
}
