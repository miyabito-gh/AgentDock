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

use crate::backend::backend::{SendMode, UserConfirmed};
use crate::backend::model::{ChatKey, TurnKey};

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

    /// ワークフローの削除後に、解放表から外す（そのチャットはもう存在しない）。
    pub fn forget_chat(&self, chat: &ChatKey) {
        self.released.lock().unwrap().remove(chat);
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

/// 実行中の turn への割り込み（`turn/steer`）に1つ。`Clone` しない。値で消費される。
/// 新しい turn を作らないので、ワークフローの送信権（`OrgGrant`）は要らない。宛先のチャットと turn は作るときに固定され、
/// `SendMode::Steer` で宛先とturnが一致する送信だけを通す（新しい turn の開始は型で拒否）。
pub struct InterjectPermit {
    chat: ChatKey,
    turn: TurnKey,
}

impl InterjectPermit {
    /// 司令task（`driver.rs`）だけが作る。手動に切り替えたチャットには出さない。
    pub(in crate::host::org) fn issue(reg: &GrantRegistry, chat: ChatKey, turn: TurnKey) -> Result<Self, GrantError> {
        if reg.is_released(&chat) {
            return Err(GrantError::ChatReleased);
        }
        Ok(Self { chat, turn })
    }

    pub fn chat(&self) -> &ChatKey {
        &self.chat
    }
    pub fn turn(&self) -> &TurnKey {
        &self.turn
    }
    /// 送る直前の再確認（手動に切り替えられていないか）。
    pub fn still_valid(&self, reg: &GrantRegistry) -> Result<(), GrantError> {
        if reg.is_released(&self.chat) {
            return Err(GrantError::ChatReleased);
        }
        Ok(())
    }
    /// この許可で送ってよい要求か: 宛先のチャットが同じで、`Steer` の turn が許可の turn と一致するものだけ。
    pub fn allows(&self, chat: &ChatKey, mode: &SendMode) -> bool {
        &self.chat == chat && matches!(mode, SendMode::Steer { turn } if turn == &self.turn)
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

    fn turn(agent: &str, id: &str) -> TurnKey {
        TurnKey { agent: crate::backend::model::AgentKey { backend: BackendKind::Codex, id: ExternalId(agent.into()) }, turn_id: ExternalId(id.into()) }
    }

    #[test]
    fn an_interject_permit_passes_only_a_steer_into_its_own_turn_and_never_a_new_turn() {
        let reg = GrantRegistry::default();
        let p = InterjectPermit::issue(&reg, chat("a"), turn("a", "t1")).unwrap();
        assert!(p.allows(&chat("a"), &SendMode::Steer { turn: turn("a", "t1") }));
        assert!(!p.allows(&chat("a"), &SendMode::NewTurn), "a new turn is never allowed");
        assert!(!p.allows(&chat("a"), &SendMode::Steer { turn: turn("a", "t2") }), "another turn");
        assert!(!p.allows(&chat("b"), &SendMode::Steer { turn: turn("a", "t1") }), "another chat");
        // needs no workflow grant, but a released chat gets none
        reg.release_chat(&chat("a"));
        assert!(p.still_valid(&reg).is_err());
        assert_eq!(InterjectPermit::issue(&reg, chat("a"), turn("a", "t1")).err(), Some(GrantError::ChatReleased));
    }

    #[test]
    fn restored_release_records_apply_before_any_grant_exists() {
        let reg = GrantRegistry::default();
        reg.restore_released([chat("a")]);
        let g = OrgGrant::issue_for_test(WorkflowId::new("w1"), &reg);
        assert!(g.permit(&reg, &TaskId::new("t"), &chat("a")).is_err());
    }

    /// 送信権・利用者の確認の「型保証」は、呼ぶ場所を限る約束に依る。約束が崩れていないことをソースで確かめる（R-ORG L7）。
    #[test]
    fn user_confirmation_and_send_grants_are_made_only_where_the_user_acts() {
        fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() {
                    rs_files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        assert!(files.len() > 50, "the scan found the sources");
        // 製品コード（`#[cfg(test)]` より前）だけを見る。
        let product = |p: &std::path::Path| {
            let text = std::fs::read_to_string(p).unwrap();
            text.split("#[cfg(test)]").next().unwrap_or("").to_string()
        };
        let rel = |p: &std::path::Path| p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        // 利用者の確認を作ってよいのは、コマンド層・その定義・既存の利用者操作（名前付け・変更確認）だけ。組織実行の経路（host/org・gitops・store）からは作らない。
        let allowed_confirm = ["commands.rs", "backend/backend.rs", "host/changes.rs", "host/mod.rs"];
        // 組織の送信権を作ってよいのは、利用者の開始・再開の操作だけ（host/org/mod.rs）。
        let mut confirm_users = Vec::new();
        let mut grant_issuers = Vec::new();
        for f in &files {
            let name = rel(f);
            let text = product(f);
            if text.contains("from_user_command(") && !name.ends_with("backend/backend.rs") {
                confirm_users.push(name.clone());
            }
            if text.contains("OrgGrant::issue(") || (text.contains("fn issue(") && name == "host/org/grant.rs") {
                grant_issuers.push(name);
            }
        }
        for n in &confirm_users {
            assert!(allowed_confirm.contains(&n.as_str()), "{n} makes a UserConfirmed outside the user-action layer");
            assert!(!n.starts_with("host/org/") && !n.starts_with("gitops/") && !n.starts_with("store/"), "{n} is on the organisation path");
        }
        grant_issuers.sort();
        assert_eq!(grant_issuers, ["host/org/grant.rs", "host/org/mod.rs"], "OrgGrant is made only by the start / resume operations");
    }
}
