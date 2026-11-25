//! agy の出力行（`--output-format stream-json`、1行1JSON）の解析と、1ターン分のイベント組立て（純粋。I/Oなし）。
//!
//! 規則:
//! - 壊れた行・未知のイベントは捨てず、`Gap`（欠落の可能性）か `Unrecognized` に回す。本文は警告に含めない。
//! - 終端は `result` の明示、またはプロセス終了の観測からだけ作る。出力を読めなかった・プロセスの終了を確認できない
//!   場合は `TurnEnded` を作らず、結果不明（`TurnStatus::Unknown`）にする（完了・失敗にしない）。
//! - 承認が必要な操作は print モードで自動拒否される。`denied_actions` は警告にし、成功を偽らない。

use super::store::{DeniedAction, TurnStatus, Usage};
use crate::backend::backend::{BackendEvent, GapScope};
use crate::backend::model::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const SUMMARY_MAX_CHARS: usize = 200;
/// 失敗文に含める stderr 末尾の最大文字数。
const STDERR_ERROR_MAX_CHARS: usize = 1500;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

fn tail_chars(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - max_chars).collect::<String>())
    }
}

// ───────────────────────────── 行の解析 ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum StepState {
    Active,
    Done,
    Error,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitInfo {
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepUpdate {
    pub conversation_id: Option<String>,
    pub step_index: u64,
    pub state: StepState,
    pub step_type: String,
    pub text_delta: Option<String>,
    pub tool_name: Option<String>,
    /// コマンド（`parameters.CommandLine`）。他のツールは None。
    pub tool_detail: Option<String>,
    /// `tool_info.error` の「type: message」。
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultInfo {
    pub conversation_id: Option<String>,
    pub status: String,
    pub response: Option<String>,
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub denied_actions: Vec<DeniedAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgyEvent {
    Init(InitInfo),
    Step(StepUpdate),
    Result(ResultInfo),
    /// 知らない `event` 名。
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Empty,
    Event(AgyEvent),
    /// 解釈できない行（理由は短い固定文。行の内容は含めない）。
    Malformed(String),
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn usage_of(v: Option<&Value>) -> Option<Usage> {
    let u = v?.as_object()?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64);
    Some(Usage { input_tokens: n("input_tokens"), output_tokens: n("output_tokens"), thinking_tokens: n("thinking_tokens"), cache_read_tokens: n("cache_read_tokens"), total_tokens: n("total_tokens") })
}

/// `{type, message}` または文字列を「type: message」にする。
fn error_text(v: &Value) -> Option<String> {
    match v {
        Value::String(t) if !t.is_empty() => Some(t.clone()),
        Value::Object(o) => {
            let ty = o.get("type").and_then(Value::as_str).filter(|t| !t.is_empty());
            let msg = o.get("message").and_then(Value::as_str).filter(|t| !t.is_empty());
            match (ty, msg) {
                (Some(t), Some(m)) => Some(format!("{t}: {m}")),
                (None, Some(m)) => Some(m.to_string()),
                (Some(t), None) => Some(t.to_string()),
                (None, None) => None,
            }
        }
        _ => None,
    }
}

pub fn parse_line(raw: &str) -> Line {
    let raw = raw.trim();
    if raw.is_empty() {
        return Line::Empty;
    }
    let Ok(v) = serde_json::from_str::<Value>(raw) else { return Line::Malformed("not valid JSON".into()) };
    if !v.is_object() {
        return Line::Malformed("not a JSON object".into());
    }
    let Some(event) = v.get("event").and_then(Value::as_str) else { return Line::Malformed("missing event".into()) };
    match event {
        "init" => {
            let init = v.get("init").unwrap_or(&Value::Null);
            Line::Event(AgyEvent::Init(InitInfo {
                conversation_id: s(init, "conversation_id").or_else(|| s(&v, "conversation_id")),
                model: s(init, "model"),
                permission_mode: s(init, "permission_mode"),
            }))
        }
        "step_update" => {
            let Some(u) = v.get("step_update").filter(|u| u.is_object()) else { return Line::Malformed("step_update without body".into()) };
            let Some(step_index) = u.get("step_index").and_then(Value::as_u64) else { return Line::Malformed("step_update without step_index".into()) };
            let info = u.get("tool_info");
            let error = info.and_then(|i| i.get("error")).and_then(error_text).or_else(|| u.get("error").and_then(error_text));
            Line::Event(AgyEvent::Step(StepUpdate {
                conversation_id: s(u, "conversation_id"),
                step_index,
                state: match u.get("state").and_then(Value::as_str) {
                    Some(t) if t.eq_ignore_ascii_case("ACTIVE") => StepState::Active,
                    Some(t) if t.eq_ignore_ascii_case("DONE") => StepState::Done,
                    Some(t) if t.eq_ignore_ascii_case("ERROR") => StepState::Error,
                    Some(t) => StepState::Other(t.to_string()),
                    None => StepState::Other(String::new()),
                },
                step_type: s(u, "step_type").unwrap_or_default(),
                text_delta: s(u, "text_delta"),
                tool_name: s(u, "tool_name").or_else(|| info.and_then(|i| s(i, "name"))),
                tool_detail: info.and_then(|i| i.get("parameters")).and_then(|p| s(p, "CommandLine")).filter(|c| !c.is_empty()),
                error,
            }))
        }
        "result" => {
            let Some(r) = v.get("result").filter(|r| r.is_object()) else { return Line::Malformed("result without body".into()) };
            let Some(status) = s(r, "status").filter(|t| !t.is_empty()) else { return Line::Malformed("result without status".into()) };
            let denied_actions = r
                .get("denied_actions")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter(|d| d.is_object()).map(|d| DeniedAction { action: s(d, "action"), display_name: s(d, "display_name") }).collect())
                .unwrap_or_default();
            Line::Event(AgyEvent::Result(ResultInfo {
                conversation_id: s(r, "conversation_id"),
                status,
                response: s(r, "response"),
                error: r.get("error").and_then(error_text).or_else(|| s(r, "message")),
                usage: usage_of(r.get("usage")),
                denied_actions,
            }))
        }
        other => Line::Event(AgyEvent::Unknown(other.to_string())),
    }
}

// ───────────────────────────── 1ターンのイベント組立て ─────────────────────────────

/// プロセスの終了時に分かっていること。
#[derive(Debug, Clone, Default)]
pub struct ExitInfo {
    /// 中断を依頼してプロセスを止めた。
    pub interrupted: bool,
    pub exit_code: Option<i32>,
    /// stderr の末尾（失敗時のエラー文に使う）。
    pub stderr_tail: String,
    /// 結果もプロセス終了も確認できなかった理由（出力を読めない・終了しない等）。あれば結果不明。
    pub lost: Option<String>,
}

/// ターンの確定内容（保存用）。
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub status: TurnStatus,
    pub error: Option<String>,
    pub response: String,
    pub denied: Vec<DeniedAction>,
    pub usage: Option<Usage>,
}

struct StepTrack {
    kind: ActivityKind,
    summary: Option<String>,
    text: String,
    closed: bool,
    state: StepState,
}

pub struct TurnStream {
    turn: TurnKey,
    expected_conversation: Option<String>,
    conversation_id: Option<String>,
    learned: Option<String>,
    steps: HashMap<u64, StepTrack>,
    response: String,
    result: Option<ResultInfo>,
    malformed_reported: bool,
    unknown_reported: HashSet<String>,
    mismatch_reported: bool,
}

fn evidence(label: &str, now: UnixMillis) -> Evidence {
    Evidence { source: EvidenceSource::LiveEvent, raw_label: Some(label.to_string()), source_time: None, observed_at: now }
}

fn unrecognized(label: &str, note: impl Into<String>) -> BackendEvent {
    BackendEvent::Unrecognized { raw_label: label.to_string(), note: Some(note.into()) }
}

fn step_kind(step_type: &str, tool_name: Option<&str>) -> ActivityKind {
    match (step_type, tool_name) {
        ("agent_response", _) => ActivityKind::AgentMessage,
        (_, Some("run_command")) => ActivityKind::Command,
        ("tool", _) | (_, Some(_)) => ActivityKind::ToolCall,
        (other, None) => ActivityKind::Other { raw: other.to_string() },
    }
}

pub fn status_label(end: TurnEnd) -> &'static str {
    match end {
        TurnEnd::Completed => "turn:completed",
        TurnEnd::Failed => "turn:failed",
        TurnEnd::Interrupted => "turn:interrupted",
    }
}

fn end_state(end: TurnEnd) -> AgentState {
    match end {
        TurnEnd::Completed => AgentState::Done,
        TurnEnd::Failed => AgentState::Failed,
        TurnEnd::Interrupted => AgentState::Interrupted,
    }
}

/// 拒否された操作の一覧文（`Bash (command)` のように）。
pub fn denied_note(denied: &[DeniedAction]) -> String {
    denied
        .iter()
        .map(|d| match (d.display_name.as_deref(), d.action.as_deref()) {
            (Some(n), Some(a)) => format!("{n} ({a})"),
            (Some(n), None) => n.to_string(),
            (None, Some(a)) => a.to_string(),
            (None, None) => "(unknown)".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn with_stderr(headline: String, stderr_tail: &str) -> String {
    let tail = stderr_tail.trim();
    if tail.is_empty() {
        headline
    } else {
        format!("{headline}\n{}", tail_chars(tail, STDERR_ERROR_MAX_CHARS))
    }
}

impl TurnStream {
    /// `expected_conversation` は2回目以降のターンで `--conversation` に渡した会話ID。
    pub fn new(turn: TurnKey, expected_conversation: Option<String>) -> Self {
        TurnStream {
            turn,
            expected_conversation,
            conversation_id: None,
            learned: None,
            steps: HashMap::new(),
            response: String::new(),
            result: None,
            malformed_reported: false,
            unknown_reported: HashSet::new(),
            mismatch_reported: false,
        }
    }

    fn agent(&self) -> &AgentKey {
        &self.turn.agent
    }

    fn item(&self, index: u64) -> ItemKey {
        ItemKey { agent: self.turn.agent.clone(), turn_id: Some(self.turn.turn_id.clone()), item_id: ExternalId(format!("step-{index}")) }
    }

    fn status(&self, state: AgentState, label: &str, now: UnixMillis) -> BackendEvent {
        BackendEvent::AgentStatus {
            agent: self.agent().clone(),
            status: AgentStatus { state, raw: RawState { label: label.to_string() }, scope: StateScope::Turn, turn: Some(self.turn.turn_id.clone()), wait: None, evidence: evidence("antigravity.process", now) },
        }
    }

    /// プロセスを起動できたとき（最初の出力を待たずに）出す開始イベント。
    pub fn start(&self, now: UnixMillis) -> Vec<BackendEvent> {
        vec![BackendEvent::TurnStarted { turn: self.turn.clone(), evidence: evidence("antigravity.process", now) }, self.status(AgentState::Running, "turn:inProgress", now)]
    }

    /// `result` を受け取った（以後の行は読まない）。
    pub fn has_result(&self) -> bool {
        self.result.is_some()
    }

    /// 新しく判明した agy の会話ID（保存用。取り出すと空になる）。
    pub fn take_learned_conversation(&mut self) -> Option<String> {
        self.learned.take()
    }

    fn learn_conversation(&mut self, id: Option<String>, out: &mut Vec<BackendEvent>) {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        match &self.expected_conversation {
            Some(expected) if *expected != id => {
                // 別の会話として動いた。保存済みの会話IDは書き換えず、知らせる。
                if !self.mismatch_reported {
                    self.mismatch_reported = true;
                    out.push(unrecognized("antigravity.conversation", "agy が指定した会話と異なる会話IDを返しました。保存済みの会話IDは変更しません"));
                }
            }
            Some(_) => self.conversation_id = Some(id),
            None => {
                if self.conversation_id.is_none() {
                    self.conversation_id = Some(id.clone());
                    self.learned = Some(id);
                }
            }
        }
    }

    pub fn on_line(&mut self, line: Line, now: UnixMillis) -> Vec<BackendEvent> {
        let mut out = Vec::new();
        match line {
            Line::Empty => {}
            Line::Malformed(why) => {
                // 欠落の可能性。1ターンに1回だけ知らせる（行の内容は載せない）。
                if !self.malformed_reported {
                    self.malformed_reported = true;
                    out.push(BackendEvent::Gap { scope: GapScope::Agent { agent: self.agent().clone() }, reason: format!("agy output line could not be parsed: {why}") });
                    out.push(unrecognized("antigravity.malformedLine", "agy の出力に解釈できない行があったため無視しました（内容は記録しません）"));
                }
            }
            Line::Event(AgyEvent::Init(i)) => {
                self.learn_conversation(i.conversation_id, &mut out);
                if let Some(model) = i.model.filter(|m| !m.is_empty()) {
                    out.push(BackendEvent::ModelAccepted { agent: self.agent().clone(), choice: ModelChoice { model, effort: None, speed_tier: None }, speed_unspecified: true });
                }
            }
            Line::Event(AgyEvent::Step(step)) => {
                self.learn_conversation(step.conversation_id.clone(), &mut out);
                self.on_step(step, now, &mut out);
            }
            Line::Event(AgyEvent::Result(r)) => {
                self.learn_conversation(r.conversation_id.clone(), &mut out);
                self.result = Some(r);
            }
            Line::Event(AgyEvent::Unknown(name)) => {
                if self.unknown_reported.insert(name.clone()) {
                    out.push(unrecognized(&format!("antigravity.event.{name}"), format!("未対応の agy イベント「{name}」を無視しました")));
                }
            }
        }
        out
    }

    fn activity(&self, index: u64, t: &StepTrack, phase: ActivityPhase, now: UnixMillis) -> BackendEvent {
        let summary = match (&t.kind, phase) {
            // 発言は完了時にだけ本文の先頭を載せる（途中は ActivityDelta が運ぶ）。
            (ActivityKind::AgentMessage, ActivityPhase::Started | ActivityPhase::InProgress) => Known::Missing,
            (ActivityKind::AgentMessage, _) => match t.text.trim() {
                "" => Known::Missing,
                text => Known::direct(truncate(text, SUMMARY_MAX_CHARS)),
            },
            _ => match &t.summary {
                Some(x) => Known::direct(truncate(x, SUMMARY_MAX_CHARS)),
                None => Known::Missing,
            },
        };
        BackendEvent::Activity {
            activity: Activity { key: self.item(index), kind: t.kind.clone(), phase, message_phase: MessagePhase::Unknown, summary, evidence: evidence("antigravity.step_update", now) },
        }
    }

    fn on_step(&mut self, step: StepUpdate, now: UnixMillis, out: &mut Vec<BackendEvent>) {
        // 利用者の入力の記録。AgentDock が送った本文なので活動にしない。
        if step.step_type == "user_input" {
            return;
        }
        let idx = step.step_index;
        let is_message = step.step_type == "agent_response";
        let new = !self.steps.contains_key(&idx);
        if new {
            let summary = step.tool_detail.clone().or_else(|| step.tool_name.clone());
            let t = StepTrack { kind: step_kind(&step.step_type, step.tool_name.as_deref()), summary, text: String::new(), closed: false, state: step.state.clone() };
            out.push(self.activity(idx, &t, ActivityPhase::Started, now));
            self.steps.insert(idx, t);
        }
        if is_message {
            if let Some(d) = step.text_delta.as_deref().filter(|d| !d.is_empty()) {
                self.response.push_str(d);
                if let Some(t) = self.steps.get_mut(&idx) {
                    t.text.push_str(d);
                }
                out.push(BackendEvent::ActivityDelta { item: self.item(idx), delta: d.to_string() });
            }
        }
        let closed_now = match &step.state {
            StepState::Active => false,
            StepState::Done | StepState::Error => true,
            StepState::Other(name) => {
                // 知らない状態名。進行中のまま扱い、1種類につき1回だけ知らせる。
                if self.unknown_reported.insert(format!("state:{name}")) {
                    out.push(unrecognized("antigravity.stepState", format!("未対応の agy ステップ状態「{name}」を進行中として扱いました")));
                }
                false
            }
        };
        let Some(t) = self.steps.get_mut(&idx) else { return };
        if t.closed {
            return;
        }
        if closed_now {
            if let (StepState::Error, Some(e)) = (&step.state, &step.error) {
                t.summary = Some(match &t.summary {
                    Some(s) => format!("{s} — エラー: {e}"),
                    None => format!("エラー: {e}"),
                });
            }
            t.closed = true;
            t.state = step.state.clone();
            let ev = self.activity(idx, &self.steps[&idx], ActivityPhase::Completed, now);
            out.push(ev);
        } else if !new && t.state != step.state {
            t.state = step.state.clone();
            let ev = self.activity(idx, &self.steps[&idx], ActivityPhase::InProgress, now);
            out.push(ev);
        }
    }

    /// プロセスの終了（または `result` 受信）の後に呼ぶ。保存する確定内容と、終端のイベントを返す。
    /// 1ターンにつき1回だけ呼ぶこと。
    pub fn finish(&mut self, exit: &ExitInfo, now: UnixMillis) -> (Finished, Vec<BackendEvent>) {
        let mut out = Vec::new();
        let fin = match self.result.take() {
            Some(r) => self.finish_with_result(r, exit, now, &mut out),
            None => self.finish_without_result(exit, now, &mut out),
        };
        (fin, out)
    }

    fn end_events(&self, end: TurnEnd, error: Option<String>, label: &str, now: UnixMillis, out: &mut Vec<BackendEvent>) {
        out.push(BackendEvent::TurnEnded { turn: self.turn.clone(), end, error, evidence: evidence(label, now) });
        out.push(self.status(end_state(end), status_label(end), now));
    }

    fn finish_with_result(&mut self, r: ResultInfo, exit: &ExitInfo, now: UnixMillis, out: &mut Vec<BackendEvent>) -> Finished {
        if let Some(total) = r.usage.as_ref().and_then(|u| u.total_tokens) {
            // 取れた値だけ。使用量の上限（文脈の大きさ）は agy が示さないので None。
            out.push(BackendEvent::TokenUsage { agent: self.agent().clone(), turn: Some(self.turn.turn_id.clone()), last_total: Some(total), thread_total: None, context_window: None });
        }
        if !r.denied_actions.is_empty() {
            out.push(unrecognized("antigravity.denied_actions", format!("承認が必要な操作を自動拒否しました: {}", denied_note(&r.denied_actions))));
        }
        let (end, error) = if r.status.eq_ignore_ascii_case("SUCCESS") {
            (TurnEnd::Completed, None)
        } else if exit.interrupted {
            (TurnEnd::Interrupted, None)
        } else {
            let headline = match &r.error {
                Some(e) => format!("agy が失敗を報告しました（{}）: {e}", r.status),
                None => format!("agy が失敗を報告しました（{}）", r.status),
            };
            (TurnEnd::Failed, Some(with_stderr(headline, &exit.stderr_tail)))
        };
        self.end_events(end, error.clone(), "antigravity.result", now, out);
        // 最終回答は result.response を優先し、無ければ逐次本文の累積。
        let response = r.response.filter(|t| !t.is_empty()).unwrap_or_else(|| std::mem::take(&mut self.response));
        Finished {
            status: match end {
                TurnEnd::Completed => TurnStatus::Completed,
                TurnEnd::Failed => TurnStatus::Failed,
                TurnEnd::Interrupted => TurnStatus::Interrupted,
            },
            error,
            response,
            denied: r.denied_actions,
            usage: r.usage,
        }
    }

    fn finish_without_result(&mut self, exit: &ExitInfo, now: UnixMillis, out: &mut Vec<BackendEvent>) -> Finished {
        let response = std::mem::take(&mut self.response);
        // 閉じていない活動は、完了扱いにしない。
        let mut open: Vec<u64> = self.steps.iter().filter(|(_, t)| !t.closed).map(|(i, _)| *i).collect();
        open.sort_unstable();
        for i in open {
            let ev = self.activity(i, &self.steps[&i], ActivityPhase::Unconfirmed, now);
            out.push(ev);
        }
        if let Some(why) = &exit.lost {
            // 結果もプロセスの終了も確認できない。終端を作らず、欠落として知らせる。
            let note = format!("agy の結果を確認できませんでした（{why}）");
            out.push(BackendEvent::Gap { scope: GapScope::Agent { agent: self.agent().clone() }, reason: note.clone() });
            out.push(self.status(AgentState::Unknown, "turn:outcomeUnknown", now));
            return Finished { status: TurnStatus::Unknown, error: Some(note), response, denied: Vec::new(), usage: None };
        }
        if exit.interrupted {
            self.end_events(TurnEnd::Interrupted, None, "antigravity.exit", now, out);
            return Finished { status: TurnStatus::Interrupted, error: None, response, denied: Vec::new(), usage: None };
        }
        let headline = match exit.exit_code {
            Some(code) => format!("agy が結果を返さずに終了しました（終了コード {code}）"),
            None => "agy が結果を返さずに終了しました".to_string(),
        };
        let error = with_stderr(headline, &exit.stderr_tail);
        self.end_events(TurnEnd::Failed, Some(error.clone()), "antigravity.exit", now, out);
        Finished { status: TurnStatus::Failed, error: Some(error), response, denied: Vec::new(), usage: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 実機（agy 1.5.x）の出力を固定したもの。
    const INIT: &str = r#"{"event":"init","conversation_id":"9db82f11-0000","init":{"model":"gemini-3.8-flash-low","cwd":"C:\\work","tools":[],"permission_mode":"request-review"}}"#;
    const USER: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":0,"state":"DONE","step_type":"user_input"}}"#;
    const MSG1: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"PONG"}}"#;
    const MSG2: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"\n","duration_seconds":3.77,"usage":{"input_tokens":10647,"output_tokens":2,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":10649}}}"#;
    const RESULT: &str = r#"{"event":"result","result":{"conversation_id":"9db82f11-0000","status":"SUCCESS","response":"PONG\n","duration_seconds":3.86,"num_turns":1,"usage":{"input_tokens":10647,"output_tokens":2,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":10649}}}"#;
    const TOOL_ACTIVE: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"dir C:\\work"}}}}"#;
    const TOOL_ERROR: &str = r#"{"event":"step_update","step_update":{"conversation_id":"9db82f11-0000","step_index":2,"state":"ERROR","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"dir C:\\work"},"error":{"type":"PermissionDenied","message":"denied by policy"}}}}"#;
    const DENIED: &str = r#"{"event":"result","result":{"conversation_id":"9db82f11-0000","status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"Bash"}],"usage":{"total_tokens":5}}}"#;

    fn turn() -> TurnKey {
        TurnKey { agent: AgentKey { backend: BackendKind::Antigravity, id: ExternalId("agy-1".into()) }, turn_id: ExternalId("agy-1-t1".into()) }
    }

    const NOW: UnixMillis = UnixMillis(1000);

    fn feed(stream: &mut TurnStream, lines: &[&str]) -> Vec<BackendEvent> {
        let mut out = Vec::new();
        for l in lines {
            out.extend(stream.on_line(parse_line(l), NOW));
        }
        out
    }

    fn kinds(events: &[BackendEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                BackendEvent::TurnStarted { .. } => "TurnStarted",
                BackendEvent::TurnEnded { .. } => "TurnEnded",
                BackendEvent::AgentStatus { .. } => "AgentStatus",
                BackendEvent::Activity { .. } => "Activity",
                BackendEvent::ActivityDelta { .. } => "ActivityDelta",
                BackendEvent::TokenUsage { .. } => "TokenUsage",
                BackendEvent::ModelAccepted { .. } => "ModelAccepted",
                BackendEvent::Unrecognized { .. } => "Unrecognized",
                BackendEvent::Gap { .. } => "Gap",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn parses_the_observed_lines() {
        match parse_line(INIT) {
            Line::Event(AgyEvent::Init(i)) => {
                assert_eq!(i.conversation_id.as_deref(), Some("9db82f11-0000"));
                assert_eq!(i.model.as_deref(), Some("gemini-3.8-flash-low"));
                assert_eq!(i.permission_mode.as_deref(), Some("request-review"));
            }
            other => panic!("{other:?}"),
        }
        match parse_line(MSG2) {
            Line::Event(AgyEvent::Step(s)) => {
                assert_eq!((s.step_index, s.state, s.step_type.as_str(), s.text_delta.as_deref()), (1, StepState::Done, "agent_response", Some("\n")));
            }
            other => panic!("{other:?}"),
        }
        match parse_line(RESULT) {
            Line::Event(AgyEvent::Result(r)) => {
                assert_eq!((r.status.as_str(), r.response.as_deref()), ("SUCCESS", Some("PONG\n")));
                assert_eq!(r.usage.unwrap().total_tokens, Some(10649));
                assert!(r.denied_actions.is_empty());
            }
            other => panic!("{other:?}"),
        }
        match parse_line(TOOL_ERROR) {
            Line::Event(AgyEvent::Step(s)) => {
                assert_eq!((s.state, s.tool_name.as_deref(), s.tool_detail.as_deref()), (StepState::Error, Some("run_command"), Some("dir C:\\work")));
                assert_eq!(s.error.as_deref(), Some("PermissionDenied: denied by policy"));
            }
            other => panic!("{other:?}"),
        }
        match parse_line(DENIED) {
            Line::Event(AgyEvent::Result(r)) => assert_eq!(r.denied_actions, [DeniedAction { action: Some("command".into()), display_name: Some("Bash".into()) }]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn broken_and_unusual_lines_are_classified_not_dropped_silently() {
        assert_eq!(parse_line("   "), Line::Empty);
        assert!(matches!(parse_line("jetski: no output produced"), Line::Malformed(_)));
        assert!(matches!(parse_line("[1,2]"), Line::Malformed(_)));
        assert!(matches!(parse_line(r#"{"x":1}"#), Line::Malformed(_)));
        assert!(matches!(parse_line(r#"{"event":"step_update","step_update":{"state":"DONE"}}"#), Line::Malformed(_)));
        assert!(matches!(parse_line(r#"{"event":"result","result":{"response":"x"}}"#), Line::Malformed(_)));
        assert_eq!(parse_line(r#"{"event":"future","future":{}}"#), Line::Event(AgyEvent::Unknown("future".into())));
    }

    #[test]
    fn successful_turn_follows_the_codex_event_shape() {
        let mut s = TurnStream::new(turn(), None);
        let mut ev = s.start(NOW);
        ev.extend(feed(&mut s, &[INIT, USER, MSG1, MSG2, RESULT]));
        assert!(s.has_result());
        assert_eq!(s.take_learned_conversation().as_deref(), Some("9db82f11-0000"));
        assert_eq!(s.take_learned_conversation(), None);
        let (fin, end) = s.finish(&ExitInfo::default(), NOW);
        ev.extend(end);
        assert_eq!(
            kinds(&ev),
            ["TurnStarted", "AgentStatus", "ModelAccepted", "Activity", "ActivityDelta", "ActivityDelta", "Activity", "TokenUsage", "TurnEnded", "AgentStatus"]
        );
        // 逐次本文と、完了時の要約。
        let deltas: Vec<_> = ev.iter().filter_map(|e| if let BackendEvent::ActivityDelta { delta, .. } = e { Some(delta.as_str()) } else { None }).collect();
        assert_eq!(deltas, ["PONG", "\n"]);
        let acts: Vec<_> = ev.iter().filter_map(|e| if let BackendEvent::Activity { activity } = e { Some(activity) } else { None }).collect();
        assert_eq!((acts[0].phase, acts[0].summary.clone()), (ActivityPhase::Started, Known::Missing));
        assert_eq!((acts[1].phase, acts[1].kind.clone(), acts[1].summary.clone()), (ActivityPhase::Completed, ActivityKind::AgentMessage, Known::direct("PONG".to_string())));
        assert_eq!(acts[0].key.item_id.0, "step-1");
        assert_eq!(acts[0].key.turn_id.as_ref().unwrap().0, "agy-1-t1");
        assert!(matches!(ev.iter().find(|e| matches!(e, BackendEvent::TokenUsage { .. })), Some(BackendEvent::TokenUsage { last_total: Some(10649), thread_total: None, context_window: None, .. })));
        let BackendEvent::TurnEnded { end, error, .. } = &ev[ev.len() - 2] else { panic!("{ev:?}") };
        assert_eq!((*end, error.clone()), (TurnEnd::Completed, None));
        let BackendEvent::AgentStatus { status, .. } = &ev[ev.len() - 1] else { panic!() };
        assert_eq!((status.state, status.scope, status.turn.clone()), (AgentState::Done, StateScope::Turn, Some(ExternalId("agy-1-t1".into()))));
        assert_eq!((fin.status, fin.response.as_str(), fin.error), (TurnStatus::Completed, "PONG\n", None));
    }

    #[test]
    fn later_turn_with_a_different_conversation_id_is_reported_not_adopted() {
        let mut s = TurnStream::new(turn(), Some("old-conversation".into()));
        let ev = feed(&mut s, &[INIT, MSG1]);
        assert!(ev.iter().any(|e| matches!(e, BackendEvent::Unrecognized { raw_label, .. } if raw_label == "antigravity.conversation")));
        assert_eq!(s.take_learned_conversation(), None);
        let mut same = TurnStream::new(turn(), Some("9db82f11-0000".into()));
        let ev = feed(&mut same, &[INIT]);
        assert!(!ev.iter().any(|e| matches!(e, BackendEvent::Unrecognized { .. })));
        assert_eq!(same.take_learned_conversation(), None);
    }

    #[test]
    fn tool_steps_become_simple_activities_and_errors_are_kept_in_the_summary() {
        let mut s = TurnStream::new(turn(), None);
        let ev = feed(&mut s, &[TOOL_ACTIVE, TOOL_ACTIVE, TOOL_ERROR]);
        let acts: Vec<_> = ev.iter().filter_map(|e| if let BackendEvent::Activity { activity } = e { Some(activity) } else { None }).collect();
        // 同じ状態の繰り返しは増やさない。
        assert_eq!(acts.iter().map(|a| a.phase).collect::<Vec<_>>(), [ActivityPhase::Started, ActivityPhase::Completed]);
        assert!(acts.iter().all(|a| a.kind == ActivityKind::Command));
        assert_eq!(acts[0].summary, Known::direct("dir C:\\work".to_string()));
        assert_eq!(acts[1].summary, Known::direct("dir C:\\work — エラー: PermissionDenied: denied by policy".to_string()));
        // 閉じたあとの更新は無視。
        assert!(feed(&mut s, &[TOOL_ACTIVE]).iter().all(|e| !matches!(e, BackendEvent::Activity { .. })));
    }

    #[test]
    fn denied_actions_are_reported_and_the_turn_is_not_silently_clean() {
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[INIT, DENIED]);
        let (fin, ev) = s.finish(&ExitInfo::default(), NOW);
        let warn = ev.iter().find_map(|e| match e {
            BackendEvent::Unrecognized { raw_label, note } if raw_label == "antigravity.denied_actions" => note.clone(),
            _ => None,
        });
        assert_eq!(warn.as_deref(), Some("承認が必要な操作を自動拒否しました: Bash (command)"));
        assert_eq!(fin.status, TurnStatus::Completed);
        assert_eq!(fin.denied.len(), 1);
        assert_eq!(fin.response, "", "an empty response stays empty");
    }

    #[test]
    fn failed_result_and_missing_result_end_as_failed_with_stderr() {
        let failed = r#"{"event":"result","result":{"conversation_id":"c","status":"ERROR","error":{"type":"Quota","message":"limit"}}}"#;
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[failed]);
        let (fin, ev) = s.finish(&ExitInfo { stderr_tail: "jetski: boom\n".into(), ..ExitInfo::default() }, NOW);
        let BackendEvent::TurnEnded { end, error, .. } = ev.iter().find(|e| matches!(e, BackendEvent::TurnEnded { .. })).unwrap() else { panic!() };
        assert_eq!(*end, TurnEnd::Failed);
        let msg = error.clone().unwrap();
        assert!(msg.contains("ERROR") && msg.contains("Quota: limit") && msg.contains("jetski: boom"), "{msg}");
        assert_eq!(fin.status, TurnStatus::Failed);

        // result が来る前にプロセスが終了した。
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[INIT, MSG1, TOOL_ACTIVE]);
        let (fin, ev) = s.finish(&ExitInfo { exit_code: Some(1), stderr_tail: "auth required".into(), ..ExitInfo::default() }, NOW);
        let k = kinds(&ev);
        assert_eq!(k, ["Activity", "Activity", "TurnEnded", "AgentStatus"]);
        // 閉じていない活動は完了にしない。
        assert!(ev.iter().filter_map(|e| if let BackendEvent::Activity { activity } = e { Some(activity.phase) } else { None }).all(|p| p == ActivityPhase::Unconfirmed));
        let BackendEvent::TurnEnded { end, error, .. } = &ev[2] else { panic!() };
        assert_eq!(*end, TurnEnd::Failed);
        assert!(error.as_deref().unwrap().contains("終了コード 1") && error.as_deref().unwrap().contains("auth required"));
        assert_eq!((fin.status, fin.response.as_str()), (TurnStatus::Failed, "PONG"));
    }

    #[test]
    fn interrupted_process_without_result_ends_as_interrupted() {
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[INIT, MSG1]);
        let (fin, ev) = s.finish(&ExitInfo { interrupted: true, exit_code: Some(1), ..ExitInfo::default() }, NOW);
        let BackendEvent::TurnEnded { end, error, .. } = ev.iter().find(|e| matches!(e, BackendEvent::TurnEnded { .. })).unwrap() else { panic!() };
        assert_eq!((*end, error.clone()), (TurnEnd::Interrupted, None));
        assert_eq!(fin.status, TurnStatus::Interrupted);
        // result があればそれが優先（停止要求より先に完了していた）。
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[INIT, MSG1, MSG2, RESULT]);
        let (fin, _) = s.finish(&ExitInfo { interrupted: true, ..ExitInfo::default() }, NOW);
        assert_eq!(fin.status, TurnStatus::Completed);
    }

    #[test]
    fn unreadable_output_is_outcome_unknown_not_failed_or_done() {
        let mut s = TurnStream::new(turn(), None);
        feed(&mut s, &[INIT, MSG1]);
        let (fin, ev) = s.finish(&ExitInfo { lost: Some("read error".into()), ..ExitInfo::default() }, NOW);
        assert!(!ev.iter().any(|e| matches!(e, BackendEvent::TurnEnded { .. })), "no terminal is invented");
        assert!(ev.iter().any(|e| matches!(e, BackendEvent::Gap { scope: GapScope::Agent { .. }, .. })));
        let BackendEvent::AgentStatus { status, .. } = ev.last().unwrap() else { panic!() };
        assert_eq!(status.state, AgentState::Unknown);
        assert_eq!(fin.status, TurnStatus::Unknown);
        assert!(fin.error.unwrap().contains("read error"));
    }

    #[test]
    fn malformed_lines_become_one_gap_per_turn_and_unknown_events_are_warned_once() {
        let mut s = TurnStream::new(turn(), None);
        let ev = feed(&mut s, &["garbage", "more garbage", r#"{"event":"future"}"#, r#"{"event":"future"}"#]);
        assert_eq!(kinds(&ev), ["Gap", "Unrecognized", "Unrecognized"]);
        let BackendEvent::Gap { scope, reason } = &ev[0] else { panic!() };
        assert!(matches!(scope, GapScope::Agent { .. }));
        assert!(!reason.contains("garbage"), "line content is not logged");
    }

    #[test]
    fn unknown_step_types_and_states_are_kept_visible() {
        let mut s = TurnStream::new(turn(), None);
        let line = r#"{"event":"step_update","step_update":{"step_index":3,"state":"PAUSED","step_type":"planning"}}"#;
        let ev = feed(&mut s, &[line, line]);
        let acts: Vec<_> = ev.iter().filter_map(|e| if let BackendEvent::Activity { activity } = e { Some(activity) } else { None }).collect();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].kind, ActivityKind::Other { raw: "planning".into() });
        assert_eq!(ev.iter().filter(|e| matches!(e, BackendEvent::Unrecognized { raw_label, .. } if raw_label == "antigravity.stepState")).count(), 1);
    }
}
