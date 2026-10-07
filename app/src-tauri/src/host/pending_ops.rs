//! 結果が未確認の操作（`PendingOp`）の記録（段階③ DESIGN_P3 §0.2）。
//!
//! turnを開始する操作（レビュー・圧縮）は、送る前に記録を保存してから送る（異常終了後も受理不明として残る）。
//! 未解決の間は、同じ操作の重複実行と、そのチャットのキュー自動送信を止める（`QueueHold::OperationUnconfirmed`）。
//! 解消できるのは操作ごとの読取り専用の照合だけ。受理不明を再送で解消しない。
//! 各操作（P3-2など）はこの2つの入口だけを使う。

use std::sync::Arc;
use std::time::Duration;

use super::{blocked, err, now_ms, Host};
use crate::backend::ipc::{BlockedReason, HostEvent, IpcError, IpcErrorCode};
use crate::backend::local::SaveScope;
use crate::backend::model::*;
use crate::store::records::ChatLocalFile;

/// 同じ操作の未解決の記録があるか（あればその記録のID）。
pub fn unresolved_same_op(pending: &[PendingOp], op: ParityOp) -> Option<&PendingOp> {
    pending.iter().find(|p| p.op == op)
}

impl Host {
    /// 操作を送る前に記録を保存する。保存に失敗したら記録を戻して `Err`（操作は送らない）。
    /// 同じ操作が未解決なら `Blocked{OperationUnconfirmed}`（重複実行しない）。
    pub(super) async fn begin_pending_op(self: &Arc<Self>, chat: &ChatKey, op: ParityOp) -> Result<PendingOp, IpcError> {
        let dir_id = self.persist.dir_id_for(chat);
        let pending = PendingOp { id: self.local_id("op"), op, since: now_ms() };
        let existing = self.mutate(|d| {
            let file = d.locals.entry(chat.clone()).or_insert_with(|| ChatLocalFile::new(dir_id, Some(chat.clone())));
            if let Some(p) = unresolved_same_op(&file.pending_ops, op) {
                return (Some(p.id.clone()), vec![]);
            }
            file.pending_ops.push(pending.clone());
            (None, vec![HostEvent::PendingOpsUpdated { chat: chat.clone(), pending_ops: file.pending_ops.clone() }])
        });
        if let Some(id) = existing {
            return Err(blocked(BlockedReason::OperationUnconfirmed { op: id }, "この操作は結果が未確認のため、続けて実行できません（状態を確認してください）"));
        }
        if let Some(st) = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await {
            if !matches!(st.state, SaveState::Saved { .. }) {
                self.remove_pending_op(chat, &pending.id);
                return Err(err(IpcErrorCode::Io, "実行前の保存に失敗したため、操作は送っていません"));
            }
        }
        Ok(pending)
    }

    /// 照合（読取り）で結果が確定したとき、または送る前に失敗が確定したときだけ呼ぶ。
    pub(super) fn resolve_pending_op(self: &Arc<Self>, chat: &ChatKey, id: &LocalId) {
        if self.remove_pending_op(chat, id) {
            self.schedule_save(SaveScope::ChatLocal { chat: chat.clone() }, Duration::ZERO);
            self.kick_queue();
        }
    }

    fn remove_pending_op(&self, chat: &ChatKey, id: &LocalId) -> bool {
        self.mutate(|d| {
            let Some(file) = d.locals.get_mut(chat) else { return (false, vec![]) };
            let n = file.pending_ops.len();
            file.pending_ops.retain(|p| &p.id != id);
            let changed = file.pending_ops.len() != n;
            let ev = if changed { vec![HostEvent::PendingOpsUpdated { chat: chat.clone(), pending_ops: file.pending_ops.clone() }] } else { vec![] };
            (changed, ev)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_op_is_found_but_other_ops_are_not() {
        let list = vec![PendingOp { id: LocalId("a".into()), op: ParityOp::Compact, since: UnixMillis(1) }];
        assert_eq!(unresolved_same_op(&list, ParityOp::Compact).map(|p| p.id.0.as_str()), Some("a"));
        assert!(unresolved_same_op(&list, ParityOp::CodeReview).is_none());
    }
}
