//! 削除・アーカイブ・Markdownエクスポート・使用量（P7、DESIGN_P2 §6、要件§3.6・§3.12・M33・M41・M43・M46）。
//!
//! 規則:
//! - 判断は純粋ロジック（`rules::manage`・`rules::export`）。ここは状態・保存・Codex呼出し・ファイルをつなぐ。
//! - 停止を確認できないまま削除しない。作業中なら中断して停止を照合し、確認できなければ削除保留（再起動後も保持、送信禁止）。
//!   停止確認後も自動では削除せず、ユーザーが再度「削除」を選んだときだけ実行する。
//! - 実行順は Codex の履歴削除 → チャット専用領域の削除。どちらかが部分失敗なら完了と偽らず保留を残す。
//!   外部作成の会話は Codex 側を削除しない（参照扱い、M36）。アプリの補足情報だけ消して一覧から外す。
//! - 領域の外（元ファイル・別途保存したファイル・プロジェクト内ファイル・他チャット）には触れない。
//! - 作業中のアーカイブは、アプリの一覧から即座に隠し、Codex の `archive` は作業終了・停止確認の後に送る。
//!   送るための証票（`UserConfirmed`）はユーザーのアーカイブ操作のもので、アプリを再起動すると失われる（自動では送らない）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::lifecycle::work_of;
use super::state::{agent_key_of, HistoryTail, HostData};
use super::{blocked, err, now_ms, stop, Host};
use crate::backend::backend::*;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::rules::export::{render_markdown, ExportInput};
use crate::rules::manage::{archive_ready, backend_delete_target_missing,delete_decision, delete_result, refresh_delete_pending, save_blocks_chat, ChatWork, DeleteDecision};
use crate::store::records::ChatLocalFile;
use crate::store::{atomic, layout, legacy_area_usage, StoreError};

/// 管理操作の作業状態（保存しない）。
#[derive(Default)]
pub struct ManageRuntime {
    /// アーカイブ要求のときの証票。作業終了後に Codex へ `archive` を送るために保持する。再起動で失われる。
    archive_tokens: Mutex<HashMap<ChatKey, UserConfirmed>>,
    /// 削除を実行中のチャット（二重実行と、その間の送信を防ぐ）。
    deleting: Mutex<HashSet<ChatKey>>,
}

impl ManageRuntime {
    pub fn is_deleting(&self, chat: &ChatKey) -> bool {
        self.deleting.lock().unwrap().contains(chat)
    }
}

/// 管理の監視間隔（アーカイブ送信の可否・削除保留の更新）。
const MANAGE_TICK: Duration = Duration::from_secs(2);
/// 削除のための中断後、停止を待つ時間（停止確認の期限＋余裕）。
const STOP_WAIT_MS: u64 = stop::STOP_CONFIRM_DEADLINE_MS as u64 + 1_500;

impl HostData {
    /// 削除が完了したチャットのアプリ側の状態をすべて消す。
    pub(super) fn forget_deleted_chat(&mut self, chat: &ChatKey) -> Vec<HostEvent> {
        let agents: Vec<AgentKey> = self.agents.iter().filter(|v| &v.agent.chat == chat).map(|v| v.agent.key.clone()).collect();
        for a in &agents {
            self.running_turn.remove(a);
            self.last_end.remove(a);
        }
        self.stops.retain(|r| &r.chat != chat);
        // 主会話に属するside相談の会話（一時の分岐）の状態も消す（記録ファイルはチャット領域ごと消える）。
        let sides: Vec<ChatKey> = self.side_threads.iter().filter(|(_, m)| *m == chat).map(|(t, _)| t.clone()).collect();
        for t in &sides {
            self.side_threads.remove(t);
            self.stops.retain(|r| &r.chat != t);
            self.requests.retain(|r| &r.chat != t);
            for a in self.agents.iter().filter(|v| &v.agent.chat == t).map(|v| v.agent.key.clone()).collect::<Vec<_>>() {
                self.running_turn.remove(&a);
                self.last_end.remove(&a);
            }
            self.agents.retain(|v| &v.agent.chat != t);
        }
        self.pinned.remove(chat);
        self.hosted.remove(chat);
        self.model_settings.remove(chat);
        self.locals.remove(chat);
        self.queues.remove(chat);
        self.save_status.retain(|s, _| !scope_is_of(s, chat));
        self.remove_chat(chat)
    }

    /// 外部作成の会話を「一覧から外す」。補足情報（ピン・下書き・添付・キュー等）は消し、元の履歴には触れない。
    fn reset_removed_from_list(&mut self, chat: &ChatKey, new_dir: LocalId, at: UnixMillis) -> Vec<HostEvent> {
        self.stops.retain(|r| &r.chat != chat);
        self.pinned.remove(chat);
        self.model_settings.remove(chat);
        self.queues.remove(chat);
        self.save_status.retain(|s, _| !scope_is_of(s, chat));
        let mut file = ChatLocalFile::new(new_dir, Some(chat.clone()));
        file.visibility = ListVisibility::RemovedFromList { at };
        self.locals.insert(chat.clone(), file);
        self.local_view(chat).map(|local| HostEvent::ChatLocalUpdated { local }).into_iter().collect()
    }
}

fn scope_is_of(s: &SaveScope, chat: &ChatKey) -> bool {
    matches!(s, SaveScope::ChatLocal { chat: c } | SaveScope::Queue { chat: c } | SaveScope::Activity { chat: c } if c == chat)
}

impl Host {
    /// 管理の監視を始める（`start_event_pump` から1回）。
    pub(super) fn start_manage_watch(self: &Arc<Self>) {
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(MANAGE_TICK).await;
                host.confirm_unknown_terminals(None, false).await;
                host.drive_archives();
                host.refresh_delete_pendings().await;
            }
        });
    }

    /// 状態が不明（notLoaded等）で live でないエージェントの最新turnの末尾を、履歴の読取りで確認する（読み取りのみ。resumeしない）。
    /// 終端（またはturnなし）を確認できたものは、終了・削除・アーカイブの判断で未完了と数えない（`work_of`）。
    /// 読めなかったものは何も記録せず、未完了のまま。`only` 指定はそのチャットだけ、`force` は確認済みも読み直す。
    pub(super) async fn confirm_unknown_terminals(self: &Arc<Self>, only: Option<&ChatKey>, force: bool) {
        if !self.is_connected() {
            return;
        }
        let now = now_ms();
        let limit = if only.is_some() { 64 } else { 8 };
        let cands: Vec<AgentKey> = self.read(|d| {
            d.agents
                .iter()
                .filter(|v| v.status.state == AgentState::Unknown && !matches!(v.freshness, Freshness::Live | Freshness::Unsupported))
                .filter(|v| only.is_none_or(|c| &v.agent.chat == c))
                .filter(|v| {
                    force
                        || match d.history_terminal.get(&v.agent.key) {
                            None => true,
                            // 進行中と読めたものは、終わったかを一定間隔で見直す。終端を確認済みのものは、新しいturnが来るまで読み直さない。
                            Some((HistoryTail::NotTerminal, at)) => now.0 - at.0 >= 10_000,
                            Some(_) => false,
                        }
                })
                .map(|v| v.agent.key.clone())
                .take(limit)
                .collect()
        });
        for agent in cands {
            let Ok(h) = self.read_history(agent.clone(), ReadOptions { include_turns: true }).await else { continue };
            let tail = super::state::history_tail_of(&h.turns);
            self.mutate(|d| {
                let ev = d.note_history_tail(&agent, tail, now_ms());
                ((), ev)
            });
        }
    }

    /// 作業状況（受理不明の送信を含む）。
    fn chat_work(&self, chat: &ChatKey) -> ChatWork {
        let mut w = self.read(|d| work_of(d, chat));
        w.unresolved_send = self.unresolved.lock().unwrap().blocking(chat).is_some();
        w
    }

    fn chat_origin(&self, chat: &ChatKey) -> Option<ChatOrigin> {
        self.read(|d| {
            d.chat(chat).map(|c| c.origin).or_else(|| d.locals.get(chat).and_then(|l| l.cached_meta.as_ref().map(|m| m.origin))).filter(|o| *o != ChatOrigin::Unknown)
        })
    }

    fn local_view_of(&self, chat: &ChatKey) -> Option<ChatLocalView> {
        self.read(|d| d.local_view(chat))
    }

    /// 送信を止める削除保留があるか（`send_message` が確認する）。
    pub(super) fn check_not_delete_pending(&self, chat: &ChatKey) -> Result<(), IpcError> {
        if self.manage_rt.is_deleting(chat) || self.read(|d| d.locals.get(chat).is_some_and(|l| l.delete_pending.is_some())) {
            return Err(blocked(BlockedReason::DeletePending, "このチャットは削除の保留中です。停止を確認して削除を完了するまで、新しい送信は止めています"));
        }
        Ok(())
    }

    // ───────────────────────── アーカイブ（M43） ─────────────────────────

    /// アプリの一覧から即座に隠す。作業・通知・キューは継続し、Codex の `archive` は作業終了・停止確認の後に送る。
    pub async fn archive_chat(self: &Arc<Self>, args: ChatArgs, confirmed: UserConfirmed) -> Result<ChatLocalView, IpcError> {
        let chat = args.chat;
        self.check_not_delete_pending(&chat)?;
        self.precheck_space()?;
        let (known, visibility) = self.read(|d| (d.chat(&chat).is_some() || d.locals.contains_key(&chat), d.locals.get(&chat).map(|l| l.visibility.clone())));
        if !known {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let sync = if self.backend.capabilities().archive == Support::Unsupported { ArchiveSync::Unsupported } else { ArchiveSync::WaitingForWorkEnd };
        match visibility {
            Some(ListVisibility::Archived { sync: ArchiveSync::Synced { .. }, .. }) => {
                if let Some(v) = self.local_view_of(&chat) {
                    return Ok(v);
                }
            }
            Some(ListVisibility::RemovedFromList { .. }) => return Err(err(IpcErrorCode::InvalidArgs, "一覧から外したチャットはアーカイブできません")),
            _ => {}
        }
        if sync == ArchiveSync::WaitingForWorkEnd {
            self.manage_rt.archive_tokens.lock().unwrap().insert(chat.clone(), confirmed);
        }
        let now = now_ms();
        self.update_local(&chat, true, Duration::ZERO, |f| {
            let at = match &f.visibility {
                ListVisibility::Archived { at, .. } => *at,
                _ => now,
            };
            f.visibility = ListVisibility::Archived { at, sync };
        });
        self.drive_archives();
        self.local_view_of(&chat).ok_or_else(|| err(IpcErrorCode::NotFound, "チャットが見つかりません"))
    }

    /// 作業終了・停止確認がそろった `WaitingForWorkEnd` のアーカイブを Codex へ反映する（証票を持つものだけ）。
    pub(super) fn drive_archives(self: &Arc<Self>) {
        let waiting: Vec<ChatKey> = self.read(|d| {
            d.locals.iter().filter(|(_, l)| matches!(l.visibility, ListVisibility::Archived { sync: ArchiveSync::WaitingForWorkEnd, .. })).map(|(k, _)| k.clone()).collect()
        });
        for chat in waiting {
            if !self.manage_rt.archive_tokens.lock().unwrap().contains_key(&chat) || !archive_ready(&self.chat_work(&chat)) {
                continue;
            }
            // 通常の接続が無いうちは送らない（接続後の次の周期で送る）。
            if !self.read(|d| d.root_view(&chat).is_some_and(|v| v.freshness == Freshness::Live || v.freshness == Freshness::HistoryOnly)) {
                continue;
            }
            let Some(token) = self.manage_rt.archive_tokens.lock().unwrap().remove(&chat) else { continue };
            self.set_archive_sync(&chat, ArchiveSync::Sending);
            let host = self.clone();
            tokio::spawn(async move {
                let result = host.backend.manage_chat(chat.clone(), ManageOp::Archive, &token).await;
                let now = now_ms();
                match result {
                    Ok(ManageOutcome::Done) => {
                        host.set_archive_sync(&chat, ArchiveSync::Synced { at: now });
                        host.mutate(|d| ((), set_codex_archived(d, &chat, true)));
                    }
                    Ok(ManageOutcome::Partial { failed, .. }) => {
                        host.set_archive_sync(&chat, ArchiveSync::Failed { message: format!("一部を反映できませんでした: {}", failed.join("、")), at: now });
                    }
                    Err(e) => {
                        // 結果不明（応答なし）でも、反映できたとは表示しない。アプリ上はアーカイブのまま、再試行できる。
                        host.set_archive_sync(&chat, ArchiveSync::Failed { message: e.to_string(), at: now });
                    }
                }
            });
        }
    }

    fn set_archive_sync(self: &Arc<Self>, chat: &ChatKey, sync: ArchiveSync) {
        self.update_local(chat, true, Duration::ZERO, |f| {
            if let ListVisibility::Archived { at, .. } = f.visibility {
                f.visibility = ListVisibility::Archived { at, sync };
            }
        });
    }

    /// アーカイブを解除する。Codex へ未送信なら取り消すだけ、送信済み（または結果不明）なら `unarchive` を送る。
    pub async fn unarchive_chat(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<ChatLocalView, IpcError> {
        let chat = args.chat;
        self.precheck_space()?;
        let (visibility, codex_archived) = self.read(|d| (d.locals.get(&chat).map(|l| l.visibility.clone()), d.chat(&chat).is_some_and(|c| c.archived.value() == Some(&true))));
        let needs_codex = match &visibility {
            Some(ListVisibility::Archived { sync, .. }) => match sync {
                ArchiveSync::WaitingForWorkEnd | ArchiveSync::Unsupported => {
                    self.manage_rt.archive_tokens.lock().unwrap().remove(&chat);
                    codex_archived
                }
                ArchiveSync::Sending => return Err(err(IpcErrorCode::Rejected, "Codexへアーカイブを反映している最中です。完了してからもう一度お試しください")),
                ArchiveSync::Synced { .. } | ArchiveSync::Failed { .. } => true,
            },
            Some(ListVisibility::RemovedFromList { .. }) => return Err(err(IpcErrorCode::InvalidArgs, "一覧から外したチャットは解除できません")),
            _ => codex_archived,
        };
        if needs_codex {
            self.backend.manage_chat(chat.clone(), ManageOp::Unarchive, confirmed).await?;
            self.mutate(|d| ((), set_codex_archived(d, &chat, false)));
        }
        self.update_local(&chat, true, Duration::ZERO, |f| f.visibility = ListVisibility::Visible);
        self.local_view_of(&chat).ok_or_else(|| err(IpcErrorCode::NotFound, "チャットが見つかりません"))
    }

    // ───────────────────────── 削除（M33・M46） ─────────────────────────

    /// 削除確認に出す内容。実行はしない。
    pub async fn preview_delete(self: &Arc<Self>, args: ChatArgs) -> Result<DeletePreview, IpcError> {
        let chat = args.chat;
        let origin = self.chat_origin(&chat).ok_or_else(|| err(IpcErrorCode::NotFound, "チャットの種別を確認できません"))?;
        let work = self.chat_work(&chat);
        let (descendants, attachments, artifacts, pending) = self.read(|d| {
            let desc = if d.root_view(&chat).is_some() {
                let n = d.agents.iter().filter(|v| v.agent.chat == chat && !d.is_root(&v.agent.key)).count();
                Known::Value { value: n as u32, basis: Basis::Derived }
            } else {
                Known::NotFetched
            };
            let l = d.locals.get(&chat);
            (
                desc,
                l.map(|l| l.attachments.len()).unwrap_or(0) as u32,
                l.map(|l| l.artifacts.iter().filter(|a| a.in_chat_area).count()).unwrap_or(0) as u32,
                l.and_then(|l| l.delete_pending.clone()),
            )
        });
        let chat_area_bytes = match self.persist.store().cloned() {
            Some(store) => {
                let c = chat.clone();
                match tokio::task::spawn_blocking(move || store.chat_area_bytes(&c)).await {
                    Ok(Some(n)) => Known::direct(n),
                    _ => Known::NotFetched,
                }
            }
            None => Known::NotFetched,
        };
        let (worktree_path, worktree_removed) = self.chat_worktree_path(&chat).unwrap_or((None, false));
        Ok(DeletePreview {
            chat,
            deletes_backend_history: origin != ChatOrigin::External,
            descendants,
            attachments,
            artifacts_in_chat_area: artifacts,
            chat_area_bytes,
            requires_stop: delete_decision(&work, pending.as_ref()) != DeleteDecision::Proceed,
            worktree_path,
            worktree_removed,
        })
    }

    /// 削除を実行する（ユーザーが確認画面で選んだ後だけ）。結果は `DeleteOutcome`。停止を確認できなければ `Pending`。
    pub async fn delete_chat(self: &Arc<Self>, args: ChatArgs, confirmed: &UserConfirmed) -> Result<DeleteOutcome, IpcError> {
        let chat = args.chat;
        if !self.manage_rt.deleting.lock().unwrap().insert(chat.clone()) {
            return Err(err(IpcErrorCode::Rejected, "このチャットの削除を実行中です"));
        }
        // 控えの取得（B・E）と同じ作業キューで排他し、削除後に控えの領域が再作成されないようにする。
        let queue = self.baseline_rt.queue_lock(&chat);
        let _baseline_q = queue.lock().await;
        let res = self.delete_inner(&chat, confirmed).await;
        self.manage_rt.deleting.lock().unwrap().remove(&chat);
        res
    }

    async fn delete_inner(self: &Arc<Self>, chat: &ChatKey, confirmed: &UserConfirmed) -> Result<DeleteOutcome, IpcError> {
        let origin = self.chat_origin(chat).ok_or_else(|| err(IpcErrorCode::NotFound, "チャットの種別を確認できません"))?;
        // 保存に失敗している内容があれば、削除（と保留の記録）を完了と見せない。
        if let Some(scope) = self.read(|d| save_blocks_chat(&d.save_status.values().cloned().collect::<Vec<_>>(), chat)) {
            return Err(blocked(
                BlockedReason::SaveFailed { scope },
                "このチャットの保存に失敗している内容があります。保存を再試行して成功を確認してから削除してください",
            ));
        }
        // 状態不明（notLoaded等）のエージェントは、履歴で終端を確認し直してから判断する（確認できなければ未完了のまま）。
        self.confirm_unknown_terminals(Some(chat), true).await;
        let mut work = self.chat_work(chat);
        let mut pending = self.read(|d| d.locals.get(chat).and_then(|l| l.delete_pending.clone()));
        // 前回の保留を、現在の停止状況で更新してから判断する（確認できていれば、ユーザーの再操作を受けて削除へ進める）。
        if let Some(p) = pending.as_mut() {
            if self.pending_refreshable(chat) && refresh_delete_pending(p, &work) {
                let p = p.clone();
                self.update_local(chat, true, Duration::ZERO, |f| f.delete_pending = Some(p));
            }
        }
        let mut decision = delete_decision(&work, pending.as_ref());
        if decision == DeleteDecision::StopFirst {
            if work.has_unfinished || work.queue_in_flight {
                // 中断の間も送信・キュー送信を止める（保留の記録を先に残す）。
                self.set_delete_pending(chat, DeletePendingReason::StopUnconfirmed, work.stop_unconfirmed.clone()).await;
                if work.has_unfinished {
                    if let Err(e) = self.interrupt_chat(InterruptChatArgs { chat: chat.clone() }, confirmed).await {
                        self.warn(format!("削除のための中断要求を送れませんでした: {}", e.message));
                    }
                }
                work = self.wait_for_stop(chat).await;
                // 削除すれば登録だけの送信待ち（Waiting）は消える。送信中・終端待ちは停止の確認の対象なので残す。
                work.queue_pending = work.queue_in_flight;
                decision = delete_decision(&work, None);
                if decision == DeleteDecision::StopFirst {
                    decision = DeleteDecision::Pend(DeletePendingReason::StopUnconfirmed);
                }
            } else {
                // 登録済み（Waiting）のキューだけが残っている。削除すれば消える。
                decision = DeleteDecision::Proceed;
            }
        }
        match decision {
            DeleteDecision::Pend(reason) => {
                let p = self.set_delete_pending(chat, reason, work.stop_unconfirmed.clone()).await;
                Ok(DeleteOutcome::Pending { pending: p })
            }
            DeleteDecision::Proceed => self.execute_delete(chat, origin, pending, confirmed).await,
            DeleteDecision::StopFirst => Err(err(IpcErrorCode::Rejected, "停止を確認できていないため、削除できません")),
        }
    }

    /// 停止の再確認に使える観測があるか（接続・履歴の観測がないチャットは、確認できたと見なさない）。
    fn pending_refreshable(&self, chat: &ChatKey) -> bool {
        self.read(|d| d.root_view(chat).is_some_and(|v| matches!(v.freshness, Freshness::Live | Freshness::HistoryOnly)))
    }

    /// 削除保留を記録する（理由を更新。依頼時刻は最初のまま）。保存できなければ警告する。
    async fn set_delete_pending(self: &Arc<Self>, chat: &ChatKey, reason: DeletePendingReason, stop_record: Option<LocalId>) -> DeletePending {
        let now = now_ms();
        let mut out = None;
        self.update_local(chat, true, Duration::ZERO, |f| {
            let requested_at = f.delete_pending.as_ref().map(|p| p.requested_at).unwrap_or(now);
            let p = DeletePending { requested_at, reason, stop_record };
            f.delete_pending = Some(p.clone());
            out = Some(p);
        });
        if let Some(st) = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await {
            if matches!(st.state, SaveState::SaveFailed { .. }) {
                self.warn("削除保留を保存できていません。再起動すると保留が失われます。保存を再試行してください。");
            }
        }
        out.expect("set by the closure")
    }

    /// 中断後、停止を確認できる（または期限になる）まで待つ。時間が過ぎただけでは停止を確認したことにしない。
    async fn wait_for_stop(self: &Arc<Self>, chat: &ChatKey) -> ChatWork {
        let deadline = Instant::now() + Duration::from_millis(STOP_WAIT_MS);
        loop {
            self.mutate(|d| ((), d.refresh_stops(now_ms())));
            let w = self.chat_work(chat);
            if !w.has_unfinished && w.stop_unconfirmed.is_none() && !w.ownership_unknown && !w.unresolved_send && !w.queue_in_flight {
                return w;
            }
            if Instant::now() >= deadline {
                return w;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn execute_delete(self: &Arc<Self>, chat: &ChatKey, origin: ChatOrigin, prior: Option<DeletePending>, confirmed: &UserConfirmed) -> Result<DeleteOutcome, IpcError> {
        let external = origin == ChatOrigin::External;
        let mut done: Vec<DeleteStep> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        match prior.as_ref().map(|p| &p.reason) {
            Some(DeletePendingReason::PartialFailure { done: d, .. }) => done = d.clone(),
            // 実行中に落ちても分かるよう、保留の記録（再操作待ち）を先に残す。
            _ => {
                self.set_delete_pending(chat, DeletePendingReason::ReadyForUserRetry, None).await;
            }
        }
        if !external && !done.iter().any(DeleteStep::backend_settled) {
            match self.backend.manage_chat(chat.clone(), ManageOp::Delete, confirmed).await {
                Ok(ManageOutcome::Done) => done.push(DeleteStep::BackendHistory),
                Ok(ManageOutcome::Partial { done: d, failed: f }) => {
                    done.extend(d.into_iter().map(|label| DeleteStep::BackendItem { label }));
                    failed.extend(f);
                }
                // Codex に履歴が無い（発話前のチャットなど）。消す対象がないので、Codex 側は完了扱いにして領域の削除へ進む。
                Err(e) if backend_delete_target_missing(&e) => done.push(DeleteStep::BackendHistoryNone),
                Err(e) => failed.push(format!("履歴の削除に失敗しました（{e}）")),
            }
        }
        if failed.is_empty() {
            let host = self.clone();
            let c = chat.clone();
            let r = tokio::task::spawn_blocking(move || host.remove_area(&c, external)).await.unwrap_or_else(|e| Err((Vec::new(), vec![format!("削除処理が異常終了しました: {e}")])));
            match r {
                Ok(()) => done.push(DeleteStep::ChatArea),
                Err((d, f)) => {
                    done.extend(d.into_iter().map(|name| DeleteStep::AreaItem { name }));
                    failed.extend(f);
                }
            }
        }
        match delete_result(done, failed) {
            DeleteOutcome::Deleted => {
                if external {
                    // 一覧から外した印（RemovedFromList）を保存する。保存できなければ再起動後に一覧へ戻る。
                    if let Some(st) = self.save_now(SaveScope::ChatLocal { chat: chat.clone() }).await {
                        if matches!(st.state, SaveState::SaveFailed { .. }) {
                            self.warn("一覧から外した印を保存できていません。再起動すると一覧に戻ります。保存を再試行してください。");
                        }
                    }
                }
                Ok(DeleteOutcome::Deleted)
            }
            DeleteOutcome::Partial { done, failed } => {
                // 完了と偽らない。保留を残し、再度の「削除」で残りを実行できる。
                let _ = self.set_delete_pending(chat, DeletePendingReason::PartialFailure { done: done.clone(), failed: failed.clone() }, None).await;
                Ok(DeleteOutcome::Partial { done, failed })
            }
            pending @ DeleteOutcome::Pending { .. } => Ok(pending),
        }
    }

    /// チャット専用領域を消し、アプリ側の状態を片付ける（`spawn_blocking` から）。書込みと並ばないよう保存のロックを持つ。
    /// 領域の削除に失敗したら、状態は残す（保留を残して再実行できる）。
    fn remove_area(self: &Arc<Self>, chat: &ChatKey, external: bool) -> Result<(), (Vec<String>, Vec<String>)> {
        let store = self.persist.store().cloned();
        let _g = self.persist.io_guard();
        if let Some(store) = &store {
            store.remove_chat_dir(chat)?;
        }
        self.persist.forget_chat(chat);
        if external {
            let new_dir = self.persist.dir_id_for(chat);
            let at = now_ms();
            self.mutate(|d| ((), d.reset_removed_from_list(chat, new_dir, at)));
        } else {
            self.mutate(|d| ((), d.forget_deleted_chat(chat)));
        }
        Ok(())
    }

    /// 削除保留の理由を、停止の状況に合わせて更新する（確認できても自動では削除しない）。
    async fn refresh_delete_pendings(self: &Arc<Self>) {
        let chats: Vec<ChatKey> = self.read(|d| {
            d.locals
                .iter()
                .filter(|(_, l)| l.delete_pending.as_ref().is_some_and(|p| !matches!(p.reason, DeletePendingReason::PartialFailure { .. })))
                .map(|(k, _)| k.clone())
                .collect()
        });
        for chat in chats {
            if self.manage_rt.is_deleting(&chat) || !self.pending_refreshable(&chat) {
                continue;
            }
            self.mutate(|d| ((), d.refresh_stops(now_ms())));
            let work = self.chat_work(&chat);
            let Some(mut p) = self.read(|d| d.locals.get(&chat).and_then(|l| l.delete_pending.clone())) else { continue };
            if refresh_delete_pending(&mut p, &work) {
                self.update_local(&chat, true, Duration::ZERO, |f| f.delete_pending = Some(p));
            }
        }
    }

    // ───────────────────────── エクスポート（M41） ─────────────────────────

    /// 会話本文を取り直して Markdown に書き出す。保存の成功を確認できてから成功を返す（D06）。
    pub async fn export_markdown(self: &Arc<Self>, args: ExportMarkdownArgs) -> Result<(), IpcError> {
        let dest = std::path::PathBuf::from(args.dest.trim());
        if args.dest.trim().is_empty() || !dest.is_absolute() {
            return Err(err(IpcErrorCode::InvalidArgs, "保存先のフルパスを指定してください"));
        }
        if dest.is_dir() {
            return Err(err(IpcErrorCode::InvalidArgs, "保存先にフォルダは指定できません"));
        }
        if dest.exists() && !args.overwrite_confirmed {
            return Err(blocked(BlockedReason::TargetExists { path: args.dest.clone() }, "同名のファイルがあります。上書きを確認してください"));
        }
        let chat = args.chat;
        // 本文はCodexの保存履歴から取り直す（アプリは本文を保存していない）。取得できなければ、空のファイルを成功にしない。
        let history = self.read_history(agent_key_of(&chat), ReadOptions { include_turns: true }).await?;
        let (name, attachments, artifacts) = self.read(|d| {
            let l = d.locals.get(&chat);
            (
                d.chat(&chat).map(|c| c.name.clone()).unwrap_or(Known::NotFetched),
                l.map(|l| l.attachments.clone()).unwrap_or_default(),
                l.map(|l| l.artifacts.clone()).unwrap_or_default(),
            )
        });
        let activity = if args.include_monitor_activity {
            Some(match self.persist.store().cloned() {
                Some(store) => {
                    let c = chat.clone();
                    tokio::task::spawn_blocking(move || store.read_activity(&c))
                        .await
                        .map_err(|e| err(IpcErrorCode::Io, format!("監視活動の読込みが異常終了しました: {e}")))?
                        .map_err(|e| err(IpcErrorCode::Io, format!("監視活動の履歴を読み込めません: {e}")))?
                }
                None => Vec::new(),
            })
        } else {
            None
        };
        let md = render_markdown(&ExportInput {
            chat_name: name,
            turns: &history.turns,
            transcript_complete: history.turns.iter().all(|t| t.complete),
            attachments: &attachments,
            artifacts: &artifacts,
            monitor_activity: activity.as_deref(),
            exported_at_local: crate::win::clock::local_time_text(),
        });
        let bytes = md.into_bytes();
        let needed = bytes.len() as u64;
        let store = self.persist.store().cloned();
        tokio::task::spawn_blocking(move || -> Result<(), IpcError> {
            if let Some(store) = store {
                if let Err(StoreError::InsufficientSpace { required, available }) = store.check_space(needed) {
                    return Err(blocked(BlockedReason::InsufficientSpace { required, available }, "保存先の空き容量が足りません。何も書き込んでいません"));
                }
            }
            atomic::write_atomic(&dest, &bytes).map_err(|e| err(IpcErrorCode::Io, format!("ファイルを書き込めませんでした: {e}")))
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("書込みが異常終了しました: {e}")))?
    }

    // ───────────────────────── 使用量（§3.12） ─────────────────────────

    /// 使用量の集計（背景で実行）。`chat` 指定ならそのチャットだけ。旧領域は全体のときだけ別に数える。
    pub async fn get_usage(self: &Arc<Self>, args: GetUsageArgs) -> Result<UsageReport, IpcError> {
        let Some(store) = self.persist.store().cloned() else {
            return Err(err(IpcErrorCode::Io, "保存先を使えていないため、使用量を集計できません"));
        };
        let chat = args.chat;
        let legacy = if chat.is_none() { legacy_chats_dir(&self.app_data_dir) } else { None };
        tokio::task::spawn_blocking(move || {
            let mut report = store.usage(chat.as_ref());
            if let Some(dir) = legacy {
                let (n, mut bad) = legacy_area_usage(&dir);
                report.legacy_area = n;
                report.unreadable.append(&mut bad);
            }
            report
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("集計が異常終了しました: {e}")))
    }
}

/// 段階①で作った一般チャットの作業領域（`%APPDATA%\<アプリ識別子>\chats`）。
fn legacy_chats_dir(app_local_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(roaming).join(app_local_dir.file_name()?).join(layout::CHATS_DIR))
}

/// 一覧用の `Chat.archived`（Codex側のarchive状態）を更新する。
fn set_codex_archived(d: &mut HostData, chat: &ChatKey, archived: bool) -> Vec<HostEvent> {
    match d.chats.iter_mut().find(|c| &c.key == chat) {
        Some(c) => {
            c.archived = Known::direct(archived);
            vec![HostEvent::ChatUpdated { chat: c.clone() }]
        }
        None => Vec::new(),
    }
}
