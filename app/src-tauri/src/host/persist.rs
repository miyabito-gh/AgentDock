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
use crate::backend::backend::BackendError;
use crate::backend::ipc::*;
use crate::backend::local::*;
use crate::backend::model::*;
use crate::host::state::HostData;
use crate::store::records::{ActivityLine, AppSettingsFile, CachedChatMeta, ChatLocalFile};
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
pub fn problem_warning(e: &StoreError) -> StartupWarning {
    let text = problem_text(e);
    match e {
        // 領域・ファイルに紐づく（パスを含む）ものだけ確認済みにできる。空き不足・パスなしの読取り失敗は毎回出す。
        StoreError::Corrupt { .. } | StoreError::NewerSchema { .. } | StoreError::Notice { .. } => StartupWarning::acknowledgeable(text),
        StoreError::InsufficientSpace { .. } | StoreError::Io(_) => StartupWarning::transient(text),
    }
}

fn problem_text(e: &StoreError) -> String {
    match e {
        StoreError::Notice { message } => message.clone(),
        StoreError::Corrupt { path, moved_to: Some(to) } => {
            format!("保存ファイルを読めなかったため退避しました（削除・上書きはしていません）: {path} → {to}")
        }
        StoreError::Corrupt { path, moved_to: None } => format!("保存ファイルを読めず、退避もできませんでした。このファイルは上書きしません: {path}"),
        StoreError::NewerSchema { path, .. } => format!("新しい版で作られた保存ファイルです。この版では読まず、上書きもしません: {path}"),
        StoreError::InsufficientSpace { .. } => "保存に必要な空き容量が足りません。".to_string(),
        StoreError::Io(e) => format!("保存ファイルを読めませんでした: {e}"),
    }
}

/// 一覧表示用の記録（正本はバックエンド。接続後の取得で上書きされる）。
pub fn cached_meta_of(c: &Chat, at: UnixMillis) -> CachedChatMeta {
    CachedChatMeta { name: c.name.clone(), cwd: c.cwd.clone(), kind: c.kind, origin: c.origin, saved_at: at }
}

/// 保存時刻を除いて同じか（変わっていなければ書き直さない）。
pub fn same_cached_meta(a: &CachedChatMeta, b: &CachedChatMeta) -> bool {
    a.name == b.name && a.cwd == b.cwd && a.kind == b.kind && a.origin == b.origin
}

/// `dir-<Unix ms>-<連番>` の作成時刻。形式が違う・数値でなければ None。
pub fn created_at_from_dir_id(id: &str) -> Option<UnixMillis> {
    let mut it = id.strip_prefix("dir-")?.split('-');
    let ms: i64 = it.next()?.parse().ok()?;
    it.next()?.parse::<u64>().ok()?;
    (it.next().is_none() && ms > 0).then_some(UnixMillis(ms))
}

/// バックエンドに履歴が無いチャット（発話前など）を、アプリの記録だけから一覧に出す。名前・要約は未取得のまま（作らない）。
/// 記録（`cachedMeta`）が無ければ種類・作業フォルダが分からないので出さない。
pub fn chat_from_record(key: &ChatKey, local: &ChatLocalFile) -> Option<Chat> {
    let meta = local.cached_meta.as_ref()?;
    Some(Chat {
        key: key.clone(),
        kind: meta.kind,
        cwd: meta.cwd.clone(),
        name: meta.name.clone(),
        preview: Known::NotFetched,
        pinned: local.pinned,
        archived: Known::NotFetched,
        origin: ChatOrigin::AppManaged,
        draft: None,
        // AgentDock が付けた専用領域名 `dir-<作成時のUnix ms>-<連番>` から作成時刻を機械的に導く（解析できなければ不明のまま）。
        created_at: created_at_from_dir_id(&local.dir_id.0).map_or(Known::NotFetched, |t| Known::Value { value: t, basis: Basis::Derived }),
        last_used_at: local.last_used_at,
        no_history: true,
    })
}

/// 起動時の警告のうち、確認済みにしていないもの（警告文が完全に一致するものだけを隠す。順序は保つ）。
/// 文には領域のパスと警告の種類が入るので、別の種類・別の領域の警告は隠れない。
/// 確認済みにできない警告（`acknowledgeable=false`）は、一覧に同じ文があっても隠さない。
pub fn pending_warnings(all: &[StartupWarning], acknowledged: &[String]) -> Vec<StartupWarning> {
    all.iter().filter(|w| !(w.acknowledgeable && acknowledged.contains(&w.message))).cloned().collect()
}

/// 一覧に載らなかったアプリ管理の会話の履歴を読んだ結果の扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlistedAction {
    /// 読めた（バックエンドの内容を使う）。
    FromBackend,
    /// バックエンドが明示的に拒否した。記録だけから「履歴なし」として出す。
    FromRecord,
    /// 通信断・結果不明・想定外の応答形式などで取得できなかった（履歴が実在するかもしれない）。何も出さず、次の更新で再試行する（「履歴なし」と断定しない）。
    Skip,
}

pub fn unlisted_action(read: Result<(), &BackendError>) -> UnlistedAction {
    match read {
        Ok(()) => UnlistedAction::FromBackend,
        Err(BackendError::Rejected { .. }) => UnlistedAction::FromRecord,
        Err(_) => UnlistedAction::Skip,
    }
}

impl HostData {
    /// 一覧（先頭ページ）に載らず、まだ表示していない、表示対象のアプリ管理の会話。表示記録（`cachedMeta`）が無いものは出せない。
    /// 削除保留（部分失敗など）のものも出す（一覧から消えて保留が見えなくなるのを防ぐ、§3.6）。
    pub fn unlisted_hosted_candidates(&self, listed: &std::collections::HashSet<ChatKey>) -> Vec<ChatKey> {
        let mut v: Vec<ChatKey> = self
            .hosted
            .iter()
            .filter(|k| !listed.contains(*k) && self.chat(k).is_none())
            .filter(|k| {
                self.locals
                    .get(*k)
                    .is_some_and(|l| matches!(l.visibility, ListVisibility::Visible) && l.cached_meta.is_some())
            })
            .cloned()
            .collect();
        v.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        v
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
        StoreError::Notice { message } => message.clone(),
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
            memory_mode: f.memory_mode.clone(),
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
            if c.local.hosted {
                self.hosted.insert(key.clone());
            }
            if c.local.model.is_some() || c.local.work_mode.is_some() {
                // 選択値だけを戻す。受理値・実効値は未取得のまま（適用済みとは表示しない）。
                self.model_settings.insert(
                    key.clone(),
                    ChatModelSettings { selected: c.local.model.clone(), work_mode: c.local.work_mode, ..ChatModelSettings::blank(ApplyTiming::NextTurn) },
                );
            }
            self.save_status.insert(SaveScope::ChatLocal { chat: key.clone() }, saved(SaveScope::ChatLocal { chat: key.clone() }));
            if let Some(q) = c.queue {
                self.save_status.insert(SaveScope::Queue { chat: key.clone() }, saved(SaveScope::Queue { chat: key.clone() }));
                self.queues.insert(key.clone(), q);
            }
            self.locals.insert(key, c.local);
        }
        self.startup_warnings.extend(r.problems.iter().map(problem_warning));
    }
}

// ───────────────────────────── Host ─────────────────────────────

impl Host {
    /// 保存層から復元した状態でホストを作る。
    pub fn with_store(app_data_dir: std::path::PathBuf, store: Arc<Store>) -> Self {
        let restored = store.load_all();
        let exe = restored.settings.as_ref().and_then(|s| s.settings.executable_for("codex").map(str::to_string));
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
        // 保存領域を開けない等の状況依存の警告。確認済みにはせず毎回出す。
        self.data.lock().unwrap().startup_warnings.push(StartupWarning::transient(message));
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
        let mut settings = args.settings;
        self.mutate(|d| {
            // 確認済みの警告は専用の操作（acknowledge_warnings）だけが変える。設定画面の古い写しで上書きしない。
            settings.acknowledged_warnings = d.settings.acknowledged_warnings.clone();
            d.settings = settings.clone();
            ((), vec![HostEvent::SettingsUpdated { settings: settings.clone() }])
        });
        if let Some(p) = settings.executable_for("codex") {
            // 次回の接続から使う（接続済みのApp Serverは変えない）。
            *self.executable.lock().unwrap() = p.to_string();
        }
        self.schedule_save(SaveScope::AppSettings, Duration::ZERO);
        Ok(settings)
    }

    /// 起動時の保存データ警告の確認（ユーザーの「閉じる」）。表示中の警告を確認済みとして設定へ保存する（ファイルは触らない）。
    /// `reset` なら確認済みを解除する（次回起動から再び出る）。
    pub fn acknowledge_warnings(self: &Arc<Self>, args: AcknowledgeWarningsArgs) -> Result<AppSettings, IpcError> {
        self.precheck_space()?;
        let settings = self.mutate(|d| {
            if args.reset {
                d.settings.acknowledged_warnings.clear();
            } else {
                let shown = pending_warnings(&d.startup_warnings, &d.settings.acknowledged_warnings);
                // 確認済みにできるのは、領域・ファイルに紐づく警告だけ。
                d.settings.acknowledged_warnings.extend(shown.into_iter().filter(|w| w.acknowledgeable).map(|w| w.message));
            }
            let s = d.settings.clone();
            (s.clone(), vec![HostEvent::SettingsUpdated { settings: s }])
        });
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
        StoreError::Notice { .. } => "notice",
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
        local.model = Some(ModelChoice { model: "m".into(), effort: Some("high".into()), speed_tier: None });
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
    fn restore_brings_back_the_app_managed_mark() {
        let mut local = ChatLocalFile::new(LocalId("dir-1".into()), Some(key("t1")));
        local.hosted = true;
        let mut other = ChatLocalFile::new(LocalId("dir-2".into()), Some(key("t2")));
        other.pinned = true;
        let restored = Restored {
            settings: None,
            windows: None,
            chats: vec![
                crate::store::RestoredChat { dir: "x".into(), local, queue: None },
                crate::store::RestoredChat { dir: "y".into(), local: other, queue: None },
            ],
            problems: vec![],
        };
        let mut d = HostData::default();
        d.restore(restored, UnixMillis(7));
        assert!(d.hosted.contains(&key("t1")), "started/resumed by this app: still app-managed after a restart");
        assert!(!d.hosted.contains(&key("t2")));
        // 再起動後の一覧で、バックエンドが外部作成（vscode）と返しても、アプリ管理を保つ。
        let mut c = chat_from_record(&key("t1"), &{
            let mut l = ChatLocalFile::new(LocalId("d".into()), Some(key("t1")));
            l.cached_meta = Some(CachedChatMeta { name: Known::NotFetched, cwd: Known::direct("C:\\w".into()), kind: ChatKind::General, origin: ChatOrigin::AppManaged, saved_at: UnixMillis(1) });
            l
        })
        .unwrap();
        c.origin = ChatOrigin::External;
        d.upsert_chat(c);
        assert_eq!(d.chat(&key("t1")).unwrap().origin, ChatOrigin::AppManaged);
    }

    #[test]
    fn resumed_external_chat_returns_to_external_after_a_restart() {
        // 外部会話をユーザー確認のうえ再開しても、永続する印（hosted）は付かない。再起動（新しい読込み）後の一覧は外部のまま。
        let local = ChatLocalFile::new(LocalId("dir-1".into()), Some(key("ext")));
        assert!(!local.hosted);
        let restored = Restored { settings: None, windows: None, chats: vec![crate::store::RestoredChat { dir: "x".into(), local, queue: None }], problems: vec![] };
        let mut d = HostData::default();
        d.restore(restored, UnixMillis(7));
        assert!(!d.hosted.contains(&key("ext")));
        let mut l = ChatLocalFile::new(LocalId("d".into()), Some(key("ext")));
        l.cached_meta = Some(CachedChatMeta { name: Known::NotFetched, cwd: Known::NotFetched, kind: ChatKind::Development, origin: ChatOrigin::External, saved_at: UnixMillis(1) });
        let mut c = chat_from_record(&key("ext"), &l).unwrap();
        c.origin = ChatOrigin::External;
        d.upsert_chat(c);
        assert_eq!(d.chat(&key("ext")).unwrap().origin, ChatOrigin::External);
    }

    #[test]
    fn acknowledged_warnings_are_hidden_but_other_kinds_and_areas_still_show() {
        let missing = |dir: &str| format!("チャットの記録（chat.json）が見つかりません: C:/data/chats/{dir}");
        let corrupt = |dir: &str| format!("保存ファイルを読めなかったため退避しました: C:/data/chats/{dir}/chat.json");
        let w = |s: String| StartupWarning::acknowledgeable(s);
        let all = vec![w(missing("dir-1")), w(missing("dir-2")), w(corrupt("dir-1"))];
        assert_eq!(pending_warnings(&all, &[]), all);
        // 領域dir-1の「chat.json なし」だけを確認済みにしても、別の領域・別の種類は出る。
        let acked = vec![missing("dir-1")];
        assert_eq!(pending_warnings(&all, &acked), vec![w(missing("dir-2")), w(corrupt("dir-1"))]);
        // 同じ警告が再起動後にもう一度出ても隠れる。確認済みの一覧は警告が無くなっても残る（解除は専用の操作）。
        assert!(pending_warnings(&[w(missing("dir-1"))], &acked).is_empty());
        assert_eq!(pending_warnings(&[w(missing("dir-3"))], &acked), vec![w(missing("dir-3"))]);
        // 保存できない状態の警告は、同じ文が確認済みに入っていても隠さない（種類で決める。文字列判定ではない）。
        let space = problem_warning(&StoreError::InsufficientSpace { required: 1, available: 0 });
        let open = StartupWarning::transient("保存領域を開けませんでした（x）。");
        assert!(!space.acknowledgeable && !open.acknowledgeable);
        let acked_all = vec![space.message.clone(), open.message.clone()];
        assert_eq!(pending_warnings(&[space.clone(), open.clone()], &acked_all), vec![space, open]);
        assert!(problem_warning(&StoreError::Notice { message: "p".into() }).acknowledgeable);
        assert!(problem_warning(&StoreError::Corrupt { path: "p".into(), moved_to: None }).acknowledgeable);
        assert!(!problem_warning(&StoreError::Io(std::io::Error::other("x"))).acknowledgeable);
        // 確認済みはsettings.jsonへ保存される。以前の版の設定（項目なし）も読める。
        let mut st = AppSettings::default();
        st.acknowledged_warnings = acked.clone();
        let back: AppSettings = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back.acknowledged_warnings, acked);
        let mut v = serde_json::to_value(AppSettings::default()).unwrap();
        v.as_object_mut().unwrap().remove("acknowledgedWarnings");
        let old: AppSettings = serde_json::from_value(v).unwrap();
        assert!(old.acknowledged_warnings.is_empty());
    }

    #[test]
    fn hosted_chat_missing_from_the_backend_list_is_shown_from_the_record() {
        let mut d = HostData::default();
        let meta = |l: &mut ChatLocalFile| {
            l.cached_meta = Some(CachedChatMeta { name: Known::Missing, cwd: Known::direct("C:/w".into()), kind: ChatKind::General, origin: ChatOrigin::AppManaged, saved_at: UnixMillis(1) })
        };
        for id in ["listed", "shown-already", "hidden", "no-record", "plain-external"] {
            if id != "plain-external" {
                d.hosted.insert(key(id));
            }
            let mut l = ChatLocalFile::new(LocalId(format!("dir-{id}")), Some(key(id)));
            if id != "no-record" {
                meta(&mut l);
            }
            if id == "hidden" {
                l.visibility = ListVisibility::RemovedFromList { at: UnixMillis(1) };
            }
            d.locals.insert(key(id), l);
        }
        let l = d.locals.get(&key("shown-already")).unwrap().clone();
        d.upsert_chat(chat_from_record(&key("shown-already"), &l).unwrap());
        let listed: std::collections::HashSet<ChatKey> = [key("listed")].into_iter().collect();
        // 先頭ページに載っていない・まだ出していない・表示対象・表示記録がある、アプリ管理の会話だけが候補。
        assert!(d.unlisted_hosted_candidates(&listed).is_empty());
        d.hosted.insert(key("late"));
        let mut l = ChatLocalFile::new(LocalId("dir-late".into()), Some(key("late")));
        meta(&mut l);
        d.locals.insert(key("late"), l);
        assert_eq!(d.unlisted_hosted_candidates(&listed), vec![key("late")]);
        // 読めた→バックエンドの内容、明示的な拒否→記録から「履歴なし」、通信断・結果不明・想定外の形→出さない（履歴なしと断定しない）。
        assert_eq!(unlisted_action(Ok(())), UnlistedAction::FromBackend);
        assert_eq!(unlisted_action(Err(&BackendError::Rejected { code: None, message: "no rollout".into() })), UnlistedAction::FromRecord);
        assert_eq!(unlisted_action(Err(&BackendError::Protocol { message: "bad thread".into() })), UnlistedAction::Skip);
        assert_eq!(unlisted_action(Err(&BackendError::NotConnected)), UnlistedAction::Skip);
        assert_eq!(unlisted_action(Err(&BackendError::OutcomeUnknown { message: "timeout".into() })), UnlistedAction::Skip);
        let l = d.locals.get(&key("late")).unwrap().clone();
        let c = chat_from_record(&key("late"), &l).unwrap();
        assert!(c.no_history);
        assert_eq!(c.name, Known::Missing, "the name is not made up");
        d.upsert_chat(c);
        assert!(d.unlisted_hosted_candidates(&listed).is_empty());
    }

    #[test]
    fn chat_without_backend_history_is_listed_from_the_record_and_marked() {
        let mut l = ChatLocalFile::new(LocalId("d".into()), Some(key("t")));
        assert!(chat_from_record(&key("t"), &l).is_none(), "no display record: nothing is invented");
        l.pinned = true;
        l.last_used_at = Some(UnixMillis(9));
        l.cached_meta = Some(CachedChatMeta { name: Known::NotFetched, cwd: Known::direct("C:\\w".into()), kind: ChatKind::General, origin: ChatOrigin::AppManaged, saved_at: UnixMillis(1) });
        let c = chat_from_record(&key("t"), &l).unwrap();
        assert!(c.no_history && c.pinned);
        assert_eq!(c.name, Known::NotFetched, "the name is not made up");
        assert_eq!(c.preview, Known::NotFetched);
        assert_eq!(c.last_used_at, Some(UnixMillis(9)));
        let m = cached_meta_of(&c, UnixMillis(5));
        assert!(same_cached_meta(&m, l.cached_meta.as_ref().unwrap()), "saved_at is not part of the comparison");
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

    #[test]
    fn dir_id_creation_time_is_parsed_or_unknown() {
        assert_eq!(created_at_from_dir_id("dir-1791329043410-7"), Some(UnixMillis(1791329043410)));
        for bad in ["dir-x-1", "dir-123", "chat-1-2", "dir-1-2-3", "dir--1-2", ""] {
            assert_eq!(created_at_from_dir_id(bad), None, "{bad}");
        }
    }
}
