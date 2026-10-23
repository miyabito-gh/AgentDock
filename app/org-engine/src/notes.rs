//! Which user notes ride along with a task's next turn (DESIGN_ORG_R2 2.2). Pure.
use crate::persist::{UserNote, WorkflowState};
use crate::types::*;

/// Notes unseen by the task and not yet delivered: tasks of the plan route read `notes_seen_seq`, graph nodes also
/// stop at their latest output. A note is not added to the same task twice, and a withdrawn note is never added.
pub const MAX_PENDING_NOTES: usize = 50;

fn applies(n: &UserNote, t: &Task) -> bool {
    match &n.scope {
        NoteScope::AllFollowing => true,
        NoteScope::Task { task } => task == &t.id,
        // A graph node, or (plan route) a re-plan Planner whose note is keyed by its task id.
        NoteScope::Node { node } => t.node.as_ref() == Some(node) || node.0 == t.id.0,
    }
}

/// The notes to add to the next turn of `task`, oldest first, and the newest seq among them (None = no note).
pub fn notes_for<'a>(state: &'a WorkflowState, task: &TaskId) -> (Vec<&'a UserNote>, Option<u64>) {
    let Some(t) = state.task(task) else { return (Vec::new(), None) };
    let floor = t.notes_seen_seq().max(t.last_output_seq().unwrap_or(0));
    let notes: Vec<&UserNote> = state.notes().iter().filter(|n| !n.withdrawn && n.seq > floor && applies(n, t)).collect();
    let upto = notes.iter().map(|n| n.seq).max();
    (notes, upto)
}

fn finished(t: &Task) -> bool {
    matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)
}

/// Whether a note can still reach a task that has not finished (the ones that count against `MAX_PENDING_NOTES`).
pub fn pending_note_count(state: &WorkflowState) -> usize {
    state
        .notes()
        .iter()
        .filter(|n| {
            !n.withdrawn
                && state.tasks().iter().any(|t| {
                    !finished(t) && applies(n, t) && n.seq > t.notes_seen_seq().max(t.last_output_seq().unwrap_or(0))
                })
        })
        .count()
}

/// Per note: the unfinished tasks it has not reached yet (`pending`) and those it did reach (`delivered`).
pub fn note_reach(state: &WorkflowState, note: &UserNote) -> (Vec<TaskId>, Vec<TaskId>) {
    let (mut pending, mut delivered) = (Vec::new(), Vec::new());
    for t in state.tasks() {
        if !applies(note, t) {
            continue;
        }
        if note.seq <= t.notes_seen_seq().max(t.last_output_seq().unwrap_or(0)) {
            delivered.push(t.id.clone());
        } else if !finished(t) {
            pending.push(t.id.clone());
        }
    }
    (pending, delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::OrgEventKind;
    use crate::types::Limits;

    fn st() -> WorkflowState {
        let mut s = WorkflowState::new(WorkflowId::new("w"), Limits::default());
        for id in ["a", "b"] {
            s.record(
                OrgEventKind::TaskAdded {
                    task: PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new(), kind: TaskKind::Change },
                },
                0,
            )
            .unwrap();
        }
        s
    }
    fn note(s: &mut WorkflowState, id: &str, scope: NoteScope) {
        s.record(OrgEventKind::UserNoteAdded { id: NoteId::new(id), text: id.into(), scope }, 0).unwrap();
    }
    fn ids(v: &[&UserNote]) -> Vec<String> {
        v.iter().map(|n| n.id.0.clone()).collect()
    }

    #[test]
    fn scope_and_withdraw() {
        let mut s = st();
        note(&mut s, "all", NoteScope::AllFollowing);
        note(&mut s, "ta", NoteScope::Task { task: TaskId::new("a") });
        note(&mut s, "nb", NoteScope::Node { node: crate::graph::def::NodeId::new("b") });
        assert_eq!(ids(&notes_for(&s, &TaskId::new("a")).0), ["all", "ta"]);
        assert_eq!(ids(&notes_for(&s, &TaskId::new("b")).0), ["all", "nb"]);
        s.record(OrgEventKind::UserNoteWithdrawn { id: NoteId::new("ta") }, 0).unwrap();
        let (n, upto) = notes_for(&s, &TaskId::new("a"));
        assert_eq!(ids(&n), ["all"]);
        assert_eq!(upto, Some(n[0].seq));
        assert_eq!(notes_for(&s, &TaskId::new("zz")), (vec![], None));
    }

    #[test]
    fn seen_threshold_moves_only_on_accepted_send() {
        let mut s = st();
        let a = TaskId::new("a");
        note(&mut s, "n1", NoteScope::AllFollowing);
        let (_, upto) = notes_for(&s, &a);
        let ts = |s: &mut WorkflowState, k| s.record(k, 0).unwrap();
        ts(&mut s, OrgEventKind::StatusChanged { task: a.clone(), to: TaskStatus::Ready, cause: crate::state::Cause::Engine });
        ts(&mut s, OrgEventKind::ChatRequested { task: a.clone(), cwd: "c".into() });
        ts(&mut s, OrgEventKind::ChatBound { task: a.clone(), chat: ChatRef::new("c") });
        ts(&mut s, OrgEventKind::Dispatched { task: a.clone(), attempt: AttemptRef::new("x1"), client_message_id: "m".into(), kind: crate::TurnKind::Task, notes_upto: upto });
        ts(&mut s, OrgEventKind::DispatchSettled { task: a.clone(), attempt: AttemptRef::new("x1"), outcome: SettleOutcome::Rejected });
        // rejected: the note is still due
        assert_eq!(notes_for(&s, &a).0.len(), 1);
        ts(&mut s, OrgEventKind::Dispatched { task: a.clone(), attempt: AttemptRef::new("x2"), client_message_id: "m".into(), kind: crate::TurnKind::Task, notes_upto: upto });
        ts(&mut s, OrgEventKind::DispatchSettled { task: a.clone(), attempt: AttemptRef::new("x2"), outcome: SettleOutcome::Accepted { turn: TurnRef::new("t") } });
        assert!(notes_for(&s, &a).0.is_empty());
        // a later note is due again, and the other task still has the first one
        note(&mut s, "n2", NoteScope::AllFollowing);
        assert_eq!(ids(&notes_for(&s, &a).0), ["n2"]);
        assert_eq!(ids(&notes_for(&s, &TaskId::new("b")).0), ["n1", "n2"]);
        assert_eq!(pending_note_count(&s), 2);
    }
}
