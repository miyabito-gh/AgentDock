//! 組織実行のポート（DESIGN_ORG_P4 §3.3）と、`Host` による実装。
//!
//! - `AgentRuntime`（書く側）は、役割チャットの作成と送信だけ。許可（`CreatePermit`／`DispatchPermit`）を値で受け取る。
//!   中断・承認の回答・resume は持たない（利用者の既存の操作だけが行う）。
//! - `RuntimeRead`（読む側）は許可が要らない読取りだけ。送信・resume・承認をしない。
//! - 送信は `Host::org_send` だけ。`SendAuth::Org` で既存の送信経路（`dispatch_send_as`）を通り、受理不明の記録・保存・照合を共用する。
//!   再送の分岐は持たない。受理不明は照合（読取り）の結果を待つ。

use std::sync::Arc;

use async_trait::async_trait;
use org_engine::{ApprovalPolicy, CreateResult, OrgNotice, RoleChatSpec, SandboxKind, TaskId, WorkflowId, WorkspaceAction, WorkspaceOutcome};

use super::driver::OrgSink;
use super::grant::{CreatePermit, DispatchPermit, GrantRegistry, InterjectPermit};
use crate::backend::backend::*;
use crate::backend::ipc::HostEvent;
use crate::backend::local::SaveScope;
use crate::backend::model::*;
use crate::backend::parity::{ParityOps, SandboxCheck};
use crate::host::state::agent_key_of;
use crate::host::{now_ms, Host, SendAuth};
use crate::gitops::GitOp;
use crate::rules::diff::parse_status_v2;
use crate::rules::org::{classify_trust_append, final_message_of, turn_end_of, turn_failure_of, DescendantReport, TrustCompare};
use crate::store::records::OrgBinding;

/// 送信1件の指示（エンジンが決めた試行IDと照合用IDをそのまま使う）。
#[derive(Debug, Clone)]
pub struct OrgSendSpec {
    pub chat: ChatKey,
    pub attempt: LocalId,
    pub client_message_id: String,
    pub text: String,
    pub output_schema: serde_json::Value,
}

/// 実行中のturnへの割り込み1件の指示。宛先のチャットとturnは許可（`InterjectPermit`）が持つ。
#[derive(Debug, Clone)]
pub struct OrgSteerSpec {
    /// 割り込みのID（エンジンの `int-N`）。送信の試行IDにそのまま使い、結果を割り込みの決着として受け取る。
    pub attempt: LocalId,
    pub client_message_id: String,
    pub text: String,
}

/// 送信の結果。`Dispatched` の送信状態（受理・拒否・受理不明）は、監視の受け口が別にエンジンへ知らせる。
#[derive(Debug)]
pub enum OrgSendResult {
    Dispatched(SendAttempt),
    /// live でないので送っていない（resume しない）。
    NotLive,
    /// 送信を止める条件（停止未確認・受理不明・削除保留・外部実行・実行中）に当たり、送っていない。
    Held(String),
}

/// 役割チャットの作成結果と、そのときの config.toml の前後照合。
#[derive(Debug)]
pub struct CreateOutcome {
    pub result: CreateResult,
    pub trust: Option<TrustCompare>,
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    /// 許可の失効を確認するための管理表（許可を作るときに使う）。
    fn grants(&self) -> &GrantRegistry;
    async fn create_role_chat(&self, permit: CreatePermit<'_>, spec: RoleChatSpec) -> CreateOutcome;
    async fn send(&self, permit: DispatchPermit<'_>, req: OrgSendSpec) -> OrgSendResult;
    /// 実行中のturnへの割り込み（`turn/steer`）。新しいturnは始めない。結果は監視の受け口が `InterjectSettled` で知らせる。
    async fn steer(&self, permit: InterjectPermit, req: OrgSteerSpec) -> OrgSendResult;
}

#[async_trait]
pub trait RuntimeRead: Send + Sync {
    /// そのturnの最終回答。読めない・まだ確定していないときは `Ok(None)`（推定しない）。
    async fn read_final_message(&self, chat: &ChatKey, turn: &TurnKey) -> Result<Option<String>, BackendError>;
    /// 作業フォルダが一致するチャット（作成結果が不明なときの候補表示用。自動では結び付けない）。
    async fn list_by_cwd(&self, cwd: &str) -> Result<Vec<ChatKey>, BackendError>;
    async fn sandbox_check(&self, cwd: &str) -> Result<SandboxCheck, BackendError>;
    /// 子孫の様子（キューの判断の読取り。送信しない）。
    async fn descendants(&self, chat: &ChatKey) -> DescendantReport;
    /// 再起動・wake後の照合の読取り（turnの終端・作業場所の実在と未コミット変更）。履歴の読取りと `git status` だけで、
    /// resume・送信・中断・書込みをしない。読めなかったものは「確認できていない」として返す（終端なし・不明で代用しない）。
    async fn read_reconcile(&self, chat: &ChatKey, turn: &TurnKey, cwd: Option<&str>) -> ReconcileRead;
}

/// 作業場所（組織が作ったworktree）の照合結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeFinding {
    /// 組織の所有記録にある作業場所ではない（利用者のリポジトリ等。状態を調べない）。
    NotOwned,
    /// 所有記録はあるがフォルダがない。
    Missing,
    Clean,
    /// 未コミットの変更がある（中断時点の変更は破棄しない。次のタスク表に出す）。
    Uncommitted { entries: u32 },
    /// 状態を読めなかった。
    Unreadable,
}

/// 照合の読取り結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ReconcileRead {
    /// 履歴を読めたか。
    pub turn_read: bool,
    /// 履歴で終端を確認できたときだけ Some。None は「確認できていない」。
    pub turn_end: Option<TurnEnd>,
    /// 失敗で終わったときの区分（失敗以外・読めていないときは未取得、区分が欠けていれば欠落）。
    pub turn_failure: Known<FailureClass>,
    pub worktree: WorktreeFinding,
}

/// 作業場所のポート（P-ORG-5）。組織実行のタスク用のworktree作成と、所有worktree内のコミットだけ。
/// 失敗・拒否・非対応を成功にしない（`WorkspaceOutcome` で返す）。マージ・push・利用者のブランチへの操作は持たない。
#[async_trait]
pub trait Workspace: Send + Sync {
    async fn run(&self, workflow: &WorkflowId, action: WorkspaceAction) -> WorkspaceOutcome;
}

#[async_trait]
impl Workspace for Arc<Host> {
    async fn run(&self, workflow: &WorkflowId, action: WorkspaceAction) -> WorkspaceOutcome {
        self.org_workspace(workflow, action).await
    }
}

/// 画面へ出す通知の材料（エンジンの通知と、ホストが観測したもの）。
#[derive(Debug, Clone, PartialEq)]
pub enum OrgHostNotice {
    Engine(OrgNotice),
    /// config.toml の前後照合（AgentDock は書いていない）。
    Config(TrustCompare),
    /// 送らなかった（理由つき）。
    SendRefused { task: TaskId, why: String },
    /// events.jsonl に追記できなかったため、以後は何も送らない。
    PersistFailed,
    /// 再起動・wake後の照合（読取りだけ）の結果。表示のための材料で、状態の変更はエンジンへの入力（`TurnEnded` 等）が行う。
    Reconciled { task: TaskId, turn_read: bool, turn_end: Option<TurnEnd>, worktree: WorktreeFinding },
    /// 実行中のまま新しい出力がない（通知だけ。自動で中断・再送・resume しない）。組織の通知種別（P-ORG-7 が画面・OS通知にする）。
    Stalled { task: TaskId, quiet_minutes: u32 },
    /// スロットの値（`slots\` のファイル）を書けなかったため、以後は何も送らない（次の工程を、値なしで選ばない）。
    SlotWriteFailed { slot: String },
    /// 定義（グラフ・役割）についての案内。`attention` は利用者の対処が要るもの（写しを使えない・スロットを読めない）。
    DefinitionNote { message: String, attention: bool },
}

/// 組織の通知の急ぎ度（通知集約の窓に載せる区分。承認待ち・停止系は即時、完了・失敗はワークフロー単位で2秒集約）。
impl OrgHostNotice {
    pub fn urgency(&self) -> org_engine::NoticeUrgency {
        match self {
            OrgHostNotice::Engine(n) => n.urgency(),
            // 照合結果・設定の照合は知らせるだけ（集約）。停滞・保存失敗・送らなかった理由は利用者の対処が要るので即時。
            OrgHostNotice::Reconciled { .. } | OrgHostNotice::Config(_) | OrgHostNotice::DefinitionNote { attention: false, .. } => org_engine::NoticeUrgency::Aggregated,
            OrgHostNotice::SendRefused { .. }
            | OrgHostNotice::PersistFailed
            | OrgHostNotice::SlotWriteFailed { .. }
            | OrgHostNotice::DefinitionNote { attention: true, .. }
            | OrgHostNotice::Stalled { .. } => org_engine::NoticeUrgency::Immediate,
        }
    }
}

#[async_trait]
impl AgentRuntime for Arc<Host> {
    fn grants(&self) -> &GrantRegistry {
        &self.org_rt.grants
    }

    async fn create_role_chat(&self, permit: CreatePermit<'_>, spec: RoleChatSpec) -> CreateOutcome {
        self.org_create_role_chat(permit, spec).await
    }

    async fn send(&self, permit: DispatchPermit<'_>, req: OrgSendSpec) -> OrgSendResult {
        self.org_send(permit, req).await
    }

    async fn steer(&self, permit: InterjectPermit, req: OrgSteerSpec) -> OrgSendResult {
        self.org_steer(permit, req).await
    }
}

#[async_trait]
impl RuntimeRead for Arc<Host> {
    async fn read_final_message(&self, chat: &ChatKey, turn: &TurnKey) -> Result<Option<String>, BackendError> {
        let history = self.read_history(agent_key_of(chat), ReadOptions { include_turns: true }).await?;
        Ok(final_message_of(&history.turns, turn))
    }

    async fn list_by_cwd(&self, cwd: &str) -> Result<Vec<ChatKey>, BackendError> {
        let page = self
            .backend
            .list_chats(ListChatsQuery { limit: Some(50), include_external: true, cwd: Some(vec![cwd.to_string()]), ..Default::default() })
            .await?;
        Ok(page.items.into_iter().map(|s| s.chat.key).collect())
    }

    async fn sandbox_check(&self, cwd: &str) -> Result<SandboxCheck, BackendError> {
        ParityOps::sandbox_check(self.backend.as_ref(), cwd.to_string()).await
    }

    async fn descendants(&self, chat: &ChatKey) -> DescendantReport {
        self.org_descendants(chat).await
    }

    async fn read_reconcile(&self, chat: &ChatKey, turn: &TurnKey, cwd: Option<&str>) -> ReconcileRead {
        let (turn_read, turn_end, turn_failure) = match self.read_history(agent_key_of(chat), ReadOptions { include_turns: true }).await {
            Ok(h) => (true, turn_end_of(&h.turns, turn), turn_failure_of(&h.turns, turn)),
            Err(_) => (false, None, Known::NotFetched),
        };
        ReconcileRead { turn_read, turn_end, turn_failure, worktree: self.org_worktree_finding(cwd).await }
    }
}

/// 画面への公開先（状態の写しを保持し、更新の合図を出す。通知は控える）。
impl OrgSink for Host {
    fn publish(&self, snapshot: super::driver::OrgSnapshot) {
        let workflow_id = snapshot.workflow.0.clone();
        *self.org_rt.view.lock().unwrap() = Some(snapshot);
        self.mutate(|_| ((), vec![HostEvent::OrgUpdated { workflow_id }]));
    }

    fn persisted(&self, workflow: &org_engine::WorkflowId, event: &org_engine::OrgEvent) {
        let mut g = self.org_rt.timeline.lock().unwrap();
        // 別のワークフローの記録が来たら（新しく開始したとき）、作り直す。
        if g.as_ref().is_none_or(|b| &b.workflow != workflow) {
            *g = Some(super::timeline::TimelineBuf::new(workflow.clone()));
        }
        if let Some(b) = g.as_mut() {
            b.push(event);
        }
    }

    fn timeline_updated(&self, workflow: &org_engine::WorkflowId, last_seq: u64) {
        let workflow_id = workflow.0.clone();
        self.mutate(|_| ((), vec![HostEvent::OrgTimelineUpdated { workflow_id, last_seq }]));
    }

    fn notice(&self, workflow: &org_engine::WorkflowId, notice: OrgHostNotice) {
        crate::diag::log("org", &format!("notice workflow={} {}", workflow.0, notice_label(&notice)));
        let serial = self.org_rt.push_notice(notice.clone());
        // OS通知（計画承認待ち・停止系は即時、完了・失敗は2秒集約）。画面の通知欄とは別に、既存の集約器へ渡す。
        // 名前は依頼文由来にしない（OS通知に依頼文の抜粋を載せない）。
        let title = Some(super::notify::notice_name(workflow));
        let chats = {
            let view = self.org_rt.view.lock().unwrap();
            let v: Vec<(org_engine::TaskId, Option<org_engine::ChatRef>)> = view.as_ref().map(|v| v.tasks.iter().map(|t| (t.id.clone(), t.chat.clone())).collect()).unwrap_or_default();
            v
        };
        let task_chat = |t: &org_engine::TaskId| chats.iter().find(|(id, _)| id == t).and_then(|(_, c)| c.as_ref()).and_then(crate::rules::org::chat_key_of_ref);
        if let Some(input) = super::notify::notify_input_of(workflow, title.as_deref(), serial, &task_chat, &notice) {
            self.notify_submit(vec![input]);
        }
    }
}

/// 診断ログ用の種別名（本文・パス・設定の内容は出さない）。
fn notice_label(n: &OrgHostNotice) -> &'static str {
    match n {
        OrgHostNotice::Engine(_) => "engine",
        OrgHostNotice::Config(TrustCompare::Unchanged) => "config-unchanged",
        OrgHostNotice::Config(TrustCompare::CodexTrustAppended { .. }) => "config-trust-appended",
        OrgHostNotice::Config(TrustCompare::Other { .. }) => "config-other-change",
        OrgHostNotice::Config(TrustCompare::Unverified { .. }) => "config-unverified",
        OrgHostNotice::SendRefused { .. } => "send-refused",
        OrgHostNotice::PersistFailed => "persist-failed",
        OrgHostNotice::Reconciled { .. } => "reconciled",
        OrgHostNotice::Stalled { .. } => "stalled",
        OrgHostNotice::SlotWriteFailed { .. } => "slot-write-failed",
        OrgHostNotice::DefinitionNote { .. } => "definition-note",
    }
}

/// 役割の sandbox と承認方針 → チャットの権限。untrusted は確認を増やす側だけ写す（never は定義の読込みで拒否済み。ここでも広げない）。
fn role_permission(sandbox: SandboxKind, approval: ApprovalPolicy) -> PermissionPreset {
    match (sandbox, approval) {
        (SandboxKind::ReadOnly, ApprovalPolicy::Untrusted) => PermissionPreset::ReadOnlyUntrusted,
        (SandboxKind::WorkspaceWrite, ApprovalPolicy::Untrusted) => PermissionPreset::WorkspaceWriteUntrusted,
        (SandboxKind::ReadOnly, _) => PermissionPreset::ReadOnly,
        (SandboxKind::WorkspaceWrite, _) => PermissionPreset::WorkspaceWriteOnRequest,
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

impl Host {
    /// config.toml の本文を読む（読むだけ。保存しない・ログに出さない）。読めなければ None。
    fn read_config_text() -> Option<String> {
        let path = crate::codex::cli::config_toml_path()?;
        std::fs::read_to_string(path).ok()
    }

    /// 組織の役割チャットを作る。一般の `start_chat`（一般領域・最初の依頼・名前付け）は使わない。
    /// 作成の前後で config.toml を読み比べる（`ext_lock` の中。書かない）。
    pub(in crate::host) async fn org_create_role_chat(self: &Arc<Self>, permit: CreatePermit<'_>, spec: RoleChatSpec) -> CreateOutcome {
        let none = |result: CreateResult| CreateOutcome { result, trust: None };
        if let Err(e) = permit.still_valid(&self.org_rt.grants) {
            return none(CreateResult::Rejected(e.message().to_string()));
        }
        if !std::path::Path::new(&spec.cwd).is_dir() {
            return none(CreateResult::Rejected(format!("作業フォルダが見つかりません: {}", spec.cwd)));
        }
        // 承認方針はエンジンが渡す（開始時の写しの役割定義の値。今の役割ファイルは見ない）。
        let permission = role_permission(spec.sandbox, spec.approval);
        let model = spec.model.clone().map(|m| ModelChoice { model: m, effort: spec.effort.clone(), speed_tier: None });
        let params = StartChatParams { kind: ChatKind::Development, cwd: spec.cwd.clone(), model: model.clone(), permission, extra_folders: Vec::new() };

        let (started, before, after) = {
            let _serial = self.ext_lock().await;
            let before = Self::read_config_text();
            let started = self.backend.start_chat(params).await;
            let after = Self::read_config_text();
            (started, before, after)
        };
        let trust = Some(classify_trust_append(before.as_deref(), after.as_deref()));
        let started = match started {
            Ok(s) => s,
            // 作られたかもしれない。照合用のIDが無いので、推定して結び付けない（利用者が選ぶ）。
            Err(BackendError::OutcomeUnknown { .. }) => return CreateOutcome { result: CreateResult::Unknown, trust },
            Err(e) => return CreateOutcome { result: CreateResult::Rejected(e.to_string()), trust },
        };
        let key = started.chat.key.clone();
        let binding = OrgBinding {
            workflow_id: permit.workflow().0.clone(),
            task_id: permit.task().0.clone(),
            role_id: spec.role.0.clone(),
            released_at: None,
        };
        let meta = crate::host::persist::cached_meta_of(&started.chat, now_ms());
        self.update_local(&key, false, std::time::Duration::ZERO, |f| {
            f.hosted = true;
            f.org = Some(binding.clone());
            f.permission = Some(permission);
            f.model = model.clone();
            f.cached_meta = Some(meta);
        });
        // 所有の記録を保存できなければ、このチャットへは送らない（再起動後に所有が分からなくなるため）。
        let saved = self.save_now(SaveScope::ChatLocal { chat: key.clone() }).await;
        if !saved.is_some_and(|s| matches!(s.state, SaveState::Saved { .. })) {
            return CreateOutcome { result: CreateResult::SaveFailed, trust };
        }
        self.register_started_chat(&started, model);
        self.org_rt.observer.lock().unwrap().bind(key.clone(), permit.workflow().clone(), permit.task().clone());
        CreateOutcome { result: CreateResult::Created(crate::rules::org::chat_ref_of(&key)), trust }
    }

    /// 組織の送信（`DispatchPermit` を消費する唯一の経路）。再送の分岐は持たない。
    /// live でなければ送らない（resume しない）。停止未確認・受理不明などの条件は利用者の送信と同じものを通る。
    pub(in crate::host) async fn org_send(self: &Arc<Self>, permit: DispatchPermit<'_>, req: OrgSendSpec) -> OrgSendResult {
        let chat = req.chat.clone();
        if permit.chat() != &chat {
            return OrgSendResult::Held("許可の宛先と違うチャットです".to_string());
        }
        if let Err(e) = permit.still_valid(&self.org_rt.grants) {
            return OrgSendResult::Held(e.message().to_string());
        }
        // 手動送信・キューと同時に turn/start を送らない。
        let lock = self.queue_rt.send_lock(&chat);
        let _send_guard = lock.lock().await;
        if let Err(e) = self.check_sendable_core(&chat) {
            return OrgSendResult::Held(e.message);
        }
        let root = agent_key_of(&chat);
        let (live, running, selected) = self.read(|d| {
            (
                d.root_view(&chat).map(|v| v.freshness) == Some(Freshness::Live),
                d.running_turn.contains_key(&root),
                d.model_settings.get(&chat).and_then(|s| s.selected.clone()),
            )
        });
        if !live {
            return OrgSendResult::NotLive;
        }
        if running {
            return OrgSendResult::Held("このチャットは実行中です".to_string());
        }
        let request = SendRequest {
            chat: chat.clone(),
            mode: SendMode::NewTurn,
            text: req.text,
            attachments: Vec::new(),
            speed_tier: selected.as_ref().and_then(|m| m.speed_tier.clone()),
            model: selected,
            permission: None,
            cwd: None,
            work_folders: None,
            work_mode: None,
            client_message_id: req.client_message_id,
            output_schema: Some(req.output_schema),
        };
        let attempt = SendAttempt { attempt_id: req.attempt, client_message_id: request.client_message_id.clone(), at: now_ms(), state: SendState::Sending };
        OrgSendResult::Dispatched(self.dispatch_send_as(request, attempt, SendAuth::Org(permit)).await)
    }

    /// 実行中のturnへの割り込み（`InterjectPermit` を消費する唯一の経路）。新しいturnは始めない。再送の分岐は持たない。
    /// live でなければ送らない（resume しない）。実行中のturnが許可のturnと違えば送らない。停止未確認・受理不明などの条件は
    /// 利用者の送信と同じものを通る（その場合は送っていないので、割り込みは「届かなかった」と確定する）。
    pub(in crate::host) async fn org_steer(self: &Arc<Self>, permit: InterjectPermit, req: OrgSteerSpec) -> OrgSendResult {
        let chat = permit.chat().clone();
        if let Err(e) = permit.still_valid(&self.org_rt.grants) {
            return OrgSendResult::Held(e.message().to_string());
        }
        let lock = self.queue_rt.send_lock(&chat);
        let _send_guard = lock.lock().await;
        if let Err(e) = self.check_sendable_core(&chat) {
            return OrgSendResult::Held(e.message);
        }
        let root = agent_key_of(&chat);
        let (live, running) = self.read(|d| (d.root_view(&chat).map(|v| v.freshness) == Some(Freshness::Live), d.running_turn.get(&root).cloned()));
        if !live {
            return OrgSendResult::NotLive;
        }
        if running.as_ref() != Some(&permit.turn().turn_id) {
            return OrgSendResult::Held("実行中のturnが、割り込む対象のturnと違います".to_string());
        }
        let request = SendRequest {
            chat: chat.clone(),
            mode: SendMode::Steer { turn: permit.turn().clone() },
            text: req.text,
            attachments: Vec::new(),
            speed_tier: None,
            model: None,
            permission: None,
            cwd: None,
            work_folders: None,
            work_mode: None,
            client_message_id: req.client_message_id,
            output_schema: None,
        };
        // 結果の受け口が、これを通常の送信の結果ではなく割り込みの決着として扱えるように、送る前に控える。
        self.org_rt.observer.lock().unwrap().note_interject(req.attempt.clone(), permit.turn().turn_id.clone());
        let attempt = SendAttempt { attempt_id: req.attempt, client_message_id: request.client_message_id.clone(), at: now_ms(), state: SendState::Sending };
        OrgSendResult::Dispatched(self.dispatch_send_as(request, attempt, SendAuth::OrgInterject(permit)).await)
    }

    /// 組織の所有記録にある作業場所の実在と未コミット変更を読む（`git status` だけ。書込みなし）。所有記録にない場所は調べない。
    pub(in crate::host) async fn org_worktree_finding(self: &Arc<Self>, cwd: Option<&str>) -> WorktreeFinding {
        let Some(cwd) = cwd else { return WorktreeFinding::NotOwned };
        let owned = self.read(|d| d.worktrees.iter().any(|r| r.org.is_some() && same_path(&r.path, cwd)));
        if !owned {
            return WorktreeFinding::NotOwned;
        }
        let path = std::path::Path::new(cwd);
        if !path.is_dir() {
            return WorktreeFinding::Missing;
        }
        match self.git_for_worktree().run(path, &GitOp::Status).await {
            Ok(o) if o.success() => match parse_status_v2(&o.stdout_text()).len() {
                0 => WorktreeFinding::Clean,
                n => WorktreeFinding::Uncommitted { entries: n as u32 },
            },
            _ => WorktreeFinding::Unreadable,
        }
    }

    /// 子孫の様子（キューの判断の読取り）。履歴の確認は読取りだけで、resume・送信・承認をしない。
    pub(in crate::host) async fn org_descendants(self: &Arc<Self>, chat: &ChatKey) -> DescendantReport {
        let mut input = self.queue_gate_input(chat).await;
        crate::rules::org::drop_terminal_failures(&mut input);
        let decision = crate::rules::queue::evaluate_gate(&input, None, false, &LocalId(String::new()));
        crate::rules::org::descendants_from_gate(&decision, &agent_key_of(chat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_approval_policy_of_the_spec_makes_the_chat_permission() {
        // 承認方針は役割ファイルではなく、エンジンが渡す値（開始時の写し）だけで決まる。
        assert_eq!(role_permission(SandboxKind::ReadOnly, ApprovalPolicy::OnRequest), PermissionPreset::ReadOnly);
        assert_eq!(role_permission(SandboxKind::WorkspaceWrite, ApprovalPolicy::OnRequest), PermissionPreset::WorkspaceWriteOnRequest);
        let permission = role_permission(SandboxKind::WorkspaceWrite, ApprovalPolicy::Untrusted);
        assert_eq!(permission, PermissionPreset::WorkspaceWriteUntrusted);
        assert_eq!(crate::codex::convert::permission_params(permission), ("untrusted", "workspace-write"));
        assert_eq!(role_permission(SandboxKind::ReadOnly, ApprovalPolicy::Untrusted), PermissionPreset::ReadOnlyUntrusted);
    }
}
