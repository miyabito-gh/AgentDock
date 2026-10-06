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
    /// キューの送信中（Sending）・受理不明・自動送信したturnの終端待ち。送信待ち（Waiting）だけのときは false。
    pub queue_in_flight: bool,
    pub unresolved_send: bool,
}

/// Codexへ `archive` を送ってよいか（M43。作業完了・停止確認・キュー消化の後）。
pub fn archive_ready(work: &ChatWork) -> bool {
    !work.has_unfinished && work.stop_unconfirmed.is_none() && !work.ownership_unknown && !work.queue_pending && !work.unresolved_send
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

/// 削除の可否。停止を確認できないもの（停止未確認・所有不明・受理不明の送信）は保留する。
/// 保留の理由が `StopUnconfirmed` / `OwnershipUnknown` のままなら、停止の再確認（`refresh_delete_pending`）が済むまで Pend のまま。
/// `ReadyForUserRetry`・`PartialFailure` は、作業が残っていなければ Proceed（ユーザーの再操作で実行する）。
pub fn delete_decision(work: &ChatWork, pending: Option<&DeletePending>) -> DeleteDecision {
    if work.ownership_unknown || work.unresolved_send {
        return DeleteDecision::Pend(DeletePendingReason::OwnershipUnknown);
    }
    if work.stop_unconfirmed.is_some() {
        return DeleteDecision::Pend(DeletePendingReason::StopUnconfirmed);
    }
    if work.has_unfinished || work.queue_pending {
        return DeleteDecision::StopFirst;
    }
    match pending.map(|p| &p.reason) {
        Some(r @ (DeletePendingReason::StopUnconfirmed | DeletePendingReason::OwnershipUnknown)) => DeleteDecision::Pend(r.clone()),
        _ => DeleteDecision::Proceed,
    }
}

/// 作業状況から導く、停止確認の側の保留理由。確認できていれば None。
fn unconfirmed_reason(work: &ChatWork) -> Option<DeletePendingReason> {
    if work.ownership_unknown || work.unresolved_send {
        Some(DeletePendingReason::OwnershipUnknown)
    } else if work.stop_unconfirmed.is_some() || work.has_unfinished || work.queue_in_flight {
        Some(DeletePendingReason::StopUnconfirmed)
    } else {
        None
    }
}

/// 停止照合の更新後、削除保留の理由を更新する。停止を確認できても自動削除せず `ReadyForUserRetry` にするだけ。
/// `PartialFailure` は停止とは別の失敗なので触らない。変更があれば true。
pub fn refresh_delete_pending(pending: &mut DeletePending, work: &ChatWork) -> bool {
    if matches!(pending.reason, DeletePendingReason::PartialFailure { .. }) {
        return false;
    }
    let reason = unconfirmed_reason(work).unwrap_or(DeletePendingReason::ReadyForUserRetry);
    let record = work.stop_unconfirmed.clone().or_else(|| pending.stop_record.clone());
    if pending.reason == reason && pending.stop_record == record {
        return false;
    }
    pending.reason = reason;
    pending.stop_record = record;
    true
}

/// 削除の途中結果から結果を決める。失敗が1つでもあれば完了と偽らず `Partial`（保留を残す）。
pub fn delete_result(done: Vec<String>, failed: Vec<String>) -> DeleteOutcome {
    if failed.is_empty() {
        DeleteOutcome::Deleted
    } else {
        DeleteOutcome::Partial { done, failed }
    }
}

/// Codex の履歴削除（`thread/delete`）の拒否が「対象の履歴が存在しない」を意味するか。
/// 発話前のチャットなど、Codex に履歴（rollout）が作られていない会話の削除は、消す対象がないだけなので、Codex 側は「対象なし」として完了扱いにできる。
/// 根拠: App Server は履歴の無い thread の削除を、専用のエラーコードを持たない一般の拒否（実機では
/// `no rollout found for thread id <id>`）で返す。コードでは他の拒否と区別できないため、この文言だけを限定的に判定する。
/// 他の拒否・通信断・結果不明は対象外（従来どおり部分失敗として保留を残す）。
pub fn backend_delete_target_missing(e: &crate::backend::backend::BackendError) -> bool {
    matches!(e, crate::backend::backend::BackendError::Rejected { message, .. } if message.contains("no rollout found for thread id"))
}

/// 保存に失敗している単位があれば、削除・アーカイブを完了と表示しない。対象チャットの単位だけを見る。
pub fn save_blocks_chat(statuses: &[SaveStatus], chat: &ChatKey) -> Option<SaveScope> {
    statuses
        .iter()
        .find(|s| {
            matches!(s.state, SaveState::SaveFailed { .. })
                && match &s.scope {
                    SaveScope::ChatLocal { chat: c } | SaveScope::Queue { chat: c } | SaveScope::Activity { chat: c } => c == chat,
                    _ => false,
                }
        })
        .map(|s| s.scope.clone())
}

/// 完全終了の確認対象（作業中・停止未確認・受理不明・キュー送信待ちのチャット）。
pub fn busy_for_quit(works: &[ChatWork]) -> Vec<ChatKey> {
    works
        .iter()
        .filter(|w| w.has_unfinished || w.stop_unconfirmed.is_some() || w.ownership_unknown || w.queue_pending || w.unresolved_send)
        .map(|w| w.chat.clone())
        .collect()
}

/// 終了手順の途中で新しく現れた作業（Flushing へ進む前の再確認）。すでに確認・中断の対象にしたチャット（`handled`）は除く。
/// 送信待ち（Waiting）だけは数えない（確認のときに承知済みで、終了中は送信を保留している）。
pub fn new_work_for_quit(works: &[ChatWork], handled: &[ChatKey]) -> Vec<ChatKey> {
    works
        .iter()
        .filter(|w| !handled.contains(&w.chat))
        .filter(|w| w.has_unfinished || w.stop_unconfirmed.is_some() || w.ownership_unknown || w.queue_in_flight || w.unresolved_send)
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
        ChatWork { chat: ck(c), has_unfinished: false, stop_unconfirmed: None, ownership_unknown: false, queue_pending: false, queue_in_flight: false, unresolved_send: false }
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

    fn pending(reason: DeletePendingReason) -> DeletePending {
        DeletePending { requested_at: UnixMillis(1), reason, stop_record: None }
    }

    #[test]
    fn archive_waits_for_every_kind_of_unfinished_work() {
        assert!(archive_ready(&work("a")));
        let mut w = work("a");
        w.has_unfinished = true;
        assert!(!archive_ready(&w));
        let mut w = work("a");
        w.stop_unconfirmed = Some(lid("s"));
        assert!(!archive_ready(&w));
        let mut w = work("a");
        w.ownership_unknown = true;
        assert!(!archive_ready(&w));
        let mut w = work("a");
        w.queue_pending = true;
        assert!(!archive_ready(&w));
        let mut w = work("a");
        w.unresolved_send = true;
        assert!(!archive_ready(&w));
    }

    #[test]
    fn delete_proceeds_only_when_idle_and_stop_is_confirmed() {
        assert_eq!(delete_decision(&work("a"), None), DeleteDecision::Proceed);
        let mut running = work("a");
        running.has_unfinished = true;
        assert_eq!(delete_decision(&running, None), DeleteDecision::StopFirst);
        let mut queued = work("a");
        queued.queue_pending = true;
        assert_eq!(delete_decision(&queued, None), DeleteDecision::StopFirst);
    }

    #[test]
    fn unconfirmed_stop_ownership_unknown_and_unknown_acceptance_pend() {
        let mut w = work("a");
        w.stop_unconfirmed = Some(lid("s"));
        w.has_unfinished = true;
        assert_eq!(delete_decision(&w, None), DeleteDecision::Pend(DeletePendingReason::StopUnconfirmed));
        let mut w = work("a");
        w.ownership_unknown = true;
        assert_eq!(delete_decision(&w, None), DeleteDecision::Pend(DeletePendingReason::OwnershipUnknown));
        // 受理不明の送信は、実行が始まっているかどうかを確認できない。
        let mut w = work("a");
        w.unresolved_send = true;
        assert_eq!(delete_decision(&w, None), DeleteDecision::Pend(DeletePendingReason::OwnershipUnknown));
    }

    #[test]
    fn stale_pending_stays_pending_until_refreshed_then_waits_for_the_user() {
        let idle = work("a");
        let stale = pending(DeletePendingReason::StopUnconfirmed);
        assert_eq!(delete_decision(&idle, Some(&stale)), DeleteDecision::Pend(DeletePendingReason::StopUnconfirmed));
        let mut p = stale;
        assert!(refresh_delete_pending(&mut p, &idle));
        assert_eq!(p.reason, DeletePendingReason::ReadyForUserRetry);
        // 確認できても自動では削除しない。ユーザーが再度選んだときだけ Proceed になる。
        assert_eq!(delete_decision(&idle, Some(&p)), DeleteDecision::Proceed);
    }

    #[test]
    fn partial_failure_can_be_retried_and_is_not_changed_by_stop_refresh() {
        let mut p = pending(DeletePendingReason::PartialFailure { done: vec!["a".into()], failed: vec!["b".into()] });
        let mut busy = work("a");
        busy.stop_unconfirmed = Some(lid("s"));
        assert!(!refresh_delete_pending(&mut p, &busy));
        assert!(matches!(p.reason, DeletePendingReason::PartialFailure { .. }));
        assert_eq!(delete_decision(&work("a"), Some(&p)), DeleteDecision::Proceed);
        // 作業が残っていれば、部分失敗の保留があっても停止確認が先。
        assert_eq!(delete_decision(&busy, Some(&p)), DeleteDecision::Pend(DeletePendingReason::StopUnconfirmed));
    }

    #[test]
    fn refresh_follows_the_work_in_both_directions() {
        let mut p = pending(DeletePendingReason::StopUnconfirmed);
        let mut owner = work("a");
        owner.ownership_unknown = true;
        assert!(refresh_delete_pending(&mut p, &owner));
        assert_eq!(p.reason, DeletePendingReason::OwnershipUnknown);
        assert!(!refresh_delete_pending(&mut p, &owner), "no change is reported as false");
        let mut stopping = work("a");
        stopping.stop_unconfirmed = Some(lid("s9"));
        assert!(refresh_delete_pending(&mut p, &stopping));
        assert_eq!((p.reason.clone(), p.stop_record.clone()), (DeletePendingReason::StopUnconfirmed, Some(lid("s9"))));
        // 停止を確認できても Ready で止まる（自動削除しない）。再び作業が現れたら保留へ戻る。
        assert!(refresh_delete_pending(&mut p, &work("a")));
        assert_eq!(p.reason, DeletePendingReason::ReadyForUserRetry);
        assert!(refresh_delete_pending(&mut p, &stopping));
        assert_eq!(p.reason, DeletePendingReason::StopUnconfirmed);
    }

    #[test]
    fn partial_failure_is_never_reported_as_deleted() {
        assert_eq!(delete_result(vec!["x".into()], vec![]), DeleteOutcome::Deleted);
        assert_eq!(delete_result(vec!["x".into()], vec!["y".into()]), DeleteOutcome::Partial { done: vec!["x".into()], failed: vec!["y".into()] });
    }

    #[test]
    fn failed_save_of_this_chat_blocks_but_other_chats_do_not() {
        let failed = |scope: SaveScope| SaveStatus {
            scope,
            state: SaveState::SaveFailed { message: "m".into(), saved_part: None, unsaved_part: None },
            last_attempt_at: None,
            retries: 3,
        };
        let st = vec![failed(SaveScope::ChatLocal { chat: ck("other") }), failed(SaveScope::AppSettings)];
        assert_eq!(save_blocks_chat(&st, &ck("a")), None);
        let st = vec![failed(SaveScope::Queue { chat: ck("a") })];
        assert_eq!(save_blocks_chat(&st, &ck("a")), Some(SaveScope::Queue { chat: ck("a") }));
        let unsaved = vec![SaveStatus { scope: SaveScope::ChatLocal { chat: ck("a") }, state: SaveState::Unsaved, last_attempt_at: None, retries: 0 }];
        assert_eq!(save_blocks_chat(&unsaved, &ck("a")), None);
    }

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

    #[test]
    fn in_flight_queue_work_is_not_a_registered_only_queue_and_new_work_skips_handled_chats() {
        // 送信中・終端待ちは「停止を確認できていない」側に倒す（登録だけの Waiting とは別）。
        let mut w = work("a");
        w.queue_pending = true;
        w.queue_in_flight = true;
        assert!(archive_ready(&w) == false);
        let mut p = pending(DeletePendingReason::ReadyForUserRetry);
        assert!(refresh_delete_pending(&mut p, &w));
        assert_eq!(p.reason, DeletePendingReason::StopUnconfirmed);
        // 終了手順: 確認済みのチャットと、送信待ちだけのチャットは新しい作業に数えない。
        let mut waiting_only = work("b");
        waiting_only.queue_pending = true;
        let mut fresh = work("c");
        fresh.has_unfinished = true;
        let mut sending = work("d");
        sending.queue_in_flight = true;
        let works = [w, waiting_only, fresh, sending];
        assert_eq!(new_work_for_quit(&works, &[ck("a")]), vec![ck("c"), ck("d")]);
    }

    #[test]
    fn delete_rejection_for_a_thread_without_history_is_treated_as_nothing_to_delete() {
        use crate::backend::backend::BackendError;
        let rej = |m: &str| BackendError::Rejected { code: Some(-32600), message: m.into() };
        assert!(backend_delete_target_missing(&rej("no rollout found for thread id 01a1")));
        assert!(!backend_delete_target_missing(&rej("permission denied")));
        assert!(!backend_delete_target_missing(&BackendError::OutcomeUnknown { message: "no rollout found for thread id x".into() }));
        assert!(!backend_delete_target_missing(&BackendError::NotConnected));
    }
}
