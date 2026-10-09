//! 変更の控えの取得と区間の管理（P3B-3、`app/DESIGN_P3B.md` §2・§3・§7.1）。
//!
//! - 基準 B: ユーザーの送信（新しいturn）の直前、`backend.send` の前に取る。取得は10秒上限。失敗しても送信は止めず、警告は理由ごとにチャット1回。
//! - 終了 E: 対応付けたturnの終端を観測し、かつそのチャットのエージェントが全員止まったときに取る。終端を観測できなかった区間
//!   （切断・再起動・次の送信まで観測なし）は `Ended` を書かない＝終了未確認。後から履歴で終端を確認しても E は作らない。
//! - turnとの対応: 送信の受理で `Bound`、受理なしの確定で `Abandoned`。受理不明は照合の結果で同じように確定する。
//!   turn終了の通知が受理の応答より先に来ることがあるので、対応付け前の終端は覚えておく。
//! - 同時作業: 同じリポジトリで別チャット（または外部の会話）が動いていた区間に `Concurrent` を残す。判定は `paths_overlap`（`FolderOpGuard` と同じ）。
//! - 並行制御: チャットごとに1本の作業キュー（B・E・控えの削除を直列化）。リポジトリごとの `RwLock`（控えは読取り側、
//!   「戻す」は書込み側 = P3B-4）。ロック順は「send_lock → チャットの作業キュー → リポジトリロック」のみ。Eの作業は send_lock を取らない。
//! - 控えの失敗で送信を止めない。自動削除しない（削除は明示操作 `delete_baselines` とチャット削除だけ）。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::baseline_snap::{capture, probe_repo, RepoProbe};
use super::cloud::paths_overlap;
use super::state::{agent_key_of, HostData};
use super::{blocked, err, now_ms, Host};
use crate::backend::backend::*;
use crate::backend::baseline::*;
use crate::backend::ipc::*;
use crate::backend::local::ChatArgs;
use crate::backend::model::*;
use crate::gitops::Git;
use crate::rules::baseline::norm_path;
use crate::store::{Store, StoreError};

/// Eの待機判定の周期（通知が来ない状態変化の取りこぼし防止）。
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 対応付け前に覚えておく終端の最大数。
const EARLY_ENDS_MAX: usize = 16;
/// 取得の時間の下限（リポジトリの調査に時間を使ったあとでも、取得に最低これだけは与える）。
const MIN_CAPTURE_MS: u64 = 1_000;

// ───────────────────────────── 純粋な部分 ─────────────────────────────

/// 開いている区間（Bを取れて、Eが未確定のもの）。チャットごとに高々1つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSeg {
    pub seg: LocalId,
    pub attempt: LocalId,
    pub turn: Option<ExternalId>,
    /// リポジトリのルート（Eのgit実行の場所）。
    pub repo_root: String,
    /// 同時作業を記録済みの相手（重複して書かない）。
    pub concurrent: HashSet<String>,
    /// 対応付けたturnのルートの終端を観測した。
    pub root_ended: bool,
    /// Eの作業を予約した（重複して予約しない）。
    pub end_in_flight: bool,
}

/// 次の送信の時点で、前の区間をどうするか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrevAction {
    /// 前の区間なし。
    None,
    /// 終端観測済みでチャットも止まっている: 先にEを取る。
    EndNow(LocalId),
    /// 終端観測済みだが子孫が動いたまま: 次のBを前の区間のEとする。
    EndIsNextBase(LocalId),
    /// 終端を観測できていない: 終了未確認のまま閉じる（Eは作らない）。
    Unknown(LocalId),
}

/// チャットごとの区間の追跡（メモリのみ。再起動後は、Eのない区間がそのまま「終了未確認」になる）。
#[derive(Debug, Default)]
pub struct ChatSegs {
    pub open: Option<OpenSeg>,
    /// 送信の試行 → 区間（受理・受理なしの確定を待っているもの）。
    pub pending: HashMap<LocalId, LocalId>,
    /// 対応付け前に観測したルートのturn終端。
    pub early_ends: Vec<ExternalId>,
    /// 警告済みの理由。
    pub warned: HashSet<String>,
}

impl ChatSegs {
    pub fn prev_action(&self, quiescent: bool) -> PrevAction {
        match &self.open {
            None => PrevAction::None,
            Some(p) if p.root_ended && quiescent => PrevAction::EndNow(p.seg.clone()),
            Some(p) if p.root_ended => PrevAction::EndIsNextBase(p.seg.clone()),
            Some(p) => PrevAction::Unknown(p.seg.clone()),
        }
    }

    /// ルートのturnの終端を観測した。開いている区間のturnなら終端済みにし、まだ対応付け前なら覚えておく。
    pub fn on_root_turn_end(&mut self, turn: &ExternalId) {
        if let Some(o) = &mut self.open {
            if o.turn.as_ref() == Some(turn) {
                o.root_ended = true;
                return;
            }
        }
        if self.early_ends.len() >= EARLY_ENDS_MAX {
            self.early_ends.remove(0);
        }
        self.early_ends.push(turn.clone());
    }

    /// 送信の受理でturnと対応付ける。区間が開いていて、その終端を既に観測していれば終端済みにする。区間を返す。
    pub fn bind(&mut self, attempt: &LocalId, turn: &ExternalId) -> Option<LocalId> {
        let seg = self.pending.remove(attempt)?;
        if let Some(o) = self.open.as_mut().filter(|o| o.seg == seg) {
            o.turn = Some(turn.clone());
            if self.early_ends.contains(turn) {
                o.root_ended = true;
            }
        }
        Some(seg)
    }

    /// 受理なしが確定した。区間を外す（開いていれば閉じる）。
    pub fn abandon(&mut self, attempt: &LocalId) -> Option<LocalId> {
        let seg = self.pending.remove(attempt)?;
        if self.open.as_ref().is_some_and(|o| o.seg == seg) {
            self.open = None;
        }
        Some(seg)
    }

    /// Eを取る時機か（終端観測済み・予約前・チャットが止まっている）。取れるなら予約して区間を返す。
    pub fn reserve_end(&mut self, quiescent: bool) -> Option<LocalId> {
        let o = self.open.as_mut()?;
        if o.root_ended && !o.end_in_flight && quiescent {
            o.end_in_flight = true;
            Some(o.seg.clone())
        } else {
            None
        }
    }

    /// 切断: 終端を観測できなくなるので、開いている区間は終了未確認のまま閉じる。
    pub fn on_disconnect(&mut self) {
        self.open = None;
        self.early_ends.clear();
    }

    /// 同時作業の相手を記録する。新しい相手なら true。
    pub fn note_concurrent(&mut self, label: &str) -> bool {
        self.open.as_mut().is_some_and(|o| o.concurrent.insert(label.to_string()))
    }

    /// 警告を出すべきか（同じ理由は1回だけ）。
    pub fn should_warn(&mut self, key: String) -> bool {
        self.warned.insert(key)
    }
}

/// 警告の理由キー。警告しない理由（リポジトリでない・作業フォルダ不明・設定で無効）は None。
pub fn warn_key(reason: &BaselineFailure) -> Option<String> {
    Some(match reason {
        BaselineFailure::NotARepository | BaselineFailure::WorkFolderUnknown | BaselineFailure::Disabled => return None,
        BaselineFailure::GitUnavailable => "gitUnavailable".into(),
        BaselineFailure::GitTooOld { .. } => "gitTooOld".into(),
        BaselineFailure::Timeout => "timeout".into(),
        BaselineFailure::InsufficientSpace { .. } => "space".into(),
        BaselineFailure::GitBusy => "gitBusy".into(),
        BaselineFailure::Git { message } => format!("git:{message}"),
    })
}

/// 失敗理由の説明（警告・表示用）。
pub fn failure_text(reason: &BaselineFailure) -> String {
    match reason {
        BaselineFailure::NotARepository => "作業フォルダがGitリポジトリではありません".into(),
        BaselineFailure::GitUnavailable => "Gitを実行できません（未導入、または設定のGitの場所が違います）".into(),
        BaselineFailure::GitTooOld { found } => format!("Gitの版が古いため使えません（{found}。2.31以上が必要です）"),
        BaselineFailure::WorkFolderUnknown => "作業フォルダが分かりません".into(),
        BaselineFailure::Timeout => "時間内に終わりませんでした".into(),
        BaselineFailure::InsufficientSpace { required, available } => format!("空き容量が足りません（必要 {required} バイト、空き {available} バイト）"),
        BaselineFailure::GitBusy => "Gitの操作（rebase・merge等）の途中です".into(),
        BaselineFailure::Disabled => "設定で無効です".into(),
        BaselineFailure::Git { message } => format!("Gitのエラー: {message}"),
    }
}

/// 控えを取れなかったときの警告文。
pub fn warn_text(reason: &BaselineFailure, phase: BaselinePhase) -> String {
    let what = match phase {
        BaselinePhase::Base => "このturnの変更は控えられていません",
        BaselinePhase::End => "このturnの終了時の変更を控えられていません",
    };
    format!("{what}（{}）。『変更を戻す』の対象になりません", failure_text(reason))
}

/// 同時作業の判定に使う、他のチャットの状況。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherChat {
    pub chat: ChatKey,
    pub label: String,
    pub cwd: Option<String>,
    /// 作業中（エージェントが動いている、または開いている区間がある）。
    pub busy: bool,
    /// AgentDock外で作られ、実行中と観測されている会話。
    pub external_running: bool,
    /// 相手の開いている区間（区間ID、リポジトリルート）。
    pub open_seg: Option<(LocalId, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcurrentHit {
    pub with: ConcurrentWith,
    /// 相手の区間（あれば、相手側にも記録する）。
    pub other_seg: Option<LocalId>,
    /// 重複判定に使う相手の識別子。
    pub id: String,
}

/// このリポジトリで同時に作業している相手。相手の作業フォルダがリポジトリルートと重なる（親子を含む）か、相手の開いている区間が同じルート。
pub fn concurrent_hits(repo_root: &str, others: &[OtherChat]) -> Vec<ConcurrentHit> {
    let mut hits = Vec::new();
    for o in others {
        if !o.busy && !o.external_running {
            continue;
        }
        let same_open = o.open_seg.as_ref().is_some_and(|(_, r)| norm_path(r) == norm_path(repo_root));
        let overlaps = o.cwd.as_deref().is_some_and(|w| paths_overlap(w, repo_root));
        if !(same_open || overlaps) {
            continue;
        }
        if o.external_running {
            hits.push(ConcurrentHit { with: ConcurrentWith::External { label: o.label.clone() }, other_seg: None, id: format!("ext:{}", o.chat.id.0) });
        } else {
            let other_seg = o.open_seg.as_ref().filter(|(_, r)| norm_path(r) == norm_path(repo_root)).map(|(s, _)| s.clone());
            hits.push(ConcurrentHit { with: ConcurrentWith::Chat { chat: o.chat.clone() }, other_seg, id: format!("chat:{}", o.chat.id.0) });
        }
    }
    hits
}

/// 記録から区間の状態を組み立てる。`running` は今、開いている（Eを待つ）区間。Eのない区間は終了未確認。
pub fn derive_status(lines: &[BaselineLine], running: &HashSet<LocalId>) -> Vec<SegmentStatus> {
    enum Tmp {
        Open,
        Ready,
        EndUnknown,
        Failed(BaselineFailure),
        Abandoned,
    }
    struct Info {
        turn: Option<ExternalId>,
        started_at: UnixMillis,
        state: Tmp,
        concurrent: bool,
        seg: Option<LocalId>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Info> = HashMap::new();
    let create = |key: String, at: UnixMillis, state: Tmp, seg: Option<LocalId>, order: &mut Vec<String>, map: &mut HashMap<String, Info>| {
        if !map.contains_key(&key) {
            order.push(key.clone());
            map.insert(key, Info { turn: None, started_at: at, state, concurrent: false, seg });
            return;
        }
        let i = map.get_mut(&key).unwrap();
        i.state = state;
    };
    for l in lines {
        match l {
            BaselineLine::Started { seg, at, .. } => create(seg.0.clone(), *at, Tmp::Open, Some(seg.clone()), &mut order, &mut map),
            BaselineLine::Bound { seg, turn } => {
                if let Some(i) = map.get_mut(&seg.0) {
                    i.turn = Some(turn.clone());
                }
            }
            BaselineLine::Ended { seg, .. } => {
                if let Some(i) = map.get_mut(&seg.0) {
                    if matches!(i.state, Tmp::Open | Tmp::EndUnknown) {
                        i.state = Tmp::Ready;
                    }
                }
            }
            BaselineLine::Abandoned { seg, .. } => {
                if let Some(i) = map.get_mut(&seg.0) {
                    i.state = Tmp::Abandoned;
                }
            }
            BaselineLine::Failed { seg, at, attempt, phase, reason } => {
                let key = seg.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| format!("attempt:{}", attempt.0));
                match phase {
                    // 終了の控えを取れなかった区間は終了未確認（基準は取れている）。
                    BaselinePhase::End => {
                        if let Some(i) = map.get_mut(&key) {
                            i.state = Tmp::EndUnknown;
                        }
                    }
                    BaselinePhase::Base => create(key, *at, Tmp::Failed(reason.clone()), seg.clone(), &mut order, &mut map),
                }
            }
            BaselineLine::Concurrent { seg, .. } => {
                if let Some(i) = map.get_mut(&seg.0) {
                    i.concurrent = true;
                }
            }
            BaselineLine::Reverted { .. } => {}
        }
    }
    order
        .into_iter()
        .filter_map(|k| map.remove(&k))
        .map(|i| SegmentStatus {
            turn: i.turn,
            started_at: i.started_at,
            state: match i.state {
                Tmp::Open => {
                    if i.seg.as_ref().is_some_and(|s| running.contains(s)) {
                        SegmentState::Running
                    } else {
                        SegmentState::EndUnknown
                    }
                }
                Tmp::Ready => SegmentState::Ready,
                Tmp::EndUnknown => SegmentState::EndUnknown,
                Tmp::Failed(reason) => SegmentState::Failed { reason },
                Tmp::Abandoned => SegmentState::Abandoned,
            },
            concurrent: i.concurrent,
        })
        .collect()
}

fn is_working(s: AgentState) -> bool {
    matches!(s, AgentState::Running | AgentState::Waiting | AgentState::Initializing)
}

/// チャットのエージェントが全員止まっているか（動いているturnも、作業中の状態もない）。
fn chat_quiescent(d: &HostData, chat: &ChatKey) -> bool {
    !d.agents.iter().any(|v| &v.agent.chat == chat && (d.running_turn.contains_key(&v.agent.key) || is_working(v.status.state)))
}

// ───────────────────────────── 実行時の状態 ─────────────────────────────

/// 控えの作業状態（保存しない）。
#[derive(Default)]
pub struct BaselineRuntime {
    chats: Mutex<HashMap<ChatKey, ChatSegs>>,
    /// チャットごとの作業キュー（B・E・控えの削除を1本に直列化する）。
    queues: Mutex<HashMap<ChatKey, Arc<tokio::sync::Mutex<()>>>>,
    /// リポジトリごとの読み書きロック（キーは正規化したリポジトリルート）。控えは読取り側、戻す（P3B-4）は書込み側。
    repos: Mutex<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
    notify: Notify,
}

impl BaselineRuntime {
    pub fn queue_lock(&self, chat: &ChatKey) -> Arc<tokio::sync::Mutex<()>> {
        self.queues.lock().unwrap().entry(chat.clone()).or_default().clone()
    }

    pub fn repo_lock(&self, root: &str) -> Arc<tokio::sync::RwLock<()>> {
        self.repos.lock().unwrap().entry(norm_path(root)).or_default().clone()
    }

    fn with<R>(&self, chat: &ChatKey, f: impl FnOnce(&mut ChatSegs) -> R) -> R {
        f(self.chats.lock().unwrap().entry(chat.clone()).or_default())
    }

    /// 追跡が既にあるチャットだけに作用する（控えを使っていないチャットの記録を増やさない）。
    fn with_existing<R>(&self, chat: &ChatKey, f: impl FnOnce(&mut ChatSegs) -> R) -> Option<R> {
        self.chats.lock().unwrap().get_mut(chat).map(f)
    }

    fn running_segs(&self, chat: &ChatKey) -> HashSet<LocalId> {
        self.chats.lock().unwrap().get(chat).and_then(|c| c.open.as_ref().map(|o| o.seg.clone())).into_iter().collect()
    }

    fn open_seg_of(&self, chat: &ChatKey) -> Option<(LocalId, String)> {
        self.chats.lock().unwrap().get(chat).and_then(|c| c.open.as_ref().map(|o| (o.seg.clone(), o.repo_root.clone())))
    }
}

fn store_failure(e: StoreError) -> BaselineFailure {
    match e {
        StoreError::InsufficientSpace { required, available } => BaselineFailure::InsufficientSpace { required, available },
        other => BaselineFailure::Git { message: format!("storage: {other}") },
    }
}

impl Host {
    // ───────────── 記録 ─────────────

    /// 控えの記録を1行追記する。削除中・保存先なしは書かない。書けたら `ChangesUpdated` を出す。
    pub(super) fn baseline_append(&self, chat: &ChatKey, line: BaselineLine) -> bool {
        let Some(store) = self.persist.store() else { return false };
        let result = {
            let _g = self.persist.io_guard();
            if self.manage_rt.is_deleting(chat) {
                return false;
            }
            store.append_baseline(chat, &line)
        };
        match result {
            Ok(()) => {
                if matches!(line, BaselineLine::Started { .. } | BaselineLine::Ended { .. } | BaselineLine::Failed { .. } | BaselineLine::Reverted { .. }) {
                    let chat = chat.clone();
                    self.mutate(|_| ((), vec![HostEvent::ChangesUpdated { chat, turn: None }]));
                }
                true
            }
            Err(e) => {
                self.warn(format!("変更の控えの記録を保存できませんでした: {}", super::persist::save_failure_message(&e)));
                false
            }
        }
    }

    /// 失敗の警告（理由ごとにチャット1回。警告しない理由は出さない）。
    fn baseline_warn(&self, chat: &ChatKey, reason: &BaselineFailure, phase: BaselinePhase) {
        let Some(key) = warn_key(reason) else { return };
        if self.baseline_rt.with(chat, |c| c.should_warn(key)) {
            self.warn(warn_text(reason, phase));
        }
    }

    // ───────────── 基準 B ─────────────

    /// 送信の直前（`backend.send` の前）に基準 B を取る。失敗しても送信は止めない。新しいturnの送信（`NewTurn`）でだけ呼ぶ。
    /// 戻ったときには、成功なら区間が開き、いずれの場合も `attempt` は区間と結び付いている（受理・受理なしの確定で `bound`／`abandoned`）。
    pub(super) async fn baseline_take_base(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, request_cwd: Option<String>) {
        let Some(store) = self.persist.store().cloned() else { return };
        if self.manage_rt.is_deleting(chat) {
            return;
        }
        let seg = self.local_id("seg");
        // 送信前の処理全体（作業キュー待ち・前の区間のE・ロック待ち・取得）に総上限を設ける。超えたら諦めて送信へ進む。
        let limit = Duration::from_millis(BASELINE_TIME_LIMIT_MS);
        if tokio::time::timeout(limit, self.baseline_take_base_inner(chat, attempt, request_cwd, &store, &seg)).await.is_err() {
            let reason = BaselineFailure::Timeout;
            self.baseline_append(chat, BaselineLine::Failed { seg: Some(seg.clone()), at: now_ms(), attempt: attempt.clone(), phase: BaselinePhase::Base, reason: reason.clone() });
            self.baseline_rt.with(chat, |c| c.pending.insert(attempt.clone(), seg));
            self.baseline_warn(chat, &reason, BaselinePhase::Base);
        }
    }

    async fn baseline_take_base_inner(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, request_cwd: Option<String>, store: &Arc<Store>, seg: &LocalId) {
        let seg = seg.clone();
        let queue = self.baseline_rt.queue_lock(chat);
        let _q = queue.lock().await;
        let at = now_ms();
        // 前の区間: 終端観測済みで止まっていれば先にEを取る。子孫が動いたままなら、このBを前の区間のEとする。終端未観測は終了未確認。
        let quiescent = self.read(|d| chat_quiescent(d, chat));
        let carry = match self.baseline_rt.with(chat, |c| c.prev_action(quiescent)) {
            PrevAction::None => None,
            PrevAction::EndNow(prev) => {
                self.baseline_end_inner(chat, &prev, store).await;
                None
            }
            PrevAction::EndIsNextBase(prev) => {
                self.baseline_rt.with(chat, |c| c.open = None);
                Some(prev)
            }
            PrevAction::Unknown(_) => {
                self.baseline_rt.with(chat, |c| c.open = None);
                None
            }
        };
        let result = self.baseline_capture_base(chat, store, request_cwd).await;
        match result {
            Ok((repo, cwd, base)) => {
                let root = repo.root.clone();
                let started = BaselineLine::Started { seg: seg.clone(), at, attempt: attempt.clone(), repo, cwd, base };
                if !self.baseline_append(chat, started) {
                    // 記録できなければ区間にしない（警告は記録の失敗として出ている）。
                    return;
                }
                if let Some(prev) = carry {
                    self.baseline_append(chat, BaselineLine::Ended { seg: prev, at, end: SegmentEnd::EndIsNextBase });
                }
                self.baseline_rt.with(chat, |c| {
                    c.pending.insert(attempt.clone(), seg.clone());
                    c.open = Some(OpenSeg {
                        seg: seg.clone(),
                        attempt: attempt.clone(),
                        turn: None,
                        repo_root: root.clone(),
                        concurrent: HashSet::new(),
                        root_ended: false,
                        end_in_flight: false,
                    });
                });
                self.baseline_record_concurrency(chat, &seg, &root);
            }
            Err(reason) => {
                self.baseline_append(chat, BaselineLine::Failed { seg: Some(seg.clone()), at, attempt: attempt.clone(), phase: BaselinePhase::Base, reason: reason.clone() });
                self.baseline_rt.with(chat, |c| c.pending.insert(attempt.clone(), seg));
                self.baseline_warn(chat, &reason, BaselinePhase::Base);
            }
        }
    }

    async fn baseline_capture_base(self: &Arc<Self>, chat: &ChatKey, store: &Arc<Store>, request_cwd: Option<String>) -> Result<(RepoRef, String, Snapshot), BaselineFailure> {
        if !self.read(|d| d.settings.baselines.enabled) {
            return Err(BaselineFailure::Disabled);
        }
        let cwd = request_cwd
            .filter(|c| !c.trim().is_empty())
            .or_else(|| self.read(|d| d.chat(chat).and_then(|c| c.cwd.value().cloned())))
            .ok_or(BaselineFailure::WorkFolderUnknown)?;
        let (repo, snapshot) = self.baseline_capture_at(chat, store, Path::new(&cwd)).await?;
        Ok((repo, cwd, snapshot))
    }

    /// 作業フォルダのリポジトリを調べ、リポジトリロック（読取り）を取って控えを取る。調査と取得はそれぞれ10秒の上限。
    async fn baseline_capture_at(self: &Arc<Self>, chat: &ChatKey, store: &Arc<Store>, cwd: &Path) -> Result<(RepoRef, Snapshot), BaselineFailure> {
        let tool = self.read(|d| d.settings.tools.git.clone());
        let git = Git::new(tool.as_deref());
        let limit = Duration::from_millis(BASELINE_TIME_LIMIT_MS);
        let t0 = Instant::now();
        let probe = tokio::time::timeout(limit, probe_repo(&git, cwd)).await.map_err(|_| BaselineFailure::Timeout)??;
        let repo = probe.repo_ref();
        // 戻す操作の実行中はその終了を待つ（待ち時間は取得の時間に数えない）。
        let lock = self.baseline_rt.repo_lock(&repo.root);
        let _read = lock.read().await;
        let snapshot = self.baseline_capture_locked(chat, store, &git, &probe, t0).await?;
        Ok((repo, snapshot))
    }

    /// リポジトリロック（読取りか書込み）を持った状態で、調べ済みのリポジトリの控えを取る（P3B-4: 戻す処理の現在状態 C もこれで取る）。
    /// 専用objects置き場にだけ書き、ユーザーのリポジトリは変えない。`t0` は調査を始めた時刻（総時間の上限の起点）。
    pub(super) async fn baseline_capture_locked(self: &Arc<Self>, chat: &ChatKey, store: &Arc<Store>, git: &Git, probe: &RepoProbe, t0: Instant) -> Result<Snapshot, BaselineFailure> {
        let limit = Duration::from_millis(BASELINE_TIME_LIMIT_MS);
        if self.manage_rt.is_deleting(chat) {
            return Err(BaselineFailure::Git { message: "chat is being deleted".into() });
        }
        let (objects, tmp) = store.ensure_baseline_dirs(chat).map_err(store_failure)?;
        // AgentDockのデータ領域全体（他チャットの履歴・添付を含む）は、リポジトリの中にあっても控えない。
        let exclude = store.root().to_path_buf();
        let remaining = limit.saturating_sub(t0.elapsed()).max(Duration::from_millis(MIN_CAPTURE_MS));
        let space = |need: u64| store.check_space(need).map_err(store_failure);
        tokio::time::timeout(remaining, capture(git, probe, &objects, &tmp, &exclude, &space)).await.map_err(|_| BaselineFailure::Timeout)?
    }

    // ───────────── 受理・受理なしの確定 ─────────────

    /// 送信が受理された（応答、または照合）: 区間とturnを対応付ける。
    pub(super) fn baseline_bound(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, turn: &ExternalId) {
        if let Some(seg) = self.baseline_rt.with_existing(chat, |c| c.bind(attempt, turn)).flatten() {
            self.baseline_append(chat, BaselineLine::Bound { seg, turn: turn.clone() });
            self.baseline_rt.notify.notify_one();
        }
    }

    /// 送信が受理なしで確定した: 区間から外す。
    pub(super) fn baseline_abandoned(self: &Arc<Self>, chat: &ChatKey, attempt: &LocalId, reason: &str) {
        if let Some(seg) = self.baseline_rt.with_existing(chat, |c| c.abandon(attempt)).flatten() {
            self.baseline_append(chat, BaselineLine::Abandoned { seg, reason: reason.to_string() });
        }
    }

    /// 起動時: 受理不明のまま残った送信の区間を、照合の結果で対応付けられるよう復元する。区間は開かない（Eのない区間は終了未確認のまま）。
    pub(super) fn baseline_restore(self: &Arc<Self>) {
        let Some(store) = self.persist.store().cloned() else { return };
        let attempts: Vec<(LocalId, ChatKey)> = {
            let u = self.unresolved.lock().unwrap();
            self.unconfirmed.lock().unwrap().iter().filter(|(id, _)| u.contains(id)).map(|(id, s)| (id.clone(), s.chat().clone())).collect()
        };
        for (attempt, chat) in attempts {
            let Ok(lines) = store.read_baselines(&chat) else { continue };
            let seg_of = lines.iter().find_map(|l| match l {
                BaselineLine::Started { seg, attempt: a, .. } if a == &attempt => Some(seg.clone()),
                BaselineLine::Failed { seg: Some(seg), attempt: a, phase: BaselinePhase::Base, .. } if a == &attempt => Some(seg.clone()),
                _ => None,
            });
            let Some(seg) = seg_of else { continue };
            let decided = lines.iter().any(|l| matches!(l, BaselineLine::Bound { seg: s, .. } | BaselineLine::Abandoned { seg: s, .. } if s == &seg));
            if !decided {
                self.baseline_rt.with(&chat, |c| c.pending.insert(attempt.clone(), seg));
            }
        }
    }

    // ───────────── 終了 E ─────────────

    pub(super) fn start_baseline_driver(self: &Arc<Self>) {
        self.baseline_restore();
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = host.baseline_rt.notify.notified() => {}
                    _ = tokio::time::sleep(POLL_INTERVAL) => {}
                }
                host.baseline_poll();
            }
        });
    }

    /// バックエンドのイベントを見る（待たない）。ルートのturn終端の記録・切断での終了未確認。
    pub(super) fn baseline_observe(self: &Arc<Self>, event: &BackendEvent) {
        match event {
            BackendEvent::TurnEnded { turn, .. } => {
                let chat = ChatKey { backend: turn.agent.backend, id: turn.agent.id.clone() };
                if turn.agent == agent_key_of(&chat) {
                    self.baseline_rt.with_existing(&chat, |c| c.on_root_turn_end(&turn.turn_id));
                }
                self.baseline_rt.notify.notify_one();
            }
            BackendEvent::Connection { state: ConnectionState::Disconnected { .. } } => {
                let chats: Vec<ChatKey> = self.baseline_rt.chats.lock().unwrap().keys().cloned().collect();
                for c in chats {
                    self.baseline_rt.with_existing(&c, |s| s.on_disconnect());
                }
            }
            _ => {}
        }
    }

    /// Eを取る時機の区間を見つけて、作業キューへ積む。
    pub(super) fn baseline_poll(self: &Arc<Self>) {
        let chats: Vec<ChatKey> = self.baseline_rt.chats.lock().unwrap().iter().filter(|(_, c)| c.open.as_ref().is_some_and(|o| o.root_ended && !o.end_in_flight)).map(|(k, _)| k.clone()).collect();
        for chat in chats {
            let quiescent = self.read(|d| chat_quiescent(d, &chat));
            if let Some(seg) = self.baseline_rt.with(&chat, |c| c.reserve_end(quiescent)) {
                let host = self.clone();
                tokio::spawn(async move {
                    let Some(store) = host.persist.store().cloned() else { return };
                    let queue = host.baseline_rt.queue_lock(&chat);
                    let _q = queue.lock().await;
                    // 待っている間に次の送信が前の区間を閉じていたら何もしない。
                    if host.baseline_rt.with(&chat, |c| c.open.as_ref().is_some_and(|o| o.seg == seg)) {
                        host.baseline_end_inner(&chat, &seg, &store).await;
                    }
                });
            }
        }
    }

    /// 区間のEを取って記録し、区間を閉じる（チャットの作業キューを持った状態で呼ぶ）。取れなければEなし＝終了未確認。
    async fn baseline_end_inner(self: &Arc<Self>, chat: &ChatKey, seg: &LocalId, store: &Arc<Store>) {
        let Some(open) = self.baseline_rt.with(chat, |c| c.open.clone()).filter(|o| &o.seg == seg) else { return };
        let at = now_ms();
        // 区間の終わりでも同時作業を確認する（終了直前に始まった別チャットの作業）。
        self.baseline_record_concurrency(chat, seg, &open.repo_root);
        match self.baseline_capture_at(chat, store, Path::new(&open.repo_root)).await {
            Ok((_, snapshot)) => {
                // 取得中に切断・次の送信で区間が閉じられていたら書かない（終了未確認のまま）。確認と書込みは追跡のロック内で行う。
                self.baseline_rt.with(chat, |c| {
                    if c.open.as_ref().is_some_and(|o| &o.seg == seg) {
                        self.baseline_append(chat, BaselineLine::Ended { seg: seg.clone(), at, end: SegmentEnd::Snapshot { snapshot } });
                    }
                });
            }
            Err(reason) => {
                self.baseline_append(chat, BaselineLine::Failed { seg: Some(seg.clone()), at, attempt: open.attempt.clone(), phase: BaselinePhase::End, reason: reason.clone() });
                self.baseline_warn(chat, &reason, BaselinePhase::End);
            }
        }
        self.baseline_rt.with(chat, |c| {
            if c.open.as_ref().is_some_and(|o| &o.seg == seg) {
                c.open = None;
            }
        });
    }

    // ───────────── 同時作業 ─────────────

    fn baseline_others(&self, chat: &ChatKey) -> Vec<OtherChat> {
        let base: Vec<(ChatKey, String, Option<String>, bool, bool)> = self.read(|d| {
            d.chats
                .iter()
                .filter(|c| &c.key != chat && !d.is_side_thread(&c.key))
                .map(|c| {
                    let working = d.agents.iter().any(|v| v.agent.chat == c.key && (d.running_turn.contains_key(&v.agent.key) || is_working(v.status.state)));
                    let external = c.origin == ChatOrigin::External && working;
                    let label = c.name.value().cloned().unwrap_or_else(|| c.key.id.0.clone());
                    (c.key.clone(), label, c.cwd.value().cloned(), working, external)
                })
                .collect()
        });
        base.into_iter()
            .map(|(key, label, cwd, working, external)| {
                let open_seg = self.baseline_rt.open_seg_of(&key);
                OtherChat { busy: working || open_seg.is_some(), external_running: external, open_seg, chat: key, label, cwd }
            })
            .collect()
    }

    /// 同じリポジトリで同時に作業している相手を、双方の区間に記録する。
    fn baseline_record_concurrency(self: &Arc<Self>, chat: &ChatKey, seg: &LocalId, repo_root: &str) {
        for hit in concurrent_hits(repo_root, &self.baseline_others(chat)) {
            if self.baseline_rt.with(chat, |c| c.note_concurrent(&hit.id)) {
                self.baseline_append(chat, BaselineLine::Concurrent { seg: seg.clone(), with: hit.with.clone() });
            }
            if let (Some(other_seg), ConcurrentWith::Chat { chat: other }) = (&hit.other_seg, &hit.with) {
                if self.baseline_rt.with(other, |c| c.note_concurrent(&format!("chat:{}", chat.id.0))) {
                    self.baseline_append(other, BaselineLine::Concurrent { seg: other_seg.clone(), with: ConcurrentWith::Chat { chat: chat.clone() } });
                }
            }
        }
    }

    // ───────────── IPC ─────────────

    /// 控えの状況（区間ごとの状態）。読取りのみ。
    pub async fn get_baseline_status(self: &Arc<Self>, args: ChatArgs) -> Result<Vec<SegmentStatus>, IpcError> {
        self.require_chat(&args.chat)?;
        let Some(store) = self.persist.store().cloned() else { return Ok(Vec::new()) };
        let chat = args.chat.clone();
        let lines = tokio::task::spawn_blocking(move || store.read_baselines(&chat))
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("控えの記録の読込みに失敗しました: {e}")))?
            .map_err(|e| err(IpcErrorCode::Io, super::persist::save_failure_message(&e)))?;
        Ok(derive_status(&lines, &self.baseline_rt.running_segs(&args.chat)))
    }

    /// このチャットの控えを削除する（ユーザーの明示操作。確認は呼出し側で済んでいる）。作業中は削除しない。自動削除はしない。
    pub async fn delete_baselines(self: &Arc<Self>, args: ChatArgs, _confirmed: &UserConfirmed) -> Result<(), IpcError> {
        self.require_chat(&args.chat)?;
        let chat = args.chat;
        let Some(store) = self.persist.store().cloned() else { return Ok(()) };
        let queue = self.baseline_rt.queue_lock(&chat);
        let _q = queue.lock().await;
        let busy = self.read(|d| !chat_quiescent(d, &chat) || d.open_stop(&chat).is_some() || d.locals.get(&chat).is_some_and(|l| !l.pending_ops.is_empty()))
            || self.baseline_rt.running_segs(&chat).len() > 0
            || self.unknown_attempt(&chat).is_some();
        if busy {
            return Err(blocked(BlockedReason::ChatBusy, "作業中、または送信の受理が未確認のため、控えは削除できません。完了または停止の確認後に実行してください"));
        }
        let host = self.clone();
        let c2 = chat.clone();
        let res = tokio::task::spawn_blocking(move || {
            let _g = host.persist.io_guard();
            store.delete_baselines(&c2)
        })
        .await
        .map_err(|e| err(IpcErrorCode::Io, format!("控えの削除に失敗しました: {e}")))?;
        // 削除した記録に結び付く待ちは外す（消えた区間への対応付けを新しい記録に書かない）。
        self.baseline_rt.with_existing(&chat, |c| {
            c.pending.clear();
            c.early_ends.clear();
        });
        self.mutate(|_| ((), vec![HostEvent::ChangesUpdated { chat: chat.clone(), turn: None }]));
        res.map_err(|failed| err(IpcErrorCode::Io, format!("控えの一部を削除できませんでした: {}", failed.join(" / "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> LocalId {
        LocalId(s.into())
    }
    fn eid(s: &str) -> ExternalId {
        ExternalId(s.into())
    }
    fn key(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: eid(s) }
    }
    fn open(seg: &str, attempt: &str) -> OpenSeg {
        OpenSeg { seg: id(seg), attempt: id(attempt), turn: None, repo_root: "C:/repo".into(), concurrent: HashSet::new(), root_ended: false, end_in_flight: false }
    }
    fn started(c: &mut ChatSegs, seg: &str, attempt: &str) {
        c.pending.insert(id(attempt), id(seg));
        c.open = Some(open(seg, attempt));
    }

    #[test]
    fn started_bound_ended_in_order_reserves_the_end_only_when_quiet() {
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        assert_eq!(c.bind(&id("a1"), &eid("t1")), Some(id("s1")));
        assert!(c.pending.is_empty());
        assert_eq!(c.reserve_end(true), None, "the turn end is not observed yet");
        c.on_root_turn_end(&eid("t1"));
        assert_eq!(c.reserve_end(false), None, "children are still working");
        assert_eq!(c.reserve_end(true), Some(id("s1")));
        assert_eq!(c.reserve_end(true), None, "reserved once");
    }

    #[test]
    fn a_turn_end_before_the_acceptance_is_remembered_and_applied_on_bind() {
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        c.on_root_turn_end(&eid("t1"));
        assert!(!c.open.as_ref().unwrap().root_ended, "not matched yet");
        c.bind(&id("a1"), &eid("t1"));
        assert!(c.open.as_ref().unwrap().root_ended);
        // 別のturnの終端は影響しない。
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        c.on_root_turn_end(&eid("t0"));
        c.bind(&id("a1"), &eid("t1"));
        assert!(!c.open.as_ref().unwrap().root_ended);
    }

    #[test]
    fn abandoned_removes_the_segment_and_unknown_acceptance_keeps_it_pending() {
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        // 受理不明のまま: 区間は開いたまま、turnは不明。
        assert!(c.open.is_some() && c.open.as_ref().unwrap().turn.is_none());
        assert_eq!(c.abandon(&id("a1")), Some(id("s1")));
        assert!(c.open.is_none());
        assert_eq!(c.abandon(&id("a1")), None);
    }

    #[test]
    fn the_next_send_decides_the_previous_segment() {
        let mut c = ChatSegs::default();
        assert_eq!(c.prev_action(true), PrevAction::None);
        started(&mut c, "s1", "a1");
        assert_eq!(c.prev_action(true), PrevAction::Unknown(id("s1")), "end never observed -> end unconfirmed, no end is invented");
        c.bind(&id("a1"), &eid("t1"));
        c.on_root_turn_end(&eid("t1"));
        assert_eq!(c.prev_action(true), PrevAction::EndNow(id("s1")));
        assert_eq!(c.prev_action(false), PrevAction::EndIsNextBase(id("s1")), "children still running: the next base is this segment's end");
    }

    #[test]
    fn disconnect_closes_the_open_segment_without_an_end() {
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        c.bind(&id("a1"), &eid("t1"));
        c.on_disconnect();
        assert!(c.open.is_none());
        c.on_root_turn_end(&eid("t1"));
        assert_eq!(c.reserve_end(true), None, "a late end after reconnection does not create an end");
    }

    #[test]
    fn warnings_are_limited_to_once_per_reason_and_skip_quiet_reasons() {
        let mut c = ChatSegs::default();
        let git = BaselineFailure::Git { message: "boom".into() };
        let k = warn_key(&git).unwrap();
        assert!(c.should_warn(k.clone()));
        assert!(!c.should_warn(k));
        assert!(c.should_warn(warn_key(&BaselineFailure::Timeout).unwrap()));
        assert!(c.should_warn(warn_key(&BaselineFailure::Git { message: "other".into() }).unwrap()), "a different reason warns again");
        for quiet in [BaselineFailure::NotARepository, BaselineFailure::WorkFolderUnknown, BaselineFailure::Disabled] {
            assert_eq!(warn_key(&quiet), None);
        }
        let t = warn_text(&BaselineFailure::Timeout, BaselinePhase::Base);
        assert!(t.contains("控えられていません") && t.contains("変更を戻す"));
    }

    fn other(name: &str, cwd: Option<&str>, busy: bool, ext: bool, seg: Option<(&str, &str)>) -> OtherChat {
        OtherChat { chat: key(name), label: name.into(), cwd: cwd.map(str::to_string), busy, external_running: ext, open_seg: seg.map(|(s, r)| (id(s), r.to_string())) }
    }

    #[test]
    fn concurrency_uses_paths_overlap_on_the_repository_root_and_ignores_idle_chats() {
        let hits = concurrent_hits(
            "C:/repo",
            &[
                other("sub", Some(r"c:\repo\app"), true, false, None),
                other("idle", Some("C:/repo"), false, false, None),
                other("far", Some("C:/repo2"), true, false, None),
                other("ext", Some("C:/repo"), true, true, None),
                other("elsewhere", Some("D:/x"), true, false, Some(("s9", "c:\\repo"))),
            ],
        );
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["chat:sub", "ext:ext", "chat:elsewhere"]);
        assert_eq!(hits[0].other_seg, None, "no open segment on the other side to annotate");
        assert!(matches!(&hits[1].with, ConcurrentWith::External { .. }));
        assert_eq!(hits[2].other_seg, Some(id("s9")), "the other chat's open segment in the same repository is annotated too");
    }

    #[test]
    fn concurrency_is_recorded_once_per_partner() {
        let mut c = ChatSegs::default();
        started(&mut c, "s1", "a1");
        assert!(c.note_concurrent("chat:x"));
        assert!(!c.note_concurrent("chat:x"));
        assert!(c.note_concurrent("chat:y"));
    }

    #[test]
    fn status_is_derived_from_the_records_and_unfinished_segments_are_end_unknown() {
        use crate::backend::baseline::{RepoRef, Snapshot};
        let snap = Snapshot { head: None, branch: None, tree: "t".into(), raw: vec![], skipped: vec![], took_ms: 1 };
        let repo = RepoRef { root: "C:/repo".into(), objects_dir: "C:/repo/.git/objects".into() };
        let started = |seg: &str, at: i64| BaselineLine::Started { seg: id(seg), at: UnixMillis(at), attempt: id(&format!("a-{seg}")), repo: repo.clone(), cwd: "C:/repo".into(), base: snap.clone() };
        let lines = vec![
            started("s1", 1),
            BaselineLine::Bound { seg: id("s1"), turn: eid("t1") },
            BaselineLine::Ended { seg: id("s1"), at: UnixMillis(2), end: SegmentEnd::Snapshot { snapshot: snap.clone() } },
            started("s2", 3),
            BaselineLine::Bound { seg: id("s2"), turn: eid("t2") },
            BaselineLine::Concurrent { seg: id("s2"), with: ConcurrentWith::External { label: "x".into() } },
            started("s3", 5),
            BaselineLine::Ended { seg: id("s3"), at: UnixMillis(6), end: SegmentEnd::EndIsNextBase },
            started("s4", 7),
            BaselineLine::Failed { seg: Some(id("s4")), at: UnixMillis(8), attempt: id("a-s4"), phase: BaselinePhase::End, reason: BaselineFailure::Timeout },
            BaselineLine::Failed { seg: Some(id("s5")), at: UnixMillis(9), attempt: id("a5"), phase: BaselinePhase::Base, reason: BaselineFailure::NotARepository },
            BaselineLine::Bound { seg: id("s5"), turn: eid("t5") },
            started("s6", 10),
            BaselineLine::Abandoned { seg: id("s6"), reason: "rejected".into() },
        ];
        let none = HashSet::new();
        let st = derive_status(&lines, &none);
        let states: Vec<&SegmentState> = st.iter().map(|s| &s.state).collect();
        assert_eq!(
            states,
            [&SegmentState::Ready, &SegmentState::EndUnknown, &SegmentState::Ready, &SegmentState::EndUnknown, &SegmentState::Failed { reason: BaselineFailure::NotARepository }, &SegmentState::Abandoned]
        );
        assert_eq!(st[0].turn, Some(eid("t1")));
        assert!(st[1].concurrent && !st[0].concurrent);
        assert_eq!(st[4].turn, Some(eid("t5")), "a failed base is still bound to its turn");
        // 開いている区間は作業中。
        let running: HashSet<LocalId> = [id("s2")].into_iter().collect();
        assert_eq!(derive_status(&lines, &running)[1].state, SegmentState::Running);
    }

    // ── ホスト結合: 一時リポジトリ＋一時の保存先（gitがなければスキップ）──

    use std::process::Command;

    fn git_ok() -> bool {
        Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// ユーザーのリポジトリの写し（索引のバイト列・refs・objectsのファイル一覧）。
    fn repo_state(repo: &Path) -> (Vec<u8>, String, Vec<String>) {
        let index = std::fs::read(repo.join(".git").join("index")).unwrap();
        let refs = String::from_utf8_lossy(&Command::new("git").current_dir(repo).args(["for-each-ref"]).output().unwrap().stdout).into_owned();
        fn walk(d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push(e.path().to_string_lossy().into_owned());
                }
            }
        }
        let mut files = Vec::new();
        walk(&repo.join(".git").join("objects"), &mut files);
        files.sort();
        (index, refs, files)
    }

    struct Fixture {
        base: std::path::PathBuf,
        repo: std::path::PathBuf,
        host: Arc<Host>,
        warnings: Arc<Mutex<Vec<String>>>,
    }

    fn fixture(tag: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!("agentdock-bhost-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q"]);
        for (k, v) in [("user.email", "t@example.com"), ("user.name", "t")] {
            sh(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), b"one\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), b"one\r\nchanged\r\n").unwrap();
        let store = Arc::new(Store::open(base.join("data")).unwrap());
        let host = Arc::new(Host::with_store(base.join("data"), store));
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let w = warnings.clone();
        host.set_emitter(Arc::new(move |env| {
            if let HostEvent::Warning { message, .. } = env.event {
                w.lock().unwrap().push(message);
            }
        }));
        Fixture { base, repo, host, warnings }
    }

    fn lines_of(f: &Fixture, chat: &ChatKey) -> Vec<BaselineLine> {
        f.host.persist.store().unwrap().read_baselines(chat).unwrap()
    }

    fn status_of(f: &Fixture, chat: &ChatKey) -> Vec<SegmentStatus> {
        derive_status(&lines_of(f, chat), &f.host.baseline_rt.running_segs(chat))
    }

    fn turn_ended(chat: &ChatKey, turn: &str) -> BackendEvent {
        BackendEvent::TurnEnded {
            turn: TurnKey { agent: agent_key_of(chat), turn_id: eid(turn) },
            end: TurnEnd::Completed,
            error: None,
            evidence: Evidence { source: EvidenceSource::LiveEvent, raw_label: None, source_time: None, observed_at: UnixMillis(1) },
        }
    }

    async fn wait_for(f: &Fixture, chat: &ChatKey, what: impl Fn(&[BaselineLine]) -> bool) -> bool {
        for _ in 0..100 {
            if what(&lines_of(f, chat)) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn base_and_end_are_recorded_around_a_turn_and_the_user_repository_is_untouched() {
        if !git_ok() {
            return;
        }
        let f = fixture("flow");
        let chat = key("c1");
        let repo_path = f.repo.to_string_lossy().into_owned();
        let before = repo_state(&f.repo);

        f.host.baseline_take_base(&chat, &id("att-1"), Some(repo_path.clone())).await;
        let l = lines_of(&f, &chat);
        if matches!(l.first(), Some(BaselineLine::Failed { reason: BaselineFailure::GitTooOld { .. }, .. })) {
            std::fs::remove_dir_all(&f.base).ok();
            return;
        }
        assert!(matches!(l.as_slice(), [BaselineLine::Started { .. }]), "{l:?}");
        assert!(f.host.baseline_rt.with(&chat, |c| c.open.is_some()));
        assert_eq!(status_of(&f, &chat)[0].state, SegmentState::Running);

        // 受理 → ルートのturn終端 → エージェントは全員止まっている → Eを取る。
        f.host.baseline_bound(&chat, &id("att-1"), &eid("t1"));
        f.host.baseline_observe(&turn_ended(&chat, "t1"));
        f.host.baseline_poll();
        assert!(wait_for(&f, &chat, |l| l.iter().any(|x| matches!(x, BaselineLine::Ended { end: SegmentEnd::Snapshot { .. }, .. }))).await, "{:?}", lines_of(&f, &chat));
        assert!(f.host.baseline_rt.with(&chat, |c| c.open.is_none()));
        let st = status_of(&f, &chat);
        assert_eq!((st[0].state.clone(), st[0].turn.clone()), (SegmentState::Ready, Some(eid("t1"))));

        // 次の送信の前に終端が観測されていなければ、前の区間は終了未確認のまま（Eは作らない）。
        f.host.baseline_take_base(&chat, &id("att-2"), Some(repo_path.clone())).await;
        f.host.baseline_take_base(&chat, &id("att-3"), Some(repo_path.clone())).await;
        let states: Vec<SegmentState> = status_of(&f, &chat).iter().map(|s| s.state.clone()).collect();
        assert_eq!(states, [SegmentState::Ready, SegmentState::EndUnknown, SegmentState::Running]);

        // 受理なしが確定した区間は外れる。切断で開いている区間は終了未確認のまま閉じる。
        f.host.baseline_abandoned(&chat, &id("att-3"), "rejected");
        assert!(f.host.baseline_rt.with(&chat, |c| c.open.is_none()));
        assert_eq!(status_of(&f, &chat).last().unwrap().state, SegmentState::Abandoned);
        f.host.baseline_take_base(&chat, &id("att-4"), Some(repo_path)).await;
        f.host.baseline_observe(&BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } });
        assert!(f.host.baseline_rt.with(&chat, |c| c.open.is_none()));
        assert_eq!(status_of(&f, &chat).last().unwrap().state, SegmentState::EndUnknown);
        assert!(f.warnings.lock().unwrap().is_empty(), "{:?}", f.warnings.lock().unwrap());

        // ユーザーの索引・refs・objectsは何も変わらない。
        assert_eq!(repo_state(&f.repo), before);
        assert_eq!(std::fs::read(f.repo.join("a.txt")).unwrap(), b"one\r\nchanged\r\n");
        std::fs::remove_dir_all(&f.base).ok();
    }

    #[tokio::test]
    async fn failures_do_not_stop_the_send_and_warn_once_per_reason() {
        if !git_ok() {
            return;
        }
        let f = fixture("fail");
        let chat = key("c2");
        let repo_path = f.repo.to_string_lossy().into_owned();
        // 無効: 取らない。警告なし。
        f.host.data.lock().unwrap().settings.baselines.enabled = false;
        f.host.baseline_take_base(&chat, &id("a1"), Some(repo_path.clone())).await;
        assert!(matches!(lines_of(&f, &chat).as_slice(), [BaselineLine::Failed { reason: BaselineFailure::Disabled, phase: BaselinePhase::Base, .. }]));
        assert!(f.warnings.lock().unwrap().is_empty());
        // 作業フォルダ不明: 警告なし。
        f.host.data.lock().unwrap().settings.baselines.enabled = true;
        f.host.baseline_take_base(&chat, &id("a2"), None).await;
        assert!(matches!(lines_of(&f, &chat).last(), Some(BaselineLine::Failed { reason: BaselineFailure::WorkFolderUnknown, .. })));
        assert!(f.warnings.lock().unwrap().is_empty());
        // Gitを起動できない: 2回続けても警告は1回。区間は開かない。
        f.host.data.lock().unwrap().settings.tools.git = Some("agentdock-no-such-git".into());
        f.host.baseline_take_base(&chat, &id("a3"), Some(repo_path.clone())).await;
        f.host.baseline_take_base(&chat, &id("a4"), Some(repo_path)).await;
        let unavailable = lines_of(&f, &chat).iter().filter(|l| matches!(l, BaselineLine::Failed { reason: BaselineFailure::GitUnavailable, .. })).count();
        assert_eq!(unavailable, 2);
        assert_eq!(f.warnings.lock().unwrap().len(), 1, "{:?}", f.warnings.lock().unwrap());
        assert!(f.warnings.lock().unwrap()[0].contains("控えられていません"));
        assert!(f.host.baseline_rt.with(&chat, |c| c.open.is_none()));
        // 失敗した区間も、受理されたturnと結び付く。
        f.host.baseline_bound(&chat, &id("a3"), &eid("t9"));
        assert!(status_of(&f, &chat).iter().any(|s| s.turn == Some(eid("t9")) && matches!(s.state, SegmentState::Failed { reason: BaselineFailure::GitUnavailable })));
        std::fs::remove_dir_all(&f.base).ok();
    }

    fn chat_in(key: &ChatKey, cwd: &str) -> Chat {
        Chat {
            key: key.clone(),
            kind: ChatKind::Development,
            cwd: Known::direct(cwd.to_string()),
            name: Known::NotFetched,
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
            no_history: false,
        }
    }

    #[tokio::test]
    async fn a_concurrent_chat_in_the_same_repository_is_recorded_on_both_sides() {
        if !git_ok() {
            return;
        }
        let f = fixture("conc");
        let (c1, c2) = (key("c1"), key("c2"));
        let repo_path = f.repo.to_string_lossy().into_owned();
        f.host.baseline_take_base(&c1, &id("a1"), Some(repo_path.clone())).await;
        if matches!(lines_of(&f, &c1).first(), Some(BaselineLine::Failed { .. })) {
            std::fs::remove_dir_all(&f.base).ok();
            return;
        }
        // c1の区間が開いている間に、c2が同じリポジトリで始める。
        f.host.data.lock().unwrap().chats.push(chat_in(&c1, &repo_path));
        f.host.baseline_take_base(&c2, &id("a2"), Some(repo_path)).await;
        let has_concurrent = |l: &[BaselineLine], other: &ChatKey| l.iter().any(|x| matches!(x, BaselineLine::Concurrent { with: ConcurrentWith::Chat { chat }, .. } if chat == other));
        assert!(has_concurrent(&lines_of(&f, &c2), &c1));
        assert!(has_concurrent(&lines_of(&f, &c1), &c2));
        assert!(status_of(&f, &c1)[0].concurrent);
        std::fs::remove_dir_all(&f.base).ok();
    }
}
