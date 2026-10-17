//! 組織実行の送信権（DESIGN_ORG_P4 §3.2）。
//!
//! - `OrgGrant` はワークフロー単位の送信権。`Clone`・`Serialize`・`Default` を実装せず、フィールドは非公開。
//!   利用者の「開始」「再開」操作（`UserConfirmed` を値で消費）でだけ作り、`OrgDriver` だけが所有する。
//! - `DispatchPermit`／`CreatePermit` は `OrgGrant` からだけ作れる1回分の許可。`Host::org_send` などが値で消費する。
//! - 失効は epoch で行う。古い `OrgGrant` を持っていても、`GrantRegistry` の現在の epoch と合わなければ許可は出ない。
//! - `Host` が持つのは `GrantRegistry`（失効の管理）だけ。`OrgGrant` 自体は `Host` にも監視・照合のコードにも置かない。

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use org_engine::{TaskId, WorkflowId};

use crate::backend::backend::UserConfirmed;
use crate::backend::model::ChatKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// 送信権が失効した（一時停止・取消・終了・再起動・wake・別の開始）。
    Revoked,
    /// 「手動に切り替える」で組織から外れたチャット。
    ChatReleased,
}

impl GrantError {
    pub fn message(&self) -> &'static str {
        match self {
            GrantError::Revoked => "送信権が失効しています（再開の操作が必要です）",
            GrantError::ChatReleased => "手動に切り替えたチャットのため、組織実行からは送信しません",
        }
    }
}

/// 失効の管理。`Host` が持つのはこれだけ。
#[derive(Default)]
pub struct GrantRegistry {
    epoch: AtomicU64,
    current: Mutex<Option<(WorkflowId, u64)>>,
    released: Mutex<HashSet<ChatKey>>,
}

impl GrantRegistry {
    /// すべての送信権を失効させる。一時停止・取消・完全終了の開始・再起動・wake で呼ぶ。
    pub fn revoke_all(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *self.current.lock().unwrap() = None;
    }

    /// 「手動に切り替える」。以後このチャットには許可が出ない。
    pub fn release_chat(&self, chat: &ChatKey) {
        self.released.lock().unwrap().insert(chat.clone());
    }

    pub fn is_released(&self, chat: &ChatKey) -> bool {
        self.released.lock().unwrap().contains(chat)
    }

    /// 起動時に、保存済みの解放記録（`chat.json.org.released_at`）を反映する。
    pub fn restore_released(&self, chats: impl IntoIterator<Item = ChatKey>) {
        self.released.lock().unwrap().extend(chats);
    }

    /// そのワークフローの送信権が今あるか（画面の「再開が要るか」の表示用。読むだけ）。
    pub fn is_active(&self, workflow: &WorkflowId) -> bool {
        self.current.lock().unwrap().as_ref().is_some_and(|(w, _)| w == workflow)
    }

    fn is_current(&self, workflow: &WorkflowId, epoch: u64) -> bool {
        self.current.lock().unwrap().as_ref().is_some_and(|(w, e)| w == workflow && *e == epoch)
    }

    /// 新しい送信権の epoch を確定する（以前の送信権はここで失効する）。
    fn begin(&self, workflow: WorkflowId) -> u64 {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        *self.current.lock().unwrap() = Some((workflow, epoch));
        epoch
    }
}

/// ワークフロー単位の送信権。
pub struct OrgGrant {
    workflow: WorkflowId,
    epoch: u64,
}

impl OrgGrant {
    /// `UserConfirmed` を値で消費する（参照からは作れない）。利用者の開始・再開の操作だけが呼ぶ。
    pub(in crate::host) fn issue(_confirmed: UserConfirmed, workflow: WorkflowId, reg: &GrantRegistry) -> Self {
        let epoch = reg.begin(workflow.clone());
        OrgGrant { workflow, epoch }
    }

    #[cfg(test)]
    pub(in crate::host) fn issue_for_test(workflow: WorkflowId, reg: &GrantRegistry) -> Self {
        let epoch = reg.begin(workflow.clone());
        OrgGrant { workflow, epoch }
    }

    pub fn workflow(&self) -> &WorkflowId {
        &self.workflow
    }

    fn check(&self, reg: &GrantRegistry) -> Result<(), GrantError> {
        if reg.is_current(&self.workflow, self.epoch) {
            Ok(())
        } else {
            Err(GrantError::Revoked)
        }
    }

    /// 1回分の送信許可。失効（epoch 不一致）・手動に切り替えたチャットなら Err。
    pub(in crate::host::org) fn permit(&self, reg: &GrantRegistry, task: &TaskId, chat: &ChatKey) -> Result<DispatchPermit<'_>, GrantError> {
        self.check(reg)?;
        if reg.is_released(chat) {
            return Err(GrantError::ChatReleased);
        }
        Ok(DispatchPermit { _grant: self, task: task.clone(), chat: chat.clone() })
    }

    /// 役割チャットの作成許可（チャットはまだ無い）。
    pub(in crate::host::org) fn create_permit(&self, reg: &GrantRegistry, task: &TaskId) -> Result<CreatePermit<'_>, GrantError> {
        self.check(reg)?;
        Ok(CreatePermit { _grant: self, task: task.clone() })
    }
}

/// 1回の送信に1つ。`Clone` しない。値で消費される。
pub struct DispatchPermit<'g> {
    _grant: &'g OrgGrant,
    task: TaskId,
    chat: ChatKey,
}

impl DispatchPermit<'_> {
    pub fn task(&self) -> &TaskId {
        &self.task
    }
    pub fn chat(&self) -> &ChatKey {
        &self.chat
    }
    /// 送る直前の再確認（許可を得てから送るまでの間に失効していないか）。
    pub fn still_valid(&self, reg: &GrantRegistry) -> Result<(), GrantError> {
        self._grant.check(reg)?;
        if reg.is_released(&self.chat) {
            return Err(GrantError::ChatReleased);
        }
        Ok(())
    }
}

/// 役割チャットの作成に1つ。`Clone` しない。
pub struct CreatePermit<'g> {
    _grant: &'g OrgGrant,
    task: TaskId,
}

impl CreatePermit<'_> {
    pub fn task(&self) -> &TaskId {
        &self.task
    }
    pub fn workflow(&self) -> &WorkflowId {
        &self._grant.workflow
    }
    pub fn still_valid(&self, reg: &GrantRegistry) -> Result<(), GrantError> {
        self._grant.check(reg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{BackendKind, ExternalId};

    fn chat(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    #[test]
    fn permit_is_issued_only_while_the_grant_is_current() {
        let reg = GrantRegistry::default();
        let g = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        let p = g.permit(&reg, &TaskId::new("t1"), &chat("a")).unwrap();
        assert!(p.still_valid(&reg).is_ok());
        reg.revoke_all();
        assert_eq!(p.still_valid(&reg), Err(GrantError::Revoked), "an already issued permit is re-checked just before sending");
        assert!(g.permit(&reg, &TaskId::new("t1"), &chat("a")).is_err());
        assert!(g.create_permit(&reg, &TaskId::new("t1")).is_err());
    }

    #[test]
    fn a_newer_grant_revokes_the_older_one() {
        let reg = GrantRegistry::default();
        let old = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        let new = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        assert_eq!(old.permit(&reg, &TaskId::new("t"), &chat("a")).err(), Some(GrantError::Revoked));
        assert!(new.permit(&reg, &TaskId::new("t"), &chat("a")).is_ok());
    }

    #[test]
    fn a_released_chat_never_gets_a_permit() {
        let reg = GrantRegistry::default();
        let g = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        reg.release_chat(&chat("a"));
        assert_eq!(g.permit(&reg, &TaskId::new("t"), &chat("a")).err(), Some(GrantError::ChatReleased));
        assert!(g.permit(&reg, &TaskId::new("t"), &chat("b")).is_ok());
        let p = g.permit(&reg, &TaskId::new("t"), &chat("b")).unwrap();
        reg.release_chat(&chat("b"));
        assert_eq!(p.still_valid(&reg), Err(GrantError::ChatReleased));
    }

    #[test]
    fn restored_release_records_apply_before_any_grant_exists() {
        let reg = GrantRegistry::default();
        reg.restore_released([chat("a")]);
        let g = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        assert!(g.permit(&reg, &TaskId::new("t"), &chat("a")).is_err());
    }
}
