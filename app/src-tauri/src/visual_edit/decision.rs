use super::{FileAccess, Snapshot, VisualError, VisualResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Add,
    Change,
    Delete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDecision {
    pub path: String,
    pub change: Option<ChangeKind>,
    pub conflict: Option<ConflictKind>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    ExternalChange,
    ReadOnlyChanged,
    LatestAlreadyAdopted,
    NotLatest,
    StopUnconfirmed,
    ScopeAuditUnconfirmed,
}

/// A read-only decision. Even an empty conflict list grants no write permission;
/// host stop/operation locks and native per-file replacement are still required.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDecision {
    pub files: Vec<FileDecision>,
    pub blockers: Vec<ConflictKind>,
}
impl ReviewDecision {
    pub fn has_conflicts(&self) -> bool {
        !self.blockers.is_empty() || self.files.iter().any(|f| f.conflict.is_some())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ReviewEvidence {
    pub stopped: bool,
    pub draft_scope_audited: bool,
}

/// Adoption is cumulative: baseline is the last adopted source state, after is
/// the current draft, current is the current original. External changes anywhere
/// in the reviewed set block the whole proposal, including nonoverlapping ones.
pub fn review_adoption(
    baseline: &Snapshot,
    after: &Snapshot,
    current: &Snapshot,
    evidence: ReviewEvidence,
) -> VisualResult<ReviewDecision> {
    review(baseline, after, current, baseline, evidence)
}
/// Latest undo reverses E -> B only inside the draft, and requires current == E.
/// This does not write, consume a plan, or establish durable transaction ordering.
pub fn review_latest_undo(
    before: &Snapshot,
    after: &Snapshot,
    current: &Snapshot,
    is_latest: bool,
    already_adopted: bool,
    evidence: ReviewEvidence,
) -> VisualResult<ReviewDecision> {
    let mut result = review(after, before, current, before, evidence)?;
    if !is_latest {
        result.blockers.push(ConflictKind::NotLatest);
    }
    if already_adopted {
        result.blockers.push(ConflictKind::LatestAlreadyAdopted);
    }
    Ok(result)
}
fn review(
    expected: &Snapshot,
    desired: &Snapshot,
    current: &Snapshot,
    immutable_baseline: &Snapshot,
    evidence: ReviewEvidence,
) -> VisualResult<ReviewDecision> {
    if !expected.same_scope(desired)
        || !expected.same_scope(current)
        || !expected.same_scope(immutable_baseline)
    {
        return Err(VisualError::Conflict("different scopes"));
    }
    let mut blockers = Vec::new();
    if !evidence.stopped {
        blockers.push(ConflictKind::StopUnconfirmed);
    }
    if !evidence.draft_scope_audited {
        blockers.push(ConflictKind::ScopeAuditUnconfirmed);
    }
    let files = expected
        .selected()
        .iter()
        .map(|f| {
            let b = expected.state(&f.path);
            let e = desired.state(&f.path);
            let c = current.state(&f.path);
            let change = if b == e {
                None
            } else {
                Some(match (b, e) {
                    (None, Some(_)) => ChangeKind::Add,
                    (Some(_), None) => ChangeKind::Delete,
                    _ => ChangeKind::Change,
                })
            };
            let conflict = if f.access == FileAccess::ReadOnly
                && (e != immutable_baseline.state(&f.path)
                    || c != immutable_baseline.state(&f.path))
            {
                Some(ConflictKind::ReadOnlyChanged)
            } else if c != b {
                Some(ConflictKind::ExternalChange)
            } else {
                None
            };
            FileDecision {
                path: f.path.clone(),
                change,
                conflict,
            }
        })
        .collect();
    Ok(ReviewDecision { files, blockers })
}
