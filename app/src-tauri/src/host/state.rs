//! ホストが持つ表示用の状態と、バックエンドのイベントからUIイベントへの変換（reducer）。
//!
//! すべて同期・純粋（時間は引数）。通信は持たない。規則:
//! - 状態（`AgentStatus`）と鮮度（`Freshness`）は別に更新する。切断は鮮度だけを変え、状態は最後の明示値のまま。
//! - 古いturnの後着の状態は捨てる（`latest_turn_start`）。
//! - 承認・質問は自動回答しない。解決通知は「保留中」のものだけ更新し、回答済みを上書きしない。
//! - 活動の逐次本文（`ActivityDelta`）は描画用に転送するだけで、状態判定に使わない。

use std::collections::{HashMap, HashSet};

use super::stop;
use crate::backend::backend::*;
use crate::backend::changes::FileChange;
use crate::backend::ipc::*;
use crate::backend::local::{AppSettings, ChatMarks, SaveScope, SaveStatus};
use crate::backend::model::*;
use crate::rules::notify::NotifyInput;
use crate::store::records::{ActivityLine, ChatLocalFile, QueueFile};

/// イベント適用後にホストが非同期で行う追加作業（reducer内では待たない）。
#[derive(Debug, Clone, PartialEq)]
pub enum Followup {
    /// ルートの新しいturn開始を契機に、子孫を再走査する（読み取りのみ）。
    ScanDescendants(AgentKey),
    /// 会話で作られたファイルの候補。実在を確認できたものだけ成果物にする（ファイルを読むので別taskで行う）。
    ObserveArtifact { agent: AgentKey, item: ItemKey, path: String },
    /// 完了したファイル変更の報告。ファイルを読んで観測記録（`changes.jsonl`）に追記する（別taskで行う）。
    ObserveChanges { agent: AgentKey, item: ItemKey, changes: Vec<FileChange> },
}

/// 履歴の読取りで確認した、最新turnの末尾の様子（状態が不明のエージェントを未完了と数えるかの根拠）。
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryTail {
    /// turnがまだない。
    NoTurns,
    /// 最新turnの終端（完了・失敗・中断）を確認した。
    Ended(ExternalId, TurnEnd),
    /// 最新turnが進行中のまま、または終端を判別できない（外部で実行中の可能性）。
    NotTerminal,
}

pub struct HostData {
    pub seq: u64,
    pub sources: Vec<SourceInfo>,
    pub chats: Vec<Chat>,
    pub agents: Vec<AgentView>,
    pub requests: Vec<PendingRequest>,
    pub stops: Vec<StopRecord>,
    pub monitor_scope: MonitorScope,
    pub pinned: HashSet<ChatKey>,
    /// このアプリが開始した会話（アプリ管理。外部会話の再開は含めない）。originに関わらず送信可能として扱う。再起動後も `chat.json` から復元する。
    pub hosted: HashSet<ChatKey>,
    pub model_settings: HashMap<ChatKey, ChatModelSettings>,
    /// アプリ側の補足情報（保存ファイルと同じ形。ピン・下書き・モデル/権限など）。保存は `host::persist` が行う。
    pub locals: HashMap<ChatKey, ChatLocalFile>,
    /// 復元したキュー記録（P3が使う。起動時に Active→PausedAfterRestart、Sending→AcceptanceUnknown に変換済み）。
    pub queues: HashMap<ChatKey, QueueFile>,
    pub save_status: HashMap<SaveScope, SaveStatus>,
    pub settings: AppSettings,
    /// worktree台帳（AgentDockが作ったもの。保存は `host::worktree` が `worktrees.json` へ書く）。
    pub worktrees: Vec<WorktreeRecord>,
    /// クラウド委任の記録（保存は `host::cloud` が `cloud-tasks.json` へ書く）。
    pub cloud_tasks: Vec<CloudTaskRecord>,
    /// 起動時に読めなかった保存ファイルなどの警告。
    pub startup_warnings: Vec<StartupWarning>,
    /// side相談の一時の会話 → 主会話（P3-4）。一覧に出さず、キュー・子孫の条件に入れない。停止対象には入れる（`stop_scope`）。
    /// 終了した相談も、停止の確認が済むまで外さない（主会話の削除で外す）。
    pub side_threads: HashMap<ChatKey, ChatKey>,
    /// 親のspawn依頼から確定できた子の担当（エージェントの再登録で失わないよう保持）。
    assignments: HashMap<AgentKey, String>,
    /// 実行中と分かっているturn（中断・追加指示の対象）。
    pub running_turn: HashMap<AgentKey, ExternalId>,
    /// 開始を観測した最新turn（古いturnの後着を捨てる基準）。
    latest_turn_start: HashMap<AgentKey, ExternalId>,
    /// 終端を観測したturn（中断記録の作成が終端通知より遅れた場合の補完）。
    pub(super) last_end: HashMap<AgentKey, (ExternalId, TurnEnd, UnixMillis)>,
    /// このセッションで非終端（initializing/running/waiting）を観測したエージェント。通知は「非終端→終端」だけを起点にする。
    nonterminal_seen: HashSet<AgentKey>,
    /// 現在の状態が失敗のエージェント（印の更新イベントを状態の出入りのときだけ出すため）。
    failed_seen: HashSet<AgentKey>,
    /// 通知の入力（ホストが `mutate` の直後に取り出して `Notifier` へ渡す）。
    pub notify_inbox: Vec<NotifyInput>,
    /// 状態が不明（notLoaded等）で live でないエージェントについて、履歴の読取りで確認した最新turnの末尾。liveになる・新しいturnで無効にする。
    pub history_terminal: HashMap<AgentKey, (HistoryTail, UnixMillis)>,
    /// wake後の照合に成功し、まだ live 通知を受けていないエージェント（照合を終えた時刻）。切断・新しい接続で破棄する。
    wake_reconciled: HashMap<AgentKey, UnixMillis>,
    /// 監視活動履歴（`activity.jsonl`）へ追記する行（ホストが `mutate` の直後に取り出す）。
    pub activity_outbox: Vec<(ChatKey, ActivityLine)>,
    /// turn単位の集約diffの最新値（描画・差分表示用。再起動後は持たない）。古いものから `TURN_DIFF_KEEP` 件まで。
    turn_diffs: HashMap<TurnKey, String>,
    turn_diff_order: Vec<TurnKey>,
}

/// 保持するturn集約diffの件数（超えたら古いturnから捨てる）。
const TURN_DIFF_KEEP: usize = 64;

impl Default for HostData {
    fn default() -> Self {
        HostData {
            seq: 0,
            sources: Vec::new(),
            chats: Vec::new(),
            agents: Vec::new(),
            requests: Vec::new(),
            stops: Vec::new(),
            monitor_scope: MonitorScope::SelectedChat { chat: None },
            pinned: HashSet::new(),
            hosted: HashSet::new(),
            model_settings: HashMap::new(),
            locals: HashMap::new(),
            queues: HashMap::new(),
            save_status: HashMap::new(),
            settings: AppSettings::default(),
            worktrees: Vec::new(),
            cloud_tasks: Vec::new(),
            startup_warnings: Vec::new(),
            side_threads: HashMap::new(),
            assignments: HashMap::new(),
            running_turn: HashMap::new(),
            latest_turn_start: HashMap::new(),
            last_end: HashMap::new(),
            nonterminal_seen: HashSet::new(),
            failed_seen: HashSet::new(),
            notify_inbox: Vec::new(),
            history_terminal: HashMap::new(),
            wake_reconciled: HashMap::new(),
            activity_outbox: Vec::new(),
            turn_diffs: HashMap::new(),
            turn_diff_order: Vec::new(),
        }
    }
}

/// 外部作成の会話で、ユーザーが再開して live 購読が成立するまで送信を止めるか。
/// 外部実行中かどうかは断定しない（再開はユーザーの確認後だけ）。
pub fn external_send_locked(origin: ChatOrigin, root_freshness: Option<Freshness>) -> bool {
    origin == ChatOrigin::External && root_freshness != Some(Freshness::Live)
}

/// 作業フォルダがアプリ専用領域内（一般チャット）の会話は、AgentDockが領域を割り当てて開始したものなので、
/// 記録（hosted）を失っていてもアプリ管理として扱う（App Serverは自分の開始分も `vscode` 等と返すことがある）。
pub fn is_app_workspace_chat(c: &Chat) -> bool {
    c.kind == ChatKind::General
}

/// 一覧の並びに使う時刻（最近利用時刻。無ければ作成時刻）。どちらも無ければ比較しない。
fn list_time(c: &Chat) -> Option<UnixMillis> {
    c.last_used_at.or_else(|| c.created_at.value().copied())
}

/// 一覧の並び（§3.6）。ピンを上、次に最近利用時刻の降順（新規作成は作成時刻）、時刻不明は最後。安定ソートなので同順位は元の順を保つ。
/// 最近利用時刻はユーザー操作（作成・開く・送信）でだけ更新される。背景活動・一覧取得・走査では変えない。
pub fn sort_chats(chats: &mut [Chat]) {
    chats.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned).then_with(|| match (list_time(a), list_time(b)) {
            (Some(x), Some(y)) => y.0.cmp(&x.0),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
}

fn is_active(s: AgentState) -> bool {
    matches!(s, AgentState::Running | AgentState::Waiting | AgentState::Initializing)
}

pub fn agent_key_of(chat: &ChatKey) -> AgentKey {
    AgentKey { backend: chat.backend, id: chat.id.clone() }
}

fn evidence(source: EvidenceSource, label: &str, now: UnixMillis) -> Evidence {
    Evidence { source, raw_label: Some(label.to_string()), source_time: None, observed_at: now }
}

impl HostData {
    pub fn snapshot(&self) -> HostSnapshot {
        HostSnapshot {
            seq: self.seq,
            sources: self.sources.clone(),
            chats: self.chats.clone(),
            agents: self.agents.clone(),
            requests: self.requests.clone(),
            stops: self.stops.clone(),
            queues: self.queues.values().map(|f| f.queue.clone()).collect(),
            monitor_scope: self.monitor_scope.clone(),
            chat_locals: self.local_views(),
            save_status: self.save_status.values().cloned().collect(),
            settings: self.settings.clone(),
            model_settings: self.model_settings.iter().map(|(chat, settings)| ChatModelEntry { chat: chat.clone(), settings: settings.clone() }).collect(),
            startup_warnings: crate::host::persist::pending_warnings(&self.startup_warnings, &self.settings.acknowledged_warnings),
            attachments: self.locals.values().flat_map(|f| f.attachments.iter().cloned()).collect(),
            artifacts: self.locals.values().flat_map(|f| f.artifacts.iter().cloned()).collect(),
            quit: crate::backend::local::QuitPhase::Idle, // 手順の状態は `Host::snapshot` が重ねる
            op_capabilities: Vec::new(), // `Host::snapshot` がバックエンドの宣言を重ねる
            pending_ops: self
                .locals
                .iter()
                .flat_map(|(chat, f)| f.pending_ops.iter().map(|p| crate::backend::model::ChatPendingOp { chat: chat.clone(), pending: p.clone() }))
                .collect(),
            side_sessions: self.locals.values().flat_map(|f| f.side_sessions.iter().cloned()).collect(),
            worktrees: self.worktrees.clone(),
            cloud_tasks: self.cloud_tasks.clone(),
        }
    }

    // ── 検索 ──

    pub fn chat(&self, k: &ChatKey) -> Option<&Chat> {
        self.chats.iter().find(|c| &c.key == k)
    }

    pub fn view(&self, k: &AgentKey) -> Option<&AgentView> {
        self.agents.iter().find(|v| &v.agent.key == k)
    }

    fn view_mut(&mut self, k: &AgentKey) -> Option<&mut AgentView> {
        self.agents.iter_mut().find(|v| &v.agent.key == k)
    }

    /// 所属チャットが分からないまま作られた仮のエージェント（状態通知だけが先に届いた子など）。
    /// 自身のIDを仮の所属にし、親は不明。ドック（チャット単位の表示）には出ないので、走査で正しい所属へ直す。
    pub fn is_synthetic(v: &AgentView) -> bool {
        matches!(v.agent.parent, ParentLink::Unknown) && v.agent.chat.id == v.agent.key.id
    }

    /// 走査で見つかったエージェントを状態へ反映する必要があるか（未登録、または仮の登録のまま）。
    pub fn needs_adoption(&self, k: &AgentKey) -> bool {
        self.view(k).is_none_or(Self::is_synthetic)
    }

    /// 状態・鮮度は変えず、所属チャット・親などエージェントの属性だけを正しいものに置き換える。
    pub fn adopt_agent(&mut self, agent: Agent) -> Vec<HostEvent> {
        let agent = self.with_assignment(agent);
        match self.view_mut(&agent.key) {
            Some(v) => {
                v.agent = agent;
                vec![HostEvent::AgentUpdated { view: v.clone() }]
            }
            None => Vec::new(),
        }
    }

    pub fn root_view(&self, chat: &ChatKey) -> Option<&AgentView> {
        self.view(&agent_key_of(chat))
    }

    /// 担当が未取得なら、確定済みの担当を入れる。
    fn with_assignment(&self, mut a: Agent) -> Agent {
        // 親のspawn依頼で確定した担当（直接）は、子の最初の依頼からの導出値より優先する。
        let derived_or_none = !matches!(a.assignment, Known::Value { basis: Basis::Direct, .. });
        if derived_or_none {
            if let Some(t) = self.assignments.get(&a.key) {
                a.assignment = Known::direct(t.clone());
            }
        }
        a
    }

    pub fn is_root(&self, k: &AgentKey) -> bool {
        self.view(k).is_some_and(|v| matches!(v.agent.parent, ParentLink::Root))
    }

    /// 停止未確認・中断要求中・所有不明を含む記録（新しい送信・削除を止める）。
    pub fn open_stop(&self, chat: &ChatKey) -> Option<&StopRecord> {
        self.stops.iter().find(|r| &r.chat == chat && r.blocks_deletion())
    }

    // ── 履歴で確認した末尾・監視活動の記録・wake後の復帰 ──

    /// 状態が不明のエージェントの最新turnの末尾を、履歴の読取り結果として記録する。
    pub fn note_history_tail(&mut self, agent: &AgentKey, tail: HistoryTail, now: UnixMillis) {
        self.history_terminal.insert(agent.clone(), (tail, now));
    }

    /// 状態が不明のエージェントのうち、履歴で終端（またはturnなし）を確認できたもの。未完了と数えない。
    /// 最新turnが記録と食い違う（新しいturnが始まった）ときは根拠にしない。
    pub fn unknown_confirmed_idle(&self, v: &AgentView) -> bool {
        if v.status.state != AgentState::Unknown {
            return false;
        }
        match self.history_terminal.get(&v.agent.key) {
            Some((HistoryTail::NoTurns, _)) => v.agent.latest_turn.is_none(),
            Some((HistoryTail::Ended(t, _), _)) => v.agent.latest_turn.as_ref().is_none_or(|l| l == t),
            _ => false,
        }
    }

    /// 履歴を読めたうえで、最新turnが進行中（終端を確認できない）の状態不明のエージェント。外部実行の可能性がある。
    pub fn unknown_maybe_running(&self, v: &AgentView) -> bool {
        v.status.state == AgentState::Unknown && matches!(self.history_terminal.get(&v.agent.key), Some((HistoryTail::NotTerminal, _)))
    }

    fn chat_of_agent(&self, a: &AgentKey) -> Option<ChatKey> {
        self.view(a).map(|v| v.agent.chat.clone())
    }

    fn record_activity(&mut self, chat: ChatKey, line: ActivityLine) {
        // side相談の会話の記録は、監視活動履歴（チャット領域）に残さない（一時の会話。記録は主会話のside記録に別に残す）。
        if self.side_threads.contains_key(&chat) {
            return;
        }
        self.activity_outbox.push((chat, line));
    }

    /// side相談の一時の会話か。
    pub fn is_side_thread(&self, chat: &ChatKey) -> bool {
        self.side_threads.contains_key(chat)
    }

    /// 主会話の停止の範囲（主会話＋その side の会話）。完全終了・強制終了・削除の停止照合が使う。
    pub fn stop_scope(&self, main: &ChatKey) -> Vec<ChatKey> {
        crate::rules::side::stop_scope(main, &self.side_threads)
    }

    fn record_status(&mut self, agent: &AgentKey, status: &AgentStatus, at: UnixMillis) {
        if let Some(chat) = self.chat_of_agent(agent) {
            let line = ActivityLine::StatusChanged {
                at,
                agent: agent.clone(),
                state: status.state,
                raw: status.raw.label.clone(),
                turn: status.turn.clone(),
                source: status.evidence.source,
            };
            self.record_activity(chat, line);
        }
    }

    /// 鮮度を変えて記録する（変化があったときだけ）。
    pub fn set_agent_freshness(&mut self, key: &AgentKey, to: Freshness, now: UnixMillis) -> Vec<HostEvent> {
        let mut out = Vec::new();
        let mut line = None;
        if let Some(v) = self.view_mut(key) {
            if v.freshness != to {
                v.freshness = to;
                out.push(HostEvent::AgentUpdated { view: v.clone() });
                line = Some((v.agent.chat.clone(), ActivityLine::Freshness { at: now, agent: Some(key.clone()), freshness: to }));
            }
        }
        if let Some((chat, l)) = line {
            self.record_activity(chat, l);
        }
        out
    }

    /// wake後の照合の読み取りが成功した。以後、同じ接続でlive通知を受けたら live に戻す（H4）。
    /// あわせて、照合が終わるまでの再観測を「非終端→終端」の通知にしない（L1）。
    pub fn note_wake_reconciled(&mut self, agent: &AgentKey, at: UnixMillis) {
        if self.view(agent).is_some_and(|v| v.freshness == Freshness::HistoryOnly) {
            self.wake_reconciled.insert(agent.clone(), at);
        }
    }

    /// wakeで要照合にしたエージェントの「非終端を観測した」印を外す（sleep中に終わったものを後から通知しない）。
    pub fn forget_nonterminal(&mut self, agents: &[AgentKey]) {
        for a in agents {
            self.nonterminal_seen.remove(a);
        }
    }

    /// 照合の後にlive通知を受けたエージェントを live に戻す。照合に成功していないもの・切断をまたいだものは戻さない。
    fn restore_live_after_wake(&mut self, agent: &AgentKey, now: UnixMillis) -> Vec<HostEvent> {
        match self.wake_reconciled.get(agent) {
            Some(at) if now.0 > at.0 => {
                self.wake_reconciled.remove(agent);
                if self.view(agent).is_some_and(|v| v.freshness == Freshness::HistoryOnly) {
                    return self.set_agent_freshness(agent, Freshness::Live, now);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    // ── 更新（呼び出し側が返ったイベントを発行する） ──

    pub fn set_source(&mut self, mut info: SourceInfo) -> Vec<HostEvent> {
        if let Some(old) = self.sources.iter().find(|s| s.source == info.source) {
            // 切断通知が先に届いていたら、後から返った接続成功で上書きしない。
            if matches!(old.connection, ConnectionState::Disconnected { .. }) {
                info.connection = old.connection.clone();
            }
        }
        if self.sources.iter().any(|s| s.source != info.source) {
            // 新しい接続。以前の購読は失われているので、wake後の照合の記録は引き継がない。
            // アプリ管理（hosted）は会話の属性として永続するので、接続をまたいでも消さない。
            self.wake_reconciled.clear();
        }
        // 同じIDの仮の項目（接続通知が結果より先に届いた場合）も置き換える。残すと先頭の仮項目が表示される。
        self.sources.clear();
        self.sources.push(info.clone());
        vec![HostEvent::SourceUpdated { source: info }]
    }

    /// バックエンドが示した、会話に設定されているモデルを「受理した設定」として記録する（選択値・実効値とは別）。
    pub fn note_accepted_model(&mut self, chat: &ChatKey, choice: ModelChoice) -> Vec<HostEvent> {
        let s = self.model_settings.entry(chat.clone()).or_insert_with(|| ChatModelSettings::blank(ApplyTiming::NextTurn));
        let accepted = Known::direct(choice);
        if s.accepted == accepted {
            return Vec::new();
        }
        s.accepted = accepted;
        vec![HostEvent::ModelSettingsUpdated { chat: chat.clone(), settings: s.clone() }]
    }

    /// バックエンドが示した、会話の計画／実行の設定を「受理した設定」として記録する（選択値とは別）。
    pub fn note_accepted_work_mode(&mut self, chat: &ChatKey, mode: WorkMode) -> Vec<HostEvent> {
        let s = self.model_settings.entry(chat.clone()).or_insert_with(|| ChatModelSettings::blank(ApplyTiming::NextTurn));
        let accepted = Known::direct(mode);
        if s.accepted_work_mode == accepted {
            return Vec::new();
        }
        s.accepted_work_mode = accepted;
        vec![HostEvent::ModelSettingsUpdated { chat: chat.clone(), settings: s.clone() }]
    }

    /// ユーザー操作での再開（`thread/resume`）の応答を反映する。履歴・live購読を戻し、応答が示した会話のモデル設定を
    /// 「受理した設定」として記録する（示されなければ未取得のまま。選択値は変えない）。
    pub fn apply_resumed(&mut self, chat: &ChatKey, history: AgentHistory, accepted_model: Known<ModelChoice>) -> Vec<HostEvent> {
        let mut ev = Vec::new();
        if let Some(c) = &history.chat {
            ev.extend(self.upsert_chat(c.clone()));
        }
        ev.extend(self.set_live(history.agent, history.status));
        if let Known::Value { value, .. } = accepted_model {
            ev.extend(self.note_accepted_model(chat, value));
        }
        ev
    }

    /// このホストが開始・再開した会話として記録する（origin表示を自管理に揃える）。
    pub fn mark_hosted(&mut self, chat: &ChatKey) -> Vec<HostEvent> {
        self.hosted.insert(chat.clone());
        match self.chats.iter_mut().find(|c| &c.key == chat) {
            Some(c) if c.origin != ChatOrigin::AppManaged => {
                c.origin = ChatOrigin::AppManaged;
                vec![HostEvent::ChatUpdated { chat: c.clone() }]
            }
            _ => Vec::new(),
        }
    }

    /// ユーザー操作（作成・開く・送信）で最近利用時刻を更新する。背景活動からは呼ばない。
    pub fn mark_used(&mut self, chat: &ChatKey, now: UnixMillis) -> Vec<HostEvent> {
        let Some(c) = self.chats.iter_mut().find(|c| &c.key == chat) else { return Vec::new() };
        c.last_used_at = Some(now);
        let c = c.clone();
        sort_chats(&mut self.chats);
        vec![HostEvent::ChatUpdated { chat: c }]
    }

    pub fn upsert_chat(&mut self, mut chat: Chat) -> Vec<HostEvent> {
        chat.pinned = self.pinned.contains(&chat.key);
        if self.hosted.contains(&chat.key) || is_app_workspace_chat(&chat) {
            chat.origin = ChatOrigin::AppManaged;
        }
        match self.chats.iter_mut().find(|c| c.key == chat.key) {
            Some(old) => {
                chat.draft = old.draft.clone();
                if chat.preview.value().is_none() {
                    chat.preview = old.preview.clone();
                }
                chat.last_used_at = old.last_used_at;
                *old = chat.clone();
            }
            None => {
                // 再起動後の最初の表示では、保存してある最近利用時刻を使う（背景活動では更新しない）。
                if chat.last_used_at.is_none() {
                    chat.last_used_at = self.locals.get(&chat.key).and_then(|l| l.last_used_at);
                }
                self.chats.push(chat.clone());
            }
        }
        sort_chats(&mut self.chats);
        vec![HostEvent::ChatUpdated { chat }]
    }

    /// 履歴・一覧から得た状態を反映する。live購読中のエージェントの状態は、古い可能性がある読み取りで上書きしない。
    /// 子孫（ルート以外）の発見・状態変化は監視活動として記録する（一覧のルートは記録しない）。
    pub fn upsert_history_agent(&mut self, agent: Agent, status: AgentStatus) -> Vec<HostEvent> {
        let agent = self.with_assignment(agent);
        let key = agent.key.clone();
        let is_root = matches!(agent.parent, ParentLink::Root);
        let at = status.evidence.observed_at;
        let mut applied = None;
        if let Some(v) = self.view_mut(&key) {
            v.agent = agent;
            let before = (v.status.state, v.status.turn.clone());
            if v.freshness != Freshness::Live {
                applied = Some((status.state, status.turn.clone()));
                v.status = status.clone();
                v.freshness = Freshness::HistoryOnly;
            }
            let changed = applied.is_some() && before != (status.state, status.turn.clone());
            let mut out = vec![HostEvent::AgentUpdated { view: v.clone() }];
            if changed && !is_root {
                self.record_status(&key, &status, at);
            }
            if let Some((state, turn)) = applied {
                out.extend(self.note_state(&key, state, turn));
            }
            return out;
        }
        let (state, turn) = (status.state, status.turn.clone());
        let view = AgentView { agent: agent.clone(), status: status.clone(), freshness: Freshness::HistoryOnly, current_activity: None };
        self.agents.push(view.clone());
        if !is_root {
            self.record_activity(agent.chat.clone(), ActivityLine::AgentSeen { at, agent });
            self.record_status(&key, &status, at);
        }
        let mut out = vec![HostEvent::AgentUpdated { view }];
        out.extend(self.note_state(&key, state, turn));
        out
    }

    /// 再開・新規作成でlive購読が戻ったエージェントを反映する。
    pub fn set_live(&mut self, agent: Agent, status: AgentStatus) -> Vec<HostEvent> {
        let agent = self.with_assignment(agent);
        let key = agent.key.clone();
        self.history_terminal.remove(&key);
        self.wake_reconciled.remove(&key);
        if let Some(t) = &status.turn {
            if is_active(status.state) {
                self.running_turn.insert(key.clone(), t.clone());
            }
        }
        // 作業中でなければ、残っていた実行中turnの印を外す（切断・再起動をまたいだ死んだturnで送信を止めない）。
        if !is_active(status.state) {
            self.running_turn.remove(&key);
        }
        let (state, turn) = (status.state, status.turn.clone());
        let at = status.evidence.observed_at;
        let is_new = self.view(&key).is_none();
        let prev = self.view(&key).map(|v| (v.status.state, v.status.turn.clone(), v.freshness));
        let view = match self.view_mut(&key) {
            Some(v) => {
                v.agent = agent;
                v.status = status.clone();
                v.freshness = Freshness::Live;
                v.clone()
            }
            None => {
                let view = AgentView { agent, status: status.clone(), freshness: Freshness::Live, current_activity: None };
                self.agents.push(view.clone());
                view
            }
        };
        let chat = view.agent.chat.clone();
        if is_new {
            self.record_activity(chat.clone(), ActivityLine::AgentSeen { at, agent: view.agent.clone() });
        }
        if prev.as_ref().is_none_or(|(s, t, _)| (*s, t) != (state, &turn)) {
            self.record_status(&key, &status, at);
        }
        if prev.as_ref().is_some_and(|(_, _, f)| *f != Freshness::Live) {
            self.record_activity(chat, ActivityLine::Freshness { at, agent: Some(key.clone()), freshness: Freshness::Live });
        }
        let mut out = vec![HostEvent::AgentUpdated { view }];
        out.extend(self.note_state(&key, state, turn));
        out
    }

    pub fn remove_chat(&mut self, chat: &ChatKey) -> Vec<HostEvent> {
        self.history_terminal.retain(|a, _| a.id != chat.id);
        self.wake_reconciled.retain(|a, _| a.id != chat.id);
        self.chats.retain(|c| &c.key != chat);
        self.agents.retain(|v| &v.agent.chat != chat);
        self.requests.retain(|r| &r.chat != chat);
        vec![HostEvent::ChatRemoved { chat: chat.clone() }]
    }

    /// 中断記録を作る。作成が終端通知より遅れた場合に備え、観測済みの終端を反映する。
    pub fn add_stop_record(&mut self, mut record: StopRecord, now: UnixMillis) -> Vec<HostEvent> {
        let turns: Vec<TurnKey> = record
            .targets
            .iter()
            .filter_map(|t| match &t.target {
                StopTargetRef::Turn { turn } => Some(turn.clone()),
                _ => None,
            })
            .collect();
        for turn in &turns {
            if let Some((tid, end, at)) = self.last_end.get(&turn.agent) {
                if tid == &turn.turn_id {
                    stop::apply_turn_end(&mut record, turn, *end, *at, now);
                }
            }
        }
        stop::refresh(&mut record, now);
        self.stops.push(record.clone());
        vec![HostEvent::StopUpdated { record }]
    }

    /// 10秒の判定などで要約を再計算する。
    pub fn refresh_stops(&mut self, now: UnixMillis) -> Vec<HostEvent> {
        let mut out = Vec::new();
        for r in &mut self.stops {
            if stop::refresh(r, now) {
                out.push(HostEvent::StopUpdated { record: r.clone() });
            }
        }
        out
    }

    fn set_freshness_where(&mut self, agent: Option<&AgentKey>, to: Freshness, now: UnixMillis, from: impl Fn(Freshness) -> bool) -> Vec<HostEvent> {
        let mut out = Vec::new();
        let mut lines = Vec::new();
        for v in &mut self.agents {
            if agent.is_some_and(|a| a != &v.agent.key) {
                continue;
            }
            if from(v.freshness) && v.freshness != to {
                v.freshness = to;
                out.push(HostEvent::AgentUpdated { view: v.clone() });
                lines.push((v.agent.chat.clone(), ActivityLine::Freshness { at: now, agent: Some(v.agent.key.clone()), freshness: to }));
            }
        }
        self.activity_outbox.extend(lines);
        out
    }

    // ── 通知の入力・一覧の印（P5） ──

    /// 現在の状態が失敗のエージェントの、失敗したturn（確認済みの照合キー）。turnが取れないときは最新turn、それも無ければ空ID。
    fn failed_turn_of(v: &AgentView) -> Option<TurnKey> {
        if v.status.state != AgentState::Failed {
            return None;
        }
        let turn_id = v.status.turn.clone().or_else(|| v.agent.latest_turn.clone()).unwrap_or_else(|| ExternalId(String::new()));
        Some(TurnKey { agent: v.agent.key.clone(), turn_id })
    }

    /// まだ確認していない失敗（チャット内。`agent` 指定ならそのエージェントだけ）。現在の状態から導く（再起動後も同じ）。
    pub fn unacknowledged_failures(&self, chat: &ChatKey, agent: Option<&AgentKey>) -> Vec<TurnKey> {
        let acked = self.locals.get(chat).map(|l| l.acknowledged_failures.as_slice()).unwrap_or(&[]);
        self.agents
            .iter()
            .filter(|v| &v.agent.chat == chat && agent.map_or(true, |a| a == &v.agent.key))
            .filter_map(Self::failed_turn_of)
            .filter(|t| !acked.contains(t))
            .collect()
    }

    /// 一覧の印。状態から導く（通知設定・通知の起点に関係なく付ける）。
    pub fn marks_of(&self, chat: &ChatKey) -> ChatMarks {
        ChatMarks {
            awaiting_answer: self.requests.iter().any(|r| &r.chat == chat && r.state == RequestState::Pending),
            unacknowledged_failure: !self.unacknowledged_failures(chat, None).is_empty(),
        }
    }

    /// 印が変わり得るチャットの更新イベント（補足情報がまだ無いチャットは、UIが状態から出す）。
    pub fn marks_event(&self, chat: &ChatKey) -> Option<HostEvent> {
        self.local_view(chat).map(|local| HostEvent::ChatLocalUpdated { local })
    }

    /// 状態の観測を反映する。①失敗状態の出入りで印を更新（通知の起点とは無関係） ②通知の入力（非終端→終端だけ）。
    fn note_state(&mut self, agent: &AgentKey, state: AgentState, turn: Option<ExternalId>) -> Vec<HostEvent> {
        let mut out = Vec::new();
        let changed = if state == AgentState::Failed { self.failed_seen.insert(agent.clone()) } else { self.failed_seen.remove(agent) };
        if changed {
            if let Some(chat) = self.view(agent).map(|v| v.agent.chat.clone()) {
                out.extend(self.marks_event(&chat));
            }
        }
        match state {
            AgentState::Initializing | AgentState::Running | AgentState::Waiting => {
                self.nonterminal_seen.insert(agent.clone());
            }
            AgentState::Done | AgentState::Failed | AgentState::Interrupted => {
                let end = match state {
                    AgentState::Done => TurnEnd::Completed,
                    AgentState::Failed => TurnEnd::Failed,
                    _ => TurnEnd::Interrupted,
                };
                if let Some(t) = turn {
                    let was = self.nonterminal_seen.remove(agent);
                    self.note_end(agent, t, end, was);
                }
            }
            _ => {}
        }
        out
    }

    /// 終端を通知の入力にする（非終端を観測していたものだけ。初観測は通知しない）。
    fn note_end(&mut self, agent: &AgentKey, turn: ExternalId, end: TurnEnd, was_nonterminal: bool) {
        if !was_nonterminal {
            return;
        }
        let Some(chat) = self.view(agent).map(|v| v.agent.chat.clone()) else { return };
        // side相談の終了は通知しない（相談のパネルで見える。一覧にないチャットを通知の対象にしない）。
        if self.is_side_thread(&chat) {
            return;
        }
        let is_root = self.is_root(agent);
        self.notify_inbox.push(NotifyInput::Ended { chat, agent: agent.clone(), is_root, turn, end, was_nonterminal });
    }

    // ── turn集約diff ──

    fn note_turn_diff(&mut self, turn: &TurnKey, diff: &str) {
        if self.turn_diffs.insert(turn.clone(), diff.to_string()).is_none() {
            self.turn_diff_order.push(turn.clone());
            while self.turn_diff_order.len() > TURN_DIFF_KEEP {
                let old = self.turn_diff_order.remove(0);
                self.turn_diffs.remove(&old);
            }
        }
    }

    /// turnの集約diff（このセッションでliveに受け取った最新値）。
    pub fn turn_diff(&self, turn: &TurnKey) -> Option<&String> {
        self.turn_diffs.get(turn)
    }

    /// チャット内（親・子孫）の集約diffを、受け取った順に返す。
    pub fn turn_diffs_of_chat(&self, chat: &ChatKey) -> Vec<(TurnKey, String)> {
        self.turn_diff_order
            .iter()
            .filter(|t| self.view(&t.agent).is_some_and(|v| &v.agent.chat == chat))
            .filter_map(|t| self.turn_diffs.get(t).map(|d| (t.clone(), d.clone())))
            .collect()
    }

    // ── バックエンドイベント ──

    pub fn apply_event(&mut self, env: &EventEnvelope, caps: &Capabilities) -> (Vec<HostEvent>, Vec<Followup>) {
        let now = env.received_at;
        let mut out = Vec::new();
        let mut follow = Vec::new();
        // wake後の照合に成功したエージェントが、その後にlive通知を受けたら live に戻す（自動のresumeはしない）。
        let touched = match &env.event {
            BackendEvent::AgentStatus { agent, .. } => Some(agent.clone()),
            BackendEvent::TurnStarted { turn, .. } | BackendEvent::TurnEnded { turn, .. } => Some(turn.agent.clone()),
            BackendEvent::Activity { activity } => Some(activity.key.agent.clone()),
            _ => None,
        };
        if let Some(a) = touched {
            out.extend(self.restore_live_after_wake(&a, now));
        }
        match &env.event {
            BackendEvent::Connection { state } => {
                match self.sources.iter_mut().find(|s| s.source == env.source) {
                    Some(s) => {
                        s.connection = state.clone();
                        out.push(HostEvent::SourceUpdated { source: s.clone() });
                    }
                    None => {
                        // 接続コマンドの結果より先に届いた通知。詳細は結果で上書きされる。
                        let info = SourceInfo {
                            source: env.source.clone(),
                            backend: BackendKind::Codex,
                            pid: Known::NotFetched,
                            started_at: now,
                            version: VersionCheck::Unknown { message: "確認中".into() },
                            capabilities: caps.clone(),
                            connection: state.clone(),
                        };
                        self.sources.retain(|s| s.source == env.source);
                        self.sources.push(info.clone());
                        out.push(HostEvent::SourceUpdated { source: info });
                    }
                }
                if matches!(state, ConnectionState::Disconnected { .. }) {
                    // 状態は変えず、鮮度だけを切断にする（doneにもfailedにもしない）。
                    out.extend(self.set_freshness_where(None, Freshness::Disconnected, now, |f| f != Freshness::Unsupported));
                    // 切断で、実行中と分かっていたturnの印と、新しいturnの基準を外す（死んだturnが送信・追加指示・走査を止めない）。
                    // 状態と鮮度は変えず、停止の証拠にもしない。wake後の照合の成立も無効にする。
                    self.running_turn.clear();
                    self.latest_turn_start.clear();
                    self.wake_reconciled.clear();
                    let mut expired_chats = Vec::new();
                    for r in &mut self.requests {
                        if r.key.source == env.source && r.state == RequestState::Pending {
                            r.state = RequestState::Expired { at: now };
                            out.push(HostEvent::RequestUpdated { request: r.clone() });
                            expired_chats.push(r.chat.clone());
                        }
                    }
                    for chat in expired_chats {
                        out.extend(self.marks_event(&chat));
                    }
                }
            }
            BackendEvent::AgentAssignment { agent, assignment } => {
                self.assignments.insert(agent.clone(), assignment.clone());
                if let Some(v) = self.view_mut(agent) {
                    v.agent.assignment = Known::direct(assignment.clone());
                    out.push(HostEvent::AgentUpdated { view: v.clone() });
                }
            }
            BackendEvent::AgentDiscovered { agent } => {
                let agent = &self.with_assignment(agent.clone());
                if self.view(&agent.key).is_none() {
                    let view = AgentView {
                        agent: agent.clone(),
                        status: AgentStatus {
                            state: AgentState::Unknown,
                            raw: RawState { label: "discovered".into() },
                            scope: StateScope::Agent,
                            turn: None,
                            wait: None,
                            evidence: evidence(EvidenceSource::LiveEvent, "agent/discovered", now),
                        },
                        freshness: Freshness::Live,
                        current_activity: None,
                    };
                    self.record_activity(agent.chat.clone(), ActivityLine::AgentSeen { at: now, agent: agent.clone() });
                    self.agents.push(view.clone());
                    out.push(HostEvent::AgentUpdated { view });
                } else if let Some(v) = self.view_mut(&agent.key) {
                    v.agent = agent.clone();
                    out.push(HostEvent::AgentUpdated { view: v.clone() });
                }
            }
            BackendEvent::AgentStatus { agent, status } => {
                let stale = status.scope == StateScope::Turn
                    && matches!((&status.turn, self.latest_turn_start.get(agent)), (Some(t), Some(l)) if t != l);
                if stale {
                    out.push(HostEvent::Warning {
                        source: Some(env.source.clone()),
                        message: "古いturnの状態通知を破棄しました（新しいturnの状態を保持）".into(),
                        raw_label: status.evidence.raw_label.clone(),
                    });
                } else {
                    if is_active(status.state) {
                        if let Some(t) = &status.turn {
                            self.running_turn.insert(agent.clone(), t.clone());
                        }
                    } else if matches!(status.state, AgentState::Idle | AgentState::Done | AgentState::Failed | AgentState::Interrupted | AgentState::Closed) {
                        self.running_turn.remove(agent);
                    }
                    let changed = self.view(agent).is_none_or(|v| (v.status.state, &v.status.turn) != (status.state, &status.turn));
                    if changed {
                        self.record_status(agent, status, now);
                    }
                    let view = self.view_mut(agent).map(|v| {
                        v.status = status.clone();
                        v.clone()
                    });
                    match view {
                        Some(view) => out.push(HostEvent::AgentUpdated { view }),
                        None => {
                            // 未知のエージェント。所属チャットが分からないので自身のIDを仮の所属にする（親は不明）。
                            let chat = ChatKey { backend: agent.backend, id: agent.id.clone() };
                            let view = AgentView {
                                agent: Agent {
                                    key: agent.clone(),
                                    chat,
                                    parent: ParentLink::Unknown,
                                    forked_from: Known::NotFetched,
                                    display_name: Known::NotFetched,
                                    role: Known::NotFetched,
                                    assignment: Known::NotFetched,
                                    agent_path: Known::NotFetched,
                                    latest_turn: status.turn.clone(),
                                },
                                status: status.clone(),
                                freshness: Freshness::Live,
                                current_activity: None,
                            };
                            self.agents.push(view.clone());
                            out.push(HostEvent::AgentUpdated { view });
                        }
                    }
                    out.extend(self.note_state(agent, status.state, status.turn.clone()));
                }
            }
            BackendEvent::TurnStarted { turn, .. } => {
                self.history_terminal.remove(&turn.agent);
                self.nonterminal_seen.insert(turn.agent.clone());
                self.latest_turn_start.insert(turn.agent.clone(), turn.turn_id.clone());
                self.running_turn.insert(turn.agent.clone(), turn.turn_id.clone());
                if let Some(v) = self.view_mut(&turn.agent) {
                    v.agent.latest_turn = Some(turn.turn_id.clone());
                }
                // side相談の会話は、子孫の走査をしない（読取りでも不要な負荷。主会話の子孫に数えない）。
                if self.is_root(&turn.agent) && !self.is_side_thread(&ChatKey { backend: turn.agent.backend, id: turn.agent.id.clone() }) {
                    follow.push(Followup::ScanDescendants(turn.agent.clone()));
                }
                out.push(HostEvent::TurnUpdated { turn: turn.clone(), end: None });
            }
            BackendEvent::TurnEnded { turn, end, evidence: ev, .. } => {
                let mut was_nonterminal = self.nonterminal_seen.remove(&turn.agent);
                if self.running_turn.get(&turn.agent) == Some(&turn.turn_id) {
                    self.running_turn.remove(&turn.agent);
                    was_nonterminal = true;
                }
                self.note_end(&turn.agent, turn.turn_id.clone(), *end, was_nonterminal);
                let at = ev.source_time.unwrap_or(now);
                self.last_end.insert(turn.agent.clone(), (turn.turn_id.clone(), *end, at));
                if let Some(chat) = self.chat_of_agent(&turn.agent) {
                    self.record_activity(chat, ActivityLine::TurnEnded { at, turn: turn.clone(), end: *end });
                }
                out.push(HostEvent::TurnUpdated { turn: turn.clone(), end: Some(*end) });
                for r in &mut self.stops {
                    if stop::apply_turn_end(r, turn, *end, at, now) {
                        out.push(HostEvent::StopUpdated { record: r.clone() });
                    }
                }
            }
            BackendEvent::Activity { activity } => {
                out.push(HostEvent::ActivityUpdated { activity: activity.clone() });
                if matches!(activity.phase, ActivityPhase::Completed | ActivityPhase::Unconfirmed) {
                    if let Some(chat) = self.chat_of_agent(&activity.key.agent) {
                        self.record_activity(chat, ActivityLine::Activity { at: now, activity: activity.clone() });
                    }
                }
                // サブエージェント関連のitem（spawn・待機・送信など）が来たら、子孫の監視を始める（既に動いていれば何もしない）。
                if matches!(activity.kind, ActivityKind::SubAgent) {
                    if let Some(v) = self.view(&activity.key.agent).filter(|v| !self.is_side_thread(&v.agent.chat)) {
                        follow.push(Followup::ScanDescendants(agent_key_of(&v.agent.chat)));
                    }
                }
                if let Some(v) = self.view_mut(&activity.key.agent) {
                    match activity.phase {
                        ActivityPhase::Completed => {
                            if v.current_activity.as_ref().is_some_and(|c| c.key == activity.key) {
                                v.current_activity = None;
                            }
                        }
                        _ => v.current_activity = Some(activity.clone()),
                    }
                    out.push(HostEvent::AgentUpdated { view: v.clone() });
                }
            }
            BackendEvent::ActivityDelta { item, delta } => {
                out.push(HostEvent::ActivityDelta { item: item.clone(), delta: delta.clone() });
            }
            BackendEvent::ArtifactObserved { agent, item, path } => {
                follow.push(Followup::ObserveArtifact { agent: agent.clone(), item: item.clone(), path: path.clone() });
            }
            BackendEvent::TurnChangesUpdated { turn, diff } => {
                self.note_turn_diff(turn, diff);
                if let Some(v) = self.view(&turn.agent) {
                    out.push(HostEvent::ChangesUpdated { chat: v.agent.chat.clone(), turn: Some(turn.turn_id.clone()) });
                }
            }
            BackendEvent::FileChangeObserved { agent, item, changes } => {
                follow.push(Followup::ObserveChanges { agent: agent.clone(), item: item.clone(), changes: changes.clone() });
            }
            BackendEvent::RequestOpened { request } => {
                match self.requests.iter_mut().find(|r| r.key == request.key) {
                    Some(old) => *old = request.clone(),
                    None => self.requests.push(request.clone()),
                }
                out.push(HostEvent::RequestUpdated { request: request.clone() });
                if request.state == RequestState::Pending {
                    // side相談の承認・質問は、一覧にない会話ではなく主会話を通知の宛先にする（回答は相談のパネルで行う）。
                    let target = self.side_threads.get(&request.chat).unwrap_or(&request.chat).clone();
                    self.notify_inbox.push(NotifyInput::AwaitingAnswer { chat: target, request: request.key.clone(), kind: request.kind.clone() });
                    out.extend(self.marks_event(&request.chat));
                }
            }
            BackendEvent::RequestResolved { request, .. } => {
                let mut resolved_chat = None;
                if let Some(r) = self.requests.iter_mut().find(|r| &r.key == request) {
                    if matches!(r.state, RequestState::Pending | RequestState::AnswerUnconfirmed { .. }) {
                        r.state = RequestState::ResolvedElsewhere { at: now };
                        out.push(HostEvent::RequestUpdated { request: r.clone() });
                        resolved_chat = Some(r.chat.clone());
                    }
                }
                if let Some(chat) = resolved_chat {
                    out.extend(self.marks_event(&chat));
                }
            }
            BackendEvent::ChatMetaChanged { chat, change } => match change {
                ChatMetaChange::Renamed { name } => {
                    if let Some(c) = self.chats.iter_mut().find(|c| &c.key == chat) {
                        c.name = Known::direct(name.clone());
                        out.push(HostEvent::ChatUpdated { chat: c.clone() });
                    }
                }
                ChatMetaChange::Archived | ChatMetaChange::Unarchived => {
                    if let Some(c) = self.chats.iter_mut().find(|c| &c.key == chat) {
                        c.archived = Known::direct(matches!(change, ChatMetaChange::Archived));
                        out.push(HostEvent::ChatUpdated { chat: c.clone() });
                    }
                }
                ChatMetaChange::Deleted => out.extend(self.remove_chat(chat)),
            },
            BackendEvent::ModelAccepted { agent, choice } => {
                if let Some(chat) = self.view(agent).filter(|_| self.is_root(agent)).map(|v| v.agent.chat.clone()) {
                    out.extend(self.note_accepted_model(&chat, choice.clone()));
                }
            }
            BackendEvent::WorkModeAccepted { agent, mode } => {
                if let Some(chat) = self.view(agent).filter(|_| self.is_root(agent)).map(|v| v.agent.chat.clone()) {
                    out.extend(self.note_accepted_work_mode(&chat, *mode));
                }
            }
            // 目標の更新・解除はUIが表示している分を置き換えるだけ。エージェント状態（§4.1）には流用しない。
            BackendEvent::GoalUpdated { chat, goal } => out.push(HostEvent::GoalUpdated { chat: chat.clone(), goal: goal.clone() }),
            BackendEvent::OpCapabilitiesChanged { ops } => out.push(HostEvent::OpCapabilitiesUpdated { ops: ops.clone() }),
            // ツールサーバーの通知は、ホストの拡張管理（`extensions.rs`）が見る。エージェントの状態には使わない。
            BackendEvent::ToolServerStatusChanged { .. } | BackendEvent::ToolServerLoginCompleted { .. } => {}
            BackendEvent::ModelRerouted { agent, effective, .. } => {
                if let Some(chat) = self.view(agent).filter(|_| self.is_root(agent)).map(|v| v.agent.chat.clone()) {
                    let s = self.model_settings.entry(chat.clone()).or_insert_with(|| ChatModelSettings::blank(ApplyTiming::Unknown));
                    s.effective = Known::direct(effective.clone());
                    out.push(HostEvent::ModelSettingsUpdated { chat, settings: s.clone() });
                }
            }
            BackendEvent::Unrecognized { raw_label, note } => out.push(HostEvent::Warning {
                source: Some(env.source.clone()),
                message: note.clone().unwrap_or_else(|| raw_label.clone()),
                raw_label: Some(raw_label.clone()),
            }),
            BackendEvent::Gap { scope, reason } => {
                match scope {
                    GapScope::Source => out.extend(self.set_freshness_where(None, Freshness::NeedsReconcile, now, |f| f == Freshness::Live)),
                    GapScope::Agent { agent } => {
                        out.extend(self.set_freshness_where(Some(agent), Freshness::NeedsReconcile, now, |f| f == Freshness::Live))
                    }
                }
                out.push(HostEvent::Warning { source: Some(env.source.clone()), message: format!("イベントの欠落の可能性: {reason}"), raw_label: Some("gap".into()) });
            }
        }
        (out, follow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ak(s: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn ck(s: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(s.into()) }
    }
    fn tk(a: &str, t: &str) -> TurnKey {
        TurnKey { agent: ak(a), turn_id: ExternalId(t.into()) }
    }
    fn caps() -> Capabilities {
        crate::codex::adapter::codex_capabilities(true)
    }
    fn agent(id: &str, root: bool) -> Agent {
        Agent {
            key: ak(id),
            chat: ck("root"),
            parent: if root { ParentLink::Root } else { ParentLink::Explicit { parent: ak("root") } },
            forked_from: Known::NotFetched,
            display_name: Known::NotFetched,
            role: Known::NotFetched,
            assignment: Known::NotFetched,
            agent_path: Known::NotFetched,
            latest_turn: None,
        }
    }
    fn status(state: AgentState, turn: Option<&str>, scope: StateScope) -> AgentStatus {
        AgentStatus {
            state,
            raw: RawState { label: "x".into() },
            scope,
            turn: turn.map(|t| ExternalId(t.into())),
            wait: None,
            evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(0)),
        }
    }
    fn env(seq: u64, ms: i64, event: BackendEvent) -> EventEnvelope {
        EventEnvelope { source: SourceId("s1".into()), seq, received_at: UnixMillis(ms), event }
    }
    fn live_root(d: &mut HostData, st: AgentState) {
        d.set_live(agent("root", true), status(st, None, StateScope::Agent));
    }

    #[test]
    fn resumed_response_records_the_accepted_model_and_keeps_the_selection() {
        let mut d = HostData::default();
        // 再起動後の復元: 選択値だけで、受理値は未取得。
        let selected = ModelChoice { model: "sel".into(), effort: Some("low".into()), speed_tier: None };
        d.model_settings.insert(
            ck("root"),
            ChatModelSettings { selected: Some(selected.clone()), ..ChatModelSettings::blank(ApplyTiming::NextTurn) },
        );
        let history = AgentHistory { agent: agent("root", true), chat: None, status: status(AgentState::Idle, None, StateScope::Agent), turns: vec![] };
        // 応答が会話のモデルを示さなければ、未取得のまま（受理済みにしない）。
        let ev = d.apply_resumed(&ck("root"), history.clone(), Known::NotFetched);
        assert!(!ev.iter().any(|e| matches!(e, HostEvent::ModelSettingsUpdated { .. })));
        assert_eq!(d.model_settings.get(&ck("root")).unwrap().accepted, Known::NotFetched);
        // 示されたら受理値として記録し、UIへ出す。選択値は変えない。
        let got = ModelChoice { model: "resumed".into(), effort: None, speed_tier: None };
        let ev = d.apply_resumed(&ck("root"), history, Known::direct(got.clone()));
        assert!(ev.iter().any(|e| matches!(e, HostEvent::ModelSettingsUpdated { chat, .. } if chat == &ck("root"))));
        let s = d.model_settings.get(&ck("root")).unwrap();
        assert_eq!((s.accepted.clone(), s.selected.clone()), (Known::direct(got), Some(selected)));
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::Live);
    }

    #[test]
    fn failure_mark_follows_current_state_not_notification_origin() {
        let mut d = HostData::default();
        // 起動後の初観測（履歴・走査）の失敗: 通知はしないが、印は付く（再起動相当）。
        d.set_live(agent("root", true), status(AgentState::Failed, Some("t0"), StateScope::Turn));
        assert!(d.notify_inbox.is_empty());
        assert!(d.marks_of(&ck("root")).unacknowledged_failure);
        // 確認済みで外れる。別のturnの確認済みでは外れない。
        let mut f = ChatLocalFile::new(LocalId("d".into()), Some(ck("root")));
        f.acknowledged_failures = vec![tk("root", "tX")];
        d.locals.insert(ck("root"), f);
        assert!(d.marks_of(&ck("root")).unacknowledged_failure);
        d.locals.get_mut(&ck("root")).unwrap().acknowledged_failures = d.unacknowledged_failures(&ck("root"), None);
        assert!(!d.marks_of(&ck("root")).unacknowledged_failure);
        // 新しいturnが走り出せば印は消え、そのturnが失敗すれば再び付く（通知の入力にもなる）。
        d.apply_event(&env(1, 1, BackendEvent::TurnStarted { turn: tk("root", "t1"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(1)) }), &caps());
        d.apply_event(&env(2, 2, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        d.apply_event(&env(3, 3, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Failed, Some("t1"), StateScope::Turn) }), &caps());
        assert_eq!(d.notify_inbox.len(), 1);
        assert_eq!(d.unacknowledged_failures(&ck("root"), None), vec![tk("root", "t1")]);
        // 同じ終端のTurnEndedは通知を二重にしない（状態通知で終端を消費済み）。
        d.apply_event(
            &env(4, 4, BackendEvent::TurnEnded { turn: tk("root", "t1"), end: TurnEnd::Failed, error: None, evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(4)) }),
            &caps(),
        );
        assert_eq!(d.notify_inbox.len(), 1);
    }

    #[test]
    fn disconnect_changes_freshness_only_and_expires_requests() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        let req = PendingRequest {
            key: RequestKey { backend: BackendKind::Codex, source: SourceId("s1".into()), request_id: ExternalId("n:1".into()) },
            chat: ck("root"),
            agent: ak("root"),
            turn: None,
            item: None,
            kind: RequestKind::CommandApproval,
            detail: RequestDetail {
                summary: "s".into(),
                reason: Known::NotFetched,
                command: Known::NotFetched,
                cwd: Known::NotFetched,
                files: Known::NotFetched,
                extra_permissions: Known::NotFetched,
                url: Known::NotFetched,
                questions: vec![],
            },
            options: vec![],
            state: RequestState::Pending,
            received_at: UnixMillis(0),
        };
        d.apply_event(&env(1, 1, BackendEvent::RequestOpened { request: req }), &caps());
        let (ev, _) = d.apply_event(&env(2, 2, BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } }), &caps());
        let v = d.root_view(&ck("root")).unwrap();
        assert_eq!(v.freshness, Freshness::Disconnected);
        assert_eq!(v.status.state, AgentState::Running, "state must stay the last explicit value");
        assert!(matches!(d.requests[0].state, RequestState::Expired { .. }));
        assert!(ev.iter().any(|e| matches!(e, HostEvent::SourceUpdated { .. })));
    }

    #[test]
    fn late_status_of_an_older_turn_is_dropped() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Idle);
        d.apply_event(&env(1, 1, BackendEvent::TurnStarted { turn: tk("root", "t2"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(1)) }), &caps());
        d.apply_event(&env(2, 2, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t2"), StateScope::Turn) }), &caps());
        let (ev, _) = d.apply_event(&env(3, 3, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Done, Some("t1"), StateScope::Turn) }), &caps());
        assert_eq!(d.root_view(&ck("root")).unwrap().status.state, AgentState::Running);
        assert!(matches!(ev[0], HostEvent::Warning { .. }));
    }

    #[test]
    fn side_thread_is_not_scanned_notified_or_logged_but_is_in_the_main_stop_scope() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Idle);
        // side相談の一時の会話（自分自身が所属。ルート扱い）。
        let mut side = agent("side", true);
        side.chat = ck("side");
        d.side_threads.insert(ck("side"), ck("root"));
        d.set_live(side, status(AgentState::Idle, None, StateScope::Agent));
        d.activity_outbox.clear();
        d.notify_inbox.clear();
        let (_, follow) = d.apply_event(&env(1, 1, BackendEvent::TurnStarted { turn: tk("side", "t1"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(1)) }), &caps());
        assert!(follow.is_empty(), "no descendant scan for a side conversation");
        assert!(d.running_turn.contains_key(&ak("side")), "its running turn is tracked so it can be stopped");
        d.apply_event(&env(2, 2, BackendEvent::AgentStatus { agent: ak("side"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        d.apply_event(&env(3, 3, BackendEvent::AgentStatus { agent: ak("side"), status: status(AgentState::Done, Some("t1"), StateScope::Turn) }), &caps());
        assert!(d.notify_inbox.is_empty(), "the end of a side conversation is not notified");
        assert!(d.activity_outbox.is_empty(), "nothing is written to the activity log for a side conversation");
        // 主会話の停止の範囲には入る。主会話自体の条件（agent.chat が主会話）には入らない。
        assert_eq!(d.stop_scope(&ck("root")), vec![ck("root"), ck("side")]);
        assert!(!d.agents.iter().any(|v| v.agent.chat == ck("root") && v.agent.key == ak("side")));
        let work = crate::host::lifecycle::work_of(&d, &ck("root"));
        assert!(!work.has_unfinished, "the side conversation finished");
        d.apply_event(&env(4, 4, BackendEvent::TurnStarted { turn: tk("side", "t2"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(4)) }), &caps());
        d.apply_event(&env(5, 5, BackendEvent::AgentStatus { agent: ak("side"), status: status(AgentState::Running, Some("t2"), StateScope::Turn) }), &caps());
        assert!(crate::host::lifecycle::work_of(&d, &ck("root")).has_unfinished, "a running side conversation counts for stop checks");
        let removed = d.forget_deleted_chat(&ck("root"));
        assert!(!removed.is_empty());
        assert!(d.side_threads.is_empty() && !d.running_turn.contains_key(&ak("side")), "deleting the main chat forgets its side conversations");
    }

    #[test]
    fn turn_end_confirms_stop_record_and_clears_running_turn() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        d.apply_event(&env(1, 1, BackendEvent::TurnStarted { turn: tk("root", "t1"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(1)) }), &caps());
        assert!(d.running_turn.contains_key(&ak("root")));
        let rec = StopRecord { id: LocalId("s".into()), chat: ck("root"), started_at: UnixMillis(100), targets: vec![stop::new_target(tk("root", "t1"), Some(UnixMillis(100)), UnixMillis(100))] };
        d.add_stop_record(rec, UnixMillis(100));
        assert_eq!(d.stops[0].targets[0].summary, StopSummary::InterruptRequested);
        let (ev, _) = d.apply_event(
            &env(2, 5_000, BackendEvent::TurnEnded {
                turn: tk("root", "t1"),
                end: TurnEnd::Interrupted,
                error: None,
                evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(5_000)),
            }),
            &caps(),
        );
        assert_eq!(d.stops[0].targets[0].summary, StopSummary::TurnEndConfirmed);
        assert!(!d.running_turn.contains_key(&ak("root")));
        assert!(ev.iter().any(|e| matches!(e, HostEvent::StopUpdated { .. })));
        assert!(d.open_stop(&ck("root")).is_none());
    }

    #[test]
    fn stop_record_created_after_the_end_event_still_sees_it() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        d.apply_event(
            &env(1, 50, BackendEvent::TurnEnded {
                turn: tk("root", "t1"),
                end: TurnEnd::Interrupted,
                error: None,
                evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(50)),
            }),
            &caps(),
        );
        let rec = StopRecord { id: LocalId("s".into()), chat: ck("root"), started_at: UnixMillis(100), targets: vec![stop::new_target(tk("root", "t1"), Some(UnixMillis(100)), UnixMillis(100))] };
        d.add_stop_record(rec, UnixMillis(100));
        assert_eq!(d.stops[0].targets[0].summary, StopSummary::TurnEndConfirmed);
    }

    #[test]
    fn history_read_does_not_overwrite_live_status_but_fills_unloaded() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        d.upsert_history_agent(agent("root", true), status(AgentState::Unknown, None, StateScope::Agent));
        let v = d.root_view(&ck("root")).unwrap();
        assert_eq!((v.freshness, v.status.state), (Freshness::Live, AgentState::Running));
        d.upsert_history_agent(agent("other", true), status(AgentState::Done, None, StateScope::Turn));
        assert_eq!(d.view(&ak("other")).unwrap().freshness, Freshness::HistoryOnly);
    }

    #[test]
    fn answered_request_is_not_overwritten_by_resolved_notice() {
        let mut d = HostData::default();
        let key = RequestKey { backend: BackendKind::Codex, source: SourceId("s1".into()), request_id: ExternalId("n:1".into()) };
        d.requests.push(PendingRequest {
            key: key.clone(),
            chat: ck("root"),
            agent: ak("root"),
            turn: None,
            item: None,
            kind: RequestKind::UserInput,
            detail: RequestDetail {
                summary: "s".into(),
                reason: Known::NotFetched,
                command: Known::NotFetched,
                cwd: Known::NotFetched,
                files: Known::NotFetched,
                extra_permissions: Known::NotFetched,
                url: Known::NotFetched,
                questions: vec![],
            },
            options: vec![],
            state: RequestState::Answered { option_id: None, at: UnixMillis(1) },
            received_at: UnixMillis(0),
        });
        let ev = evidence(EvidenceSource::LiveEvent, "x", UnixMillis(2));
        let (out, _) = d.apply_event(&env(1, 2, BackendEvent::RequestResolved { request: key, evidence: ev }), &caps());
        assert!(out.is_empty());
        assert!(matches!(d.requests[0].state, RequestState::Answered { .. }));
    }

    #[test]
    fn settings_notification_records_the_accepted_model_only_for_the_root() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Idle);
        d.set_live(agent("child", false), status(AgentState::Idle, None, StateScope::Agent));
        let choice = ModelChoice { model: "m".into(), effort: Some("low".into()), speed_tier: None };
        // 選択値と受理値は別。通知前は未取得のまま。
        assert!(d.model_settings.get(&ck("root")).is_none());
        let (ev, _) = d.apply_event(&env(1, 1, BackendEvent::ModelAccepted { agent: ak("root"), choice: choice.clone() }), &caps());
        assert!(matches!(ev[0], HostEvent::ModelSettingsUpdated { .. }));
        assert_eq!(d.model_settings.get(&ck("root")).unwrap().accepted, Known::direct(choice.clone()));
        // 同じ値の再通知ではイベントを出さない。子の設定はチャットのモデル設定にしない。
        let (ev, _) = d.apply_event(&env(2, 2, BackendEvent::ModelAccepted { agent: ak("root"), choice: choice.clone() }), &caps());
        assert!(ev.is_empty());
        d.apply_event(&env(3, 3, BackendEvent::ModelAccepted { agent: ak("child"), choice: ModelChoice { model: "x".into(), effort: None, speed_tier: None } }), &caps());
        assert_eq!(d.model_settings.get(&ck("root")).unwrap().accepted, Known::direct(choice));
    }

    #[test]
    fn accepted_work_mode_is_separate_from_the_selection_and_only_set_by_the_notification() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Idle);
        d.set_live(agent("child", false), status(AgentState::Idle, None, StateScope::Agent));
        // 選択値だけでは受理済みにならない。
        let mut s = ChatModelSettings::blank(ApplyTiming::NextTurn);
        s.work_mode = Some(WorkMode::Plan);
        d.model_settings.insert(ck("root"), s);
        assert_eq!(d.model_settings.get(&ck("root")).unwrap().accepted_work_mode, Known::NotFetched);
        let (ev, _) = d.apply_event(&env(1, 1, BackendEvent::WorkModeAccepted { agent: ak("root"), mode: WorkMode::Default }), &caps());
        assert!(matches!(ev[0], HostEvent::ModelSettingsUpdated { .. }));
        let s = d.model_settings.get(&ck("root")).unwrap();
        // 受理値は通知の値（選択はPlanのまま。食い違いはUIが「未受理」と示す）。
        assert_eq!(s.accepted_work_mode, Known::direct(WorkMode::Default));
        assert_eq!(s.work_mode, Some(WorkMode::Plan));
        // 同じ値の再通知ではイベントを出さない。子の設定はチャットの設定にしない。
        let (ev, _) = d.apply_event(&env(2, 2, BackendEvent::WorkModeAccepted { agent: ak("root"), mode: WorkMode::Default }), &caps());
        assert!(ev.is_empty());
        d.apply_event(&env(3, 3, BackendEvent::WorkModeAccepted { agent: ak("child"), mode: WorkMode::Plan }), &caps());
        assert_eq!(d.model_settings.get(&ck("root")).unwrap().accepted_work_mode, Known::direct(WorkMode::Default));
    }

    #[test]
    fn external_chat_is_locked_until_resumed_and_live() {
        assert!(external_send_locked(ChatOrigin::External, Some(Freshness::HistoryOnly)));
        assert!(external_send_locked(ChatOrigin::External, None));
        assert!(external_send_locked(ChatOrigin::External, Some(Freshness::Disconnected)));
        assert!(!external_send_locked(ChatOrigin::External, Some(Freshness::Live)), "resumed external chat can send");
        assert!(!external_send_locked(ChatOrigin::AppManaged, Some(Freshness::HistoryOnly)));
        // 再開（set_live）しても origin は external のまま（バナーを「再開済み」にするため）。
        let mut d = HostData::default();
        let chat = Chat {
            key: ck("root"), kind: ChatKind::Development, cwd: Known::NotFetched, name: Known::NotFetched, preview: Known::NotFetched, pinned: false,
            archived: Known::NotFetched, origin: ChatOrigin::External, draft: None, created_at: Known::NotFetched, last_used_at: None, no_history: false,
        };
        d.upsert_chat(chat);
        d.upsert_history_agent(agent("root", true), status(AgentState::Unknown, None, StateScope::Agent));
        assert!(external_send_locked(d.chat(&ck("root")).unwrap().origin, d.root_view(&ck("root")).map(|v| v.freshness)));
        d.set_live(agent("root", true), status(AgentState::Idle, None, StateScope::Agent));
        let c = d.chat(&ck("root")).unwrap();
        assert_eq!(c.origin, ChatOrigin::External);
        assert!(!external_send_locked(c.origin, d.root_view(&ck("root")).map(|v| v.freshness)));
    }

    #[test]
    fn connection_event_placeholder_is_replaced_by_the_real_source_info() {
        let mut d = HostData::default();
        d.apply_event(&env(1, 1, BackendEvent::Connection { state: ConnectionState::Starting }), &caps());
        assert!(matches!(d.sources[0].version, VersionCheck::Unknown { .. }));
        d.set_source(SourceInfo {
            source: SourceId("s1".into()),
            backend: BackendKind::Codex,
            pid: Known::NotFetched,
            started_at: UnixMillis(0),
            version: VersionCheck::Match { version: "0.160.0".into() },
            capabilities: caps(),
            connection: ConnectionState::Connected,
        });
        assert_eq!(d.sources.len(), 1);
        assert!(matches!(d.sources[0].version, VersionCheck::Match { .. }));
    }

    #[test]
    fn hosted_chat_is_app_managed_even_if_the_backend_calls_it_external() {
        let mut d = HostData::default();
        let mk = |origin| Chat {
            key: ck("root"),
            kind: ChatKind::Development,
            cwd: Known::NotFetched,
            name: Known::NotFetched,
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
            no_history: false,
        };
        d.upsert_chat(mk(ChatOrigin::External));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::External);
        let ev = d.mark_hosted(&ck("root"));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::AppManaged);
        assert!(matches!(ev[0], HostEvent::ChatUpdated { .. }));
        // 一覧の再取得でexternalと返ってもホスト管理を保つ。
        d.upsert_chat(mk(ChatOrigin::External));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::AppManaged);
        // 新しい接続でも、アプリ管理の印は保つ（会話の属性として永続する。再開の要否は鮮度で判断する）。
        let src = |id: &str| SourceInfo {
            source: SourceId(id.into()),
            backend: BackendKind::Codex,
            pid: Known::NotFetched,
            started_at: UnixMillis(0),
            version: VersionCheck::Unknown { message: String::new() },
            capabilities: caps(),
            connection: ConnectionState::Connected,
        };
        d.set_source(src("a"));
        d.set_source(src("b"));
        d.upsert_chat(mk(ChatOrigin::External));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::AppManaged);
    }

    #[test]
    fn subagent_items_trigger_descendant_scan_of_the_chat_root() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        let activity = Activity {
            key: ItemKey { agent: ak("root"), turn_id: Some(ExternalId("t1".into())), item_id: ExternalId("i1".into()) },
            kind: ActivityKind::SubAgent,
            phase: ActivityPhase::Started,
            summary: Known::Missing,
            evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(0)),
        };
        let (_, follow) = d.apply_event(&env(1, 1, BackendEvent::Activity { activity }), &caps());
        assert_eq!(follow, vec![Followup::ScanDescendants(ak("root"))]);
    }

    /// ancestorThreadIdで返る子ThreadのJSON（実際の形に近い固定例）。
    fn child_thread_json() -> serde_json::Value {
        serde_json::json!({
            "id": "child1", "status": {"type": "idle"}, "turns": [], "preview": "", "cwd": "C:\\work",
            "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "root", "depth": 1, "agent_path": "/root/luna", "agent_nickname": "Luna", "agent_role": "explorer"}}},
            "agentNickname": "Luna", "agentRole": "explorer", "createdAt": 1, "updatedAt": 2
        })
    }

    #[test]
    fn found_child_replaces_a_synthetic_view_and_becomes_visible_in_the_chat() {
        use crate::codex::convert::thread_to_agent;
        use crate::codex::wire::WireThread;
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        // 子の状態通知が thread/started より先に届くと、所属不明の仮エージェントができる。
        d.apply_event(&env(1, 1, BackendEvent::AgentStatus { agent: ak("child1"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        let synthetic = d.view(&ak("child1")).unwrap();
        assert!(HostData::is_synthetic(synthetic));
        assert!(d.agents.iter().filter(|v| v.agent.chat == ck("root")).all(|v| v.agent.key != ak("child1")), "not shown under the chat yet");
        assert!(d.needs_adoption(&ak("child1")), "scan must not skip a synthetic view");
        // 走査の結果（親の明示つき）で直す。
        let t = WireThread::from_value(&child_thread_json()).unwrap();
        let found = thread_to_agent(&t, ck("root"));
        assert!(matches!(found.parent, ParentLink::Explicit { .. }));
        let live_status_before = d.view(&ak("child1")).unwrap().status.clone();
        d.adopt_agent(found);
        let v = d.view(&ak("child1")).unwrap();
        assert_eq!(v.agent.chat, ck("root"));
        assert!(matches!(&v.agent.parent, ParentLink::Explicit { parent } if parent == &ak("root")));
        assert_eq!(v.status, live_status_before, "adoption keeps the observed state");
        assert!(!d.needs_adoption(&ak("child1")));
        assert!(d.agents.iter().any(|v| v.agent.chat == ck("root") && v.agent.key == ak("child1")));
    }

    #[test]
    fn spawn_assignment_survives_discovery_order_and_reregistration() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        // 担当の通知が子の登録より先に届いても、後の登録・走査での置き換えで失わない。
        d.apply_event(&env(1, 1, BackendEvent::AgentAssignment { agent: ak("c1"), assignment: "再送経路".into() }), &caps());
        d.apply_event(&env(2, 2, BackendEvent::AgentDiscovered { agent: agent("c1", false) }), &caps());
        assert_eq!(d.view(&ak("c1")).unwrap().agent.assignment, Known::direct("再送経路".to_string()));
        d.upsert_history_agent(agent("c1", false), status(AgentState::Idle, None, StateScope::Agent));
        assert_eq!(d.view(&ak("c1")).unwrap().agent.assignment, Known::direct("再送経路".to_string()));
        // 別の子には割り当てない。
        d.apply_event(&env(3, 3, BackendEvent::AgentDiscovered { agent: agent("c2", false) }), &caps());
        assert_eq!(d.view(&ak("c2")).unwrap().agent.assignment, Known::NotFetched);
        // 既に登録済みの子へ後から届いた場合は即反映。
        let (ev, _) = d.apply_event(&env(4, 4, BackendEvent::AgentAssignment { agent: ak("c2"), assignment: "テスト".into() }), &caps());
        assert!(matches!(ev[0], HostEvent::AgentUpdated { .. }));
        assert_eq!(d.view(&ak("c2")).unwrap().agent.assignment, Known::direct("テスト".to_string()));
    }

    #[test]
    fn gap_marks_live_agents_needs_reconcile() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        d.apply_event(&env(1, 1, BackendEvent::Gap { scope: GapScope::Source, reason: "r".into() }), &caps());
        let v = d.root_view(&ck("root")).unwrap();
        assert_eq!((v.freshness, v.status.state), (Freshness::NeedsReconcile, AgentState::Running));
    }

    // ── P8（R2レビューの修正） ──

    #[test]
    fn disconnect_drops_dead_running_turns_and_set_live_clears_them_when_not_working() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Idle);
        d.apply_event(&env(1, 1, BackendEvent::TurnStarted { turn: tk("root", "t1"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(1)) }), &caps());
        assert!(d.running_turn.contains_key(&ak("root")));
        d.apply_event(&env(2, 2, BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } }), &caps());
        assert!(d.running_turn.is_empty(), "a dead turn must not block sending after reconnect");
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::Disconnected);
        // 再開（resume）で作業中でない状態になれば、残っていた印も外れる。
        d.running_turn.insert(ak("root"), ExternalId("t1".into()));
        d.set_live(agent("root", true), status(AgentState::Idle, None, StateScope::Agent));
        assert!(d.running_turn.is_empty());
    }

    #[test]
    fn unknown_agent_is_unfinished_unless_history_confirms_an_end() {
        let mut d = HostData::default();
        d.upsert_history_agent(agent("root", true), status(AgentState::Unknown, None, StateScope::Agent));
        let v = d.root_view(&ck("root")).unwrap().clone();
        assert!(!d.unknown_confirmed_idle(&v), "nothing read: stays unfinished");
        d.note_history_tail(&ak("root"), HistoryTail::NotTerminal, UnixMillis(5));
        assert!(!d.unknown_confirmed_idle(&v) && d.unknown_maybe_running(&v));
        d.note_history_tail(&ak("root"), HistoryTail::Ended(ExternalId("t1".into()), TurnEnd::Completed), UnixMillis(6));
        assert!(d.unknown_confirmed_idle(&v) && !d.unknown_maybe_running(&v));
        // 新しいturnが始まれば、確認は無効。
        d.apply_event(&env(1, 7, BackendEvent::TurnStarted { turn: tk("root", "t2"), evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(7)) }), &caps());
        assert!(!d.unknown_confirmed_idle(d.root_view(&ck("root")).unwrap()));
    }

    #[test]
    fn history_only_returns_to_live_only_after_a_reconcile_and_a_later_live_event_on_the_same_connection() {
        let mut d = HostData::default();
        live_root(&mut d, AgentState::Running);
        d.apply_event(&env(1, 1, BackendEvent::Gap { scope: GapScope::Source, reason: "wake".into() }), &caps());
        d.upsert_history_agent(agent("root", true), status(AgentState::Running, Some("t1"), StateScope::Agent));
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::HistoryOnly);
        // 照合に成功する前のlive通知では戻さない。
        d.apply_event(&env(2, 10, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::HistoryOnly);
        d.note_wake_reconciled(&ak("root"), UnixMillis(20));
        // 照合と同時刻（以前）の通知でも戻さない。
        d.apply_event(&env(3, 20, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::HistoryOnly);
        d.apply_event(&env(4, 21, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        assert_eq!(d.root_view(&ck("root")).unwrap().freshness, Freshness::Live);
        // 切断をまたいだ照合は無効（再接続後の通知だけでは戻さない）。
        d.apply_event(&env(5, 30, BackendEvent::Gap { scope: GapScope::Source, reason: "wake".into() }), &caps());
        d.upsert_history_agent(agent("root", true), status(AgentState::Running, Some("t1"), StateScope::Agent));
        d.note_wake_reconciled(&ak("root"), UnixMillis(31));
        d.apply_event(&env(6, 32, BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } }), &caps());
        d.apply_event(&env(7, 40, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        assert_ne!(d.root_view(&ck("root")).unwrap().freshness, Freshness::Live);
    }

    #[test]
    fn wake_marked_agents_do_not_notify_when_they_are_read_as_ended_later() {
        let mut d = HostData::default();
        d.set_live(agent("c1", false), status(AgentState::Running, Some("t1"), StateScope::Turn));
        d.notify_inbox.clear();
        d.forget_nonterminal(&[ak("c1")]);
        d.upsert_history_agent(agent("c1", false), status(AgentState::Done, Some("t1"), StateScope::Turn));
        assert!(d.notify_inbox.is_empty(), "an end seen only after wake is not a notification");
    }

    #[test]
    fn monitor_activity_is_queued_for_discovery_status_end_and_freshness_but_not_for_listed_roots() {
        let mut d = HostData::default();
        // 一覧のルートは記録しない。
        d.upsert_history_agent(agent("root", true), status(AgentState::Idle, None, StateScope::Agent));
        assert!(d.activity_outbox.is_empty());
        live_root(&mut d, AgentState::Idle);
        d.activity_outbox.clear();
        d.apply_event(&env(1, 1, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t1"), StateScope::Turn) }), &caps());
        d.apply_event(&env(2, 2, BackendEvent::TurnEnded { turn: tk("root", "t1"), end: TurnEnd::Completed, error: None, evidence: evidence(EvidenceSource::LiveEvent, "x", UnixMillis(2)) }), &caps());
        d.apply_event(&env(3, 3, BackendEvent::Connection { state: ConnectionState::Disconnected { message: None } }), &caps());
        let kinds: Vec<&str> = d
            .activity_outbox
            .iter()
            .map(|(_, l)| match l {
                ActivityLine::StatusChanged { .. } => "status",
                ActivityLine::TurnEnded { .. } => "end",
                ActivityLine::Freshness { .. } => "fresh",
                ActivityLine::AgentSeen { .. } => "seen",
                ActivityLine::Activity { .. } => "act",
            })
            .collect();
        assert_eq!(kinds, vec!["status", "end", "fresh"]);
        // 同じ状態の再通知は記録しない。
        d.activity_outbox.clear();
        d.apply_event(&env(4, 4, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t2"), StateScope::Turn) }), &caps());
        d.apply_event(&env(5, 5, BackendEvent::AgentStatus { agent: ak("root"), status: status(AgentState::Running, Some("t2"), StateScope::Turn) }), &caps());
        assert_eq!(d.activity_outbox.len(), 1);
    }

    #[test]
    fn app_started_chats_stay_app_managed_after_restart_even_if_the_backend_says_vscode() {
        let mk = |id: &str, kind, created: Option<i64>| Chat {
            key: ck(id), kind, cwd: Known::NotFetched, name: Known::NotFetched, preview: Known::NotFetched, pinned: false, archived: Known::NotFetched,
            origin: ChatOrigin::External, draft: None,
            created_at: created.map_or(Known::NotFetched, |t| Known::direct(UnixMillis(t))), last_used_at: None, no_history: false,
        };
        let mut d = HostData::default();
        // 復元した hosted の印は、一覧取得（外部と返る）でも保たれ、送信は止まらない。
        d.hosted.insert(ck("dev"));
        d.upsert_chat(mk("dev", ChatKind::Development, None));
        d.upsert_history_agent(agent("dev", true), status(AgentState::Unknown, None, StateScope::Agent));
        let c = d.chat(&ck("dev")).unwrap();
        assert_eq!(c.origin, ChatOrigin::AppManaged);
        assert!(!external_send_locked(c.origin, d.root_view(&ck("dev")).map(|v| v.freshness)));
        // 記録（hosted）を失っていても、アプリ専用領域の作業フォルダ（一般チャット）はアプリ管理。
        d.upsert_chat(mk("gen", ChatKind::General, None));
        assert_eq!(d.chat(&ck("gen")).unwrap().origin, ChatOrigin::AppManaged);
        // 記録の無い開発チャットは外部のまま（外部の再開確認は変えない）。
        d.upsert_chat(mk("ext", ChatKind::Development, None));
        assert_eq!(d.chat(&ck("ext")).unwrap().origin, ChatOrigin::External);
    }

    #[test]
    fn list_order_is_pinned_then_recent_use_then_unknown_and_stable() {
        let mk = |id: &str, created: Option<i64>, used: Option<i64>, pinned: bool| Chat {
            key: ck(id), kind: ChatKind::General, cwd: Known::NotFetched, name: Known::NotFetched, preview: Known::NotFetched, pinned, archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged, draft: None, created_at: created.map_or(Known::NotFetched, |t| Known::direct(UnixMillis(t))),
            last_used_at: used.map(UnixMillis), no_history: false,
        };
        let mut v = vec![
            mk("unk1", None, None, false),
            mk("old", Some(100), None, false),
            mk("used", Some(50), Some(300), false),
            mk("pin", Some(10), None, true),
            mk("unk2", None, None, false),
            mk("new", None, Some(400), false),
        ];
        sort_chats(&mut v);
        let order: Vec<&str> = v.iter().map(|c| c.key.id.0.as_str()).collect();
        assert_eq!(order, ["pin", "new", "used", "old", "unk1", "unk2"]);
    }

    #[test]
    fn only_user_use_moves_a_chat_up_not_background_upserts() {
        let mk = |id: &str, t: i64| Chat {
            key: ck(id), kind: ChatKind::General, cwd: Known::NotFetched, name: Known::NotFetched, preview: Known::NotFetched, pinned: false, archived: Known::NotFetched,
            origin: ChatOrigin::AppManaged, draft: None, created_at: Known::direct(UnixMillis(t)), last_used_at: None, no_history: false,
        };
        let mut d = HostData::default();
        for (id, t) in [("a", 300), ("b", 200), ("c", 100)] {
            d.upsert_chat(mk(id, t));
        }
        // 一覧の再取得（背景）では順序が変わらない。
        d.upsert_chat(mk("c", 100));
        d.upsert_chat(mk("b", 200));
        let ids = |d: &HostData| d.chats.iter().map(|c| c.key.id.0.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&d), ["a", "b", "c"]);
        // 作成・開く・送信（mark_used）で先頭へ。その後の再取得でも保たれる。
        d.mark_used(&ck("c"), UnixMillis(900));
        d.upsert_chat(mk("c", 100));
        assert_eq!(ids(&d), ["c", "a", "b"]);
    }
}
