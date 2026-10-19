//! The user's interventions (DESIGN_ORG_R2 2): interjection into a running turn, notes for later turns, skip,
//! send back, accept as it is, hold. All of them are user operations (`Cause::User`). The engine still never starts
//! a turn by itself here: a steer goes into a turn that is already running, and everything else only changes what the
//! next ordinary send carries or which task it goes to.
use crate::core::{OrgAction, OrgCore, OrgNotice};
use crate::intervene::{accept_as_is_check, is_reviewer, is_worker, send_back_check, skip_plan};
use crate::notes::{pending_note_count, MAX_PENDING_NOTES};
use crate::persist::{InterjectOutcome, OrgEventKind};
use crate::prompt::NOTE_MAX_CHARS;
use crate::state::Cause;
use crate::types::*;

impl OrgCore {
    fn refuse(&mut self, reason: impl Into<String>) {
        self.notify(OrgNotice::Refused { reason: reason.into() });
    }

    /// Text of a user note: trimmed, not empty, within the limit (never cut silently).
    fn checked_text(&mut self, text: &str) -> Option<String> {
        let t = text.trim();
        if t.is_empty() {
            self.refuse("指示の内容を書いてください");
            return None;
        }
        if t.chars().count() > NOTE_MAX_CHARS {
            self.refuse(format!("指示は{NOTE_MAX_CHARS}文字までにしてください"));
            return None;
        }
        Some(t.to_string())
    }

    fn busy(&self, id: &TaskId) -> bool {
        self.launching.contains_key(id) || self.flows.contains_key(id)
    }

    // ---- interjection

    pub(crate) fn user_interject(&mut self, id: &TaskId, text: String, fallback: bool, active_turn: Option<TurnRef>) {
        let Some(text) = self.checked_text(&text) else { return };
        let Some(t) = self.task(id) else { return };
        let accepted = matches!(t.last_settle(), Some(SettleOutcome::Accepted { .. } | SettleOutcome::ReconciledAccepted { .. }));
        let ended = t.observed().is_some_and(|o| Some(&o.turn) == t.accepted_turn());
        let open = self.s().interjects().iter().any(|i| &i.task == id && i.outcome.is_none());
        let turn = match (t.accepted_turn(), active_turn) {
            (Some(a), Some(b)) if *a == b => Some(a.clone()),
            _ => None,
        };
        let ok = t.status() == TaskStatus::Running
            && t.confirm() == ConfirmState::Confirmed
            && accepted
            && !ended
            && !t.adopting()
            && !self.busy(id);
        let (Some(chat), Some(turn)) = (t.chat().cloned(), turn) else {
            self.refuse("実行中のturnを確認できないため、割り込めません");
            return;
        };
        if !ok {
            self.refuse("このタスクは今、実行中のturnに割り込める状態ではありません");
            return;
        }
        if open {
            self.refuse("前の割り込みの結果を待っています");
            return;
        }
        let nid = NoteId::new(format!("int-{}", self.s().last_seq() + 1));
        let client_message_id = format!("{}:{}:{}", self.s().workflow_id(), id, nid);
        if !self.rec(OrgEventKind::InterjectRequested { id: nid.clone(), task: id.clone(), turn: turn.clone(), text: text.clone(), fallback }) {
            return;
        }
        // Events first, then the send (the host appends the event before it steers).
        self.out.push(OrgAction::Steer { task: id.clone(), id: nid, chat, turn, client_message_id, text });
    }

    pub(crate) fn interject_settled(&mut self, id: &NoteId, outcome: InterjectOutcome) {
        let Some(i) = self.s().interject(id).cloned() else { return };
        // After a restart the host no longer remembers that the send was unknown, so a later "accepted" arrives as a
        // plain delivery: when the record says unknown, it is a reconciliation.
        let outcome = match (&i.outcome, outcome) {
            (Some(InterjectOutcome::Unknown), InterjectOutcome::Delivered { .. }) => InterjectOutcome::ReconciledDelivered,
            (_, o) => o,
        };
        if !crate::persist::interject_can_settle(i.outcome.as_ref(), &outcome) {
            return;
        }
        if !self.rec(OrgEventKind::InterjectSettled { id: id.clone(), outcome: outcome.clone() }) {
            return;
        }
        // Only a certain "not delivered" may go to the next send by itself, and only when the user chose it.
        if matches!(outcome, InterjectOutcome::NotDelivered { .. }) && i.fallback && self.add_note(&i.text, NoteScope::Task { task: i.task.clone() }).is_some() {
            self.rec(OrgEventKind::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::FallbackNoted });
        }
    }

    pub(crate) fn user_interject_fallback(&mut self, id: &NoteId, add: bool) {
        let Some(i) = self.s().interject(id).cloned() else {
            self.refuse("その割り込みは見つかりません");
            return;
        };
        if !matches!(i.outcome, Some(InterjectOutcome::Unknown | InterjectOutcome::ReconciledNotFound)) {
            self.refuse("この割り込みは、次の送信に添えるか破棄するかを選べる状態ではありません");
            return;
        }
        if add {
            if pending_note_count(self.s()) >= MAX_PENDING_NOTES {
                self.refuse(format!("届いていない追加指示が{MAX_PENDING_NOTES}件あります。取り消してから追加してください"));
                return;
            }
            if self.add_note(&i.text, NoteScope::Task { task: i.task.clone() }).is_none() {
                return;
            }
            self.rec(OrgEventKind::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::FallbackNoted });
        } else {
            self.rec(OrgEventKind::InterjectSettled { id: id.clone(), outcome: InterjectOutcome::Discarded });
        }
    }

    // ---- notes

    pub(crate) fn user_add_note(&mut self, text: String, task: Option<TaskId>) {
        let Some(text) = self.checked_text(&text) else { return };
        let scope = match task {
            Some(t) => {
                if self.task(&t).is_none() {
                    self.refuse("追加指示の宛先のタスクが見つかりません");
                    return;
                }
                NoteScope::Task { task: t }
            }
            None => NoteScope::AllFollowing,
        };
        if pending_note_count(self.s()) >= MAX_PENDING_NOTES {
            self.refuse(format!("届いていない追加指示が{MAX_PENDING_NOTES}件あります。取り消してから追加してください"));
            return;
        }
        self.add_note(&text, scope);
    }

    pub(crate) fn user_withdraw_note(&mut self, id: &NoteId) {
        if !self.s().notes().iter().any(|n| &n.id == id && !n.withdrawn) {
            self.refuse("取り消せる追加指示が見つかりません");
            return;
        }
        self.rec(OrgEventKind::UserNoteWithdrawn { id: id.clone() });
    }

    // ---- skip / send back / accept / hold

    pub(crate) fn user_skip(&mut self, id: &TaskId) {
        let plan = match skip_plan(self.s(), id) {
            Ok(p) => p,
            Err(e) => {
                self.refuse(e);
                return;
            }
        };
        if let Some(busy) = plan.skip.iter().chain(plan.complete_unreviewed.iter()).find(|t| self.busy(t)) {
            let msg = format!("{busy} は準備中または処理中のため、スキップできません");
            self.refuse(msg);
            return;
        }
        for t in &plan.skip {
            self.forget(t);
            self.rec(OrgEventKind::TaskSkipped { task: t.clone() });
            self.set_status(t, TaskStatus::Cancelled, Cause::User);
        }
        if let Some(w) = plan.complete_unreviewed {
            if self.set_status(&w, TaskStatus::Completed, Cause::User) {
                self.notify(OrgNotice::TaskCompleted { task: w });
            }
        }
    }

    pub(crate) fn user_send_back(&mut self, id: &TaskId, note: Option<String>) {
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        if let Some(n) = &note {
            if n.chars().count() > NOTE_MAX_CHARS {
                self.refuse(format!("指摘は{NOTE_MAX_CHARS}文字までにしてください"));
                return;
            }
        }
        let plan = match send_back_check(self.s(), id, self.turns_remaining(), note.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                self.refuse(e);
                return;
            }
        };
        if self.busy(id) || plan.to_pending.iter().chain(plan.to_rework.iter()).any(|t| self.busy(t)) {
            self.refuse("準備中または処理中のタスクがあるため、差し戻せません");
            return;
        }
        let from_review = self.task(id).is_some_and(|t| t.status() == TaskStatus::Reviewing);
        if let Some(text) = &note {
            if pending_note_count(self.s()) >= MAX_PENDING_NOTES {
                self.refuse(format!("届いていない追加指示が{MAX_PENDING_NOTES}件あります。取り消してから差し戻してください"));
                return;
            }
            if self.add_note(text, NoteScope::Task { task: id.clone() }).is_none() {
                return;
            }
        }
        if !self.rec(OrgEventKind::UserReworkRequested { task: id.clone() }) {
            return;
        }
        if from_review {
            // The earlier review's findings belong to a state that no longer exists; the user's note replaces them.
            self.rec(OrgEventKind::FindingsRecorded { task: id.clone(), findings: Vec::new() });
        }
        self.forget(id);
        if self.set_status(id, TaskStatus::Rework, Cause::User) {
            self.set_status(id, TaskStatus::Ready, Cause::User);
        }
        for r in &plan.to_pending {
            self.set_status(r, TaskStatus::Pending, Cause::User);
        }
        for r in &plan.to_rework {
            self.set_status(r, TaskStatus::Rework, Cause::User);
        }
    }

    pub(crate) fn user_accept_as_is(&mut self, id: &TaskId) {
        let reviewers = match accept_as_is_check(self.s(), id) {
            Ok(r) => r,
            Err(e) => {
                self.refuse(e);
                return;
            }
        };
        if self.busy(id) || reviewers.iter().any(|r| self.busy(r)) {
            self.refuse("準備中または処理中のタスクがあるため、受け入れられません");
            return;
        }
        self.forget(id);
        if !self.set_status(id, TaskStatus::Reviewing, Cause::User) {
            return;
        }
        for r in &reviewers {
            self.rec(OrgEventKind::TaskSkipped { task: r.clone() });
            self.set_status(r, TaskStatus::Cancelled, Cause::User);
        }
        if self.set_status(id, TaskStatus::Completed, Cause::User) {
            self.notify(OrgNotice::TaskCompleted { task: id.clone() });
        }
    }

    pub(crate) fn user_hold(&mut self, id: &TaskId, held: bool) {
        let Some(t) = self.task(id) else { return };
        if !(is_worker(t) || is_reviewer(t)) {
            self.refuse("このタスクは保留にできません（計画の作業・検証タスクだけです）");
            return;
        }
        if t.held() == held {
            return;
        }
        if held && (!matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready) || t.confirm() != ConfirmState::Confirmed || self.busy(id)) {
            self.refuse("まだ送っていないタスクだけ保留にできます");
            return;
        }
        self.rec(OrgEventKind::TaskHeld { task: id.clone(), held });
    }
}
