//! Codexの型 → 共通データモデルの純粋な変換（要件§4.1・§4.2・§5）。I/Oなし。時刻は呼び出し側が渡す。
//!
//! 規則:
//! - notLoaded・通信断・取得不能は `Done`/`Failed` にしない。`Unknown`＋鮮度（[`freshness_for`]）で表す。
//! - 終端（`TurnEnd`）は turn の明示的な status からだけ作る。
//! - 不明値は `Known` の NotFetched / Missing で表し、0や空文字で代用しない。

use super::wire::{WireStatus, WireThread, WireTurn, WireTurnStatus};
use crate::backend::backend::PermissionPreset;
use crate::backend::model::*;
use serde_json::Value;

const SUMMARY_MAX_CHARS: usize = 200;

// ───────────────────────────── ID・時刻 ─────────────────────────────

pub fn ext(s: &str) -> ExternalId {
    ExternalId(s.to_string())
}

pub fn agent_key(id: &str) -> AgentKey {
    AgentKey { backend: BackendKind::Codex, id: ext(id) }
}

pub fn chat_key(id: &str) -> ChatKey {
    ChatKey { backend: BackendKind::Codex, id: ext(id) }
}

pub fn turn_key(thread_id: &str, turn_id: &str) -> TurnKey {
    TurnKey { agent: agent_key(thread_id), turn_id: ext(turn_id) }
}

/// Codexの秒単位Unix時刻 → ミリ秒。
pub fn ms_from_secs(secs: i64) -> UnixMillis {
    UnixMillis(secs.saturating_mul(1000))
}

pub fn evidence(source: EvidenceSource, raw_label: &str, source_time: Option<UnixMillis>, now: UnixMillis) -> Evidence {
    Evidence { source, raw_label: Some(raw_label.to_string()), source_time, observed_at: now }
}

/// 根拠の種類から鮮度を決める（§4.2）。live購読のイベントだけが `Live`。
/// readや一覧は購読を戻さないので `HistoryOnly`。resume成功後にホストが `Live` へ上げる。
pub fn freshness_for(source: EvidenceSource) -> Freshness {
    match source {
        EvidenceSource::LiveEvent => Freshness::Live,
        EvidenceSource::HistoryRead | EvidenceSource::StatusQuery | EvidenceSource::Response => Freshness::HistoryOnly,
        EvidenceSource::HostReconcile => Freshness::NeedsReconcile,
    }
}

pub fn known_str(v: &Option<String>) -> Known<String> {
    match v {
        Some(s) if !s.is_empty() => Known::direct(s.clone()),
        _ => Known::Missing,
    }
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

// ───────────────────────────── 状態（§4.1） ─────────────────────────────

pub fn status_label(status: &WireStatus) -> String {
    match status {
        WireStatus::NotLoaded => "notLoaded".into(),
        WireStatus::Idle => "idle".into(),
        WireStatus::SystemError => "systemError".into(),
        WireStatus::Active { flags } if flags.is_empty() => "active".into(),
        WireStatus::Active { flags } => format!("active[{}]", flags.join(",")),
        WireStatus::Other(o) => format!("unrecognized:{o}"),
    }
}

fn turn_state(status: &WireTurnStatus) -> Option<AgentState> {
    match status {
        WireTurnStatus::Completed => Some(AgentState::Done),
        WireTurnStatus::Interrupted => Some(AgentState::Interrupted),
        WireTurnStatus::Failed => Some(AgentState::Failed),
        WireTurnStatus::InProgress => Some(AgentState::Running),
        WireTurnStatus::Other(_) => None,
    }
}

/// turnの終端。明示された終了statusだけ（inProgress・未知は `None`）。
pub fn turn_end(status: &WireTurnStatus) -> Option<TurnEnd> {
    match status {
        WireTurnStatus::Completed => Some(TurnEnd::Completed),
        WireTurnStatus::Interrupted => Some(TurnEnd::Interrupted),
        WireTurnStatus::Failed => Some(TurnEnd::Failed),
        _ => None,
    }
}

fn turn_label(t: &WireTurn) -> String {
    match &t.status {
        WireTurnStatus::Completed => "turn:completed".into(),
        WireTurnStatus::Interrupted => "turn:interrupted".into(),
        WireTurnStatus::Failed => "turn:failed".into(),
        WireTurnStatus::InProgress => "turn:inProgress".into(),
        WireTurnStatus::Other(o) => format!("turn:unrecognized:{o}"),
    }
}

fn wait_from_flags(flags: &[String], ev: &Evidence) -> Option<WaitInfo> {
    let reason = if flags.iter().any(|f| f == "waitingOnApproval") {
        WaitReason::Approval { request: None }
    } else if flags.iter().any(|f| f == "waitingOnUserInput") {
        WaitReason::UserInput { request: None }
    } else if let Some(other) = flags.first() {
        WaitReason::Other { raw: other.clone() }
    } else {
        return None;
    };
    Some(WaitInfo { reason, started: ev.clone() })
}

/// スレッドの `status` から [`AgentStatus`] を作る（thread/read・thread/list・status/changed で同じ規則）。
///
/// - `active`: 原状態どおり。flagsがあれば waiting（理由は flag）、なければ running。
/// - `idle`: 常に `Idle`（ロード済みでactive turnなし、§4.1）。履歴上のturn終端は `TurnRecord.end` と
///   `TurnEnded` イベントで表し、取得経路によってバッジが揺れないよう、ここでは Done/Failed にしない。
/// - `notLoaded`: 常に `Unknown`（履歴で終端が確認できても Done/Failed にしない。履歴取得の事実は鮮度 `HistoryOnly` で示す）。
/// - `systemError`: `Unknown`（失敗の明示か未確認か判断できないため保守的に。DESIGN_T1 §5-8）。警告は呼び出し側が出す。
/// - 未知の型: `Unknown`。
pub fn derive_status(thread_status: &WireStatus, latest_turn: Option<&WireTurn>, ev: Evidence) -> AgentStatus {
    let label = status_label(thread_status);
    let (state, wait, turn) = match thread_status {
        WireStatus::Active { flags } => {
            let wait = wait_from_flags(flags, &ev);
            let state = if wait.is_some() { AgentState::Waiting } else { AgentState::Running };
            (state, wait, latest_turn.filter(|t| t.status == WireTurnStatus::InProgress).map(|t| ext(t.id())))
        }
        WireStatus::Idle => (AgentState::Idle, None, None),
        WireStatus::NotLoaded | WireStatus::SystemError | WireStatus::Other(_) => (AgentState::Unknown, None, None),
    };
    AgentStatus { state, raw: RawState { label }, scope: StateScope::Agent, turn, wait, evidence: ev }
}

/// turn単位の状態（turn/started・turn/completed通知用）。未知statusは `Unknown`。
pub fn turn_status(turn: &WireTurn, ev: Evidence) -> AgentStatus {
    AgentStatus {
        state: turn_state(&turn.status).unwrap_or(AgentState::Unknown),
        raw: RawState { label: turn_label(turn) },
        scope: StateScope::Turn,
        turn: Some(ext(turn.id())),
        wait: None,
        evidence: ev,
    }
}

// ───────────────────────────── スレッド → チャット・エージェント ─────────────────────────────

/// 直接親の関係（M03）。明示されたIDだけを使い、深さからは推定しない。
pub fn parent_link(t: &WireThread) -> ParentLink {
    if let Some(p) = &t.parent_thread_id {
        return ParentLink::Explicit { parent: agent_key(p) };
    }
    if let Some(p) = t.source_parent_thread_id() {
        return ParentLink::Explicit { parent: agent_key(&p) };
    }
    if t.is_subagent_source() {
        ParentLink::Unknown
    } else {
        ParentLink::Root
    }
}

/// `chat` はこのスレッドが属するチャット。ルートは自分自身のidで呼ぶ（[`default_chat_for`]）。
pub fn thread_to_agent(t: &WireThread, chat: ChatKey) -> Agent {
    let is_root = matches!(parent_link(t), ParentLink::Root);
    Agent {
        key: agent_key(&t.id),
        chat,
        parent: parent_link(t),
        forked_from: Known::Value { value: t.forked_from_id.as_deref().map(agent_key), basis: Basis::Direct },
        // ニックネーム・役割はThread直下、無ければ source.subAgent.thread_spawn の値を使う（どちらにも無ければ欠損）。
        display_name: if is_root { known_str(&t.name) } else { known_str(&t.agent_nickname.clone().or_else(|| t.spawn_field("agent_nickname"))) },
        role: known_str(&t.agent_role.clone().or_else(|| t.spawn_field("agent_role"))),
        // 担当: 子の最初の依頼（親が渡した指示）の先頭。子自身のスレッドの値なので別の子に混ざらない（導出値）。
        assignment: match (is_root, t.preview.as_deref().map(str::trim).filter(|p| !p.is_empty())) {
            (false, Some(p)) => Known::Value { value: truncate(p, 80), basis: Basis::Derived },
            _ => Known::NotFetched,
        },
        agent_path: if is_root { Known::NotFetched } else { known_str(&t.spawn_field("agent_path")) },
        latest_turn: t.latest_turn().map(|x| ext(x.id())),
    }
}

/// 所属チャットが未解決のときの既定。ルートなら自分、親が明示されていれば親のid（親がルートとは限らない近似）。
/// 親不明の子は自分のidを仮に使う（ツリー組立てで親不明枠に入る）。
pub fn default_chat_for(t: &WireThread) -> ChatKey {
    match parent_link(t) {
        ParentLink::Explicit { parent } => ChatKey { backend: BackendKind::Codex, id: parent.id },
        _ => chat_key(&t.id),
    }
}

/// 一般チャット（アプリ専用領域の `chats/<dirId>/workspace` 配下）か開発チャットか。`..`・大文字小文字・ドライブ表記は `layout::is_chat_workspace` が吸収する。
pub fn chat_kind(cwd: Option<&str>, app_data_dir: Option<&str>) -> ChatKind {
    match (cwd, app_data_dir) {
        (Some(c), Some(a)) if !a.is_empty() && crate::store::layout::is_chat_workspace(std::path::Path::new(a), std::path::Path::new(c)) => ChatKind::General,
        _ => ChatKind::Development,
    }
}

pub fn chat_origin(t: &WireThread) -> ChatOrigin {
    match t.source.as_ref() {
        Some(Value::String(s)) => match s.as_str() {
            "appServer" => ChatOrigin::AppManaged,
            "cli" | "vscode" | "exec" => ChatOrigin::External,
            _ => ChatOrigin::Unknown,
        },
        // 自分（clientInfo.name）が開始した会話は、App Serverが `{"custom": "<name>"}` で記録する。
        Some(Value::Object(o)) if o.get("custom").and_then(Value::as_str) == Some(super::process::CLIENT_NAME) => ChatOrigin::AppManaged,
        Some(Value::Object(o)) if o.contains_key("custom") => ChatOrigin::External,
        _ => ChatOrigin::Unknown,
    }
}

pub fn thread_to_chat(t: &WireThread, app_data_dir: Option<&str>, archived: Known<bool>) -> Chat {
    Chat {
        key: chat_key(&t.id),
        kind: chat_kind(t.cwd.as_deref(), app_data_dir),
        cwd: known_str(&t.cwd),
        name: known_str(&t.name),
        preview: known_str(&t.preview),
        pinned: false,
        archived,
        origin: chat_origin(t),
        draft: None,
        created_at: match t.created_at {
            Some(s) => Known::direct(ms_from_secs(s)),
            None => Known::Missing,
        },
        last_used_at: None,
        no_history: false,
    }
}

// ───────────────────────────── item → 活動・本文（§5） ─────────────────────────────

fn join_text_content(content: Option<&Value>) -> Option<String> {
    let arr = content?.as_array()?;
    let parts: Vec<&str> = arr
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn str_of<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key).and_then(Value::as_str)
}

fn status_label_of(item: &Value) -> Option<String> {
    str_of(item, "status").map(str::to_string)
}

/// 「（a／b）」形式の補足。値が無い項目は出さない（0や空で代用しない）。
fn details(parts: [Option<String>; 2]) -> String {
    let v: Vec<String> = parts.into_iter().flatten().collect();
    if v.is_empty() {
        String::new()
    } else {
        format!("（{}）", v.join("／"))
    }
}

/// itemの種類と、全文（取得できる範囲）。未知の種類は `Other{raw}`。
pub fn item_kind_and_text(item: &Value) -> (ActivityKind, Option<String>) {
    let ty = str_of(item, "type").unwrap_or("");
    match ty {
        "userMessage" => (ActivityKind::UserMessage, join_text_content(item.get("content"))),
        "agentMessage" => (ActivityKind::AgentMessage, str_of(item, "text").map(str::to_string)),
        "reasoning" => {
            let join = |k: &str| {
                item.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
            };
            let text = join("summary").filter(|s| !s.is_empty()).or_else(|| join("content").filter(|s| !s.is_empty()));
            (ActivityKind::Reasoning, text)
        }
        "plan" => (ActivityKind::Plan, str_of(item, "text").map(str::to_string)),
        "commandExecution" => {
            let exit = item.get("exitCode").and_then(Value::as_i64).map(|c| format!("終了コード {c}"));
            let text = str_of(item, "command").map(|c| format!("{c}{}", details([status_label_of(item), exit])));
            (ActivityKind::Command, text)
        }
        "fileChange" => {
            let paths = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| str_of(c, "path")).collect::<Vec<_>>().join(", "))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "（対象パスの記載なし）".to_string());
            (ActivityKind::FileChange, Some(format!("{paths}{}", details([status_label_of(item), None]))))
        }
        "mcpToolCall" => {
            let name = match (str_of(item, "server"), str_of(item, "tool")) {
                (Some(s), Some(t)) => Some(format!("{s}/{t}")),
                (None, Some(t)) => Some(t.to_string()),
                _ => None,
            };
            let err = item.get("error").filter(|e| !e.is_null()).map(|_| "エラーあり".to_string());
            (ActivityKind::ToolCall, name.map(|n| format!("{n}{}", details([status_label_of(item), err]))))
        }
        "dynamicToolCall" => {
            let name = str_of(item, "tool").map(|t| match str_of(item, "namespace") {
                Some(ns) => format!("{ns}/{t}"),
                None => t.to_string(),
            });
            (ActivityKind::ToolCall, name.map(|n| format!("{n}{}", details([status_label_of(item), None]))))
        }
        "functionCallOutput" => (ActivityKind::ToolCall, str_of(item, "name").map(|n| format!("{n}（出力）"))),
        "webSearch" => (ActivityKind::WebSearch, str_of(item, "query").filter(|q| !q.is_empty()).map(str::to_string)),
        "collabAgentToolCall" => {
            let n = item.get("receiverThreadIds").and_then(Value::as_array).map(Vec::len);
            let target = n.map(|n| format!("対象 {n} 件"));
            let tool = str_of(item, "tool");
            let prompt = str_of(item, "prompt").filter(|p| !p.is_empty()).map(|p| format!(" — {}", truncate(p, 80)));
            (ActivityKind::SubAgent, tool.map(|t| format!("{t}{}{}", details([status_label_of(item), target]), prompt.unwrap_or_default())))
        }
        "subAgentActivity" => {
            let text = match (str_of(item, "kind"), str_of(item, "agentPath")) {
                (Some(k), Some(p)) => Some(format!("{k}: {p}")),
                (Some(k), None) => Some(k.to_string()),
                _ => None,
            };
            (ActivityKind::SubAgent, text)
        }
        "imageView" => (ActivityKind::Other { raw: "imageView".into() }, str_of(item, "path").map(str::to_string)),
        "sleep" => (ActivityKind::Other { raw: "sleep".into() }, item.get("durationMs").and_then(Value::as_i64).map(|ms| format!("待機 {ms}ms"))),
        "imageGeneration" => (ActivityKind::Other { raw: "imageGeneration".into() }, str_of(item, "status").map(|s| format!("画像生成（{s}）"))),
        "enteredReviewMode" | "exitedReviewMode" => (ActivityKind::Other { raw: ty.to_string() }, str_of(item, "review").map(str::to_string)),
        "contextCompaction" => (ActivityKind::Other { raw: "contextCompaction".into() }, Some("文脈を圧縮".to_string())),
        "hookPrompt" => (ActivityKind::Other { raw: "hookPrompt".into() }, None),
        "" => (ActivityKind::Other { raw: "(missing type)".into() }, None),
        other => (ActivityKind::Other { raw: other.to_string() }, None),
    }
}

/// 完了した `fileChange` item が作った・変更したファイルのパス（成果物の候補。削除と、完了していない変更は含めない）。
/// 移動は移動先を返す。実在の確認はホストが行う。
pub fn file_change_paths(item: &Value) -> Vec<String> {
    if str_of(item, "type") != Some("fileChange") || str_of(item, "status") != Some("completed") {
        return Vec::new();
    }
    let Some(changes) = item.get("changes").and_then(Value::as_array) else { return Vec::new() };
    changes
        .iter()
        .filter_map(|c| {
            let kind = c.get("kind");
            match kind.and_then(|k| k.get("type")).and_then(Value::as_str) {
                Some("delete") => None,
                Some("update") => kind.and_then(|k| k.get("move_path")).and_then(Value::as_str).or_else(|| str_of(c, "path")).map(str::to_string),
                _ => str_of(c, "path").map(str::to_string),
            }
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// 親の `collabAgentToolCall`（spawnAgent）から、生成された子の担当（依頼文の先頭）を取り出す。
/// 受信側が1件に確定できるときだけ（複数なら、どの子の依頼か決められないので割り当てない）。
pub fn spawn_assignment(item: &Value) -> Option<(AgentKey, String)> {
    if str_of(item, "type") != Some("collabAgentToolCall") || str_of(item, "tool") != Some("spawnAgent") {
        return None;
    }
    // 受信側は receiverThreadIds、空なら agentsStates のキー（どちらも1件に確定できるときだけ）。
    let mut ids: Vec<String> = item.get("receiverThreadIds").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if ids.is_empty() {
        ids = item.get("agentsStates").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
    }
    let [child] = ids.as_slice() else { return None };
    let child = child.as_str();
    if child.is_empty() {
        return None;
    }
    let prompt = str_of(item, "prompt").map(str::trim).filter(|p| !p.is_empty())?;
    Some((agent_key(child), truncate(prompt, 80)))
}

/// itemの `status` から進行段階を決める。status無しは `default`。
pub fn item_phase(item: &Value, default: ActivityPhase) -> ActivityPhase {
    match str_of(item, "status") {
        Some("inProgress") => ActivityPhase::InProgress,
        Some(_) => ActivityPhase::Completed,
        None => default,
    }
}

pub fn item_id(item: &Value) -> Option<&str> {
    str_of(item, "id")
}

/// 活動1件。`turn_id` はitemが属するturn。
pub fn item_to_activity(item: &Value, agent: &AgentKey, turn_id: Option<&str>, phase: ActivityPhase, ev: Evidence) -> Option<Activity> {
    let id = item_id(item)?;
    let (kind, text) = item_kind_and_text(item);
    Some(Activity {
        key: ItemKey { agent: agent.clone(), turn_id: turn_id.map(ext), item_id: ext(id) },
        kind,
        phase,
        summary: match text {
            Some(t) if !t.is_empty() => Known::direct(truncate(&t, SUMMARY_MAX_CHARS)),
            _ => Known::Missing,
        },
        evidence: ev,
    })
}

/// 履歴1turn分。items が全件ロード済みと確認できない場合は `complete=false`。
pub fn turn_to_record(thread_id: &str, t: &WireTurn) -> TurnRecord {
    let agent = agent_key(thread_id);
    let entries = t
        .raw
        .items
        .iter()
        .filter_map(|item| {
            let id = item_id(item)?;
            let (kind, text) = item_kind_and_text(item);
            Some(TranscriptEntry {
                key: ItemKey { agent: agent.clone(), turn_id: Some(ext(t.id())), item_id: ext(id) },
                kind,
                text: match text {
                    Some(s) if !s.is_empty() => Known::direct(s),
                    _ => Known::Missing,
                },
                phase: item_phase(item, ActivityPhase::Completed),
            })
        })
        .collect();
    let time = |v: Option<i64>| match v {
        Some(s) => Known::direct(ms_from_secs(s)),
        None => Known::Missing,
    };
    TurnRecord {
        key: turn_key(thread_id, t.id()),
        end: turn_end(&t.status),
        started_at: time(t.raw.started_at),
        completed_at: time(t.raw.completed_at),
        entries,
        complete: t.items_complete(),
    }
}

// ───────────────────────────── 送信照合（§3.2「受理不明」） ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessageLookup {
    /// そのclient_message_idのuserMessageが履歴にある。
    Found { turn_id: String },
    /// 全turnが完全に読めて、スレッドがactiveでもなく、十分な時間が経っても痕跡がない。
    NotFound,
    /// 判断できない（部分取得、実行中、異常状態）。再送の根拠にしない。
    Undetermined,
}

/// 送信から、この時間が経つまでは「痕跡なし」と確定しない（遅れて処理される要求を二重送信しないため）。
/// 送信timeout（60秒）の2倍。
pub const NOT_FOUND_MIN_AGE_MS: i64 = 120_000;

/// `includeTurns=true` で読んだスレッドから、client_message_id（`userMessage.clientId`）を探す。
/// `age_ms` は送信（受理不明の確定）からの経過。痕跡なしの確定には [`NOT_FOUND_MIN_AGE_MS`] 以上が必要。
pub fn find_client_message(thread: &WireThread, client_message_id: &str, age_ms: i64) -> ClientMessageLookup {
    for t in &thread.turns {
        for item in &t.raw.items {
            if str_of(item, "type") == Some("userMessage") && str_of(item, "clientId") == Some(client_message_id) {
                return ClientMessageLookup::Found { turn_id: t.id().to_string() };
            }
        }
    }
    let readable = matches!(thread.status, WireStatus::Idle | WireStatus::NotLoaded);
    let complete = thread.turns.iter().all(WireTurn::items_complete);
    if readable && complete && age_ms >= NOT_FOUND_MIN_AGE_MS {
        ClientMessageLookup::NotFound
    } else {
        ClientMessageLookup::Undetermined
    }
}

// ───────────────────────────── モデル・権限 ─────────────────────────────

fn input_kinds(v: Option<&Value>) -> Vec<AttachmentKind> {
    let mut out = Vec::new();
    for m in v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        match m {
            "image" => out.push(AttachmentKind::Image),
            "audio" => out.push(AttachmentKind::Audio),
            _ => {}
        }
    }
    out
}

/// `model/list` の1件。turn/startへ渡す識別子は `model` フィールド。
pub fn model_to_info(m: &Value) -> Option<ModelInfo> {
    let id = str_of(m, "model")?.to_string();
    let efforts = m
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(EffortOption {
                        id: str_of(e, "reasoningEffort")?.to_string(),
                        description: str_of(e, "description").filter(|d| !d.is_empty()).map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ModelInfo {
        display_name: str_of(m, "displayName").filter(|s| !s.is_empty()).unwrap_or(&id).to_string(),
        id,
        description: str_of(m, "description").filter(|s| !s.is_empty()).map(str::to_string),
        efforts,
        default_effort: str_of(m, "defaultReasoningEffort").map(str::to_string),
        is_default: m.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        hidden: m.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        input_kinds: input_kinds(m.get("inputModalities")),
    })
}

/// 権限プリセット → (approvalPolicy, sandbox)。無断でフルアクセスにしない（FullAccessはユーザー選択のみ）。
pub fn permission_params(p: PermissionPreset) -> (&'static str, &'static str) {
    match p {
        PermissionPreset::WorkspaceWriteOnRequest => ("on-request", "workspace-write"),
        PermissionPreset::ReadOnly => ("on-request", "read-only"),
        PermissionPreset::FullAccess => ("never", "danger-full-access"),
    }
}

/// 権限プリセット → `turn/start` の `sandboxPolicy`（turn単位の上書き。以後のturnにも及ぶ）。
/// workspaceWrite の書込み範囲は作業フォルダ（追加の書込みルートは足さない）。ネットワークは許可しない。
pub fn sandbox_policy_json(p: PermissionPreset) -> serde_json::Value {
    match p {
        PermissionPreset::WorkspaceWriteOnRequest => serde_json::json!({
            "type": "workspaceWrite", "writableRoots": [], "networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false
        }),
        PermissionPreset::ReadOnly => serde_json::json!({"type": "readOnly", "networkAccess": false}),
        PermissionPreset::FullAccess => serde_json::json!({"type": "dangerFullAccess"}),
    }
}

// ───────────────────────────── テスト ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev() -> Evidence {
        evidence(EvidenceSource::LiveEvent, "t", None, UnixMillis(1))
    }

    fn thread(v: Value) -> WireThread {
        WireThread::from_value(&v).unwrap()
    }

    fn base(id: &str) -> Value {
        json!({"id": id, "status": {"type": "idle"}, "turns": []})
    }

    #[test]
    fn not_loaded_is_always_unknown_even_with_terminal_turn() {
        for turns in [json!([]), json!([{"id": "t1", "status": "inProgress", "items": []}]), json!([{"id": "t1", "status": "completed", "items": []}]), json!([{"id": "t1", "status": "failed", "items": []}])] {
            let t = thread(json!({"id": "a", "status": {"type": "notLoaded"}, "turns": turns}));
            let s = derive_status(&t.status, t.latest_turn(), ev());
            assert_eq!((s.state, s.scope), (AgentState::Unknown, StateScope::Agent));
            assert_eq!(s.raw.label, "notLoaded");
        }
        assert_eq!(freshness_for(EvidenceSource::HistoryRead), Freshness::HistoryOnly);
    }

    #[test]
    fn idle_stays_idle_regardless_of_history_so_read_and_status_changed_agree() {
        let t = thread(json!({"id": "a", "status": {"type": "idle"}, "turns": [{"id": "t1", "status": "completed", "items": []}]}));
        let from_read = derive_status(&t.status, t.latest_turn(), ev());
        let from_changed = derive_status(&t.status, None, ev());
        assert_eq!(from_read.state, AgentState::Idle);
        assert_eq!((from_read.state, from_read.scope, from_read.raw.clone()), (from_changed.state, from_changed.scope, from_changed.raw.clone()));
    }

    #[test]
    fn active_flags_map_to_waiting_reasons() {
        let mk = |flags: Value| thread(json!({"id": "a", "status": {"type": "active", "activeFlags": flags}, "turns": []}));
        let t = mk(json!(["waitingOnApproval"]));
        let s = derive_status(&t.status, None, ev());
        assert_eq!(s.state, AgentState::Waiting);
        assert!(matches!(s.wait.unwrap().reason, WaitReason::Approval { .. }));
        let t = mk(json!(["waitingOnUserInput"]));
        assert!(matches!(derive_status(&t.status, None, ev()).wait.unwrap().reason, WaitReason::UserInput { .. }));
        let t = mk(json!([]));
        let s = derive_status(&t.status, None, ev());
        assert_eq!((s.state, s.wait.is_none()), (AgentState::Running, true));
        assert_eq!(derive_status(&mk(json!(["waitingOnApproval", "waitingOnUserInput"])).status, None, ev()).raw.label, "active[waitingOnApproval,waitingOnUserInput]");
    }

    #[test]
    fn idle_and_system_error_and_unknown_type() {
        let t = thread(base("a"));
        assert_eq!(derive_status(&t.status, None, ev()).state, AgentState::Idle);
        let t = thread(json!({"id": "a", "status": {"type": "systemError"}}));
        let s = derive_status(&t.status, None, ev());
        assert_eq!((s.state, s.raw.label.as_str()), (AgentState::Unknown, "systemError"));
        let t = thread(json!({"id": "a", "status": {"type": "somethingNew"}}));
        let s = derive_status(&t.status, None, ev());
        assert_eq!(s.state, AgentState::Unknown);
        assert!(s.raw.label.contains("somethingNew"));
        let t = thread(json!({"id": "a"}));
        assert_eq!(derive_status(&t.status, None, ev()).state, AgentState::Unknown);
    }

    #[test]
    fn turn_end_only_from_explicit_status() {
        assert_eq!(turn_end(&WireTurnStatus::InProgress), None);
        assert_eq!(turn_end(&WireTurnStatus::Other("x".into())), None);
        assert_eq!(turn_end(&WireTurnStatus::Interrupted), Some(TurnEnd::Interrupted));
        let t = WireTurn::from_value(&json!({"id": "t", "status": "weird"})).unwrap();
        assert_eq!(turn_status(&t, ev()).state, AgentState::Unknown);
    }

    #[test]
    fn parent_link_prefers_explicit_ids_and_does_not_guess() {
        let t = thread(json!({"id": "c", "parentThreadId": "p", "status": {"type": "idle"}}));
        assert_eq!(parent_link(&t), ParentLink::Explicit { parent: agent_key("p") });
        let t = thread(json!({"id": "c", "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p2", "depth": 1}}}}));
        assert_eq!(parent_link(&t), ParentLink::Explicit { parent: agent_key("p2") });
        let t = thread(json!({"id": "c", "source": {"subAgent": "review"}}));
        assert_eq!(parent_link(&t), ParentLink::Unknown);
        let t = thread(json!({"id": "me", "source": {"custom": "agentdock"}}));
        assert_eq!(chat_origin(&t), ChatOrigin::AppManaged);
        let t = thread(json!({"id": "o", "source": {"custom": "other-client"}}));
        assert_eq!(chat_origin(&t), ChatOrigin::External);
        let t = thread(json!({"id": "r", "source": "appServer"}));
        assert_eq!(parent_link(&t), ParentLink::Root);
    }

    #[test]
    fn file_change_paths_only_from_completed_non_delete_changes() {
        let item = json!({"type": "fileChange", "status": "completed", "changes": [
            {"path": "a.rs", "kind": {"type": "add"}},
            {"path": "b.rs", "kind": {"type": "delete"}},
            {"path": "c.rs", "kind": {"type": "update", "move_path": "d.rs"}},
            {"path": "e.rs", "kind": {"type": "update", "move_path": null}},
        ]});
        assert_eq!(file_change_paths(&item), vec!["a.rs", "d.rs", "e.rs"]);
        assert!(file_change_paths(&json!({"type": "fileChange", "status": "failed", "changes": [{"path": "a"}]})).is_empty());
        assert!(file_change_paths(&json!({"type": "commandExecution", "status": "completed"})).is_empty());
    }

    #[test]
    fn work_log_items_show_available_attributes_not_missing() {
        let t = |v: Value| item_kind_and_text(&v).1;
        assert_eq!(t(json!({"type": "commandExecution", "command": "cargo test", "status": "completed", "exitCode": 0})).as_deref(), Some("cargo test（completed／終了コード 0）"));
        assert_eq!(t(json!({"type": "commandExecution", "command": "ls", "status": "inProgress", "exitCode": null})).as_deref(), Some("ls（inProgress）"));
        assert_eq!(t(json!({"type": "fileChange", "status": "completed", "changes": [{"path": "a.rs"}, {"path": "b.rs"}]})).as_deref(), Some("a.rs, b.rs（completed）"));
        assert_eq!(t(json!({"type": "mcpToolCall", "server": "s", "tool": "t", "status": "failed", "error": {"message": "x"}})).as_deref(), Some("s/t（failed／エラーあり）"));
        let c = t(json!({"type": "collabAgentToolCall", "tool": "spawnAgent", "status": "completed", "receiverThreadIds": ["a"], "prompt": "調べて"}));
        assert_eq!(c.as_deref(), Some("spawnAgent（completed／対象 1 件） — 調べて"));
        assert_eq!(t(json!({"type": "subAgentActivity", "kind": "started", "agentPath": "root/a"})).as_deref(), Some("started: root/a"));
        // 本当に値が無いものだけ None。
        assert_eq!(t(json!({"type": "commandExecution"})), None);
        assert_eq!(t(json!({"type": "reasoning", "summary": [], "content": []})), None);
        assert!(matches!(item_kind_and_text(&json!({"type": "sleep", "durationMs": 5})).0, ActivityKind::Other { .. }));
    }

    #[test]
    fn child_role_and_nickname_fall_back_to_thread_spawn_source() {
        let t = thread(json!({
            "id": "c1", "status": {"type": "idle"},
            "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p", "depth": 1, "agent_path": "/root/plato", "agent_nickname": "Plato", "agent_role": "explorer"}}}
        }));
        let a = thread_to_agent(&t, chat_key("p"));
        assert_eq!(a.display_name, Known::direct("Plato".to_string()));
        assert_eq!(a.role, Known::direct("explorer".to_string()));
        // Thread直下の値があればそちらを使う。
        let t = thread(json!({"id": "c2", "agentNickname": "Top", "agentRole": "worker", "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p", "agent_role": "other"}}}}));
        let a = thread_to_agent(&t, chat_key("p"));
        assert_eq!((a.display_name, a.role), (Known::direct("Top".to_string()), Known::direct("worker".to_string())));
        // どこにも無ければ欠損（nullは値なし）。
        let t = thread(json!({"id": "c3", "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p", "agent_role": null}}}}));
        assert_eq!(thread_to_agent(&t, chat_key("p")).role, Known::Missing);
    }

    #[test]
    fn child_assignment_and_path_come_from_its_own_thread() {
        let t = thread(json!({
            "id": "c1", "preview": "再送経路を洗い出して。ログも見て", "status": {"type": "idle"},
            "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p", "depth": 1, "agent_path": "/root/luna", "agent_nickname": "Plato", "agent_role": null}}}
        }));
        let a = thread_to_agent(&t, chat_key("p"));
        assert_eq!(a.agent_path, Known::direct("/root/luna".to_string()));
        assert_eq!(a.role, Known::Missing);
        assert_eq!(a.assignment, Known::Value { value: "再送経路を洗い出して。ログも見て".to_string(), basis: Basis::Derived });
        // ルートや、依頼文が無い子には作らない。
        let root = thread(json!({"id": "r", "preview": "何かして", "status": {"type": "idle"}}));
        assert_eq!(thread_to_agent(&root, chat_key("r")).assignment, Known::NotFetched);
        let empty = thread(json!({"id": "c2", "preview": "  ", "status": {"type": "idle"}, "source": {"subAgent": {"thread_spawn": {"parent_thread_id": "p"}}}}));
        assert_eq!(thread_to_agent(&empty, chat_key("p")).assignment, Known::NotFetched);
    }

    #[test]
    fn spawn_assignment_only_when_the_receiver_is_unambiguous() {
        let item = |ids: Value, tool: &str| json!({"type": "collabAgentToolCall", "tool": tool, "status": "completed", "receiverThreadIds": ids, "prompt": "  再送経路を洗い出して  "});
        assert_eq!(spawn_assignment(&item(json!(["c1"]), "spawnAgent")), Some((agent_key("c1"), "再送経路を洗い出して".to_string())));
        assert_eq!(spawn_assignment(&item(json!(["c1", "c2"]), "spawnAgent")), None, "ambiguous: do not guess");
        assert_eq!(spawn_assignment(&item(json!([]), "spawnAgent")), None);
        let by_state = json!({"type": "collabAgentToolCall", "tool": "spawnAgent", "receiverThreadIds": [], "prompt": "調べて", "agentsStates": {"c9": {"status": "running", "message": null}}});
        assert_eq!(spawn_assignment(&by_state), Some((agent_key("c9"), "調べて".to_string())));
        assert_eq!(spawn_assignment(&item(json!(["c1"]), "sendInput")), None);
        let long = json!({"type": "collabAgentToolCall", "tool": "spawnAgent", "receiverThreadIds": ["c1"], "prompt": "あ".repeat(120)});
        assert_eq!(spawn_assignment(&long).unwrap().1.chars().count(), 81);
    }

    #[test]
    fn agent_role_nickname_and_assignment() {
        let t = thread(json!({"id": "c", "parentThreadId": "p", "agentNickname": "Euler", "agentRole": "explorer", "forkedFromId": null}));
        let a = thread_to_agent(&t, chat_key("p"));
        assert_eq!(a.display_name.value().map(String::as_str), Some("Euler"));
        assert_eq!(a.role.value().map(String::as_str), Some("explorer"));
        assert_eq!(a.assignment, Known::NotFetched);
        assert_eq!(a.chat, chat_key("p"));
    }

    #[test]
    fn chat_kind_by_app_dir() {
        assert_eq!(chat_kind(Some(r"C:\Data\AgentDock\chats\a\workspace\s"), Some("c:/data/agentdock/")), ChatKind::General);
        assert_eq!(chat_kind(Some(r"C:\Data\AgentDock\diag"), Some(r"C:\Data\AgentDock")), ChatKind::Development);
        assert_eq!(chat_kind(Some(r"C:\Data\AgentDock\chats\a"), Some(r"C:\Data\AgentDock")), ChatKind::Development);
        assert_eq!(chat_kind(Some(r"C:\Data\AgentDock\chats\a\workspace\..\..\..\x"), Some(r"C:\Data\AgentDock")), ChatKind::Development);
        assert_eq!(chat_kind(Some(r"C:\Work\proj"), Some(r"C:\Data\AgentDock")), ChatKind::Development);
        assert_eq!(chat_kind(None, Some("x")), ChatKind::Development);
    }

    #[test]
    fn item_kinds_and_unknown_items_are_kept() {
        let (k, t) = item_kind_and_text(&json!({"type": "commandExecution", "id": "i", "command": "ls"}));
        assert_eq!((k, t.as_deref()), (ActivityKind::Command, Some("ls")));
        let (k, t) = item_kind_and_text(&json!({"type": "userMessage", "content": [{"type": "text", "text": "a"}, {"type": "image", "url": "u"}, {"type": "text", "text": "b"}]}));
        assert_eq!((k, t.as_deref()), (ActivityKind::UserMessage, Some("a\nb")));
        let (k, _) = item_kind_and_text(&json!({"type": "futureThing"}));
        assert_eq!(k, ActivityKind::Other { raw: "futureThing".into() });
        let (k, _) = item_kind_and_text(&json!({"type": "collabAgentToolCall", "tool": "spawnAgent"}));
        assert_eq!(k, ActivityKind::SubAgent);
        assert_eq!(item_phase(&json!({"status": "inProgress"}), ActivityPhase::Started), ActivityPhase::InProgress);
        assert_eq!(item_phase(&json!({"status": "failed"}), ActivityPhase::Started), ActivityPhase::Completed);
        assert_eq!(item_phase(&json!({}), ActivityPhase::Started), ActivityPhase::Started);
    }

    #[test]
    fn turn_record_marks_partial_items() {
        let t = WireTurn::from_value(&json!({"id": "t", "status": "inProgress", "itemsView": "summary", "items": [{"type": "plan", "id": "i", "text": "p"}], "startedAt": 5})).unwrap();
        let r = turn_to_record("th", &t);
        assert!(!r.complete);
        assert_eq!(r.end, None);
        assert_eq!(r.started_at, Known::direct(UnixMillis(5000)));
        assert_eq!(r.completed_at, Known::Missing);
        assert_eq!(r.entries.len(), 1);
    }

    #[test]
    fn reconcile_lookup_is_conservative() {
        let old = NOT_FOUND_MIN_AGE_MS;
        let full = |status: &str, tstatus: &str, items: Value| {
            thread(json!({"id": "a", "status": {"type": status}, "turns": [{"id": "t1", "status": tstatus, "itemsView": "full", "items": items}]}))
        };
        let hit = full("idle", "completed", json!([{"type": "userMessage", "id": "u", "clientId": "m1", "content": []}]));
        assert_eq!(find_client_message(&hit, "m1", 0), ClientMessageLookup::Found { turn_id: "t1".into() });
        let miss = full("idle", "completed", json!([]));
        assert_eq!(find_client_message(&miss, "m1", old), ClientMessageLookup::NotFound);
        // 送信直後は、遅れて処理される可能性があるので「なし」と確定しない。
        assert_eq!(find_client_message(&miss, "m1", old - 1), ClientMessageLookup::Undetermined);
        // 実行中は未反映の可能性があるので「なし」と断定しない。
        let active = full("active", "inProgress", json!([]));
        assert_eq!(find_client_message(&active, "m1", old), ClientMessageLookup::Undetermined);
        // items が部分取得なら断定しない。
        let partial = thread(json!({"id": "a", "status": {"type": "idle"}, "turns": [{"id": "t1", "status": "completed", "itemsView": "summary", "items": []}]}));
        assert_eq!(find_client_message(&partial, "m1", old), ClientMessageLookup::Undetermined);
    }

    #[test]
    fn latest_turn_prefers_null_started_at_then_array_order() {
        let t = thread(json!({"id": "a", "turns": [
            {"id": "t1", "status": "completed", "startedAt": 100},
            {"id": "t2", "status": "inProgress", "startedAt": null},
            {"id": "t0", "status": "completed", "startedAt": 50}]}));
        assert_eq!(t.latest_turn().unwrap().id(), "t2");
        let t = thread(json!({"id": "a", "turns": [{"id": "x", "startedAt": 2}, {"id": "y", "startedAt": 9}, {"id": "z", "startedAt": 9}]}));
        assert_eq!(t.latest_turn().unwrap().id(), "z");
    }

    #[test]
    fn model_conversion() {
        let m = json!({"id": "x", "model": "gpt-5.4", "displayName": "GPT 5.4", "description": "", "hidden": false, "isDefault": true,
            "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "fast"}], "defaultReasoningEffort": "low", "inputModalities": ["text", "image"]});
        let i = model_to_info(&m).unwrap();
        assert_eq!((i.id.as_str(), i.description, i.input_kinds), ("gpt-5.4", None, vec![AttachmentKind::Image]));
        assert_eq!(i.efforts[0].id, "low");
        assert!(model_to_info(&json!({"id": "no-model-field"})).is_none());
    }

    #[test]
    fn permission_presets_never_escalate_by_default() {
        assert_eq!(permission_params(PermissionPreset::WorkspaceWriteOnRequest), ("on-request", "workspace-write"));
        assert_eq!(permission_params(PermissionPreset::ReadOnly).1, "read-only");
    }
}
