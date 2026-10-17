//! 停滞の検出（DESIGN_ORG §4.7・Q10）。純粋な判断で、時刻と活動の記録を受けて「通知してよいタスク」を返すだけ。
//!
//! - 対象は、確認状態が確定していて `running` のままのタスク。設定時間（既定30分）、そのチャットに新しい出力（活動）がなければ1回だけ知らせる。
//! - 自動で中断・再送・resume しない（通知だけ）。新しい活動があれば次の通知の権利が戻る。
//! - 承認・質問の回答待ちの間は静かでも停滞にしない（待っているのは利用者）。回答後に時計を数え直す。
//! - 時計が大きく飛んだ（sleep）ときは、飛んだ時間を静かな時間に数えず、基準を取り直す。

use std::collections::{HashMap, HashSet};

use org_engine::{ChatRef, TaskId};

/// 既定の停滞時間（30分）。
pub const DEFAULT_STALL_MS: i64 = 30 * 60 * 1000;
/// 検査の間隔がこれ以上空いたら、時計が飛んだ（sleep等）とみなす。
const CLOCK_JUMP_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stalled {
    pub task: TaskId,
    /// 新しい活動がなかった時間。
    pub quiet_ms: i64,
}

pub struct StallWatch {
    threshold_ms: i64,
    /// チャットごとの最後の活動の時刻。
    activity: HashMap<ChatRef, i64>,
    /// タスクが監視対象になった（または回答待ちが終わった）時刻。活動の記録がないときの基準。
    since: HashMap<TaskId, i64>,
    /// 通知済みの基準時刻（同じ静かな期間では1回だけ）。
    notified: HashMap<TaskId, i64>,
    open_requests: HashSet<ChatRef>,
    /// これより前の活動・基準は数えない（時計の飛び）。
    floor: i64,
    last_check: Option<i64>,
}

impl StallWatch {
    pub fn new(threshold_ms: i64) -> Self {
        Self { threshold_ms, activity: HashMap::new(), since: HashMap::new(), notified: HashMap::new(), open_requests: HashSet::new(), floor: i64::MIN, last_check: None }
    }

    pub fn threshold_ms(&self) -> i64 {
        self.threshold_ms
    }

    pub fn set_threshold_ms(&mut self, ms: i64) {
        self.threshold_ms = ms.max(1);
    }

    pub fn note_activity(&mut self, chat: &ChatRef, at_ms: i64) {
        let e = self.activity.entry(chat.clone()).or_insert(at_ms);
        *e = (*e).max(at_ms);
    }

    pub fn note_requests(&mut self, chat: &ChatRef, open: u32) {
        if open > 0 {
            self.open_requests.insert(chat.clone());
        } else {
            self.open_requests.remove(chat);
        }
    }

    /// 検査する。`running` は、確認状態が確定していて `running` のタスクと、そのチャット。
    pub fn check(&mut self, now_ms: i64, running: &[(TaskId, Option<ChatRef>)]) -> Vec<Stalled> {
        if self.last_check.is_some_and(|last| now_ms - last >= CLOCK_JUMP_MS) {
            self.floor = now_ms;
            self.since.clear();
            self.notified.clear();
        }
        self.last_check = Some(now_ms);
        let live: HashSet<&TaskId> = running.iter().map(|(t, _)| t).collect();
        self.since.retain(|t, _| live.contains(t));
        self.notified.retain(|t, _| live.contains(t));
        let mut out = Vec::new();
        for (task, chat) in running {
            if chat.as_ref().is_some_and(|c| self.open_requests.contains(c)) {
                // 利用者の回答待ち。数え直す。
                self.since.insert(task.clone(), now_ms);
                self.notified.remove(task);
                continue;
            }
            let since = *self.since.entry(task.clone()).or_insert(now_ms);
            let active = chat.as_ref().and_then(|c| self.activity.get(c)).copied().unwrap_or(i64::MIN);
            let baseline = since.max(active).max(self.floor);
            if now_ms - baseline >= self.threshold_ms && self.notified.get(task) != Some(&baseline) {
                self.notified.insert(task.clone(), baseline);
                out.push(Stalled { task: task.clone(), quiet_ms: now_ms - baseline });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn run(id: &str) -> Vec<(TaskId, Option<ChatRef>)> {
        vec![(TaskId::new(id), Some(ChatRef::new(format!("codex:{id}"))))]
    }

    /// 1分ごとに検査する（時計の飛びとみなされない間隔）。
    fn step(w: &mut StallWatch, from: i64, to: i64, running: &[(TaskId, Option<ChatRef>)]) -> Vec<Stalled> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to {
            out.extend(w.check(t, running));
            t += MIN / 2;
        }
        out
    }

    #[test]
    fn a_quiet_running_task_is_reported_once_after_the_threshold() {
        let mut w = StallWatch::new(30 * MIN);
        let r = run("a");
        assert!(step(&mut w, 0, 29 * MIN, &r).is_empty());
        let got = step(&mut w, 30 * MIN, 90 * MIN, &r);
        assert_eq!(got.len(), 1, "one notice per quiet period");
        assert_eq!(got[0].task, TaskId::new("a"));
        assert!(got[0].quiet_ms >= 30 * MIN);
    }

    #[test]
    fn new_activity_restarts_the_count_and_allows_another_notice() {
        let mut w = StallWatch::new(30 * MIN);
        let r = run("a");
        assert_eq!(step(&mut w, 0, 31 * MIN, &r).len(), 1);
        w.note_activity(&ChatRef::new("codex:a"), 40 * MIN);
        assert!(step(&mut w, 31 * MIN, 69 * MIN, &r).is_empty());
        assert_eq!(step(&mut w, 70 * MIN, 75 * MIN, &r).len(), 1);
    }

    #[test]
    fn waiting_for_the_user_is_not_a_stall_and_the_count_restarts_after_the_answer() {
        let mut w = StallWatch::new(30 * MIN);
        let r = run("a");
        let chat = ChatRef::new("codex:a");
        w.note_requests(&chat, 1);
        assert!(step(&mut w, 0, 120 * MIN, &r).is_empty());
        w.note_requests(&chat, 0);
        assert!(step(&mut w, 120 * MIN, 149 * MIN, &r).is_empty());
        assert_eq!(step(&mut w, 150 * MIN, 151 * MIN, &r).len(), 1);
    }

    #[test]
    fn leaving_running_forgets_the_task_and_a_clock_jump_is_not_quiet_time() {
        let mut w = StallWatch::new(30 * MIN);
        let r = run("a");
        step(&mut w, 0, 20 * MIN, &r);
        // not running (needs reconcile etc.): nothing is reported and the count is dropped
        assert!(step(&mut w, 20 * MIN, 25 * MIN, &[]).is_empty());
        assert!(step(&mut w, 25 * MIN, 54 * MIN, &r).is_empty());
        // the machine slept for hours: no notice right after waking
        assert!(w.check(10_000 * MIN, &r).is_empty());
        assert!(step(&mut w, 10_000 * MIN, 10_029 * MIN, &r).is_empty());
        assert_eq!(step(&mut w, 10_030 * MIN, 10_031 * MIN, &r).len(), 1);
    }
}
