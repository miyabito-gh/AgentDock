//! 組織実行のホスト側（P-ORG-4b、DESIGN_ORG_P4 §3）。
//!
//! - `grant.rs`: 送信権（`OrgGrant`・`DispatchPermit`）と失効の管理。
//! - `ports.rs`: エンジンが使うポートの trait と `Host` による実装（作成・送信・読取り）。
//! - `observe.rs`: ホストの観測（イベント・送信結果・中断）をエンジンへの入力に変える純粋な変換。
//! - `artifacts.rs`: 成果物パネルと確認つきの取り込み・戻す（DESIGN_ORG_R3）。
//! - `driver.rs`: `OrgCore` を所有し action を実行する専用task。
//! - `manage.rs`: 終わったワークフローの削除・アーカイブ（DESIGN_ORG_DELETE）。
//! - `graphs.rs`: グラフ定義ファイルの読込み・初回作成と、開始時の定義の写し（`graph.json`・`roles.json`）。
//! - `intake.rs`: 開始前の依頼整理（要約の turn・保存・開始。エンジンもドライバも作らない。DESIGN_ORG_INTAKE §1）。
//!
//! 守ること:
//! - 監視・照合・再起動の経路からは組織の送信を呼べない（送信権は司令taskだけが持つ。`Host` が持つのは失効の管理表だけ）。
//! - エンジンは Codex の承認・質問に答えない。承認は既存の承認カードで利用者が答える。
//! - 受理不明の送信を再送しない。通信断・終了を失敗・取消に変換しない。resume は利用者の再開操作の中だけ。
//! - `~/.codex` は書き換えない（config.toml は前後を読むだけ）。

pub mod artifacts;
pub mod driver;
pub mod grant;
pub mod graphs;
pub mod history;
pub mod intake;
pub mod ipc;
pub mod manage;
pub mod notify;
pub mod observe;
pub mod ports;
pub mod roles;
pub mod stall;
pub mod timeline;
pub mod view;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use org_engine::{recover_on_restart, restore, Limits, OrgCore, OrgInput, OrgStore, TaskStatus, WorkflowId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use self::driver::{DriverMsg, OrgDriver, OrgSink, OrgSnapshot};
use self::grant::{GrantRegistry, OrgGrant};
use self::observe::OrgObserver;
use self::ports::OrgHostNotice;
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::{BackendEvent, UserConfirmed};
use crate::backend::ipc::{BlockedReason, IpcError, IpcErrorCode};
use crate::backend::local::OrgRunningNote;
use crate::backend::model::{ChatKey, ConnectionState, SendAttempt};
use crate::rules::org::{chat_key_of_ref, chat_ref_of};
use crate::store::org::{list_workflow_dirs_newest, workflow_dir, FileOrgStore, ReadOnlyOrgStore};

const NOTICE_KEEP: usize = 100;

/// 稼働中の司令taskへの口。
pub struct DriverHandle {
    pub workflow: WorkflowId,
    pub tx: UnboundedSender<DriverMsg>,
}

/// `Host` が持つ組織実行の作業状態。送信権そのもの（`OrgGrant`）は持たない。
#[derive(Default)]
pub struct OrgRuntime {
    pub(in crate::host) grants: GrantRegistry,
    pub(in crate::host) observer: Mutex<OrgObserver>,
    pub(in crate::host) driver: Mutex<Option<DriverHandle>>,
    /// 直近の状態の写し（司令taskが公開する）。
    pub(in crate::host) view: Mutex<Option<OrgSnapshot>>,
    notices: Mutex<VecDeque<NoticeEntry>>,
    notice_serial: std::sync::atomic::AtomicU64,
    /// 直近に読んだ役割定義（P-ORG-8）。UI（P-ORG-7）が理由を表示する元。
    roles: Mutex<Option<roles::RoleReport>>,
    /// 直近に読んだグラフ定義（G-A4）。UI（G-A5）が一覧と誤りの理由を表示する元。
    graphs: Mutex<Option<graphs::GraphReport>>,
    /// 過去のワークフローの読込み結果の控え（読取り専用。保存ファイルが変わったときだけ読み直す）。
    pub(in crate::host) history_cache: Mutex<history::HistoryCache>,
    /// 開始前の依頼整理の作業状態（`intake.rs`）。
    pub(in crate::host) intake: intake::IntakeRuntime,
    /// 削除を実行中のワークフロー（二重実行の防止）。
    pub(in crate::host) deleting: Mutex<std::collections::HashSet<String>>,
    /// 動いている（または直前まで動いていた）ワークフローの進行タイムライン（DESIGN_ORG_R2 1）。
    pub(in crate::host) timeline: Mutex<Option<timeline::TimelineBuf>>,
    /// 取り込みの確認の控え（`artifacts.rs`。メモリだけ）。
    pub(in crate::host) artifacts: artifacts::ArtifactRuntime,
}

/// 溜めた通知の1件（通知の重複排除と画面の並びに使う通し番号つき）。
#[derive(Debug, Clone)]
pub struct NoticeEntry {
    pub serial: u64,
    pub notice: OrgHostNotice,
}

impl OrgRuntime {
    /// 通知を溜めて、その通し番号を返す。
    pub(in crate::host) fn push_notice(&self, n: OrgHostNotice) -> u64 {
        let serial = self.notice_serial.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let mut q = self.notices.lock().unwrap();
        q.push_back(NoticeEntry { serial, notice: n });
        while q.len() > NOTICE_KEEP {
            q.pop_front();
        }
        serial
    }

    pub(in crate::host) fn notice_entries(&self) -> Vec<NoticeEntry> {
        self.notices.lock().unwrap().iter().cloned().collect()
    }

    fn sender(&self) -> Option<UnboundedSender<DriverMsg>> {
        self.driver.lock().unwrap().as_ref().map(|d| d.tx.clone())
    }

    /// 司令taskが動いている（登録があり、受け口が閉じていない）。終わった・止まったtaskは含めない（R-ORG H4）。
    pub(in crate::host) fn driver_active(&self) -> bool {
        self.driver.lock().unwrap().as_ref().is_some_and(|d| !d.tx.is_closed())
    }

    /// 司令taskを登録する。確認と登録を1回のロックで行う（二重押しで前の登録を上書きしない。R-ORG L6）。
    pub(in crate::host) fn try_register_driver(&self, handle: DriverHandle) -> Result<(), DriverHandle> {
        let mut guard = self.driver.lock().unwrap();
        if guard.as_ref().is_some_and(|d| !d.tx.is_closed()) {
            return Err(handle);
        }
        *guard = Some(handle);
        Ok(())
    }

    /// 司令taskが終わった（すべて終端・panic）。受け口が閉じている同じワークフローの登録だけを外す。
    /// 状態の写し（`view`）は残し、画面に結果を出し続ける。
    pub(in crate::host) fn driver_ended(&self, workflow: &WorkflowId) {
        let mut guard = self.driver.lock().unwrap();
        let ours = guard.as_ref().is_some_and(|d| &d.workflow == workflow && d.tx.is_closed());
        if ours {
            *guard = None;
        }
        drop(guard);
        // panic で終わった場合の送信権が残らないように。別のワークフローが動き出していれば（登録が違えば）触れない。
        if ours {
            self.grants.revoke_all();
        }
    }

    fn send_all(&self, msgs: Vec<DriverMsg>) {
        if msgs.is_empty() {
            return;
        }
        if let Some(tx) = self.sender() {
            for m in msgs {
                let _ = tx.send(m);
            }
        }
    }
}

/// 組織実行の開始の指定（IPC の型は P-ORG-7）。
#[derive(Debug, Clone)]
pub struct OrgStartArgs {
    pub request: String,
    /// 作業するリポジトリ（Planner の作業フォルダ）。
    pub repo: String,
    /// 同時に動かす数（書込み役・全体。1〜4）。None は既定。
    pub parallelism: Option<(u32, u32)>,
    /// テンプレート（グラフ定義）のID。None は従来の計画経路（組込みの `standard` と同じ）。
    pub graph: Option<String>,
    /// turn 上限。None は既定（30）。
    pub max_turns: Option<u32>,
    /// 新しい試行での再送の上限（0〜10）。None は既定（3）。
    pub max_retries: Option<u32>,
    /// 依頼整理を経て始めるときの由来（`org_intake_start` だけが付ける）。あれば `intake.json` を、ドライバ登録の前に保存する（保存できなければ始めない）。
    pub origin: Option<crate::backend::intake::IntakeOrigin>,
}

/// 開始ダイアログで指定できる turn 上限の最大（暴走防止の上限は静的上限と turn 上限の両方で効く）。
pub const START_MAX_TURNS: u32 = 200;

/// 再開の結果。再開できなかったチャットは live に戻していない（エンジンは送らずに止まる）。
#[derive(Debug, Clone, Default)]
pub struct OrgResumeReport {
    pub resumed: Vec<ChatKey>,
    pub failed: Vec<(ChatKey, String)>,
    /// 発話前で再開できず、未送信が確かなので新しい役割チャットに作り直すようエンジンへ頼んだもの。
    pub recreated: Vec<ChatKey>,
}

impl Host {
    // ───────────── 監視 → エンジン（待たない・送信しない） ─────────────

    /// ホストのイベント処理から呼ぶ。ロックの外で、変換した入力をチャネルへ流すだけ。
    pub(in crate::host) fn org_observe_event(&self, event: &BackendEvent) {
        let mut msgs = {
            let mut obs = self.org_rt.observer.lock().unwrap();
            if obs.is_empty() && !matches!(event, BackendEvent::Connection { .. } | BackendEvent::Gap { .. }) {
                return;
            }
            let open = |chat: &ChatKey| -> u32 {
                self.read(|d| d.requests.iter().filter(|r| &r.chat == chat && r.state == crate::backend::model::RequestState::Pending).count() as u32)
            };
            let chat_of = |a: &crate::backend::model::AgentKey| self.read(|d| d.view(a).map(|v| v.agent.chat.clone()));
            let mut msgs = obs.on_event(event, &open, &chat_of);
            msgs.extend(obs.on_activity(event, now_ms().0, &chat_of));
            msgs
        };
        // 接続できたら（起動直後・再接続）、要照合のタスクを読取りだけで照合する。resume・送信はしない。
        if matches!(event, BackendEvent::Connection { state: ConnectionState::Connected }) {
            msgs.push(DriverMsg::Reconcile);
        }
        self.org_rt.send_all(msgs);
    }

    /// 要照合のタスクの照合（読取りだけ）を司令taskに頼む。結果は `TurnEnded` と通知（`Reconciled`）で届く。
    pub(in crate::host) fn org_request_reconcile(&self) {
        self.org_rt.send_all(vec![DriverMsg::Reconcile]);
    }

    /// 完全終了の前置。新規ディスパッチを止める（送信権の失効＋エンジンの停止）。タスクは取消・失敗にしない。
    /// 実行中の turn の中断はここではしない（既存の停止手順の `interrupt_chat` が行う）。ワークフローがなければ何もしない。
    pub(in crate::host) fn org_begin_shutdown(&self) {
        if self.org_rt.sender().is_none() {
            return;
        }
        self.org_rt.grants.revoke_all();
        self.org_rt.send_all(vec![DriverMsg::Revoke, DriverMsg::Input(OrgInput::Shutdown)]);
    }

    /// ここまでに渡した入力を司令taskが処理し終える（状態の保存を含む）のを待つ。待つのは数秒まで。保存できなかった分は
    /// 追記済みのイベントから再起動時に復元される。
    pub(in crate::host) async fn org_flush(&self) {
        let Some(tx) = self.org_rt.sender() else { return };
        let (done, wait) = tokio::sync::oneshot::channel();
        if tx.send(DriverMsg::Sync(done)).is_err() {
            return;
        }
        if tokio::time::timeout(std::time::Duration::from_secs(3), wait).await.is_err() {
            crate::diag::log("org", "flush timed out");
        }
    }

    /// 強制終了の確認に併記する、組織実行中のワークフロー（併記するだけ）。
    pub(in crate::host) fn org_running_notes(&self) -> Vec<OrgRunningNote> {
        running_notes_of(self.org_rt.view.lock().unwrap().as_ref())
    }

    /// 送信状態の更新（照合の結果を含む）をエンジンへ知らせる。送信はしない。
    pub(in crate::host) fn org_observe_send(&self, chat: &ChatKey, attempt: &SendAttempt) {
        let msgs = self.org_rt.observer.lock().unwrap().on_send(chat, attempt);
        self.org_rt.send_all(msgs);
    }

    /// 利用者の中断の操作をエンジンへ知らせる（中断そのものは既存の経路）。
    pub(in crate::host) fn org_observe_interrupt(&self, chat: &ChatKey) {
        let msgs = self.org_rt.observer.lock().unwrap().on_interrupt(chat);
        self.org_rt.send_all(msgs);
    }

    /// 接続の切断・wake・再起動で呼ぶ（P-ORG-6 が配線する）。送信権を失効させ、エンジンには鮮度の入力を送る。
    pub fn org_note_stale(&self, cause: org_engine::ReconcileCause) {
        self.org_rt.grants.revoke_all();
        self.org_rt.send_all(vec![DriverMsg::Revoke, DriverMsg::Input(OrgInput::Freshness { cause })]);
    }

    // ───────────── 利用者の操作（P-ORG-7 の IPC が呼ぶ） ─────────────

    /// 組織実行を開始する。利用者の「開始」の操作（`UserConfirmed` を値で消費して送信権を作る）。
    pub async fn org_start(self: &Arc<Self>, args: OrgStartArgs, confirmed: UserConfirmed) -> Result<WorkflowId, IpcError> {
        if args.request.trim().is_empty() {
            return Err(err(IpcErrorCode::InvalidArgs, "依頼が空です"));
        }
        if !std::path::Path::new(args.repo.trim()).is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, format!("作業フォルダが見つかりません: {}", args.repo)));
        }
        // Workerは専用worktreeで作業するため、Gitリポジトリでコミットが1つ以上あることを、始める前に読取りだけで確かめる。
        // 準備が要る状態は `RepoNotPrepared` で返す（UIが「Gitを準備」ダイアログへ誘導する）。
        self.precheck_repo_ready(args.repo.trim()).await?;
        let mut limits = Limits::default();
        if let Some((writers, total)) = args.parallelism {
            limits = limits.with_parallelism(writers, total).map_err(|_| err(IpcErrorCode::InvalidArgs, "並列数は 1〜4 で指定してください"))?;
        }
        limits.stagnation_minutes = self.read(|d| d.settings.org_stall_minutes);
        if !self.is_connected() {
            return Err(err(IpcErrorCode::NotConnected, "Codex に接続していません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、組織実行を始められません（記録を残せません）"));
        };
        self.precheck_space()?;
        if self.org_rt.driver_active() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行はすでに動いています（同時に動かせるのは1件です）"));
        }
        if let Some(m) = args.max_turns {
            if !(1..=START_MAX_TURNS).contains(&m) {
                return Err(err(IpcErrorCode::InvalidArgs, format!("turn 上限は 1〜{START_MAX_TURNS} で指定してください")));
            }
            limits.max_turns = m;
        }
        if let Some(n) = args.max_retries {
            limits = limits.with_max_retries(n).map_err(|_| err(IpcErrorCode::InvalidArgs, "再送の上限は 0〜10 で指定してください"))?;
        }
        // 役割定義を読む（初回は標準の役割を作る）。有効な役割だけがエンジンに渡る。
        let mut core = OrgCore::new(now_ms().0);
        let roles_report = roles::prepare_roles(&root, now_ms().0);
        *self.org_rt.roles.lock().unwrap() = Some(roles_report.clone());
        // テンプレート。指定がなければ従来の計画経路（定義ファイルには依存しない）。計画経路の `standard` も同じ。
        let mut graph_start: Option<(org_engine::graph::CompiledGraph, Vec<org_engine::LoadedRole>)> = None;
        if let Some(id) = args.graph.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let report = graphs::prepare_graphs(&root, &roles_report.roles);
            *self.org_rt.graphs.lock().unwrap() = Some(report.clone());
            let Some(compiled) = report.find(id) else {
                let why = report.problem_for(id).map(|p| p.reasons.join(" / ")).unwrap_or_else(|| "そのテンプレートがありません".to_string());
                return Err(err(IpcErrorCode::InvalidArgs, format!("テンプレート「{id}」は使えません: {why}")));
            };
            if let Some(why) = graphs::start_blocker(compiled) {
                return Err(err(IpcErrorCode::InvalidArgs, why));
            }
            let planned = compiled.def.nodes.iter().any(|n| matches!(n.kind, org_engine::graph::NodeKind::PlannedTasks { .. }));
            if !planned {
                let used = graphs::used_roles(compiled, &roles_report.roles).map_err(|m| err(IpcErrorCode::InvalidArgs, format!("テンプレート「{id}」は使えません: {m}")))?;
                graph_start = Some((compiled.clone(), used));
            }
        }
        // グラフを先に渡す（役割の確認は、グラフが使う役割に対して行う）。
        if let Some((compiled, _)) = &graph_start {
            if let Err(msg) = core.set_graph(compiled.clone()) {
                return Err(err(IpcErrorCode::Io, msg));
            }
        }
        if let Err(msg) = core.set_roles(&roles_report.roles) {
            return Err(err(IpcErrorCode::Io, msg));
        }
        let workflow = WorkflowId::new(format!("wf-{}", now_ms().0));
        let wf_dir = workflow_dir(&root, &workflow.0);
        // 開始時の定義の写しを先に保存する（保存できなければ始めない）。復元はこの写しで行う。
        let roles_fingerprint = match &graph_start {
            Some((compiled, used)) => Some(
                graphs::save_copies(&wf_dir, compiled, used)
                    .map_err(|e| err(IpcErrorCode::Io, format!("開始時の定義の写しを保存できないため、始めません: {e}")))?,
            ),
            None => None,
        };
        // 依頼整理を経た開始: 開始時の要約の写しを、ドライバ登録の前に保存する（保存できなければ始めない）。
        if let Some(origin) = &args.origin {
            intake::save_copy(&wf_dir, origin, now_ms()).map_err(|e| err(IpcErrorCode::Io, format!("依頼整理の写しを保存できないため、始めません: {e}")))?;
        }
        let store: Arc<dyn OrgStore + Send + Sync> = Arc::new(FileOrgStore::new(wf_dir));
        let driver = OrgDriver::new(core, store, Arc::new(self.clone()), self.clone() as Arc<dyn OrgSink>, Box::new(|| now_ms().0));
        // 登録は確認と同じロックの中で行う（先に動き出したものがあれば、ここで断る）。
        let Some(tx) = self.spawn_org_driver(workflow.clone(), driver) else {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行はすでに動いています（同時に動かせるのは1件です）"));
        };
        // 送信権 → 開始。順序をチャネルで保つ。
        let grant = OrgGrant::issue(confirmed, workflow.clone(), &self.org_rt.grants);
        let _ = tx.send(DriverMsg::Install(grant));
        let repo = args.repo.trim().to_string();
        let start = match roles_fingerprint {
            Some(roles_fingerprint) => OrgInput::StartGraph { workflow: workflow.clone(), request: args.request, repo, limits, roles_fingerprint },
            None => OrgInput::Start { workflow: workflow.clone(), request: args.request, repo, limits },
        };
        let _ = tx.send(DriverMsg::Input(start));
        Ok(workflow)
    }

    /// 直近に読んだ役割定義の結果（無効の理由を含む）。まだ読んでいなければ None。
    pub fn org_roles_report(&self) -> Option<roles::RoleReport> {
        self.org_rt.roles.lock().unwrap().clone()
    }

    /// 直近に読んだグラフ定義の結果（誤りの理由を含む）。まだ読んでいなければ None。
    pub fn org_graphs_report(&self) -> Option<graphs::GraphReport> {
        self.org_rt.graphs.lock().unwrap().clone()
    }

    /// 役割定義を読み直す。動いているワークフローがあるときは変えない（途中で権限が変わらないように）。
    pub fn org_reload_roles(&self) -> Result<roles::RoleReport, IpcError> {
        if self.org_rt.driver_active() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行が動いている間は、役割定義を読み直せません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、役割定義を読めません"));
        };
        let report = roles::prepare_roles(&root, now_ms().0);
        *self.org_rt.roles.lock().unwrap() = Some(report.clone());
        Ok(report)
    }

    /// グラフ定義を読み直す（足りない組込みテンプレートは作る。既存ファイルは上書きしない）。動いているワークフローがあるときは変えない。
    /// 役割定義も読み直す（グラフの検証が使う役割の表のため）。
    pub fn org_reload_graphs(&self) -> Result<graphs::GraphReport, IpcError> {
        if self.org_rt.driver_active() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行が動いている間は、グラフ定義を読み直せません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、グラフ定義を読めません"));
        };
        let roles_report = roles::prepare_roles(&root, now_ms().0);
        let report = graphs::prepare_graphs(&root, &roles_report.roles);
        *self.org_rt.roles.lock().unwrap() = Some(roles_report);
        *self.org_rt.graphs.lock().unwrap() = Some(report.clone());
        Ok(report)
    }

    /// 権限を広げる役割定義を、利用者の確認のうえで有効にする。確認は今の権限に対してだけ有効。
    pub fn org_enable_role(&self, id: &str, confirmed: UserConfirmed) -> Result<roles::RoleReport, IpcError> {
        if self.org_rt.driver_active() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行が動いている間は、役割を有効にできません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、役割を有効にできません"));
        };
        let mut guard = self.org_rt.roles.lock().unwrap();
        let Some(report) = guard.as_mut() else {
            return Err(err(IpcErrorCode::NotFound, "役割定義がまだ読み込まれていません"));
        };
        roles::enable_role(&root, report, id, confirmed).map_err(|m| err(IpcErrorCode::InvalidArgs, m))?;
        Ok(report.clone())
    }

    /// ワークフローを再開する。利用者の操作の中でだけ、必要なチャットを resume する（自動 resume はしない）。
    /// 順序: 各チャットの live を戻す → `Live` を知らせる → 新しい送信権 → `UserResume`。
    pub async fn org_resume(self: &Arc<Self>, confirmed: UserConfirmed) -> Result<OrgResumeReport, IpcError> {
        let Some(tx) = self.org_rt.sender() else {
            return Err(err(IpcErrorCode::NotFound, "再開できる組織実行がありません"));
        };
        let workflow = self.org_rt.driver.lock().unwrap().as_ref().map(|d| d.workflow.clone());
        let Some(workflow) = workflow else { return Err(err(IpcErrorCode::NotFound, "再開できる組織実行がありません")) };
        let chats: Vec<ChatKey> = {
            let view = self.org_rt.view.lock().unwrap();
            view.as_ref()
                .map(|v| {
                    v.tasks
                        .iter()
                        .filter(|t| !matches!(t.status, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled))
                        .filter_map(|t| t.chat.as_ref().and_then(chat_key_of_ref))
                        .filter(|c| !self.org_rt.grants.is_released(c))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut report = OrgResumeReport::default();
        for chat in chats {
            match self.resume_chat_live(&chat, &confirmed).await {
                Ok(()) => {
                    let _ = tx.send(DriverMsg::Input(OrgInput::Live { chat: chat_ref_of(&chat), live: true }));
                    report.resumed.push(chat);
                }
                Err(e) => {
                    // Codex has no history and nothing was ever sent to the chat: the engine makes a new role chat
                    // for the task (it re-checks that nothing was sent). Anything else stays stopped with the reason.
                    if self.chat_never_persisted(&chat) && self.org_task_never_sent(&chat) {
                        let task = self.org_rt.observer.lock().unwrap().task_of(&chat).cloned();
                        if let Some(task) = task {
                            let _ = tx.send(DriverMsg::Input(OrgInput::ChatUnresumable { task }));
                            self.org_rt.observer.lock().unwrap().unbind(&chat);
                            report.recreated.push(chat);
                            continue;
                        }
                    }
                    report.failed.push((chat, e.message));
                }
            }
        }
        let grant = OrgGrant::issue(confirmed, workflow, &self.org_rt.grants);
        let _ = tx.send(DriverMsg::Install(grant));
        let _ = tx.send(DriverMsg::Input(OrgInput::UserResume));
        // 照合から再開までの間に終わった turn を拾う（読取りだけ。確定でよい。R-ORG M6）。
        let _ = tx.send(DriverMsg::Reconcile);
        Ok(report)
    }

    /// Codex に履歴がなく（一覧に出ず、保存記録だけがある）、resume できないチャットか。
    fn chat_never_persisted(&self, chat: &ChatKey) -> bool {
        self.read(|d| d.root_view(chat).is_none() && d.chat(chat).is_some_and(|c| c.no_history))
    }

    /// そのチャットのタスクに、送信の記録も受理済みturnもなく、状態が確認済みか（未送信が確かか）。
    fn org_task_never_sent(&self, chat: &ChatKey) -> bool {
        let view = self.org_rt.view.lock().unwrap();
        view.as_ref().is_some_and(|v| {
            v.tasks.iter().any(|t| {
                t.chat.as_ref().and_then(chat_key_of_ref).as_ref() == Some(chat) && t.never_sent && t.confirm == org_engine::ConfirmState::Confirmed
            })
        })
    }

    /// 一時停止。送信権を失効させる（以後は再開の操作が要る）。実行中の turn は止めない。
    pub fn org_pause(&self) {
        self.org_rt.grants.revoke_all();
        self.org_rt.send_all(vec![DriverMsg::Revoke, DriverMsg::Input(OrgInput::UserPause)]);
    }

    /// 「手動に切り替える」。保存できたときだけ解放する（保存できなければ解放しない）。以後エンジンはこのチャットへ送らない。
    pub async fn org_release_chat(self: &Arc<Self>, chat: &ChatKey, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        let binding = self.read(|d| d.locals.get(chat).and_then(|l| l.org.clone()));
        let Some(binding) = binding else { return Err(err(IpcErrorCode::NotFound, "組織実行のチャットではありません")) };
        if binding.released_at.is_some() {
            return Ok(());
        }
        let at = now_ms();
        self.update_local(chat, true, std::time::Duration::ZERO, |f| {
            if let Some(o) = f.org.as_mut() {
                o.released_at = Some(at);
            }
        });
        let saved = self.save_now(crate::backend::local::SaveScope::ChatLocal { chat: chat.clone() }).await;
        if !saved.is_some_and(|s| matches!(s.state, crate::backend::model::SaveState::Saved { .. })) {
            self.update_local(chat, true, std::time::Duration::ZERO, |f| {
                if let Some(o) = f.org.as_mut() {
                    o.released_at = None;
                }
            });
            return Err(blocked(BlockedReason::SaveFailed { scope: crate::backend::local::SaveScope::ChatLocal { chat: chat.clone() } }, "保存できなかったため、手動への切り替えを行っていません"));
        }
        self.org_rt.grants.release_chat(chat);
        let task = self.org_rt.observer.lock().unwrap().task_of(chat).cloned();
        self.org_rt.observer.lock().unwrap().unbind(chat);
        if let Some(task) = task {
            self.org_rt.send_all(vec![DriverMsg::Input(OrgInput::UserManual { task })]);
        }
        Ok(())
    }

    /// そのチャットのタスクが、手動の結果の取り込み中か（その間は通常の送信を断る）。
    pub(in crate::host) fn org_chat_adopting(&self, chat: &ChatKey) -> bool {
        let view = self.org_rt.view.lock().unwrap();
        view.as_ref().is_some_and(|v| crate::rules::org::is_adopting(chat, v.tasks.iter().map(|t| (t.chat.as_ref().and_then(chat_key_of_ref), t.adopting))))
    }

    /// 直近の状態の写し（P-ORG-7 が IPC の型にする）。
    pub fn org_snapshot(&self) -> Option<OrgSnapshot> {
        self.org_rt.view.lock().unwrap().clone()
    }

    /// 溜めた通知（新しい順に古いものから消える）。
    pub fn org_notices(&self) -> Vec<OrgHostNotice> {
        self.org_rt.notices.lock().unwrap().iter().map(|e| e.notice.clone()).collect()
    }

    // ───────────── 起動時の復元（送信権なし） ─────────────

    /// 前回のワークフローを復元する。送信権は付けない（照合は読取りだけ。再び送るには利用者の再開が要る）。
    /// 結果が失われた役割チャットの作成は、`chat.json.org` の記録どうしの突合で結び付け直す（候補の推定はしない）。
    pub(in crate::host) fn org_restore(self: &Arc<Self>) {
        // 起動しただけでは送信権は存在しない（前の実行の許可は引き継がない）。
        self.org_rt.grants.revoke_all();
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else { return };
        // 依頼整理を経て始めたのに、開始済みの印を保存できていなかったものを `intake.json` から直す（読取りと記録の更新だけ）。
        self.intake_reconcile_started(&root);
        // 解放済みのチャットの印を先に反映し、未解放の組織チャットを監視の対応表へ入れる。
        let (released, owned): (Vec<ChatKey>, Vec<(ChatKey, String, String)>) = self.read(|d| {
            let mut released = Vec::new();
            let mut owned = Vec::new();
            for (k, l) in &d.locals {
                if let Some(o) = &l.org {
                    if o.released_at.is_some() {
                        released.push(k.clone());
                    } else {
                        owned.push((k.clone(), o.workflow_id.clone(), o.task_id.clone()));
                    }
                }
            }
            (released, owned)
        });
        self.org_rt.grants.restore_released(released);
        // 削除の途中（`delete.json` あり）のフォルダは復元しない（自動で続けない。一覧には最小の行で出る）。
        let Some(dir) = list_workflow_dirs_newest(&root).into_iter().find(|d| !d.join(crate::store::org::DELETE_FILE).exists()) else { return };
        // まず読取りだけで調べる。全タスクが終わっているものは、状態の分類（`halted Restart` の追記）も司令taskも作らない
        // （過去のワークフローとして `org_view` が読取り専用で出す）。
        match restore(&ReadOnlyOrgStore::new(dir.clone())) {
            Ok((state, _)) if state.tasks().iter().all(|t| matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled)) => return,
            Ok(_) => {}
            Err(e) => {
                crate::diag::log("org", &format!("restore failed: {e:?}"));
                return;
            }
        }
        let store = FileOrgStore::new(dir.clone());
        let recovered = match recover_on_restart(&store, now_ms().0) {
            Ok(r) => r,
            Err(e) => {
                crate::diag::log("org", &format!("restore failed: {e:?}"));
                return;
            }
        };
        let state = recovered.state;
        let workflow = state.workflow_id().clone();
        // 進行タイムラインは、保存された記録の全体から作り直す（分類の追記も含む）。
        {
            let mut buf = timeline::TimelineBuf::new(workflow.clone());
            if let Ok(read) = store.read_events() {
                for e in &read.events {
                    buf.push(e);
                }
            }
            *self.org_rt.timeline.lock().unwrap() = Some(buf);
        }
        let mut recoveries = Vec::new();
        {
            let mut obs = self.org_rt.observer.lock().unwrap();
            for (chat, wf, task) in &owned {
                if *wf == workflow.0 {
                    obs.bind(chat.clone(), workflow.clone(), org_engine::TaskId::new(task.clone()));
                }
            }
            // 作成を始めたが結果が失われたタスク: 同じ workflow・task を指す所有記録があれば、結び付け直す。
            for t in state.tasks() {
                if t.chat().is_none() && t.chat_cwd().is_some() {
                    if let Some((chat, _, _)) = owned.iter().find(|(_, wf, task)| *wf == workflow.0 && *task == t.id.0) {
                        recoveries.push(OrgInput::ChatRecovered { task: t.id.clone(), chat: chat_ref_of(chat) });
                    }
                }
            }
        }
        *self.org_rt.view.lock().unwrap() = Some(driver::snapshot_of(&state, None));
        // 今の定義ファイル（役割・グラフ）。画面の元として控え、開始時の定義との違いの案内にだけ使う（読むだけ。グラフは作成もしない）。
        let report = roles::prepare_roles(&root, now_ms().0);
        let current_graphs = graphs::load_graphs(&root, &report.roles);
        *self.org_rt.roles.lock().unwrap() = Some(report.clone());
        *self.org_rt.graphs.lock().unwrap() = Some(current_graphs.clone());
        // グラフで動いていたワークフローは、開始時の写しで復元する（今の定義ファイルでは代用しない）。
        let mut notes: Vec<OrgHostNotice> = Vec::new();
        let mut copy: Option<(org_engine::graph::CompiledGraph, Vec<org_engine::LoadedRole>)> = None;
        if let Some(bound) = state.graph() {
            match graphs::load_copies(&dir, bound) {
                Ok((g, r)) => {
                    let ids: Vec<org_engine::RoleId> = g.roles_used().into_iter().collect();
                    if let Some(message) = graphs::definitions_changed(bound, &ids, &current_graphs, &report.roles) {
                        notes.push(OrgHostNotice::DefinitionNote { message, attention: false });
                    }
                    copy = Some((g, r));
                }
                Err(why) => {
                    crate::diag::log("org", &format!("definition copy unusable: {why}"));
                    notes.push(OrgHostNotice::DefinitionNote {
                        message: format!("開始時の定義の写しを使えないため、このワークフローは進められません（{why}）"),
                        attention: true,
                    });
                }
            }
        }
        let store: Arc<dyn OrgStore + Send + Sync> = Arc::new(store);
        // スロットの値を読み戻す。読めないものは「未取得」のまま（その値で辺を選ばず、止まる）。
        let (slots, unreadable) = graphs::read_slots(store.as_ref(), &state);
        for slot in unreadable {
            notes.push(OrgHostNotice::DefinitionNote {
                message: format!("スロット「{slot}」の値を読めませんでした（未取得として扱い、これを使う次の工程は選びません）"),
                attention: true,
            });
        }
        // 報告を読み戻す。読めない調査報告は「未取得」のまま（それを使う次のタスクは、最終回答から1回だけ読み直し、だめなら止まる）。
        let (reports, unreadable_reports) = graphs::read_reports(store.as_ref(), &state);
        for task in unreadable_reports {
            notes.push(OrgHostNotice::DefinitionNote {
                message: format!("調査 {task} の保存済みの報告を読めませんでした（これを使う次のタスクは、最終回答から1回だけ読み直します。読めなければ止まります）"),
                attention: false,
            });
        }
        let (graph, copied_roles) = match copy {
            Some((g, r)) => (Some(g), Some(r)),
            None => (None, None),
        };
        let mut core = OrgCore::restored_with(state, graph, org_engine::RestoredExtras { slots, reports }, now_ms().0);
        // 写しの役割で動かす。写しがない（従来の計画経路のワークフロー）ときは今の役割定義（読めなければ標準の既定のまま。権限を広げない側）。
        if let Err(e) = core.set_roles(copied_roles.as_deref().unwrap_or(&report.roles)) {
            crate::diag::log("org", &format!("roles on restore: {e}"));
        }
        let driver = OrgDriver::new(core, store, Arc::new(self.clone()), self.clone() as Arc<dyn OrgSink>, Box::new(|| now_ms().0));
        let Some(tx) = self.spawn_org_driver(workflow.clone(), driver) else { return };
        for n in notes {
            OrgSink::notice(&**self, &workflow, n);
        }
        for r in recoveries {
            let _ = tx.send(DriverMsg::Input(r));
        }
        // 公開（状態の写し）だけ行う。送信権は付けない。
        let _ = tx.send(DriverMsg::Input(OrgInput::Tick { now_ms: now_ms().0 }));
        // すでに接続していれば照合（読取りだけ）。未接続なら、接続したときに `org_observe_event` が頼む。
        if self.is_connected() {
            let _ = tx.send(DriverMsg::Reconcile);
        }
    }

    /// 司令taskを登録して動かす。すでに動いているものがあれば何もせず None。task が終わったら（すべて終端・panic）登録を外す。
    fn spawn_org_driver(self: &Arc<Self>, workflow: WorkflowId, driver: OrgDriver<Arc<Host>>) -> Option<UnboundedSender<DriverMsg>> {
        let (tx, rx) = unbounded_channel();
        self.org_rt.try_register_driver(DriverHandle { workflow: workflow.clone(), tx: tx.clone() }).ok()?;
        let running = tokio::spawn(driver.run(rx));
        let host = self.clone();
        tokio::spawn(async move {
            // 正常終了でも panic でも、閉じた送信口を残さない。
            let _ = running.await;
            host.org_rt.driver_ended(&workflow);
        });
        self.org_apply_stall_setting();
        Some(tx)
    }
}

/// 組織実行中のワークフローの併記用の写し。終わっていないタスクが残るものだけ（完了・失敗・取消だけなら出さない）。
pub fn running_notes_of(view: Option<&OrgSnapshot>) -> Vec<OrgRunningNote> {
    let Some(v) = view else { return Vec::new() };
    let finished = |s: &TaskStatus| matches!(s, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled);
    let live = |t: &&driver::TaskSnapshot| !finished(&t.status);
    let active = v
        .tasks
        .iter()
        .filter(live)
        .filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Reviewing) || t.confirm != org_engine::ConfirmState::Confirmed)
        .count() as u32;
    if v.tasks.iter().filter(live).count() == 0 {
        return Vec::new();
    }
    vec![OrgRunningNote { workflow_id: v.workflow.0.clone(), title: v.title.clone(), active_tasks: active, total_tasks: v.tasks.len() as u32 }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{ConfirmState, TaskId};

    fn task(id: &str, status: TaskStatus, confirm: ConfirmState) -> driver::TaskSnapshot {
        driver::TaskSnapshot {
            id: TaskId::new(id),
            role: "worker".into(),
            status,
            confirm,
            chat: None,
            title: String::new(),
            deps: Vec::new(),
            effects: Vec::new(),
            attempts: 1,
            commit: None,
            chat_cwd: None,
            retryable: false,
            never_sent: false,
            retry: Err(org_engine::RetryRefusal::NotBlocked),
            retry_count: 0,
            adopting: false,
            manual_format: None,
            node: None,
            extras: Default::default(),
        }
    }

    fn view(tasks: Vec<driver::TaskSnapshot>) -> OrgSnapshot {
        OrgSnapshot {
            workflow: WorkflowId::new("wf-1"),
            title: Some("make it".into()),
            halt: None,
            tasks,
            request: "make it".into(),
            repo: String::new(),
            limits: Limits::default(),
            turns_used: 0,
            auto_replans_used: 0,
            graph: None,
            notes: Vec::new(),
            interjects: Vec::new(),
            notes_after_plan: 0,
        }
    }

    #[test]
    fn the_force_kill_note_counts_unfinished_tasks_and_hides_finished_workflows() {
        assert!(running_notes_of(None).is_empty());
        let v = view(vec![
            task("a", TaskStatus::Running, ConfirmState::Confirmed),
            task("b", TaskStatus::Running, ConfirmState::NeedsReconcile(org_engine::ReconcileCause::Restart)),
            task("c", TaskStatus::Ready, ConfirmState::Confirmed),
            task("d", TaskStatus::Completed, ConfirmState::Confirmed),
        ]);
        let n = running_notes_of(Some(&v));
        assert_eq!(n.len(), 1);
        assert_eq!((n[0].active_tasks, n[0].total_tasks), (2, 4));
        assert_eq!(n[0].title.as_deref(), Some("make it"));
        let done = view(vec![task("a", TaskStatus::Completed, ConfirmState::Confirmed), task("b", TaskStatus::Cancelled, ConfirmState::Confirmed)]);
        assert!(running_notes_of(Some(&done)).is_empty());
    }

    fn git_cmd(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.com"]).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    async fn ready_check(git: &crate::gitops::Git, folder: &str) -> Result<(), IpcError> {
        crate::host::gitprep::repo_ready_error(crate::gitops::prep::repo_status(git, std::path::Path::new(folder)).await)
    }

    #[tokio::test]
    async fn start_is_refused_for_a_repo_without_commits_and_allowed_with_one() {
        let dir = std::env::temp_dir().join(format!("agentdock-precheck-{}", now_ms().0));
        std::fs::create_dir_all(&dir).unwrap();
        let git = crate::gitops::Git::new(None);
        let folder = dir.to_string_lossy().into_owned();
        // not a repository: the UI is pointed at the prep dialog
        let e = ready_check(&git, &folder).await.unwrap_err();
        assert!(e.message.contains("リポジトリではありません") && matches!(e.blocked, Some(BlockedReason::RepoNotPrepared { .. })), "{e:?}");
        // a repository with no commit
        git_cmd(&dir, &["init"]);
        let e = ready_check(&git, &folder).await.unwrap_err();
        assert!(e.message.contains("まだコミットがありません") && matches!(e.blocked, Some(BlockedReason::RepoNotPrepared { .. })), "{e:?}");
        // one commit: the regular case still starts
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        git_cmd(&dir, &["add", "-A"]);
        git_cmd(&dir, &["commit", "-m", "first"]);
        assert!(ready_check(&git, &folder).await.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn handle(wf: &str) -> (DriverHandle, tokio::sync::mpsc::UnboundedReceiver<DriverMsg>) {
        let (tx, rx) = unbounded_channel();
        (DriverHandle { workflow: WorkflowId::new(wf), tx }, rx)
    }

    #[test]
    fn a_finished_driver_frees_the_slot_but_a_live_one_or_a_newer_one_is_never_replaced() {
        let rt = OrgRuntime::default();
        assert!(!rt.driver_active());
        let (h1, rx1) = handle("wf-1");
        assert!(rt.try_register_driver(h1).is_ok());
        assert!(rt.driver_active());
        // a double press: the second registration is refused while the first is alive (R-ORG L6)
        let (h2, _rx2) = handle("wf-2");
        assert!(rt.try_register_driver(h2).is_err());
        assert_eq!(rt.driver.lock().unwrap().as_ref().map(|d| d.workflow.0.clone()), Some("wf-1".to_string()));
        // an ended-for-another-workflow notice does not remove a live driver
        rt.driver_ended(&WorkflowId::new("wf-1"));
        assert!(rt.driver_active());
        // the driver finishes (its receiver is dropped): nothing is active, a new workflow may register (H4)
        drop(rx1);
        assert!(!rt.driver_active());
        let (h3, rx3) = handle("wf-3");
        assert!(rt.try_register_driver(h3).is_ok());
        // the old driver's late "ended" notice must not remove the newer registration
        rt.driver_ended(&WorkflowId::new("wf-1"));
        assert!(rt.driver_active());
        // its own ended notice does
        drop(rx3);
        rt.driver_ended(&WorkflowId::new("wf-3"));
        assert!(rt.driver.lock().unwrap().is_none());
    }
}
