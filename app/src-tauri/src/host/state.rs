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
use crate::backend::ipc::*;
use crate::backend::local::{AppSettings, SaveScope, SaveStatus};
use crate::backend::model::*;
use crate::store::records::{ChatLocalFile, QueueFile};

/// イベント適用後にホストが非同期で行う追加作業（reducer内では待たない）。
#[derive(Debug, Clone, PartialEq)]
pub enum Followup {
    /// ルートの新しいturn開始を契機に、子孫を再走査する（読み取りのみ）。
    ScanDescendants(AgentKey),
}

pub struct HostData {
    pub seq: u64,
    pub sources: Vec<SourceInfo>,
    pub chats: Vec<Chat>,
    pub agents: Vec<AgentView>,
    pub requests: Vec<PendingRequest>,
    pub stops: Vec<StopRecord>,
    pub queue: Vec<QueueItem>,
    pub monitor_scope: MonitorScope,
    pub pinned: HashSet<ChatKey>,
    /// このホスト（現在の接続）が開始・再開した会話。originに関わらず送信可能として扱う。
    pub hosted: HashSet<ChatKey>,
    pub model_settings: HashMap<ChatKey, ChatModelSettings>,
    /// アプリ側の補足情報（保存ファイルと同じ形。ピン・下書き・モデル/権限など）。保存は `host::persist` が行う。
    pub locals: HashMap<ChatKey, ChatLocalFile>,
    /// 復元したキュー記録（P3が使う。起動時に Active→PausedAfterRestart、Sending→AcceptanceUnknown に変換済み）。
    pub queues: HashMap<ChatKey, QueueFile>,
    pub save_status: HashMap<SaveScope, SaveStatus>,
    pub settings: AppSettings,
    /// 起動時に読めなかった保存ファイルなどの警告。
    pub startup_warnings: Vec<String>,
    /// 親のspawn依頼から確定できた子の担当（エージェントの再登録で失わないよう保持）。
    assignments: HashMap<AgentKey, String>,
    /// 実行中と分かっているturn（中断・追加指示の対象）。
    pub running_turn: HashMap<AgentKey, ExternalId>,
    /// 開始を観測した最新turn（古いturnの後着を捨てる基準）。
    latest_turn_start: HashMap<AgentKey, ExternalId>,
    /// 終端を観測したturn（中断記録の作成が終端通知より遅れた場合の補完）。
    last_end: HashMap<AgentKey, (ExternalId, TurnEnd, UnixMillis)>,
}

impl Default for HostData {
    fn default() -> Self {
        HostData {
            seq: 0,
            sources: Vec::new(),
            chats: Vec::new(),
            agents: Vec::new(),
            requests: Vec::new(),
            stops: Vec::new(),
            queue: Vec::new(),
            monitor_scope: MonitorScope::SelectedChat { chat: None },
            pinned: HashSet::new(),
            hosted: HashSet::new(),
            model_settings: HashMap::new(),
            locals: HashMap::new(),
            queues: HashMap::new(),
            save_status: HashMap::new(),
            settings: AppSettings::default(),
            startup_warnings: Vec::new(),
            assignments: HashMap::new(),
            running_turn: HashMap::new(),
            latest_turn_start: HashMap::new(),
            last_end: HashMap::new(),
        }
    }
}

/// 外部作成の会話で、ユーザーが再開して live 購読が成立するまで送信を止めるか。
/// 外部実行中かどうかは断定しない（再開はユーザーの確認後だけ）。
pub fn external_send_locked(origin: ChatOrigin, root_freshness: Option<Freshness>) -> bool {
    origin == ChatOrigin::External && root_freshness != Some(Freshness::Live)
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
            queue: self.queue.clone(),
            monitor_scope: self.monitor_scope.clone(),
            chat_locals: self.local_views(),
            save_status: self.save_status.values().cloned().collect(),
            settings: self.settings.clone(),
            model_settings: self.model_settings.iter().map(|(chat, settings)| ChatModelEntry { chat: chat.clone(), settings: settings.clone() }).collect(),
            startup_warnings: self.startup_warnings.clone(),
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

    // ── 更新（呼び出し側が返ったイベントを発行する） ──

    pub fn set_source(&mut self, mut info: SourceInfo) -> Vec<HostEvent> {
        if let Some(old) = self.sources.iter().find(|s| s.source == info.source) {
            // 切断通知が先に届いていたら、後から返った接続成功で上書きしない。
            if matches!(old.connection, ConnectionState::Disconnected { .. }) {
                info.connection = old.connection.clone();
            }
        }
        if self.sources.iter().any(|s| s.source != info.source) {
            // 新しい接続。以前の購読は失われているので、ホスト管理の扱いも引き継がない。
            self.hosted.clear();
        }
        // 同じIDの仮の項目（接続通知が結果より先に届いた場合）も置き換える。残すと先頭の仮項目が表示される。
        self.sources.clear();
        self.sources.push(info.clone());
        vec![HostEvent::SourceUpdated { source: info }]
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

    pub fn upsert_chat(&mut self, mut chat: Chat) -> Vec<HostEvent> {
        chat.pinned = self.pinned.contains(&chat.key);
        if self.hosted.contains(&chat.key) {
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
        vec![HostEvent::ChatUpdated { chat }]
    }

    /// 履歴・一覧から得た状態を反映する。live購読中のエージェントの状態は、古い可能性がある読み取りで上書きしない。
    pub fn upsert_history_agent(&mut self, agent: Agent, status: AgentStatus) -> Vec<HostEvent> {
        let agent = self.with_assignment(agent);
        let key = agent.key.clone();
        if let Some(v) = self.view_mut(&key) {
            v.agent = agent;
            if v.freshness != Freshness::Live {
                v.status = status;
                v.freshness = Freshness::HistoryOnly;
            }
            return vec![HostEvent::AgentUpdated { view: v.clone() }];
        }
        let view = AgentView { agent, status, freshness: Freshness::HistoryOnly, current_activity: None };
        self.agents.push(view.clone());
        vec![HostEvent::AgentUpdated { view }]
    }

    /// 再開・新規作成でlive購読が戻ったエージェントを反映する。
    pub fn set_live(&mut self, agent: Agent, status: AgentStatus) -> Vec<HostEvent> {
        let agent = self.with_assignment(agent);
        let key = agent.key.clone();
        if let Some(t) = &status.turn {
            if is_active(status.state) {
                self.running_turn.insert(key.clone(), t.clone());
            }
        }
        let view = match self.view_mut(&key) {
            Some(v) => {
                v.agent = agent;
                v.status = status;
                v.freshness = Freshness::Live;
                v.clone()
            }
            None => {
                let view = AgentView { agent, status, freshness: Freshness::Live, current_activity: None };
                self.agents.push(view.clone());
                view
            }
        };
        vec![HostEvent::AgentUpdated { view }]
    }

    pub fn remove_chat(&mut self, chat: &ChatKey) -> Vec<HostEvent> {
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

    fn set_freshness_where(&mut self, agent: Option<&AgentKey>, to: Freshness, from: impl Fn(Freshness) -> bool) -> Vec<HostEvent> {
        let mut out = Vec::new();
        for v in &mut self.agents {
            if agent.is_some_and(|a| a != &v.agent.key) {
                continue;
            }
            if from(v.freshness) && v.freshness != to {
                v.freshness = to;
                out.push(HostEvent::AgentUpdated { view: v.clone() });
            }
        }
        out
    }

    // ── バックエンドイベント ──

    pub fn apply_event(&mut self, env: &EventEnvelope, caps: &Capabilities) -> (Vec<HostEvent>, Vec<Followup>) {
        let now = env.received_at;
        let mut out = Vec::new();
        let mut follow = Vec::new();
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
                    out.extend(self.set_freshness_where(None, Freshness::Disconnected, |f| f != Freshness::Unsupported));
                    for r in &mut self.requests {
                        if r.key.source == env.source && r.state == RequestState::Pending {
                            r.state = RequestState::Expired { at: now };
                            out.push(HostEvent::RequestUpdated { request: r.clone() });
                        }
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
                }
            }
            BackendEvent::TurnStarted { turn, .. } => {
                self.latest_turn_start.insert(turn.agent.clone(), turn.turn_id.clone());
                self.running_turn.insert(turn.agent.clone(), turn.turn_id.clone());
                if let Some(v) = self.view_mut(&turn.agent) {
                    v.agent.latest_turn = Some(turn.turn_id.clone());
                }
                if self.is_root(&turn.agent) {
                    follow.push(Followup::ScanDescendants(turn.agent.clone()));
                }
                out.push(HostEvent::TurnUpdated { turn: turn.clone(), end: None });
            }
            BackendEvent::TurnEnded { turn, end, evidence: ev, .. } => {
                if self.running_turn.get(&turn.agent) == Some(&turn.turn_id) {
                    self.running_turn.remove(&turn.agent);
                }
                let at = ev.source_time.unwrap_or(now);
                self.last_end.insert(turn.agent.clone(), (turn.turn_id.clone(), *end, at));
                out.push(HostEvent::TurnUpdated { turn: turn.clone(), end: Some(*end) });
                for r in &mut self.stops {
                    if stop::apply_turn_end(r, turn, *end, at, now) {
                        out.push(HostEvent::StopUpdated { record: r.clone() });
                    }
                }
            }
            BackendEvent::Activity { activity } => {
                out.push(HostEvent::ActivityUpdated { activity: activity.clone() });
                // サブエージェント関連のitem（spawn・待機・送信など）が来たら、子孫の監視を始める（既に動いていれば何もしない）。
                if matches!(activity.kind, ActivityKind::SubAgent) {
                    if let Some(v) = self.view(&activity.key.agent) {
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
            BackendEvent::RequestOpened { request } => {
                match self.requests.iter_mut().find(|r| r.key == request.key) {
                    Some(old) => *old = request.clone(),
                    None => self.requests.push(request.clone()),
                }
                out.push(HostEvent::RequestUpdated { request: request.clone() });
            }
            BackendEvent::RequestResolved { request, .. } => {
                if let Some(r) = self.requests.iter_mut().find(|r| &r.key == request) {
                    if matches!(r.state, RequestState::Pending | RequestState::AnswerUnconfirmed { .. }) {
                        r.state = RequestState::ResolvedElsewhere { at: now };
                        out.push(HostEvent::RequestUpdated { request: r.clone() });
                    }
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
            BackendEvent::ModelRerouted { agent, effective, .. } => {
                if let Some(chat) = self.view(agent).filter(|_| self.is_root(agent)).map(|v| v.agent.chat.clone()) {
                    let s = self.model_settings.entry(chat.clone()).or_insert(ChatModelSettings {
                        selected: None,
                        accepted: Known::NotFetched,
                        effective: Known::NotFetched,
                        applies: ApplyTiming::Unknown,
                    });
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
                    GapScope::Source => out.extend(self.set_freshness_where(None, Freshness::NeedsReconcile, |f| f == Freshness::Live)),
                    GapScope::Agent { agent } => {
                        out.extend(self.set_freshness_where(Some(agent), Freshness::NeedsReconcile, |f| f == Freshness::Live))
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
            archived: Known::NotFetched, origin: ChatOrigin::External, draft: None, created_at: Known::NotFetched, last_used_at: None,
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
            kind: ChatKind::General,
            cwd: Known::NotFetched,
            name: Known::NotFetched,
            preview: Known::NotFetched,
            pinned: false,
            archived: Known::NotFetched,
            origin,
            draft: None,
            created_at: Known::NotFetched,
            last_used_at: None,
        };
        d.upsert_chat(mk(ChatOrigin::External));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::External);
        let ev = d.mark_hosted(&ck("root"));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::AppManaged);
        assert!(matches!(ev[0], HostEvent::ChatUpdated { .. }));
        // 一覧の再取得でexternalと返ってもホスト管理を保つ。
        d.upsert_chat(mk(ChatOrigin::External));
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::AppManaged);
        // 新しい接続では引き継がない。
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
        assert_eq!(d.chat(&ck("root")).unwrap().origin, ChatOrigin::External);
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
}
