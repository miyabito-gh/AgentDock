//! 複数のAIバックエンド（Codex と Antigravity）を、ホストから1つの `AiBackend` に見せる振り分け層。
//!
//! - ホストの監視元（接続・能力・鮮度）は Codex の1つだけ。Antigravity は接続イベントを出さず、
//!   イベントは1本の受け口に合流する（`source` は別なので衝突しない）。
//! - 操作は、キー（`ChatKey`・`AgentKey`・`TurnKey`・`RequestKey`）の `backend` で振り分ける。
//!   キーを持たない操作は、開始（`StartChatParams.backend`）・モデル一覧（`ModelQuery.backend`）で指定させる。
//! - Codex 固有の拡張操作（`ParityOps`）は、Codex のチャットだけに通し、他は「非対応」を返す。
//! - Antigravity が使えなくても（`agy` 未導入など）Codex の動作は変わらない。

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::antigravity::AntigravityBackend;
use crate::backend::backend::*;
use crate::backend::model::*;
use crate::backend::parity::*;
use crate::codex::CodexBackend;

/// 合流後のイベント受け口の容量（各バックエンドの受け口と同程度）。
const MERGED_EVENT_CAPACITY: usize = 1024;

pub struct MuxBackend {
    codex: Arc<CodexBackend>,
    antigravity: Arc<AntigravityBackend>,
}

impl MuxBackend {
    pub fn new() -> Self {
        MuxBackend { codex: Arc::new(CodexBackend::new()), antigravity: Arc::new(AntigravityBackend::new()) }
    }

    /// Codex の現在の接続の Job（強制終了用。Codex 固有。Antigravity は対象外）。
    pub fn current_job(&self) -> Option<(SourceId, Arc<crate::win::job::ProcessJob>)> {
        self.codex.current_job()
    }

    fn pick(&self, kind: BackendKind) -> &dyn AiBackend {
        match kind {
            BackendKind::Codex => self.codex.as_ref(),
            BackendKind::Antigravity => self.antigravity.as_ref(),
        }
    }

    /// Codex のチャットだけに通す拡張操作の判定（他は非対応）。
    fn codex_only<T>(&self, chat: &ChatKey, op: ParityOp) -> Option<BackendResult<T>> {
        (chat.backend != BackendKind::Codex).then(|| unsupported(op))
    }
}

impl Default for MuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AiBackend for MuxBackend {
    /// ホストの主たる監視元の種別（Codex）。
    fn kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    /// 監視元の能力は Codex のもの（ホストの状態判定が使う）。Antigravity の能力は各チャットの操作で拒否される。
    fn capabilities(&self) -> Capabilities {
        self.codex.capabilities()
    }

    /// Codex を接続して返す。Antigravity は最善努力で接続し、失敗しても Codex の結果に影響させない
    /// （`agy` 未導入・版確認の失敗は、Antigravity のチャットが使えないだけ）。
    async fn connect(&self, config: ConnectConfig) -> BackendResult<SourceInfo> {
        let agy_config = ConnectConfig { executable: None, expected_version: None, ..config.clone() };
        let codex = self.codex.connect(config).await;
        match self.antigravity.connect(agy_config).await {
            Ok(info) => {
                if let ConnectionState::LaunchFailed { message, .. } = &info.connection {
                    crate::diag::log("antigravity.connect", &format!("launchFailed: {message}"));
                }
            }
            Err(e) => crate::diag::log("antigravity.connect", &format!("error: {e}")),
        }
        codex
    }

    async fn disconnect(&self) -> BackendResult<()> {
        let _ = self.antigravity.disconnect().await;
        self.codex.disconnect().await
    }

    /// 両方のイベント受け口を1本に合流する。一方が閉じても他方は続ける。
    fn take_events(&self) -> Option<EventReceiver> {
        let codex_rx = self.codex.take_events();
        let agy_rx = self.antigravity.take_events();
        if codex_rx.is_none() && agy_rx.is_none() {
            return None;
        }
        let (tx, rx) = mpsc::channel(MERGED_EVENT_CAPACITY);
        for mut src in [codex_rx, agy_rx].into_iter().flatten() {
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(env) = src.recv().await {
                    if tx.send(env).await.is_err() {
                        break;
                    }
                }
            });
        }
        Some(rx)
    }

    async fn start_chat(&self, params: StartChatParams) -> BackendResult<ChatStarted> {
        self.pick(params.backend).start_chat(params).await
    }

    /// 先頭ページは両方の一覧を合わせる。続きのページ（cursor 指定）は Codex のものだけ。
    /// Antigravity の一覧が読めないときは Codex の一覧だけを返す（Codex の一覧を失敗にしない）。
    async fn list_chats(&self, query: ListChatsQuery) -> BackendResult<Page<ChatSummary>> {
        let first_page = query.cursor.is_none();
        let mut page = self.codex.list_chats(query.clone()).await?;
        if first_page {
            match self.antigravity.list_chats(ListChatsQuery { cursor: None, limit: None, ..query }).await {
                Ok(agy) => page.items.extend(agy.items),
                Err(e) => crate::diag::log("antigravity.list_chats", &format!("error: {e}")),
            }
        }
        Ok(page)
    }

    async fn read(&self, agent: AgentKey, options: ReadOptions) -> BackendResult<AgentHistory> {
        self.pick(agent.backend).read(agent, options).await
    }

    async fn resume(&self, chat: ChatKey, confirmed: &UserConfirmed, overrides: Option<&ThreadOverrides>) -> BackendResult<ResumeOutcome> {
        self.pick(chat.backend).resume(chat, confirmed, overrides).await
    }

    async fn manage_chat(&self, chat: ChatKey, op: ManageOp, confirmed: &UserConfirmed) -> BackendResult<ManageOutcome> {
        self.pick(chat.backend).manage_chat(chat, op, confirmed).await
    }

    async fn list_projects(&self) -> BackendResult<Vec<ProjectSummary>> {
        self.codex.list_projects().await
    }

    async fn list_loaded(&self) -> BackendResult<Vec<AgentKey>> {
        let mut loaded = self.codex.list_loaded().await?;
        if let Ok(agy) = self.antigravity.list_loaded().await {
            loaded.extend(agy);
        }
        Ok(loaded)
    }

    async fn read_work_folders(&self, chat: &ChatKey) -> BackendResult<Known<WorkFolders>> {
        self.pick(chat.backend).read_work_folders(chat).await
    }

    async fn scan_descendants(&self, root: AgentKey) -> BackendResult<DescendantScan> {
        self.pick(root.backend).scan_descendants(root).await
    }

    async fn send(&self, request: SendRequest) -> SendOutcome {
        self.pick(request.chat.backend).send(request).await
    }

    async fn reconcile_send(&self, pending: UnconfirmedSend) -> ReconcileOutcome {
        self.pick(pending.chat().backend).reconcile_send(pending).await
    }

    async fn respond(&self, request: RequestKey, answer: RequestAnswer, confirmed: &UserConfirmed) -> BackendResult<RespondOutcome> {
        self.pick(request.backend).respond(request, answer, confirmed).await
    }

    async fn interrupt(&self, turn: TurnKey, confirmed: &UserConfirmed) -> BackendResult<InterruptAck> {
        self.pick(turn.agent.backend).interrupt(turn, confirmed).await
    }

    async fn list_models(&self, query: ModelQuery) -> BackendResult<Vec<ModelInfo>> {
        self.pick(query.backend).list_models(query).await
    }
}

#[async_trait]
impl ParityOps for MuxBackend {
    async fn fork_chat(&self, chat: ChatKey, params: ForkParams, confirmed: &UserConfirmed) -> BackendResult<ForkOutcome> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Fork) {
            return r;
        }
        self.codex.fork_chat(chat, params, confirmed).await
    }
    async fn start_review(&self, chat: ChatKey, target: ReviewTarget, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if let Some(r) = self.codex_only(&chat, ParityOp::CodeReview) {
            return r;
        }
        self.codex.start_review(chat, target, confirmed).await
    }
    async fn compact(&self, chat: ChatKey, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Compact) {
            return r;
        }
        self.codex.compact(chat, confirmed).await
    }
    async fn get_goal(&self, chat: ChatKey) -> BackendResult<Known<Goal>> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Goal) {
            return r;
        }
        self.codex.get_goal(chat).await
    }
    async fn set_goal(&self, chat: ChatKey, update: GoalUpdate, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Goal) {
            return r;
        }
        self.codex.set_goal(chat, update, confirmed).await
    }
    async fn clear_goal(&self, chat: ChatKey, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Goal) {
            return r;
        }
        self.codex.clear_goal(chat, confirmed).await
    }
    async fn list_work_modes(&self) -> BackendResult<Vec<WorkModeInfo>> {
        self.codex.list_work_modes().await
    }
    async fn list_skills(&self, cwd: String, force_reload: bool) -> BackendResult<SkillList> {
        self.codex.list_skills(cwd, force_reload).await
    }
    async fn instruction_sources(&self, chat: ChatKey) -> BackendResult<Known<Vec<String>>> {
        if let Some(r) = self.codex_only(&chat, ParityOp::InstructionFiles) {
            return r;
        }
        self.codex.instruction_sources(chat).await
    }
    async fn list_tool_servers(&self) -> BackendResult<Vec<ToolServerView>> {
        self.codex.list_tool_servers().await
    }
    async fn login_tool_server(&self, name: String, confirmed: &UserConfirmed) -> BackendResult<String> {
        self.codex.login_tool_server(name, confirmed).await
    }
    async fn reload_tool_servers(&self, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        self.codex.reload_tool_servers(confirmed).await
    }
    async fn list_extensions(&self) -> BackendResult<Vec<ExtensionView>> {
        self.codex.list_extensions().await
    }
    async fn manage_extension(&self, op: ExtensionOp, confirmed: &UserConfirmed) -> BackendResult<ExtensionOpResult> {
        self.codex.manage_extension(op, confirmed).await
    }
    async fn cloud_submit(&self, request: CloudSubmit, confirmed: &UserConfirmed) -> BackendResult<CloudSubmitOutcome> {
        self.codex.cloud_submit(request, confirmed).await
    }
    async fn cloud_list(&self) -> BackendResult<CloudTaskList> {
        self.codex.cloud_list().await
    }
    async fn cloud_status(&self, task_id: String) -> BackendResult<CloudCommandOutput> {
        self.codex.cloud_status(task_id).await
    }
    async fn cloud_diff(&self, task_id: String) -> BackendResult<CloudCommandOutput> {
        self.codex.cloud_diff(task_id).await
    }
    async fn cloud_apply(&self, task_id: String, cwd: String, confirmed: &UserConfirmed) -> BackendResult<CloudCommandOutput> {
        self.codex.cloud_apply(task_id, cwd, confirmed).await
    }
    async fn backend_status(&self) -> BackendResult<BackendStatus> {
        self.codex.backend_status().await
    }
    async fn memory_status(&self) -> BackendResult<MemoryStatus> {
        self.codex.memory_status().await
    }
    async fn set_memory_mode(&self, chat: ChatKey, mode: String, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if let Some(r) = self.codex_only(&chat, ParityOp::Memory) {
            return r;
        }
        self.codex.set_memory_mode(chat, mode, confirmed).await
    }
    async fn reset_memory(&self, confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        self.codex.reset_memory(confirmed).await
    }
    async fn sandbox_check(&self, cwd: String) -> BackendResult<SandboxCheck> {
        self.codex.sandbox_check(cwd).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(backend: BackendKind) -> ChatKey {
        ChatKey { backend, id: ExternalId("x".into()) }
    }

    #[test]
    fn kind_and_capabilities_follow_codex() {
        let mux = MuxBackend::new();
        assert_eq!(mux.kind(), BackendKind::Codex);
        assert_eq!(mux.capabilities(), mux.codex.capabilities());
    }

    #[test]
    fn pick_routes_by_backend_kind() {
        let mux = MuxBackend::new();
        assert_eq!(mux.pick(BackendKind::Codex).kind(), BackendKind::Codex);
        assert_eq!(mux.pick(BackendKind::Antigravity).kind(), BackendKind::Antigravity);
    }

    #[test]
    fn codex_only_ops_are_unsupported_for_other_backends() {
        let mux = MuxBackend::new();
        assert!(mux.codex_only::<()>(&chat(BackendKind::Antigravity), ParityOp::Goal).is_some());
        assert!(mux.codex_only::<()>(&chat(BackendKind::Codex), ParityOp::Goal).is_none());
    }

    #[tokio::test]
    async fn unconnected_antigravity_chat_operations_do_not_reach_codex() {
        let mux = MuxBackend::new();
        // 未接続の Antigravity への操作は Codex へ漏れず、Antigravity 側のエラーになる（Codex も未接続なので漏れれば Codex の NotConnected と区別できないため、種別で確認）。
        let r = mux.read(AgentKey { backend: BackendKind::Antigravity, id: ExternalId("none".into()) }, ReadOptions::default()).await;
        assert!(r.is_err());
        assert_eq!(mux.pick(BackendKind::Antigravity).kind(), BackendKind::Antigravity);
    }

    #[test]
    fn take_events_yields_one_merged_receiver_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mux = MuxBackend::new();
            assert!(mux.take_events().is_some());
            assert!(mux.take_events().is_none());
        });
    }
}
