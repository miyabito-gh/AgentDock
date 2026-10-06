//! 完了後キューの判断（P3、要件§3.7・§4.2）。純粋ロジック。時刻・状態は引数で受け取り、I/Oをしない。
//!
//! ホスト側の流れ:
//! 1. 状態変化（AgentUpdated・TurnEnded・鮮度・停止記録・受理不明・接続）のたびに [`GateInput`] を組み、[`evaluate`] を呼ぶ。
//! 2. `Send` が返ったら [`begin_send`] で項目を `Sending` にし、**保存してから** `AiBackend::send` を呼ぶ
//!    （先に保存しておけば、送信中に落ちても再起動後は受理不明として扱え、無条件再送しない）。
//! 3. 結果を [`apply_send_result`] に渡す。照合の結果も同じ関数に渡す。
//! 4. `Hold` は表示用に `ChatQueue.hold` へ入れる。`Stop` は [`stop`] で `QueueRun::Stopped` にする。
//!
//! キューはAgentDockが保管・判断する（App Server内部キュー `thread/queue/*` は使わない）。

use crate::backend::local::*;
use crate::backend::model::*;

/// エージェント1件分の判断材料。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentFact {
    pub agent: AgentKey,
    pub state: AgentState,
    pub freshness: Freshness,
    /// 観測した最新turnとその終端（未観測は None）。`latest_end` があるときは `latest_turn` も必ずある。
    pub latest_turn: Option<ExternalId>,
    pub latest_end: Option<(TurnEnd, UnixMillis)>,
    /// 履歴の読取りで、最新turnの終端（完了・失敗・中断）を明示的に確認できたか。推定・live通知だけの値は false。
    pub terminal_explicit: bool,
    /// 状態（`terminal_explicit` のときは終端の確認）を観測した時刻。
    pub observed_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateInput {
    pub connected: bool,
    pub root: Option<AgentFact>,
    pub descendants: Vec<AgentFact>,
    /// 子孫の再発見が完了しているか（`DescendantScan.complete`）。未完了なら「状態不明」で保留する。
    pub descendant_scan_complete: bool,
    /// 開いている停止記録のうち、停止未確認・所有不明を含むもの。
    pub open_stop: Option<LocalId>,
    /// このチャットの受理不明の送信（キュー外の直接送信も含む）。
    pub unresolved_attempt: Option<LocalId>,
    pub delete_pending: bool,
    /// 完全終了の手順中（確認・停止・保存）。新しい送信を始めない。
    pub quitting: bool,
    /// ユーザーの手動送信を受理してから、そのturnの開始・終端を観測するまでの間。
    pub manual_turn_pending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    /// 送る項目がない（Waitingなし、または先頭が送信中・NotAccepted でユーザー判断待ち）。
    Nothing,
    /// キューが止まっている（Stopped・PausedAfterRestart）。明示の再開を待つ。
    Paused,
    /// 先頭の Waiting を送ってよい。
    Send { entry: LocalId },
    Hold(QueueHold),
    /// 失敗・中断を検出したので止める（[`stop`] を呼ぶ）。
    Stop(QueueStopCause),
}

fn is_working(s: AgentState) -> bool {
    matches!(s, AgentState::Running | AgentState::Waiting | AgentState::Initializing)
}

fn target(f: &AgentFact) -> HoldTarget {
    HoldTarget { agent: f.agent.clone(), state: f.state, freshness: f.freshness }
}

/// まだ終わっていない項目（Sent・Cancelled 以外）のうち、登録順で先頭のもの。
fn head(queue: &ChatQueue) -> Option<&QueueEntry> {
    queue.entries.iter().filter(|e| !matches!(e.state, QueueEntryState::Sent { .. } | QueueEntryState::Cancelled)).min_by_key(|e| e.order)
}

/// 送信待ちの項目があるか（ホストが評価・保存の要否を決めるため）。
pub fn has_pending(queue: &ChatQueue) -> bool {
    head(queue).is_some()
}

fn is_after_baseline(at: UnixMillis, baseline: Option<UnixMillis>) -> bool {
    baseline.is_none_or(|b| at >= b)
}

/// 子孫・親の失敗・中断を `baseline_at` 以降に観測していれば止める原因を返す。
/// 終端の時刻が分からない値は呼出し側が観測時刻を入れる（保守的に止める側へ倒す）。
fn failure_of(f: &AgentFact, is_root: bool, baseline: Option<UnixMillis>) -> Option<QueueStopCause> {
    let end = match (&f.latest_end, &f.latest_turn) {
        (Some((end, at)), Some(turn)) if matches!(end, TurnEnd::Failed | TurnEnd::Interrupted) && is_after_baseline(*at, baseline) => {
            Some((*end, Some(TurnKey { agent: f.agent.clone(), turn_id: turn.clone() })))
        }
        (None, _) if matches!(f.state, AgentState::Failed | AgentState::Interrupted) && is_after_baseline(f.observed_at, baseline) => {
            // 終端通知は見ていないが、状態が失敗・中断と明示されている。
            let end = if f.state == AgentState::Failed { TurnEnd::Failed } else { TurnEnd::Interrupted };
            Some((end, f.latest_turn.clone().map(|t| TurnKey { agent: f.agent.clone(), turn_id: t })))
        }
        _ => None,
    }?;
    Some(match (is_root, end.0, end.1) {
        (true, TurnEnd::Failed, Some(turn)) => QueueStopCause::ParentFailed { turn },
        (true, _, Some(turn)) => QueueStopCause::ParentInterrupted { turn },
        (true, TurnEnd::Failed, None) => return None,
        (true, _, None) => return None,
        (false, TurnEnd::Failed, _) => QueueStopCause::DescendantFailed { agent: f.agent.clone() },
        (false, _, _) => QueueStopCause::DescendantInterrupted { agent: f.agent.clone() },
    })
}

#[derive(Debug, PartialEq)]
enum Class {
    Working,
    Unknown,
    NotLive,
    Ok,
}

/// 子孫1件の分類（親の分類は別。DESIGN_P2 §2.3）。
fn classify_descendant(f: &AgentFact, baseline: Option<UnixMillis>) -> Class {
    if is_working(f.state) {
        return Class::Working;
    }
    // 終端が履歴で明示確認でき、その確認が待っている間のものなら、live購読でなくても可。
    if f.terminal_explicit && is_after_baseline(f.observed_at, baseline) {
        return Class::Ok;
    }
    if f.state == AgentState::Unknown {
        return Class::Unknown;
    }
    if f.freshness == Freshness::Live {
        Class::Ok
    } else {
        Class::NotLive
    }
}

/// 自動送信の可否を判断する。判断の順序:
/// 1. 終わっていない項目がなければ `Nothing`。`run` が Active でなければ `Paused`。先頭が Waiting でなければ
///    （送信中・NotAccepted は `Nothing`、受理不明は `Hold(AcceptanceUnknown)`）。
/// 2. 削除保留・切断・停止未確認・受理不明 → `Hold`。
/// 3. 親・子孫の `baseline_at` 以降の failed / interrupted → `Stop`。
/// 4. 親が作業中（または自動送信したturnの終端が未確認） → `Hold(ParentWorking)`。親の状態が不明 → `Hold(StateUnknown)`。
///    子孫に running / waiting / initializing → `Hold(DescendantsActive)`。
/// 5. 子孫に unknown・走査未完了 → `Hold(StateUnknown)`。
/// 6. 鮮度: 親は live 必須。子孫は live、または終端を明示的に確認済み（`terminal_explicit`）かつ
///    `observed_at >= baseline_at` の履歴取得済みなら可。それ以外 → `Hold(NotLive)`（DESIGN_P2 §2.3）。
/// 7. すべて満たせば先頭の Waiting を `Send`。
pub fn evaluate(queue: &ChatQueue, input: &GateInput) -> GateDecision {
    let Some(first) = head(queue) else { return GateDecision::Nothing };
    if queue.run != QueueRun::Active {
        return GateDecision::Paused;
    }
    match &first.state {
        QueueEntryState::Waiting => {}
        QueueEntryState::AcceptanceUnknown { attempt } => return GateDecision::Hold(QueueHold::AcceptanceUnknown { attempt: attempt.clone() }),
        _ => return GateDecision::Nothing,
    }
    if input.quitting {
        return GateDecision::Hold(QueueHold::Quitting);
    }
    if input.delete_pending {
        return GateDecision::Hold(QueueHold::DeletePending);
    }
    if !input.connected {
        return GateDecision::Hold(QueueHold::Disconnected);
    }
    if let Some(record) = &input.open_stop {
        return GateDecision::Hold(QueueHold::StopUnconfirmed { record: record.clone() });
    }
    if let Some(attempt) = &input.unresolved_attempt {
        return GateDecision::Hold(QueueHold::AcceptanceUnknown { attempt: attempt.clone() });
    }
    let Some(root) = &input.root else {
        // 親の状態を取得できていない。
        return GateDecision::Hold(QueueHold::StateUnknown { targets: Vec::new(), descendant_scan_incomplete: !input.descendant_scan_complete });
    };
    let baseline = queue.baseline_at;
    if let Some(cause) = failure_of(root, true, baseline) {
        return GateDecision::Stop(cause);
    }
    for d in &input.descendants {
        if let Some(cause) = failure_of(d, false, baseline) {
            return GateDecision::Stop(cause);
        }
    }
    // 終端を観測したのに対応するturnがない値は信用しない。
    if let Some(bad) = std::iter::once(root).chain(input.descendants.iter()).find(|f| f.latest_end.is_some() && f.latest_turn.is_none()) {
        return GateDecision::Hold(QueueHold::StateUnknown { targets: vec![target(bad)], descendant_scan_incomplete: false });
    }
    if is_working(root.state) || queue.awaiting.is_some() || input.manual_turn_pending {
        return GateDecision::Hold(QueueHold::ParentWorking);
    }
    if matches!(root.state, AgentState::Unknown | AgentState::Closed) {
        return GateDecision::Hold(QueueHold::StateUnknown { targets: vec![target(root)], descendant_scan_incomplete: false });
    }
    let classes: Vec<(&AgentFact, Class)> = input.descendants.iter().map(|d| (d, classify_descendant(d, baseline))).collect();
    let of = |c: Class| -> Vec<HoldTarget> { classes.iter().filter(|(_, k)| *k == c).map(|(f, _)| target(f)).collect() };
    let working = of(Class::Working);
    if !working.is_empty() {
        return GateDecision::Hold(QueueHold::DescendantsActive { targets: working });
    }
    let unknown = of(Class::Unknown);
    if !unknown.is_empty() || !input.descendant_scan_complete {
        return GateDecision::Hold(QueueHold::StateUnknown { targets: unknown, descendant_scan_incomplete: !input.descendant_scan_complete });
    }
    let mut not_live = of(Class::NotLive);
    if root.freshness != Freshness::Live {
        not_live.insert(0, target(root));
    }
    if !not_live.is_empty() {
        return GateDecision::Hold(QueueHold::NotLive { targets: not_live });
    }
    GateDecision::Send { entry: first.id.clone() }
}

/// 空のキュー。
pub fn new_queue(chat: ChatKey) -> ChatQueue {
    ChatQueue { chat, run: QueueRun::Active, hold: None, baseline_at: None, awaiting: None, entries: Vec::new(), next_order: 0 }
}

/// 登録（ユーザー操作）。`order` は `next_order` を使って単調増加させる。設定はここで固定しない。
/// 失敗・中断の監視の基準（`baseline_at`）は、キューが空いている（待っている依頼も送信済みturnの待ちもない）間の登録で
/// 登録時刻にする。登録前に終わった失敗では止めない。待っている依頼があるときは動かさない。
pub fn enqueue(queue: &mut ChatQueue, id: LocalId, text: String, attachments: Vec<LocalId>, now: UnixMillis) -> QueueEntry {
    if queue.baseline_at.is_none() || (!has_pending(queue) && queue.awaiting.is_none()) {
        queue.baseline_at = Some(now);
    }
    let entry = QueueEntry {
        id,
        chat: queue.chat.clone(),
        text,
        attachments,
        order: queue.next_order,
        registered_at: now,
        state: QueueEntryState::Waiting,
        attempts: Vec::new(),
        applied: None,
    };
    queue.next_order += 1;
    queue.entries.push(entry.clone());
    entry
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    NotFound,
    /// Waiting 以外は編集・取消できない（送信中・受理不明・送信済み）。
    NotEditable,
}

pub fn edit(queue: &mut ChatQueue, id: &LocalId, text: String, attachments: Vec<LocalId>) -> Result<(), EditError> {
    let e = queue.entries.iter_mut().find(|e| &e.id == id).ok_or(EditError::NotFound)?;
    if e.state != QueueEntryState::Waiting {
        return Err(EditError::NotEditable);
    }
    e.text = text;
    e.attachments = attachments;
    Ok(())
}

/// 取消。Waiting と NotAccepted だけ取り消せる。
pub fn cancel(queue: &mut ChatQueue, id: &LocalId) -> Result<(), EditError> {
    let e = queue.entries.iter_mut().find(|e| &e.id == id).ok_or(EditError::NotFound)?;
    if !matches!(e.state, QueueEntryState::Waiting | QueueEntryState::NotAccepted { .. }) {
        return Err(EditError::NotEditable);
    }
    e.state = QueueEntryState::Cancelled;
    Ok(())
}

/// 送信開始。Waiting（または、ユーザーが再送を選んだ NotAccepted）の項目を `Sending{attempt}` にし、送信時点の設定を記録する。
/// 戻り値は開始できたか（それ以外の状態の項目は変えない）。
pub fn begin_send(queue: &mut ChatQueue, id: &LocalId, attempt: SendAttempt, applied: AppliedSettings) -> bool {
    let Some(e) = queue.entries.iter_mut().find(|e| &e.id == id) else { return false };
    if !matches!(e.state, QueueEntryState::Waiting | QueueEntryState::NotAccepted { .. }) {
        return false;
    }
    e.state = QueueEntryState::Sending { attempt: attempt.attempt_id.clone() };
    e.applied = Some(applied);
    e.attempts.push(SendAttempt { state: SendState::Sending, ..attempt });
    queue.hold = None;
    true
}

/// 送信前の保存に失敗するなど、何も送らずに取り消すとき。送っていないので Waiting に戻す。
pub fn abort_send(queue: &mut ChatQueue, id: &LocalId) {
    if let Some(e) = queue.entries.iter_mut().find(|e| &e.id == id) {
        if matches!(e.state, QueueEntryState::Sending { .. }) {
            e.state = QueueEntryState::Waiting;
            e.applied = None;
            e.attempts.pop();
        }
    }
}

/// 再送（ユーザー確認つき）の送信前の保存に失敗するなど、何も送らずに取り消すとき。元の `NotAccepted` に戻す（再送権を失わない）。
pub fn abort_retry(queue: &mut ChatQueue, id: &LocalId, prior_attempt: LocalId, message: String) {
    if let Some(e) = queue.entries.iter_mut().find(|e| &e.id == id) {
        if matches!(e.state, QueueEntryState::Sending { .. }) {
            e.state = QueueEntryState::NotAccepted { attempt: prior_attempt, message };
            e.attempts.pop();
        }
    }
}

/// 強制終了でプロセスの消滅を確認できたとき、送信済みで終端待ち（`awaiting`）のturnがあれば、待ちを外してキューを止める。
/// 完了扱いにはしない。変更があれば true。
pub fn stop_after_kill(queue: &mut ChatQueue, now: UnixMillis) -> bool {
    let Some(turn) = queue.awaiting.take() else { return false };
    stop(queue, QueueStopCause::AppServerKilled { turn }, now);
    true
}

/// 送信・照合の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum SendResult {
    Accepted { attempt: SendAttempt, turn: TurnKey, turn_started_at: UnixMillis },
    Rejected { attempt: SendAttempt, message: String },
    Unknown { attempt: SendAttempt },
    ReconciledAccepted { attempt: LocalId, turn: TurnKey },
    ReconciledNotFound { attempt: LocalId },
}

fn set_attempt(e: &mut QueueEntry, attempt: &LocalId, state: SendState) {
    if let Some(a) = e.attempts.iter_mut().find(|a| &a.attempt_id == attempt) {
        a.state = state;
    }
}

/// 結果を反映する。Accepted は `Sent` にして `baseline_at` を新しいturnの開始へ進め、そのturnの終端を待つ（`awaiting`）。
/// Rejected・ReconciledNotFound は項目を `NotAccepted` にし、キューを `Stopped` にする（自動再送しない）。
/// Unknown は `AcceptanceUnknown`（評価で `Hold(AcceptanceUnknown)` になる）。
pub fn apply_send_result(queue: &mut ChatQueue, entry: &LocalId, result: SendResult, now: UnixMillis) {
    let Some(e) = queue.entries.iter_mut().find(|e| &e.id == entry) else { return };
    match result {
        SendResult::Accepted { attempt, turn, turn_started_at } => {
            set_attempt(e, &attempt.attempt_id, SendState::Accepted { turn: turn.clone() });
            e.state = QueueEntryState::Sent { turn: turn.clone() };
            queue.baseline_at = Some(turn_started_at);
            queue.awaiting = Some(turn);
            queue.hold = None;
        }
        SendResult::Rejected { attempt, message } => {
            set_attempt(e, &attempt.attempt_id, SendState::Rejected { message: message.clone() });
            e.state = QueueEntryState::NotAccepted { attempt: attempt.attempt_id, message };
            stop(queue, QueueStopCause::SendRejected { entry: entry.clone() }, now);
        }
        SendResult::Unknown { attempt } => {
            set_attempt(e, &attempt.attempt_id, SendState::AcceptanceUnknown { since: attempt.at });
            e.state = QueueEntryState::AcceptanceUnknown { attempt: attempt.attempt_id };
        }
        SendResult::ReconciledAccepted { attempt, turn } => {
            set_attempt(e, &attempt, SendState::Accepted { turn: turn.clone() });
            e.state = QueueEntryState::Sent { turn: turn.clone() };
            queue.baseline_at = Some(now);
            queue.awaiting = Some(turn);
        }
        SendResult::ReconciledNotFound { attempt } => {
            set_attempt(e, &attempt, SendState::NotFoundAfterReconcile);
            e.state = QueueEntryState::NotAccepted { attempt, message: "履歴に受理の痕跡がありませんでした".into() };
            stop(queue, QueueStopCause::NotAcceptedAfterReconcile { entry: entry.clone() }, now);
        }
    }
}

/// 自動送信した（または照合で受理と分かった）turnの終端を確認した。完了なら待ちを外し、失敗・中断ならキューを止める。
/// 戻り値は変更があったか。
pub fn turn_ended(queue: &mut ChatQueue, turn: &TurnKey, end: TurnEnd, now: UnixMillis) -> bool {
    if queue.awaiting.as_ref() != Some(turn) {
        return false;
    }
    queue.awaiting = None;
    match end {
        TurnEnd::Completed => {}
        TurnEnd::Failed => stop(queue, QueueStopCause::ParentFailed { turn: turn.clone() }, now),
        TurnEnd::Interrupted => stop(queue, QueueStopCause::ParentInterrupted { turn: turn.clone() }, now),
    }
    true
}

pub fn stop(queue: &mut ChatQueue, cause: QueueStopCause, now: UnixMillis) {
    queue.run = QueueRun::Stopped { cause, at: now };
    queue.hold = None;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeError {
    AlreadyActive,
    /// 受理不明の項目がある（照合で確定するまで再開できない）。
    AcceptanceUnknown,
}

/// 明示的な「キューを再開」（ユーザー操作）。`Active` に戻し、`baseline_at` を `now` にする。
/// NotAccepted の項目は送らない（ユーザーが再送か取消を選ぶまで先頭で止まる）。「確認済み」はこれを呼ばない。
/// 止まる前に待っていたturn（`awaiting`）は、失敗・中断を承知のうえの再開なので外す。
pub fn resume(queue: &mut ChatQueue, now: UnixMillis) -> Result<(), ResumeError> {
    if queue.run == QueueRun::Active {
        return Err(ResumeError::AlreadyActive);
    }
    if queue.entries.iter().any(|e| matches!(e.state, QueueEntryState::AcceptanceUnknown { .. })) {
        return Err(ResumeError::AcceptanceUnknown);
    }
    queue.run = QueueRun::Active;
    queue.baseline_at = Some(now);
    queue.awaiting = None;
    queue.hold = None;
    Ok(())
}

/// 起動時の復元。`Sending` は `AcceptanceUnknown` に、`Active` は `PausedAfterRestart` にする（再起動後に自動送信しない）。
pub fn restore_after_restart(queue: &mut ChatQueue) {
    for entry in &mut queue.entries {
        if let QueueEntryState::Sending { attempt } = &entry.state {
            entry.state = QueueEntryState::AcceptanceUnknown { attempt: attempt.clone() };
        }
        // 送信中のまま終わった試行も受理不明にする（受理されたかは照合まで分からない。再送しない）。
        for a in &mut entry.attempts {
            if a.state == SendState::Sending {
                a.state = SendState::AcceptanceUnknown { since: a.at };
            }
        }
    }
    if queue.run == QueueRun::Active {
        queue.run = QueueRun::PausedAfterRestart;
    }
    // 保留理由は保存しない表示用の値。評価のたびにホストが作り直す。
    queue.hold = None;
}

/// モデル・権限の変更が及ぶ送信待ちの項目（UIの影響表示用）。送信時点の設定を使うので、Waiting がすべて対象。
pub fn affected_by_settings_change(queue: &ChatQueue) -> Vec<LocalId> {
    queue.entries.iter().filter(|e| e.state == QueueEntryState::Waiting).map(|e| e.id.clone()).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum CwdChangeCheck {
    /// そのまま変更できる。
    Allowed,
    /// 送信待ちがあり、新しいフォルダが対象になることの確認が要る（M44）。
    NeedsQueueConfirmation { waiting: u32 },
    /// 作業中・停止未確認のため変更できない（完了・停止後に変更）。
    Busy,
}

/// 作業フォルダ変更の可否（M44）。`busy` は親・子孫に終端未確認の作業がある・停止記録が開いているか。
pub fn check_cwd_change(queue: &ChatQueue, busy: bool, queue_retarget_confirmed: bool) -> CwdChangeCheck {
    if busy {
        return CwdChangeCheck::Busy;
    }
    let waiting = affected_by_settings_change(queue).len() as u32;
    if waiting > 0 && !queue_retarget_confirmed {
        return CwdChangeCheck::NeedsQueueConfirmation { waiting };
    }
    CwdChangeCheck::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::backend::PermissionPreset;

    fn chat() -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId("root".into()) }
    }
    fn ak(id: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn tk(agent: &str, turn: &str) -> TurnKey {
        TurnKey { agent: ak(agent), turn_id: ExternalId(turn.into()) }
    }
    fn lid(s: &str) -> LocalId {
        LocalId(s.into())
    }
    fn fact(agent: &str, state: AgentState, fresh: Freshness) -> AgentFact {
        AgentFact { agent: ak(agent), state, freshness: fresh, latest_turn: None, latest_end: None, terminal_explicit: false, observed_at: UnixMillis(1000) }
    }
    fn ended(mut f: AgentFact, end: TurnEnd, at: i64) -> AgentFact {
        f.latest_turn = Some(ExternalId("t1".into()));
        f.latest_end = Some((end, UnixMillis(at)));
        f
    }
    fn queue_with(texts: &[&str], baseline: i64) -> ChatQueue {
        let mut q = new_queue(chat());
        for (i, t) in texts.iter().enumerate() {
            enqueue(&mut q, lid(&format!("q{i}")), (*t).into(), vec![], UnixMillis(10 + i as i64));
        }
        q.baseline_at = Some(UnixMillis(baseline));
        q
    }
    /// 親が完了・子孫なし・走査完了・接続中の基本入力。
    fn input() -> GateInput {
        GateInput {
            connected: true,
            root: Some(ended(fact("root", AgentState::Done, Freshness::Live), TurnEnd::Completed, 2000)),
            descendants: vec![],
            descendant_scan_complete: true,
            open_stop: None,
            unresolved_attempt: None,
            delete_pending: false,
            quitting: false,
            manual_turn_pending: false,
        }
    }
    fn attempt(id: &str) -> SendAttempt {
        SendAttempt { attempt_id: lid(id), client_message_id: format!("cm-{id}"), at: UnixMillis(3000), state: SendState::Sending }
    }
    fn applied() -> AppliedSettings {
        AppliedSettings { model: None, permission: PermissionPreset::WorkspaceWriteOnRequest, cwd: Known::NotFetched, decided_at: UnixMillis(3000) }
    }

    #[test]
    fn sends_head_when_parent_completed_and_no_descendants() {
        let q = queue_with(&["a", "b"], 1500);
        assert_eq!(evaluate(&q, &input()), GateDecision::Send { entry: lid("q0") });
    }

    #[test]
    fn nothing_when_empty_and_paused_when_not_active() {
        assert_eq!(evaluate(&new_queue(chat()), &input()), GateDecision::Nothing);
        let mut q = queue_with(&["a"], 1500);
        q.run = QueueRun::PausedAfterRestart;
        assert_eq!(evaluate(&q, &input()), GateDecision::Paused);
        stop(&mut q, QueueStopCause::SendRejected { entry: lid("q0") }, UnixMillis(5));
        assert_eq!(evaluate(&q, &input()), GateDecision::Paused);
    }

    #[test]
    fn parent_working_holds() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.root = Some(fact("root", AgentState::Running, Freshness::Live));
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::ParentWorking));
    }

    #[test]
    fn parent_done_but_child_working_holds_with_target() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.descendants = vec![fact("c1", AgentState::Running, Freshness::Live), fact("c2", AgentState::Idle, Freshness::Live)];
        match evaluate(&q, &i) {
            GateDecision::Hold(QueueHold::DescendantsActive { targets }) => {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].agent, ak("c1"));
            }
            other => panic!("{other:?}"),
        }
        // 待機中（waiting）も作業中として扱う。
        i.descendants = vec![fact("c1", AgentState::Waiting, Freshness::Live)];
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::DescendantsActive { .. })));
    }

    #[test]
    fn unknown_descendant_holds_as_state_unknown() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.descendants = vec![fact("c1", AgentState::Unknown, Freshness::Live)];
        match evaluate(&q, &i) {
            GateDecision::Hold(QueueHold::StateUnknown { targets, descendant_scan_incomplete }) => {
                assert_eq!(targets[0].agent, ak("c1"));
                assert!(!descendant_scan_incomplete);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn incomplete_descendant_scan_holds() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.descendant_scan_complete = false;
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { descendant_scan_incomplete: true, .. })));
    }

    #[test]
    fn history_only_child_is_ok_only_with_explicit_terminal_confirmed_after_baseline() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        // 履歴取得済み・終端の明示なし → 鮮度不足で保留。
        i.descendants = vec![fact("c1", AgentState::Idle, Freshness::HistoryOnly)];
        match evaluate(&q, &i) {
            GateDecision::Hold(QueueHold::NotLive { targets }) => assert_eq!(targets[0].agent, ak("c1")),
            other => panic!("{other:?}"),
        }
        // 終端を明示確認済み（baseline以降）→ 可。
        let mut done = ended(fact("c1", AgentState::Unknown, Freshness::HistoryOnly), TurnEnd::Completed, 1200);
        done.terminal_explicit = true;
        done.observed_at = UnixMillis(2500);
        i.descendants = vec![done.clone()];
        assert_eq!(evaluate(&q, &i), GateDecision::Send { entry: lid("q0") });
        // 確認がbaselineより前（古い確認）なら信用しない。
        done.observed_at = UnixMillis(1400);
        i.descendants = vec![done];
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { .. })));
    }

    #[test]
    fn parent_must_be_live() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.root = Some(ended(fact("root", AgentState::Done, Freshness::HistoryOnly), TurnEnd::Completed, 2000));
        match evaluate(&q, &i) {
            GateDecision::Hold(QueueHold::NotLive { targets }) => assert_eq!(targets[0].agent, ak("root")),
            other => panic!("{other:?}"),
        }
        // 親の状態が不明なら状態不明で保留。
        i.root = Some(fact("root", AgentState::Unknown, Freshness::Live));
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { .. })));
    }

    #[test]
    fn failure_before_baseline_is_ignored_and_after_baseline_stops() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        // baseline前（1000）の失敗は無視して送れる。
        i.descendants = vec![ended(fact("c1", AgentState::Failed, Freshness::Live), TurnEnd::Failed, 1000)];
        assert_eq!(evaluate(&q, &i), GateDecision::Send { entry: lid("q0") });
        // baseline以降の子の失敗・中断は止める。
        i.descendants = vec![ended(fact("c1", AgentState::Failed, Freshness::Live), TurnEnd::Failed, 1600)];
        assert_eq!(evaluate(&q, &i), GateDecision::Stop(QueueStopCause::DescendantFailed { agent: ak("c1") }));
        i.descendants = vec![ended(fact("c1", AgentState::Interrupted, Freshness::Live), TurnEnd::Interrupted, 1600)];
        assert_eq!(evaluate(&q, &i), GateDecision::Stop(QueueStopCause::DescendantInterrupted { agent: ak("c1") }));
        // 親の失敗・中断。
        i.descendants = vec![];
        i.root = Some(ended(fact("root", AgentState::Failed, Freshness::Live), TurnEnd::Failed, 1600));
        assert_eq!(evaluate(&q, &i), GateDecision::Stop(QueueStopCause::ParentFailed { turn: tk("root", "t1") }));
        i.root = Some(ended(fact("root", AgentState::Interrupted, Freshness::Live), TurnEnd::Interrupted, 1600));
        assert_eq!(evaluate(&q, &i), GateDecision::Stop(QueueStopCause::ParentInterrupted { turn: tk("root", "t1") }));
    }

    #[test]
    fn missing_or_inconsistent_parent_facts_hold_as_state_unknown() {
        let q = queue_with(&["a"], 1500);
        // 親の状態を取得できていない。
        let mut i = input();
        i.root = None;
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { ref targets, .. }) if targets.is_empty()));
        // 親が Closed。
        i.root = Some(fact("root", AgentState::Closed, Freshness::Live));
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { .. })));
        // 終端を観測したのに対応するturnがない値は信用しない（親・子孫とも）。
        let mut bad = fact("root", AgentState::Done, Freshness::Live);
        bad.latest_end = Some((TurnEnd::Completed, UnixMillis(2000)));
        i.root = Some(bad.clone());
        assert!(matches!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StateUnknown { .. })));
        let mut i = input();
        bad.agent = ak("c1");
        i.descendants = vec![bad];
        match evaluate(&q, &i) {
            GateDecision::Hold(QueueHold::StateUnknown { targets, .. }) => assert_eq!(targets[0].agent, ak("c1")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn explicit_resume_moves_baseline_so_the_old_failure_no_longer_stops() {
        let mut q = queue_with(&["a", "b"], 1500);
        let mut i = input();
        i.root = Some(ended(fact("root", AgentState::Failed, Freshness::Live), TurnEnd::Failed, 1600));
        let GateDecision::Stop(cause) = evaluate(&q, &i) else { panic!("should stop") };
        stop(&mut q, cause, UnixMillis(1700));
        assert_eq!(evaluate(&q, &i), GateDecision::Paused);
        resume(&mut q, UnixMillis(1800)).unwrap();
        // 失敗した依頼そのものは再実行せず、残りの先頭を送る。
        assert_eq!(evaluate(&q, &i), GateDecision::Send { entry: lid("q0") });
        // 再開後の新しい失敗では、また止まる。
        i.root = Some(ended(fact("root", AgentState::Failed, Freshness::Live), TurnEnd::Failed, 1900));
        assert!(matches!(evaluate(&q, &i), GateDecision::Stop(_)));
    }

    #[test]
    fn disconnected_stop_unconfirmed_unknown_send_and_delete_pending_hold() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.connected = false;
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::Disconnected));
        let mut i = input();
        i.open_stop = Some(lid("s1"));
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::StopUnconfirmed { record: lid("s1") }));
        let mut i = input();
        i.unresolved_attempt = Some(lid("at1"));
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::AcceptanceUnknown { attempt: lid("at1") }));
        let mut i = input();
        i.delete_pending = true;
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::DeletePending));
    }

    #[test]
    fn acceptance_unknown_head_holds_and_other_heads_wait_for_the_user() {
        let mut q = queue_with(&["a", "b"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        // 送信中の間は何もしない。
        assert_eq!(evaluate(&q, &input()), GateDecision::Nothing);
        apply_send_result(&mut q, &lid("q0"), SendResult::Unknown { attempt: attempt("at1") }, UnixMillis(3100));
        assert_eq!(evaluate(&q, &input()), GateDecision::Hold(QueueHold::AcceptanceUnknown { attempt: lid("at1") }));
        // 照合で受理なしが確定 → 先頭は NotAccepted。キューは止まり、再開しても先頭で止まる（再送しない）。
        apply_send_result(&mut q, &lid("q0"), SendResult::ReconciledNotFound { attempt: lid("at1") }, UnixMillis(3200));
        assert!(matches!(q.run, QueueRun::Stopped { cause: QueueStopCause::NotAcceptedAfterReconcile { .. }, .. }));
        resume(&mut q, UnixMillis(3300)).unwrap();
        assert_eq!(evaluate(&q, &input()), GateDecision::Nothing);
        // ユーザーが取り消せば、次の項目へ進む。
        cancel(&mut q, &lid("q0")).unwrap();
        assert_eq!(evaluate(&q, &input()), GateDecision::Send { entry: lid("q1") });
    }

    #[test]
    fn accepted_send_waits_for_that_turn_then_next_goes_only_after_completion() {
        let mut q = queue_with(&["a", "b"], 1500);
        assert!(begin_send(&mut q, &lid("q0"), attempt("at1"), applied()));
        apply_send_result(&mut q, &lid("q0"), SendResult::Accepted { attempt: attempt("at1"), turn: tk("root", "t2"), turn_started_at: UnixMillis(3000) }, UnixMillis(3000));
        assert_eq!(q.entries[0].state, QueueEntryState::Sent { turn: tk("root", "t2") });
        assert_eq!(q.baseline_at, Some(UnixMillis(3000)));
        // 親の状態がまだ前のturnのままでも、送ったturnの終端を確認するまで次へ進まない。
        assert_eq!(evaluate(&q, &input()), GateDecision::Hold(QueueHold::ParentWorking));
        assert!(!turn_ended(&mut q, &tk("root", "other"), TurnEnd::Completed, UnixMillis(3500)));
        assert!(turn_ended(&mut q, &tk("root", "t2"), TurnEnd::Completed, UnixMillis(3500)));
        assert_eq!(evaluate(&q, &input()), GateDecision::Send { entry: lid("q1") });
    }

    #[test]
    fn awaited_turn_failure_stops_the_queue_and_resume_does_not_rerun_it() {
        let mut q = queue_with(&["a", "b"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        apply_send_result(&mut q, &lid("q0"), SendResult::Accepted { attempt: attempt("at1"), turn: tk("root", "t2"), turn_started_at: UnixMillis(3000) }, UnixMillis(3000));
        assert!(turn_ended(&mut q, &tk("root", "t2"), TurnEnd::Failed, UnixMillis(3500)));
        assert!(matches!(q.run, QueueRun::Stopped { cause: QueueStopCause::ParentFailed { .. }, .. }));
        resume(&mut q, UnixMillis(3600)).unwrap();
        assert!(q.awaiting.is_none());
        assert_eq!(q.entries[0].state, QueueEntryState::Sent { turn: tk("root", "t2") }, "the failed request itself is not rerun");
        assert_eq!(evaluate(&q, &input()), GateDecision::Send { entry: lid("q1") });
    }

    #[test]
    fn rejected_send_stops_and_records_not_accepted() {
        let mut q = queue_with(&["a"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        apply_send_result(&mut q, &lid("q0"), SendResult::Rejected { attempt: attempt("at1"), message: "no".into() }, UnixMillis(3100));
        assert_eq!(q.entries[0].state, QueueEntryState::NotAccepted { attempt: lid("at1"), message: "no".into() });
        assert_eq!(q.entries[0].attempts[0].state, SendState::Rejected { message: "no".into() });
        assert!(matches!(q.run, QueueRun::Stopped { cause: QueueStopCause::SendRejected { .. }, at: UnixMillis(3100) }));
    }

    #[test]
    fn reconciled_accepted_marks_sent_without_resending() {
        let mut q = queue_with(&["a", "b"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        apply_send_result(&mut q, &lid("q0"), SendResult::Unknown { attempt: attempt("at1") }, UnixMillis(3100));
        apply_send_result(&mut q, &lid("q0"), SendResult::ReconciledAccepted { attempt: lid("at1"), turn: tk("root", "t2") }, UnixMillis(4000));
        assert_eq!(q.entries[0].state, QueueEntryState::Sent { turn: tk("root", "t2") });
        assert_eq!(q.entries[0].attempts[0].state, SendState::Accepted { turn: tk("root", "t2") });
        assert_eq!(q.baseline_at, Some(UnixMillis(4000)));
        assert_eq!(q.entries[0].attempts.len(), 1, "no new attempt is created by reconciliation");
    }

    #[test]
    fn abort_send_returns_to_waiting_without_leaving_an_attempt() {
        let mut q = queue_with(&["a"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        abort_send(&mut q, &lid("q0"));
        assert_eq!(q.entries[0].state, QueueEntryState::Waiting);
        assert!(q.entries[0].attempts.is_empty() && q.entries[0].applied.is_none());
    }

    #[test]
    fn enqueue_orders_edit_and_cancel_rules() {
        let mut q = new_queue(chat());
        let a = enqueue(&mut q, lid("a"), "x".into(), vec![], UnixMillis(5));
        let b = enqueue(&mut q, lid("b"), "y".into(), vec![], UnixMillis(6));
        assert_eq!((a.order, b.order, q.next_order), (0, 1, 2));
        assert_eq!(q.baseline_at, Some(UnixMillis(5)), "baseline is the first registration time and does not move while a request waits");
        // 空いたキュー（送信済みだけ）への登録では、登録時刻へ進める（登録前の失敗では止めない）。
        let mut idle = new_queue(chat());
        enqueue(&mut idle, lid("x"), "x".into(), vec![], UnixMillis(5));
        idle.entries[0].state = QueueEntryState::Sent { turn: tk("root", "t1") };
        enqueue(&mut idle, lid("y"), "y".into(), vec![], UnixMillis(50));
        assert_eq!(idle.baseline_at, Some(UnixMillis(50)));
        idle.entries[1].state = QueueEntryState::Sent { turn: tk("root", "t2") };
        idle.awaiting = Some(tk("root", "t2"));
        enqueue(&mut idle, lid("z"), "z".into(), vec![], UnixMillis(60));
        assert_eq!(idle.baseline_at, Some(UnixMillis(50)), "an awaited turn keeps the baseline");
        edit(&mut q, &lid("a"), "x2".into(), vec![]).unwrap();
        assert_eq!(q.entries[0].text, "x2");
        assert_eq!(edit(&mut q, &lid("zz"), "".into(), vec![]), Err(EditError::NotFound));
        begin_send(&mut q, &lid("a"), attempt("at1"), applied());
        assert_eq!(edit(&mut q, &lid("a"), "x3".into(), vec![]), Err(EditError::NotEditable));
        assert_eq!(cancel(&mut q, &lid("a")), Err(EditError::NotEditable));
        cancel(&mut q, &lid("b")).unwrap();
        assert_eq!(q.entries[1].state, QueueEntryState::Cancelled);
    }

    #[test]
    fn resume_rules() {
        let mut q = queue_with(&["a"], 1500);
        assert_eq!(resume(&mut q, UnixMillis(9)), Err(ResumeError::AlreadyActive));
        q.run = QueueRun::PausedAfterRestart;
        q.entries[0].state = QueueEntryState::AcceptanceUnknown { attempt: lid("at1") };
        assert_eq!(resume(&mut q, UnixMillis(9)), Err(ResumeError::AcceptanceUnknown));
        q.entries[0].state = QueueEntryState::Waiting;
        resume(&mut q, UnixMillis(9)).unwrap();
        assert_eq!((q.run.clone(), q.baseline_at), (QueueRun::Active, Some(UnixMillis(9))));
    }

    #[test]
    fn restore_after_restart_never_leaves_sending_or_active() {
        let mut q = queue_with(&["a", "b"], 1500);
        begin_send(&mut q, &lid("q0"), attempt("at1"), applied());
        q.hold = Some(QueueHold::ParentWorking);
        restore_after_restart(&mut q);
        assert_eq!(q.entries[0].state, QueueEntryState::AcceptanceUnknown { attempt: lid("at1") });
        assert_eq!(q.entries[0].attempts[0].state, SendState::AcceptanceUnknown { since: UnixMillis(3000) });
        assert_eq!(q.run, QueueRun::PausedAfterRestart);
        assert!(q.hold.is_none());
        // 再起動後は自動送信しない。
        assert_eq!(evaluate(&q, &input()), GateDecision::Paused);
        // 停止中のキューは Stopped のまま。
        let mut s = queue_with(&["a"], 1500);
        stop(&mut s, QueueStopCause::SendRejected { entry: lid("q0") }, UnixMillis(7));
        restore_after_restart(&mut s);
        assert!(matches!(s.run, QueueRun::Stopped { .. }));
    }

    #[test]
    fn cwd_change_check() {
        let mut q = queue_with(&["a", "b"], 1500);
        assert_eq!(check_cwd_change(&q, true, true), CwdChangeCheck::Busy);
        assert_eq!(check_cwd_change(&q, false, false), CwdChangeCheck::NeedsQueueConfirmation { waiting: 2 });
        assert_eq!(check_cwd_change(&q, false, true), CwdChangeCheck::Allowed);
        cancel(&mut q, &lid("q0")).unwrap();
        cancel(&mut q, &lid("q1")).unwrap();
        assert_eq!(check_cwd_change(&q, false, false), CwdChangeCheck::Allowed);
        assert_eq!(affected_by_settings_change(&queue_with(&["a"], 1)), vec![lid("q0")]);
    }

    #[test]
    fn quitting_and_a_pending_manual_turn_hold_the_queue() {
        let q = queue_with(&["a"], 1500);
        let mut i = input();
        i.quitting = true;
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::Quitting));
        i.quitting = false;
        i.manual_turn_pending = true;
        assert_eq!(evaluate(&q, &i), GateDecision::Hold(QueueHold::ParentWorking));
        i.manual_turn_pending = false;
        assert_eq!(evaluate(&q, &i), GateDecision::Send { entry: lid("q0") });
    }

    #[test]
    fn aborted_retry_keeps_the_not_accepted_state_and_kill_stops_instead_of_completing() {
        let mut q = queue_with(&["a"], 1500);
        q.entries[0].state = QueueEntryState::NotAccepted { attempt: lid("old"), message: "m".into() };
        q.entries[0].attempts.push(SendAttempt { state: SendState::Rejected { message: "m".into() }, ..attempt("old") });
        assert!(begin_send(&mut q, &lid("q0"), attempt("new"), applied()));
        abort_retry(&mut q, &lid("q0"), lid("old"), "m".into());
        assert_eq!(q.entries[0].state, QueueEntryState::NotAccepted { attempt: lid("old"), message: "m".into() });
        assert_eq!(q.entries[0].attempts.len(), 1);
        // 強制終了: 終端待ちのturnは完了扱いにせず止める。
        q.awaiting = Some(tk("root", "t1"));
        assert!(stop_after_kill(&mut q, UnixMillis(9)));
        assert!(q.awaiting.is_none());
        assert!(matches!(q.run, QueueRun::Stopped { cause: QueueStopCause::AppServerKilled { .. }, .. }));
        assert!(!stop_after_kill(&mut q, UnixMillis(10)));
    }
}
