//! 進行タイムラインの保持（DESIGN_ORG_R2 1.1）。実行中のワークフローの記録を `TimelineFold` で畳み込み、最新の行だけを持つ。
//! 追記に成功した記録だけが入る（driver が `persisted` で渡す）。履歴（読取り専用）のワークフローは開いたときに読んで畳み込み、保持しない。

use std::collections::VecDeque;

use org_engine::{OrgEvent, TaskId, WorkflowId};

use crate::backend::model::UnixMillis;
use crate::backend::org::{OrgTimelineEntry, OrgTimelineView};
use crate::rules::org_timeline::{FoldChange, TimelineFold};

/// 保持する行数（新しいものを残す）。
pub const KEEP: usize = 500;

pub struct TimelineBuf {
    pub workflow: WorkflowId,
    fold: TimelineFold,
    entries: VecDeque<OrgTimelineEntry>,
    last_seq: u64,
    /// 保持の上限で、古い行を落としたことがある。
    dropped: bool,
}

impl TimelineBuf {
    pub fn new(workflow: WorkflowId) -> Self {
        Self { workflow, fold: TimelineFold::new(), entries: VecDeque::new(), last_seq: 0, dropped: false }
    }

    /// 記録を1件入れる。同じか古い番号は無視する（二重に入れない）。
    pub fn push(&mut self, ev: &OrgEvent) {
        if ev.seq != 0 && ev.seq <= self.last_seq {
            return;
        }
        self.last_seq = self.last_seq.max(ev.seq);
        match self.fold.push(ev) {
            FoldChange::None => {}
            FoldChange::Add(e) => {
                self.entries.push_back(e);
                while self.entries.len() > KEEP {
                    self.entries.pop_front();
                    self.dropped = true;
                }
            }
            FoldChange::ReplaceLast(e) => {
                match self.entries.back_mut() {
                    Some(last) => *last = e,
                    None => self.entries.push_back(e),
                }
            }
        }
    }

    pub fn since_of(&self, task: &TaskId) -> Option<UnixMillis> {
        self.fold.since_of(task)
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// `after_seq` より後に作られた・変わった行（なければ新しい側の `limit` 件）。古い順。
    pub fn view(&self, after_seq: Option<u64>, limit: usize) -> OrgTimelineView {
        view_of(self.entries.iter().cloned().collect(), self.last_seq, self.dropped, after_seq, limit)
    }
}

fn view_of(all: Vec<OrgTimelineEntry>, last_seq: u64, dropped: bool, after_seq: Option<u64>, limit: usize) -> OrgTimelineView {
    let mut rows: Vec<OrgTimelineEntry> = match after_seq {
        Some(a) => all.into_iter().filter(|e| e.seq_to > a).collect(),
        None => all,
    };
    let mut truncated = dropped && after_seq.is_none();
    if rows.len() > limit {
        rows.drain(..rows.len() - limit);
        truncated = true;
    }
    OrgTimelineView { entries: rows, last_seq, truncated }
}

/// 保存された記録の全体から作る（過去のワークフローを開いたとき。壊れた末尾行は読む側が無視済み）。
pub fn view_from_events(events: &[OrgEvent], after_seq: Option<u64>, limit: usize) -> (OrgTimelineView, TimelineFold) {
    let mut buf = TimelineBuf::new(WorkflowId::new(""));
    for e in events {
        buf.push(e);
    }
    let view = buf.view(after_seq, limit);
    (view, buf.fold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{Cause, OrgEventKind, PlanTask, RoleId, TaskStatus};

    fn ev(seq: u64, kind: OrgEventKind) -> OrgEvent {
        OrgEvent { schema_version: 2, seq, at_ms: 1000 + seq as i64, kind }
    }
    fn added(seq: u64, id: &str) -> OrgEvent {
        ev(seq, OrgEventKind::TaskAdded { task: PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() } })
    }

    #[test]
    fn a_folded_row_is_replaced_in_place_and_the_difference_is_by_seq_to() {
        let mut b = TimelineBuf::new(WorkflowId::new("w"));
        b.push(&added(1, "a"));
        let v = b.view(None, 100);
        assert_eq!(v.entries.len(), 1);
        b.push(&added(2, "b"));
        b.push(&added(3, "c"));
        let all = b.view(None, 100);
        assert_eq!(all.entries.len(), 1, "one row for the run");
        assert_eq!((all.entries[0].seq, all.entries[0].seq_to, all.entries[0].count), (1, 3, 3));
        // a client that has seen up to 1 gets the changed row again (same seq key, newer content)
        let diff = b.view(Some(1), 100);
        assert_eq!(diff.entries.len(), 1);
        assert_eq!(diff.entries[0].count, 3);
        assert!(b.view(Some(3), 100).entries.is_empty());
        assert_eq!(all.last_seq, 3);
    }

    #[test]
    fn the_buffer_keeps_the_newest_rows_and_reports_the_cut() {
        let mut b = TimelineBuf::new(WorkflowId::new("w"));
        for i in 0..(KEEP as u64 + 20) {
            // alternate so no run forms
            b.push(&ev(i * 2 + 1, OrgEventKind::Halted { reason: Some(org_engine::HaltReason::User) }));
            b.push(&ev(i * 2 + 2, OrgEventKind::Halted { reason: None }));
        }
        let v = b.view(None, 1000);
        assert_eq!(v.entries.len(), KEEP);
        assert!(v.truncated);
        assert_eq!(b.view(None, 10).entries.len(), 10);
        // the same record twice is not doubled
        let n = b.entries.len();
        b.push(&ev(1, OrgEventKind::Halted { reason: None }));
        assert_eq!(b.entries.len(), n);
    }

    #[test]
    fn rebuilding_from_the_log_gives_the_same_since() {
        let evs = vec![added(1, "a"), ev(2, OrgEventKind::StatusChanged { task: TaskId::new("a"), to: TaskStatus::Ready, cause: Cause::Engine }), ev(3, OrgEventKind::StatusChanged { task: TaskId::new("a"), to: TaskStatus::Running, cause: Cause::Engine })];
        let (_, fold) = view_from_events(&evs, None, 10);
        assert_eq!(fold.since_of(&TaskId::new("a")), Some(UnixMillis(1003)));
    }
}
