//! 保存の呼出しと保存状態（P2、要件§3.12・D06）。
//!
//! - 補足情報（ピン・下書き・モデル/権限など）の正本は `HostData.locals`。変更はここで反映し、保存は背景taskが行う。
//! - 保存は単位（[`SaveScope`]）ごとに直列化する。書込みの直前に最新の内容を読むので、連続した変更はまとまる。
//! - 失敗は自動で最大3回まで再試行し、以後はユーザーの「再試行」（`retry_save`）を待つ。空き不足は再試行しても変わらないので
//!   自動再試行しない。保存成功を確認できるまで「保存済み」にしない（`SaveStatus`／`saveStatusUpdated`）。
//! - 下書きだけ500msまとめて書く。終了時（`flush_pending`）に未保存の単位を書き切ろうとする。
//! - 保存先が使えない（`Store` が開けなかった・テスト）ときは、メモリ上の更新だけで保存は行わない（警告を出す）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{blocked, err, now_ms, Host};
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::host::state::HostData;
use crate::store::records::{ActivityLine, AppSettingsFile, ChatLocalFile};
use crate::store::{layout, Restored, Store, StoreError, AUTO_RETRY_LIMIT, DRAFT_DEBOUNCE_MS};

/// 保存の呼出しに必要な共有状態。
pub struct Persist {
    store: Option<Arc<Store>>,
    /// 書込みを1本に直列化する。取得順は常に「このロック → データのロック」。
    io_lock: Mutex<()>,
    /// 単位ごとの世代。新しい保存要求が入ったら進め、古いtaskの結果が新しい状態を「保存済み」にしないようにする。
    generation: Mutex<HashMap<SaveScope, u64>>,
    counter: AtomicU64,
    /// 監視活動（`activity.jsonl`）の追記を、書込みtaskへ順に渡す口。taskを始めるまで（保存先なし・テスト）は捨てる。
    activity_tx: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<(ChatKey, ActivityLine)>>,
}

impl Persist {
    pub fn new(store: Option<Arc<Store>>) -> Self {
        Persist { store, io_lock: Mutex::new(()), generation: Mutex::new(HashMap::new()), counter: AtomicU64::new(1), activity_tx: std::sync::OnceLock::new() }
    }

    pub(super) fn submit_activity(&self, lines: Vec<(ChatKey, ActivityLine)>) {
        if let Some(tx) = self.activity_tx.get() {
            for l in lines {
                let _ = tx.send(l);
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.store.is_some()
    }

    pub(super) fn store(&self) -> Option<&Arc<Store>> {
        self.store.as_ref()
    }

    /// 書込みの直列化ロック（チャット領域の削除中に、同じ領域への書込みを入れないため。取得順は「このロック → データのロック」）。
    pub(super) fn io_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.io_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 削除したチャットの保存単位の世代を進める（実行中・待機中の保存が、結果を状態へ書かないようにする）。
    pub(super) fn forget_chat(&self, chat: &ChatKey) {
        for scope in [SaveScope::ChatLocal { chat: chat.clone() }, SaveScope::Queue { chat: chat.clone() }, SaveScope::Activity { chat: chat.clone() }] {
            self.bump(&scope);
        }
    }

    fn bump(&self, scope: &SaveScope) -> u64 {
        let mut g = self.generation.lock().unwrap();
        let n = g.entry(scope.clone()).or_insert(0);
        *n += 1;
        *n
    }

    fn is_current(&self, scope: &SaveScope, gen: u64) -> bool {
        self.generation.lock().unwrap().get(scope).copied() == Some(gen)
    }

    /// チャット領域のID（保存先が使えるときは保存層の割当て。使えないときはメモリ上だけの仮ID）。
    pub(super) fn dir_id_for(&self, chat: &ChatKey) -> LocalId {
        match &self.store {
            Some(s) => s.ensure_dir_id(chat),
            None => LocalId(format!("dir-mem-{}", self.counter.fetch_add(1, Ordering::SeqCst))),
        }
    }
}

/// 読めなかった保存ファイルの警告文。
pub fn problem_text(e: &StoreError) -> String {
    match e {
        StoreError::Corrupt { path, moved_to: Some(to) } => {
            format!("保存ファイルを読めなかったため退避しました（削除・上書きはしていません）: {path} → {to}")
        }
        StoreError::Corrupt { path, moved_to: None } => format!("保存ファイルを読めず、退避もできませんでした。このファイルは上書きしません: {path}"),
        StoreError::NewerSchema { path, .. } => format!("新しい版で作られた保存ファイルです。この版では読まず、上書きもしません: {path}"),
        StoreError::InsufficientSpace { .. } => "保存に必要な空き容量が足りません。".to_string(),
        StoreError::Io(e) => format!("保存ファイルを読めませんでした: {e}"),
    }
}

/// 保存失敗の案内文（原因別）。
pub fn save_failure_message(e: &StoreError) -> String {
    const MIB: u64 = 1024 * 1024;
    match e {
        StoreError::InsufficientSpace { required, available } => format!(
            "空き容量が足りません（必要 約{}MB、空き 約{}MB）。容量を空けてから「再試行」してください。保存データは自動では削除しません。",
            required.div_ceil(MIB),
            available / MIB
        ),
        StoreError::Io(e) => format!("書き込めませんでした: {e}"),
        StoreError::Corrupt { .. } => "読めなかった保存ファイルが残っているため、上書きしません。".to_string(),
        StoreError::NewerSchema { .. } => "新しい版で作られた保存ファイルのため、上書きしません。".to_string(),
    }
}

fn unsaved_label(scope: &SaveScope) -> &'static str {
    match scope {
        SaveScope::AppSettings => "アプリ設定の直近の変更",
        SaveScope::WindowBounds => "窓の位置・サイズ",
        SaveScope::ChatLocal { .. } => "このチャットのピン・下書き・モデル選択などの直近の変更",
        SaveScope::Queue { .. } => "このチャットの送信待ちの直近の変更",
        SaveScope::Activity { .. } => "監視活動の履歴",
    }
}

fn failed_state(scope: &SaveScope, e: &StoreError) -> SaveState {
    SaveState::SaveFailed {
        message: save_failure_message(e),
        saved_part: Some("前回までに保存できた内容".into()),
        unsaved_part: Some(unsaved_label(scope).into()),
    }
}

// ───────────────────────────── HostData への反映 ─────────────────────────────

impl HostData {
    /// 補足情報をUIへ出す形にする。保存状態は単位の状態から導く（記録がなければ未保存）。
    pub fn local_view(&self, chat: &ChatKey) -> Option<ChatLocalView> {
        let f = self.locals.get(chat)?;
        let save = self.save_status.get(&SaveScope::ChatLocal { chat: chat.clone() }).map(|s| s.state.clone()).unwrap_or(SaveState::Unsaved);
        Some(ChatLocalView {
            chat: chat.clone(),
            pinned: f.pinned,
            last_used_at: f.last_used_at,
            model: f.model.clone(),
            permission: f.permission,
            next_cwd: f.next_cwd.clone(),
            draft: f.draft.clone(),
            visibility: f.visibility.clone(),
            delete_pending: f.delete_pending.clone(),
            marks: self.marks_of(chat),
            acknowledged_failures: f.acknowledged_failures.clone(),
            save,
        })
    }

    pub fn local_views(&self) -> Vec<ChatLocalView> {
        let mut v: Vec<ChatLocalView> = self.locals.keys().filter_map(|k| self.local_view(k)).collect();
        v.sort_by(|a, b| a.chat.id.0.cmp(&b.chat.id.0));
        v
    }

    /// 起動時の復元。送信・再送・キュー再開はしない（キューは変換済みの記録を持つだけ）。
    pub fn restore(&mut self, r: Restored, now: UnixMillis) {
        let saved = |scope: SaveScope| SaveStatus { scope, state: SaveState::Saved { at: now }, last_attempt_at: None, retries: 0 };
        if let Some(s) = r.settings {
            self.settings = s.settings;
            self.save_status.insert(SaveScope::AppSettings, saved(SaveScope::AppSettings));
        }
        for c in r.chats {
            let Some(key) = c.local.chat.clone() else { continue };
            if c.local.pinned {
                self.pinned.insert(key.clone());
            }
            if let Some(model) = &c.local.model {
                // 選択値だけを戻す。受理値・実効値は未取得のまま（適用済みとは表示しない）。
                self.model_settings.insert(
                    key.clone(),
                    ChatModelSettings { selected: Some(model.clone()), accepted: Known::NotFetched, effective: Known::NotFetched, applies: ApplyTiming::NextTurn },
                );
            }
            self.save_status.insert(SaveScope::ChatLocal { chat: key.clone() }, saved(SaveScope::ChatLocal { chat: key.clone() }));
            if let Some(q) = c.queue {
                self.save_status.insert(SaveScope::Queue { chat: key.clone() }, saved(SaveScope::Queue { chat: key.clone() }));
                self.queues.insert(key.clone(), q);
            }
            self.locals.insert(key, c.local);
        }
        self.startup_warnings.extend(r.problems.iter().map(problem_text));
    }
}

// ───────────────────────────── Host ─────────────────────────────

impl Host {
    /// 保存層から復元した状態でホストを作る。
    pub fn with_store(app_data_dir: std::path::PathBuf, store: Arc<Store>) -> Self {
        let restored = store.load_all();
        let exe = restored.settings.as_ref().and_then(|s| s.settings.codex_executable.clone()).filter(|e| !e.trim().is_empty());
        let mut host = Host::new(app_data_dir);
        host.persist = Persist::new(Some(store));
        host.restore_windows(restored.windows.clone());
        host.data.get_mut().unwrap().restore(restored, now_ms());
        // 受理不明の送信は、接続後に照合を続ける（再送しない）。
        host.restore_unresolved();
        if let Some(e) = exe {
            *host.executable.get_mut().unwrap() = e.trim().to_string();
        }
        host
    }

    pub fn add_startup_warning(&self, message: impl Into<String>) {
        self.data.lock().unwrap().startup_warnings.push(message.into());
    }

    /// 書込みに必要な空きがなければ、対象の操作を止めて案内する（D02）。空きを取得できないときは止めない。
    pub(super) fn precheck_space(&self) -> Result<(), IpcError> {
        let Some(store) = &self.persist.store else { return Ok(()) };
        match store.check_space(64 * 1024) {
            Err(StoreError::InsufficientSpace { required, available }) => {
                Err(blocked(BlockedReason::InsufficientSpace { required, available }, save_failure_message(&StoreError::InsufficientSpace { required, available })))
            }
            _ => Ok(()),
        }
    }

    /// 一般チャットの作業フォルダを専用領域（`%LOCALAPPDATA%` 配下のチャット別領域）に作る。
    /// 戻り値は作業フォルダのパスと、結び付ける領域ID（保存先が使えないときは段階①と同じ場所・None）。
    pub(super) fn new_general_workspace(&self) -> Result<(String, Option<LocalId>), IpcError> {
        self.precheck_space()?;
        match &self.persist.store {
            Some(store) => {
                let (id, dir) = store.create_chat_dir().map_err(|e| err(IpcErrorCode::Io, format!("作業領域を作成できません: {e}")))?;
                Ok((layout::workspace_dir(&dir).to_string_lossy().into_owned(), Some(id)))
            }
            None => {
                let dir = self.app_data_dir.join("chats").join(format!("chat-{}", now_ms().0));
                std::fs::create_dir_all(&dir).map_err(|e| err(IpcErrorCode::Io, format!("作業領域を作成できません: {e}")))?;
                Ok((dir.to_string_lossy().into_owned(), None))
            }
        }
    }

    /// スレッドを開始できなかったとき、作った作業領域が空なら消す（中身があれば残す）。
    pub(super) fn discard_general_workspace(&self, area: &Option<LocalId>) {
        if let (Some(store), Some(id)) = (&self.persist.store, area) {
            let _ = store.discard_chat_dir_if_empty(id);
        }
    }

    /// 開始したチャットを、作業領域に結び付ける。
    pub(super) fn adopt_general_workspace(&self, chat: &ChatKey, area: &Option<LocalId>) {
        if let (Some(store), Some(id)) = (&self.persist.store, area) {
            store.register_dir(chat, id);
        }
    }

    /// 補足情報を更新して保存を依頼する。`emit` はUIへ `chatLocalUpdated` を出すか、`delay` は書込みまでのまとめ時間。
    pub(super) fn update_local(self: &Arc<Self>, chat: &ChatKey, emit: bool, delay: Duration, f: impl FnOnce(&mut ChatLocalFile)) {
        let dir_id = self.persist.dir_id_for(chat);
        self.mutate(|d| {
            let file = d.locals.entry(chat.clone()).or_insert_with(|| ChatLocalFile::new(dir_id, Some(chat.clone())));
            f(file);
            let ev: Vec<HostEvent> = if emit { d.local_view(chat).map(|local| HostEvent::ChatLocalUpdated { local }).into_iter().collect() } else { Vec::new() };
            ((), ev)
        });
        self.schedule_save(SaveScope::ChatLocal { chat: chat.clone() }, delay);
    }

    fn set_status(&self, scope: &SaveScope, state: SaveState, attempted: Option<UnixMillis>, retries: u32) -> SaveStatus {
        self.mutate(|d| {
            let st = d.save_status.entry(scope.clone()).or_insert_with(|| SaveStatus { scope: scope.clone(), state: SaveState::Unsaved, last_attempt_at: None, retries: 0 });
            let changed = st.state != state || st.retries != retries || attempted.is_some_and(|a| st.last_attempt_at != Some(a));
            st.state = state;
            st.retries = retries;
            if attempted.is_some() {
                st.last_attempt_at = attempted;
            }
            let status = st.clone();
            let mut ev = Vec::new();
            if changed {
                ev.push(HostEvent::SaveStatusUpdated { status: status.clone() });
                if let SaveScope::ChatLocal { chat } = scope {
                    if let Some(local) = d.local_view(chat) {
                        ev.push(HostEvent::ChatLocalUpdated { local });
                    }
                }
            }
            (status, ev)
        })
    }

    fn status_of(&self, scope: &SaveScope) -> Option<SaveStatus> {
        self.read(|d| d.save_status.get(scope).cloned())
    }

    /// 保存を依頼する。状態は書込みの成功を確認するまで「未保存」。`delay` が0でなければ、その間の追加の変更をまとめる。
    pub(super) fn schedule_save(self: &Arc<Self>, scope: SaveScope, delay: Duration) {
        if !self.persist.enabled() {
            return;
        }
        let retries = self.status_of(&scope).map(|s| s.retries).unwrap_or(0);
        self.set_status(&scope, SaveState::Unsaved, None, retries);
        let gen = self.persist.bump(&scope);
        let host = self.clone();
        let task = async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
                if !host.persist.is_current(&scope, gen) {
                    return;
                }
            }
            host.run_save(scope, gen, true).await;
        };
        // 窓イベントはtokioランタイム外（メインスレッド）から届くため、ランタイムが無ければTauriのものを使う。
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(task);
        } else {
            tauri::async_runtime::spawn(task);
        }
    }

    /// 書き終えるまで待つ保存（送信前に Sending を保存してから送る、など。再試行の待ちはしない）。
    /// 保存先が使えない（テスト等）ときは何もせず None を返す。
    pub(super) async fn save_now(self: &Arc<Self>, scope: SaveScope) -> Option<SaveStatus> {
        if !self.persist.enabled() {
            return None;
        }
        self.set_status(&scope, SaveState::Unsaved, None, 0);
        let gen = self.persist.bump(&scope);
        Some(self.clone().run_save(scope, gen, false).await)
    }

    /// 書込み（直列化）。最新の内容を読んでから書く。
    pub(super) fn write_scope(&self, scope: &SaveScope) -> Result<UnixMillis, StoreError> {
        let store = self.persist.store.as_ref().ok_or_else(|| StoreError::Io(std::io::Error::other("保存先が使えません")))?;
        let _g = self.persist.io_lock.lock().unwrap_or_else(|e| e.into_inner());
        let missing = || StoreError::Io(std::io::Error::other("保存する内容がありません"));
        match scope {
            SaveScope::ChatLocal { chat } => {
                let file = self.read(|d| d.locals.get(chat).cloned()).ok_or_else(missing)?;
                store.save_chat_local(&file)
            }
            SaveScope::AppSettings => {
                let settings = self.read(|d| d.settings.clone());
                store.save_settings(&AppSettingsFile::new(settings))
            }
            SaveScope::Queue { chat } => {
                let file = self.read(|d| d.queues.get(chat).cloned()).ok_or_else(missing)?;
                store.save_queue(&file)
            }
            SaveScope::WindowBounds => store.save_windows(&self.windows_file()),
            // 監視活動（追記）は別の担当タスクが書く。
            SaveScope::Activity { .. } => Err(StoreError::Io(std::io::Error::other("この単位の保存は別の経路で行います"))),
        }
    }

    /// 保存して結果を状態に反映する。一時的な失敗は最大 `AUTO_RETRY_LIMIT` 回まで間隔を空けて再試行する。
    /// より新しい保存要求が入っていたら、その要求のtaskに任せて結果を状態へ書かない。
    pub(super) async fn run_save(self: Arc<Self>, scope: SaveScope, gen: u64, auto_retry: bool) -> SaveStatus {
        let mut retries = 0u32;
        loop {
            let (host, sc) = (self.clone(), scope.clone());
            let res = tokio::task::spawn_blocking(move || host.write_scope(&sc)).await.unwrap_or_else(|e| Err(StoreError::Io(std::io::Error::other(e.to_string()))));
            let current = self.persist.is_current(&scope, gen);
            match res {
                Ok(at) => {
                    if !current {
                        return self.status_of(&scope).unwrap_or(SaveStatus { scope, state: SaveState::Unsaved, last_attempt_at: None, retries: 0 });
                    }
                    return self.set_status(&scope, SaveState::Saved { at }, Some(at), retries);
                }
                Err(e) => {
                    let now = now_ms();
                    if !current {
                        return self.status_of(&scope).unwrap_or(SaveStatus { scope, state: SaveState::Unsaved, last_attempt_at: None, retries: 0 });
                    }
                    if auto_retry && retries < AUTO_RETRY_LIMIT && matches!(e, StoreError::Io(_)) {
                        retries += 1;
                        self.set_status(&scope, SaveState::Unsaved, Some(now), retries);
                        tokio::time::sleep(Duration::from_millis(500u64 << (retries - 1))).await;
                        continue;
                    }
                    crate::diag::log("persist", &format!("save failed scope={} retries={retries} error={}", scope_label(&scope), error_kind(&e)));
                    return self.set_status(&scope, failed_state(&scope, &e), Some(now), retries);
                }
            }
        }
    }

    /// 終了時: 未保存・保存失敗の単位をもう一度書き切ろうとする（再試行の待ちはしない）。結果は状態に残る。
    pub async fn flush_pending(self: &Arc<Self>) {
        if !self.persist.enabled() {
            return;
        }
        let scopes: Vec<SaveScope> = self.read(|d| d.save_status.values().filter(|s| !matches!(s.state, SaveState::Saved { .. })).map(|s| s.scope.clone()).collect());
        for scope in scopes {
            let gen = self.persist.bump(&scope);
            self.clone().run_save(scope, gen, false).await;
        }
    }

    // ── 監視活動の追記（M5） ──

    /// 監視活動の書込みtaskを始める（イベントpump開始時に1回。保存先が使えないときは何もしない）。
    /// 行は届いた順に1本のtaskで追記する。失敗しても監視は止めず、失敗が続く間は1回だけ警告する（その区間の記録は欠ける）。
    pub(super) fn start_activity_writer(self: &Arc<Self>) {
        if !self.persist.enabled() {
            return;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        if self.persist.activity_tx.set(tx).is_err() {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            let mut failing = false;
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while batch.len() < 200 {
                    match rx.try_recv() {
                        Ok(x) => batch.push(x),
                        Err(_) => break,
                    }
                }
                let h = host.clone();
                let res = tokio::task::spawn_blocking(move || h.append_activity_batch(batch)).await.unwrap_or_else(|e| Err(StoreError::Io(std::io::Error::other(e.to_string()))));
                match res {
                    Ok(()) => failing = false,
                    Err(e) => {
                        if !failing {
                            failing = true;
                            host.warn(format!("監視活動の履歴を保存できませんでした（この間の記録は欠けます）: {}", save_failure_message(&e)));
                        }
                    }
                }
            }
        });
    }

    /// 追記（書込みの直列化ロックの中で行う）。削除中・削除済みのチャットには書かない（領域を作り直さない）。
    fn append_activity_batch(&self, batch: Vec<(ChatKey, ActivityLine)>) -> Result<(), StoreError> {
        let store = self.persist.store.as_ref().ok_or_else(|| StoreError::Io(std::io::Error::other("保存先が使えません")))?;
        let _g = self.persist.io_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut first_err = None;
        for (chat, line) in batch {
            if self.manage_rt.is_deleting(&chat) || !self.read(|d| d.chat(&chat).is_some() || d.locals.contains_key(&chat)) {
                continue;
            }
            if let Err(e) = store.append_activity(&chat, &line) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    // ── IPC ──

    pub fn get_app_settings(&self) -> AppSettings {
        self.read(|d| d.settings.clone())
    }

    pub fn get_chat_locals(&self) -> Vec<ChatLocalView> {
        self.read(|d| d.local_views())
    }

    /// 設定を反映して保存を依頼する。空き不足ならこの操作を止める。保存の成否は `saveStatusUpdated` で伝える。
    pub fn set_app_settings(self: &Arc<Self>, args: SetAppSettingsArgs) -> Result<AppSettings, IpcError> {
        self.precheck_space()?;
        let settings = args.settings;
        self.mutate(|d| {
            d.settings = settings.clone();
            ((), vec![HostEvent::SettingsUpdated { settings: settings.clone() }])
        });
        if let Some(p) = settings.codex_executable.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            // 次回の接続から使う（接続済みのApp Serverは変えない）。
            *self.executable.lock().unwrap() = p.to_string();
        }
        self.schedule_save(SaveScope::AppSettings, Duration::ZERO);
        Ok(settings)
    }

    /// ユーザーの「再試行」。自動再試行の回数を0に戻して書き直す。失敗してもエラーにせず、状態（失敗の内容）を返す。
    pub async fn retry_save(self: &Arc<Self>, args: RetrySaveArgs) -> Result<SaveStatus, IpcError> {
        if !self.persist.enabled() {
            return Err(err(IpcErrorCode::Unsupported, "保存先が使えないため、再試行できません"));
        }
        if self.status_of(&args.scope).is_none() {
            return Err(err(IpcErrorCode::NotFound, "再試行できる保存の記録がありません"));
        }
        self.set_status(&args.scope, SaveState::Unsaved, None, 0);
        let gen = self.persist.bump(&args.scope);
        Ok(self.clone().run_save(args.scope, gen, true).await)
    }

    /// 入力途中の文章をチャット別に保持する（500msまとめて書く）。復元しても送信しない。
    /// 本文が空で、まだ記録がないチャットでは何も作らない。
    pub fn set_draft(self: &Arc<Self>, args: SetDraftArgs) -> Result<(), IpcError> {
        if args.text.is_empty() && self.read(|d| !d.locals.contains_key(&args.chat)) {
            return Ok(());
        }
        let text = args.text;
        self.update_local(&args.chat, false, Duration::from_millis(DRAFT_DEBOUNCE_MS as u64), |f| {
            f.draft.text = text;
            f.draft.updated_at = Some(now_ms());
        });
        Ok(())
    }
}

fn scope_label(scope: &SaveScope) -> String {
    match scope {
        SaveScope::AppSettings => "appSettings".into(),
        SaveScope::WindowBounds => "windowBounds".into(),
        SaveScope::ChatLocal { chat } => format!("chatLocal:{}", layout::chat_label(chat)),
        SaveScope::Queue { chat } => format!("queue:{}", layout::chat_label(chat)),
        SaveScope::Activity { chat } => format!("activity:{}", layout::chat_label(chat)),
    }
}

/// 診断ログ用。会話本文・パスを含めず、種別だけ。
fn error_kind(e: &StoreError) -> &'static str {
    match e {
        StoreError::InsufficientSpace { .. } => "insufficientSpace",
        StoreError::Io(_) => "io",
        StoreError::Corrupt { .. } => "corrupt",
        StoreError::NewerSchema { .. } => "newerSchema",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::records::AppSettingsFile;

    fn key(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    #[test]
    fn restore_brings_back_pin_model_draft_and_settings_without_sending() {
        let mut local = ChatLocalFile::new(LocalId("dir-1".into()), Some(key("t1")));
        local.pinned = true;
        local.model = Some(ModelChoice { model: "m".into(), effort: Some("high".into()) });
        local.draft.text = "続きを書く".into();
        let mut settings = AppSettings::default();
        settings.autostart = true;
        let restored = Restored {
            settings: Some(AppSettingsFile::new(settings.clone())),
            windows: None,
            chats: vec![crate::store::RestoredChat { dir: "x".into(), local, queue: None }],
            problems: vec![StoreError::Corrupt { path: "p".into(), moved_to: Some("q".into()) }],
        };
        let mut d = HostData::default();
        d.restore(restored, UnixMillis(7));
        assert!(d.pinned.contains(&key("t1")));
        let ms = d.model_settings.get(&key("t1")).unwrap();
        assert_eq!(ms.selected.as_ref().unwrap().model, "m");
        assert_eq!(ms.accepted, Known::NotFetched, "restored selection is not an accepted value");
        assert_eq!(d.settings, settings);
        let v = d.local_view(&key("t1")).unwrap();
        assert_eq!(v.draft.text, "続きを書く");
        assert_eq!(v.save, SaveState::Saved { at: UnixMillis(7) });
        assert_eq!(d.startup_warnings.len(), 1);
        assert!(d.snapshot().chat_locals.len() == 1 && d.snapshot().model_settings.len() == 1);
    }

    #[test]
    fn view_without_a_status_record_is_unsaved_not_saved() {
        let mut d = HostData::default();
        d.locals.insert(key("t"), ChatLocalFile::new(LocalId("d".into()), Some(key("t"))));
        assert_eq!(d.local_view(&key("t")).unwrap().save, SaveState::Unsaved);
    }

    #[test]
    fn insufficient_space_message_guides_and_promises_no_deletion() {
        let m = save_failure_message(&StoreError::InsufficientSpace { required: 70 * 1024 * 1024, available: 10 * 1024 * 1024 });
        assert!(m.contains("空き容量") && m.contains("自動では削除しません"));
    }
}
