//! 組織実行のホスト側（P-ORG-4b、DESIGN_ORG_P4 §3）。
//!
//! - `grant.rs`: 送信権（`OrgGrant`・`DispatchPermit`）と失効の管理。
//! - `ports.rs`: エンジンが使うポートの trait と `Host` による実装（作成・送信・読取り）。
//! - `observe.rs`: ホストの観測（イベント・送信結果・中断）をエンジンへの入力に変える純粋な変換。
//! - `driver.rs`: `OrgCore` を所有し action を実行する専用task。
//!
//! 守ること:
//! - 監視・照合・再起動の経路からは組織の送信を呼べない（送信権は司令taskだけが持つ。`Host` が持つのは失効の管理表だけ）。
//! - エンジンは Codex の承認・質問に答えない。承認は既存の承認カードで利用者が答える。
//! - 受理不明の送信を再送しない。通信断・終了を失敗・取消に変換しない。resume は利用者の再開操作の中だけ。
//! - `~/.codex` は書き換えない（config.toml は前後を読むだけ）。

pub mod driver;
pub mod grant;
pub mod ipc;
pub mod notify;
pub mod observe;
pub mod ports;
pub mod roles;
pub mod stall;
pub mod view;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use org_engine::{recover_on_restart, Limits, OrgCore, OrgInput, OrgStore, TaskStatus, WorkflowId};
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
use crate::store::org::{workflow_dir, FileOrgStore};

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
}

/// 再開の結果。再開できなかったチャットは live に戻していない（エンジンは送らずに止まる）。
#[derive(Debug, Clone, Default)]
pub struct OrgResumeReport {
    pub resumed: Vec<ChatKey>,
    pub failed: Vec<(ChatKey, String)>,
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
        if self.org_rt.driver.lock().unwrap().is_some() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行はすでに動いています（同時に動かせるのは1件です）"));
        }
        // 役割定義を読む（初回は標準3役を作る）。有効な役割だけがエンジンに渡る。
        let mut core = OrgCore::new(now_ms().0);
        {
            let report = roles::prepare_roles(&root, now_ms().0);
            if let Err(msg) = core.set_roles(&report.roles) {
                *self.org_rt.roles.lock().unwrap() = Some(report);
                return Err(err(IpcErrorCode::Io, msg));
            }
            *self.org_rt.roles.lock().unwrap() = Some(report);
        }
        let workflow = WorkflowId::new(format!("wf-{}", now_ms().0));
        let store: Arc<dyn OrgStore + Send + Sync> = Arc::new(FileOrgStore::new(workflow_dir(&root, &workflow.0)));
        let driver = OrgDriver::new(core, store, Arc::new(self.clone()), self.clone() as Arc<dyn OrgSink>, Box::new(|| now_ms().0));
        let tx = self.spawn_org_driver(workflow.clone(), driver);
        // 送信権 → 開始。順序をチャネルで保つ。
        let grant = OrgGrant::issue(confirmed, workflow.clone(), &self.org_rt.grants);
        let _ = tx.send(DriverMsg::Install(grant));
        let _ = tx.send(DriverMsg::Input(OrgInput::Start { workflow: workflow.clone(), request: args.request, repo: args.repo.trim().to_string(), limits }));
        Ok(workflow)
    }

    /// 直近に読んだ役割定義の結果（無効の理由を含む）。まだ読んでいなければ None。
    pub fn org_roles_report(&self) -> Option<roles::RoleReport> {
        self.org_rt.roles.lock().unwrap().clone()
    }

    /// 役割定義を読み直す。動いているワークフローがあるときは変えない（途中で権限が変わらないように）。
    pub fn org_reload_roles(&self) -> Result<roles::RoleReport, IpcError> {
        if self.org_rt.driver.lock().unwrap().is_some() {
            return Err(blocked(BlockedReason::ChatBusy, "組織実行が動いている間は、役割定義を読み直せません"));
        }
        let Some(root) = self.persist.store().map(|s| s.root().to_path_buf()) else {
            return Err(err(IpcErrorCode::Io, "保存先が使えないため、役割定義を読めません"));
        };
        let report = roles::prepare_roles(&root, now_ms().0);
        *self.org_rt.roles.lock().unwrap() = Some(report.clone());
        Ok(report)
    }

    /// 権限を広げる役割定義を、利用者の確認のうえで有効にする。確認は今の権限に対してだけ有効。
    pub fn org_enable_role(&self, id: &str, confirmed: UserConfirmed) -> Result<roles::RoleReport, IpcError> {
        if self.org_rt.driver.lock().unwrap().is_some() {
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
                Err(e) => report.failed.push((chat, e.message)),
            }
        }
        let grant = OrgGrant::issue(confirmed, workflow, &self.org_rt.grants);
        let _ = tx.send(DriverMsg::Install(grant));
        let _ = tx.send(DriverMsg::Input(OrgInput::UserResume));
        Ok(report)
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
        let Some(dir) = newest_workflow_dir(&root) else { return };
        let store = FileOrgStore::new(dir.clone());
        let recovered = match recover_on_restart(&store, now_ms().0) {
            Ok(r) => r,
            Err(e) => {
                crate::diag::log("org", &format!("restore failed: {e:?}"));
                return;
            }
        };
        let state = recovered.state;
        let finished = state.tasks().iter().all(|t| matches!(t.status(), TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled));
        if finished {
            return;
        }
        let workflow = state.workflow_id().clone();
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
        *self.org_rt.view.lock().unwrap() = Some(driver::snapshot_of(&state));
        let store: Arc<dyn OrgStore + Send + Sync> = Arc::new(store);
        // 役割定義は読み直す。読めなければ標準の既定のまま（権限を広げない側）。
        let mut core = OrgCore::restored(state, now_ms().0);
        let report = roles::prepare_roles(&root, now_ms().0);
        if let Err(e) = core.set_roles(&report.roles) {
            crate::diag::log("org", &format!("roles on restore: {e}"));
        }
        *self.org_rt.roles.lock().unwrap() = Some(report);
        let driver = OrgDriver::new(core, store, Arc::new(self.clone()), self.clone() as Arc<dyn OrgSink>, Box::new(|| now_ms().0));
        let tx = self.spawn_org_driver(workflow, driver);
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

    fn spawn_org_driver(self: &Arc<Self>, workflow: WorkflowId, driver: OrgDriver<Arc<Host>>) -> UnboundedSender<DriverMsg> {
        let (tx, rx) = unbounded_channel();
        *self.org_rt.driver.lock().unwrap() = Some(DriverHandle { workflow, tx: tx.clone() });
        tokio::spawn(driver.run(rx));
        self.org_apply_stall_setting();
        tx
    }
}

/// `<root>\org\` の中で最も新しいワークフローのフォルダ（IDは `wf-<ms>` なので名前順＝作成順）。
fn newest_workflow_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("org")).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.pop()
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
}
