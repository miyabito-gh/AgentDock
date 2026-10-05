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
    let _ = works;
    todo!("P6")
}

/// 終了手順の次の段階。`now - started_at >= 10秒` かつ未確認が残れば `StopUnconfirmed`（停止確定ではない、D04）。
/// すべて確認できたら `Flushing`。保存失敗があれば `SaveFailed`。
pub fn next_quit_phase(phase: &QuitPhase, records: &[StopRecord], save_failed: &[SaveScope], now: UnixMillis) -> QuitPhase {
    let _ = (phase, records, save_failed, now);
    todo!("P6")
}

/// sleepからの復帰で、live の鮮度を要照合へ落とす対象（状態は変えない）。
pub fn agents_to_mark_on_resume(views: &[AgentView]) -> Vec<AgentKey> {
    let _ = views;
    todo!("P6")
}

/// 壁時計の飛び（sleep検出の予備）。前回の刻みから `expected_ms` を大きく超えたら復帰とみなす。
pub const WAKE_GAP_THRESHOLD_MS: i64 = 30_000;
pub fn looks_like_resume(prev_tick: UnixMillis, now: UnixMillis, expected_ms: i64) -> bool {
    let _ = (prev_tick, now, expected_ms);
    todo!("P6")
}
