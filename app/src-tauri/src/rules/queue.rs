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
    /// 観測した最新turnとその終端（未観測は None）。
    pub latest_turn: Option<ExternalId>,
    pub latest_end: Option<(TurnEnd, UnixMillis)>,
    /// 状態の根拠が終端の明示か（history由来の done 等。推定は false）。
    pub terminal_explicit: bool,
    /// 状態を観測した時刻。
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateDecision {
    /// 送る項目がない（Waitingなし、または先頭がNotAccepted等でユーザー判断待ち）。
    Nothing,
    /// キューが止まっている（Stopped・PausedAfterRestart）。明示の再開を待つ。
    Paused,
    /// 先頭の Waiting を送ってよい。
    Send { entry: LocalId },
    Hold(QueueHold),
    /// 失敗・中断を検出したので止める（[`stop`] を呼ぶ）。
    Stop(QueueStopCause),
}

/// 自動送信の可否を判断する。判断の順序:
/// 1. `run` が Active でなければ `Paused`。送る項目がなければ `Nothing`。
/// 2. 削除保留・切断・停止未確認・受理不明 → `Hold`。
/// 3. 親・子孫の `baseline_at` 以降の failed / interrupted → `Stop`。
/// 4. 親が作業中 → `Hold(ParentWorking)`。子孫に running / waiting / initializing → `Hold(DescendantsActive)`。
/// 5. 親の最新turnが Completed と明示されていない・子孫に unknown・走査未完了 → `Hold(StateUnknown)`。
/// 6. 鮮度: 親は live 必須。子孫は live、または終端を明示的に確認済み（`terminal_explicit`）かつ
///    `observed_at >= baseline_at` の履歴取得済みなら可。それ以外 → `Hold(NotLive)`（DESIGN_P2 §2.3）。
/// 7. すべて満たせば先頭の Waiting を `Send`。
pub fn evaluate(queue: &ChatQueue, input: &GateInput) -> GateDecision {
    let _ = (queue, input);
    todo!("P3")
}

/// 登録（ユーザー操作）。`order` は `next_order` を使って単調増加させる。設定はここで固定しない。
pub fn enqueue(queue: &mut ChatQueue, id: LocalId, text: String, attachments: Vec<LocalId>, now: UnixMillis) -> QueueEntry {
    let _ = (queue, id, text, attachments, now);
    todo!("P3")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    NotFound,
    /// Waiting 以外は編集・取消できない（送信中・受理不明・送信済み）。
    NotEditable,
}

pub fn edit(queue: &mut ChatQueue, id: &LocalId, text: String, attachments: Vec<LocalId>) -> Result<(), EditError> {
    let _ = (queue, id, text, attachments);
    todo!("P3")
}

/// 取消。Waiting と NotAccepted だけ取り消せる。
pub fn cancel(queue: &mut ChatQueue, id: &LocalId) -> Result<(), EditError> {
    let _ = (queue, id);
    todo!("P3")
}

/// 送信開始。先頭のWaitingを `Sending{attempt}` にし、送信時点の設定を記録する。
pub fn begin_send(queue: &mut ChatQueue, id: &LocalId, attempt: LocalId, applied: AppliedSettings) {
    let _ = (queue, id, attempt, applied);
    todo!("P3")
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

/// 結果を反映する。Accepted は `Sent` にして `baseline_at` を新しいturnの開始へ進める。
/// Rejected・ReconciledNotFound は項目を `NotAccepted` にし、キューを `Stopped` にする（自動再送しない）。
/// Unknown は `AcceptanceUnknown`（評価で `Hold(AcceptanceUnknown)` になる）。
pub fn apply_send_result(queue: &mut ChatQueue, entry: &LocalId, result: SendResult, now: UnixMillis) {
    let _ = (queue, entry, result, now);
    todo!("P3")
}

pub fn stop(queue: &mut ChatQueue, cause: QueueStopCause, now: UnixMillis) {
    let _ = (queue, cause, now);
    todo!("P3")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeError {
    AlreadyActive,
    /// 受理不明の項目がある（照合で確定するまで再開できない）。
    AcceptanceUnknown,
}

/// 明示的な「キューを再開」（ユーザー操作）。`Active` に戻し、`baseline_at` を `now` にする。
/// NotAccepted の項目は送らない（ユーザーが再送か取消を選ぶまで先頭で止まる）。「確認済み」はこれを呼ばない。
pub fn resume(queue: &mut ChatQueue, now: UnixMillis) -> Result<(), ResumeError> {
    let _ = (queue, now);
    todo!("P3")
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

/// モデル・権限の変更が及ぶ送信待ちの項目（UIの影響表示用）。
pub fn affected_by_settings_change(queue: &ChatQueue) -> Vec<LocalId> {
    let _ = queue;
    todo!("P3")
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
    let _ = (queue, busy, queue_retarget_confirmed);
    todo!("P3")
}
