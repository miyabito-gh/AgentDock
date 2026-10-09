//! 通知・サーバー要求のJSON → 共通イベント、および要求への応答JSONの組立て。純粋関数（I/Oなし）。
//!
//! 規則:
//! - 未知のメソッドは捨てず `Unrecognized` にする。意図的に写像しない既知の通知（トークン使用量等）だけ空を返す。
//! - 解釈できない要求（未対応のサーバー要求）は自動回答せず `Unrecognized` で表示に回す。
//! - 承認・質問への回答JSONは、提示した選択肢のIDに一致するものだけ作る（任意のdecisionを送らない）。

use super::convert::*;
use super::id::RpcId;
use super::wire::{WireThread, WireTurn, WireTurnStatus};
use crate::backend::backend::{BackendError, BackendEvent, ChatMetaChange, GapScope};
use crate::backend::model::*;
use serde_json::{json, Value};

/// 変換の文脈。`chat_of` はエージェントの所属チャット解決（キャッシュ）。
pub struct EventCtx<'a> {
    pub source: &'a SourceId,
    pub now: UnixMillis,
    pub chat_of: &'a dyn Fn(&AgentKey) -> ChatKey,
}

/// 応答時に必要になる、受信した要求の原文。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRequest {
    pub method: String,
    pub params: Value,
}

pub struct RequestConversion {
    pub event: BackendEvent,
    /// 回答可能な要求だけ保持する（`None` は回答手段なし＝表示のみ）。
    pub stored: Option<StoredRequest>,
}

/// 意図的に写像しない既知の通知（schema stableに存在する）。メソッド名の前方一致。
const KNOWN_UNMAPPED_PREFIXES: &[&str] = &[
    "thread/tokenUsage/",
    "account/",
    "mcpServer/",
    "app/list/",
    "fuzzyFileSearch/",
    "thread/realtime/",
    "item/reasoning/",
    "item/commandExecution/outputDelta",
    "item/commandExecution/terminalInteraction",
    "item/fileChange/",
    "item/mcpToolCall/progress",
    "item/plan/delta",
    "item/autoApprovalReview/",
    "autoApprovalReview/",
    "command/exec/",
    "process/",
    "turn/diff/",
    "turn/plan/",
    "turn/moderationMetadata",
    "rawResponse",
    "hook/",
    "skills/changed",
    "fs/changed",
    "windows",
    "remoteControl/",
    "externalAgentConfig/",
    "project/",
    "thread/project/",
    "thread/environment/",
    "thread/settings/",
    "thread/goal/",
    "thread/queue/",
    "thread/attachment/",
    "thread/compacted",
    "thread/reverted",
    "model/verification",
    "model/safetyBuffering/",
    "modelProvider/",
];

/// 警告として本文を表示に回す通知（note に message を載せる）。
const WARNING_METHODS: &[&str] = &["warning", "guardianWarning", "deprecationNotice", "configWarning"];

fn unrecognized(method: &str, note: impl Into<String>) -> BackendEvent {
    BackendEvent::Unrecognized { raw_label: method.to_string(), note: Some(note.into()) }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// 通知が示した速度（serviceTier）の受理値。欠落＝None（更新しない）、null・"default"＝Some(None)（標準）、その他の文字列＝Some(Some(id))。
pub fn accepted_speed(v: Option<&Value>) -> Option<Option<String>> {
    match v? {
        Value::Null => Some(None),
        Value::String(s) if s == "default" => Some(None),
        Value::String(s) => Some(Some(s.clone())),
        _ => None,
    }
}

pub fn notification_to_events(method: &str, params: &Value, ctx: &EventCtx) -> Vec<BackendEvent> {
    let live = |src_time: Option<UnixMillis>| evidence(EvidenceSource::LiveEvent, method, src_time, ctx.now);
    let Some(events) = convert_known(method, params, ctx, &live) else {
        if WARNING_METHODS.contains(&method) {
            let msg = s(params, "message").or_else(|| s(params, "summary")).unwrap_or("(no message)");
            return vec![unrecognized(method, msg)];
        }
        if KNOWN_UNMAPPED_PREFIXES.iter().any(|p| method.starts_with(p)) {
            return Vec::new();
        }
        return vec![unrecognized(method, "unknown notification method")];
    };
    events
}

/// 写像対象のメソッドなら `Some`（必須項目が欠けていれば Unrecognized を1件）。対象外は `None`。
fn convert_known(method: &str, p: &Value, ctx: &EventCtx, live: &dyn Fn(Option<UnixMillis>) -> Evidence) -> Option<Vec<BackendEvent>> {
    let missing = |what: &str| Some(vec![unrecognized(method, format!("missing {what}"))]);
    Some(match method {
        "thread/started" => {
            let Some(tv) = p.get("thread") else { return missing("thread") };
            let t = match WireThread::from_value(tv) {
                Ok(t) => t,
                Err(e) => return Some(vec![unrecognized(method, e)]),
            };
            let ak = agent_key(&t.id);
            let chat = match parent_link(&t) {
                ParentLink::Explicit { parent } => (ctx.chat_of)(&parent),
                _ => default_chat_for(&t),
            };
            let agent = thread_to_agent(&t, chat);
            let status = derive_status(&t.status, t.latest_turn(), live(None));
            vec![BackendEvent::AgentDiscovered { agent }, BackendEvent::AgentStatus { agent: ak, status }]
        }
        "thread/status/changed" => {
            let (Some(id), Some(_)) = (s(p, "threadId"), p.get("status")) else { return missing("threadId/status") };
            let ws = super::wire::WireStatus::from_value(p.get("status"));
            let mut out = vec![BackendEvent::AgentStatus { agent: agent_key(id), status: derive_status(&ws, None, live(None)) }];
            if ws == super::wire::WireStatus::SystemError {
                // 失敗とは断定せず（Unknown）、警告として残す。
                out.push(unrecognized(method, format!("thread {id} reported systemError (state kept unknown)")));
            }
            out
        }
        "thread/closed" => {
            let Some(id) = s(p, "threadId") else { return missing("threadId") };
            // 購読者なしのidle unloadでも出るため「終了」とは扱わない。状態は据え置き、live購読の喪失（要照合）だけ伝える。
            vec![BackendEvent::Gap {
                scope: GapScope::Agent { agent: agent_key(id) },
                reason: "thread/closed: live subscription ended; agent state unchanged".into(),
            }]
        }
        "turn/started" | "turn/completed" => {
            let (Some(id), Some(tv)) = (s(p, "threadId"), p.get("turn")) else { return missing("threadId/turn") };
            let turn = match WireTurn::from_value(tv) {
                Ok(t) => t,
                Err(e) => return Some(vec![unrecognized(method, e)]),
            };
            let tk = turn_key(id, turn.id());
            let status = turn_status(&turn, live(None));
            let mut out = Vec::new();
            if method == "turn/started" {
                out.push(BackendEvent::TurnStarted { turn: tk, evidence: live(turn.raw.started_at.map(ms_from_secs)) });
            } else {
                match turn_end(&turn.status) {
                    Some(end) => out.push(BackendEvent::TurnEnded {
                        turn: tk,
                        end,
                        error: turn.raw.error.as_ref().and_then(|e| e.message.clone()),
                        evidence: live(turn.raw.completed_at.map(ms_from_secs)),
                    }),
                    // completed通知なのに終端statusでない: 終端を作らず警告に回す。
                    None => out.push(unrecognized(method, format!("turn/completed with non-terminal status {:?}", turn.status))),
                }
            }
            if turn.status != WireTurnStatus::InProgress || method == "turn/started" {
                out.push(BackendEvent::AgentStatus { agent: agent_key(id), status });
            }
            out
        }
        "item/started" | "item/completed" => {
            let (Some(item), Some(tid), Some(turn)) = (p.get("item"), s(p, "threadId"), s(p, "turnId")) else {
                return missing("item/threadId/turnId");
            };
            let (default_phase, ms) = if method == "item/started" {
                (ActivityPhase::Started, p.get("startedAtMs"))
            } else {
                (ActivityPhase::Completed, p.get("completedAtMs"))
            };
            let phase = if method == "item/completed" { ActivityPhase::Completed } else { item_phase(item, default_phase) };
            let time = ms.and_then(Value::as_i64).map(UnixMillis);
            match item_to_activity(item, &agent_key(tid), Some(turn), phase, live(time)) {
                Some(activity) => {
                    let mut out = vec![BackendEvent::Activity { activity: activity.clone() }];
                    if method == "item/completed" {
                        for path in file_change_paths(item) {
                            out.push(BackendEvent::ArtifactObserved { agent: activity.key.agent.clone(), item: activity.key.clone(), path });
                        }
                    }
                    if let Some((agent, assignment)) = super::convert::spawn_assignment(item) {
                        out.push(BackendEvent::AgentAssignment { agent, assignment });
                    }
                    out
                }
                None => vec![unrecognized(method, "item without id")],
            }
        }
        "item/agentMessage/delta" => {
            let (Some(tid), Some(turn), Some(item), Some(delta)) = (s(p, "threadId"), s(p, "turnId"), s(p, "itemId"), s(p, "delta")) else {
                return missing("threadId/turnId/itemId/delta");
            };
            vec![BackendEvent::ActivityDelta {
                item: ItemKey { agent: agent_key(tid), turn_id: Some(ext(turn)), item_id: ext(item) },
                delta: delta.to_string(),
            }]
        }
        "serverRequest/resolved" => {
            let Some(rid) = p.get("requestId").and_then(RpcId::from_value) else { return missing("requestId") };
            let request = RequestKey { backend: BackendKind::Codex, source: ctx.source.clone(), request_id: rid.encode() };
            vec![BackendEvent::RequestResolved { request, evidence: live(None) }]
        }
        "thread/name/updated" => {
            let Some(id) = s(p, "threadId") else { return missing("threadId") };
            match s(p, "threadName") {
                Some(name) => vec![BackendEvent::ChatMetaChanged { chat: chat_key(id), change: ChatMetaChange::Renamed { name: name.to_string() } }],
                // 名前の削除は ChatMetaChange で表せない。捨てずに警告へ。
                None => vec![unrecognized(method, "thread name cleared")],
            }
        }
        "thread/archived" | "thread/unarchived" | "thread/deleted" => {
            let Some(id) = s(p, "threadId") else { return missing("threadId") };
            let change = match method {
                "thread/archived" => ChatMetaChange::Archived,
                "thread/unarchived" => ChatMetaChange::Unarchived,
                _ => ChatMetaChange::Deleted,
            };
            vec![BackendEvent::ChatMetaChanged { chat: chat_key(id), change }]
        }
        "thread/settings/updated" => {
            // 会話に設定されたモデル・推論の強さ（turn/startで指定した値の受理を確認できる根拠）。
            let (Some(tid), Some(settings)) = (s(p, "threadId"), p.get("threadSettings")) else { return missing("threadId/threadSettings") };
            let Some(model) = s(settings, "model") else { return missing("threadSettings.model") };
            let agent = agent_key(tid);
            let mut out = vec![BackendEvent::ModelAccepted {
                agent: agent.clone(),
                // 速度は設定に示された値（null＝指定なし）。示されていない項目は選択値と食い違って見えるだけで、受理済みと偽らない。
                choice: ModelChoice { model: model.to_string(), effort: s(settings, "effort").map(str::to_string), speed_tier: accepted_speed(settings.get("serviceTier")).flatten() },
                speed_unspecified: accepted_speed(settings.get("serviceTier")).is_none(),
            }];
            // 計画／実行の設定（示されていて、知っている値のときだけ受理値にする）。
            if let Some(mode) = settings.get("collaborationMode").and_then(|c| s(c, "mode")).and_then(super::parity::work_mode_from_wire) {
                out.push(BackendEvent::WorkModeAccepted { agent, mode });
            }
            out
        }
        "thread/goal/updated" => {
            let (Some(tid), Some(g)) = (s(p, "threadId"), p.get("goal")) else { return missing("threadId/goal") };
            match super::parity::goal_from_wire(g) {
                Some(goal) => vec![BackendEvent::GoalUpdated { chat: chat_key(tid), goal: Some(goal) }],
                None => return missing("goal.objective/status"),
            }
        }
        "thread/goal/cleared" => {
            let Some(tid) = s(p, "threadId") else { return missing("threadId") };
            vec![BackendEvent::GoalUpdated { chat: chat_key(tid), goal: None }]
        }
        "mcpServer/startupStatus/updated" => {
            let (Some(name), Some(status)) = (s(p, "name"), s(p, "status")) else { return missing("name/status") };
            vec![BackendEvent::ToolServerStatusChanged { name: name.to_string(), state: super::parity::tool_startup_state_from_wire(status) }]
        }
        "mcpServer/oauthLogin/completed" => {
            let (Some(name), Some(success)) = (s(p, "name"), p.get("success").and_then(Value::as_bool)) else { return missing("name/success") };
            vec![BackendEvent::ToolServerLoginCompleted { name: name.to_string(), success }]
        }
        "model/rerouted" => {
            let (Some(tid), Some(to)) = (s(p, "threadId"), s(p, "toModel")) else { return missing("threadId/toModel") };
            vec![BackendEvent::ModelRerouted {
                agent: agent_key(tid),
                turn: s(p, "turnId").map(ext),
                effective: ModelChoice { model: to.to_string(), effort: None, speed_tier: None },
            }]
        }
        "error" => {
            // 単一のエラー通知でfailedにしない（失敗はturn/completedの明示で判定）。警告として残す。
            let msg = p.get("error").and_then(|e| s(e, "message")).unwrap_or("(no message)");
            let retry = p.get("willRetry").and_then(Value::as_bool).unwrap_or(false);
            vec![unrecognized(method, format!("{msg} (willRetry={retry})"))]
        }
        _ => return None,
    })
}

// ───────────────────────────── サーバー要求 → PendingRequest ─────────────────────────────

fn opt(id: &str, label: &str, effect: DecisionEffect, scope: DecisionScope, description: Option<&str>) -> DecisionOption {
    DecisionOption { id: id.into(), label: label.into(), effect, scope, description: description.map(str::to_string) }
}

fn known_opt_str(v: Option<&str>) -> Known<String> {
    match v {
        Some(x) if !x.is_empty() => Known::direct(x.to_string()),
        _ => Known::Missing,
    }
}

fn base_detail(summary: &str) -> RequestDetail {
    RequestDetail {
        summary: summary.into(),
        reason: Known::Missing,
        command: Known::Unsupported,
        cwd: Known::Unsupported,
        files: Known::Unsupported,
        extra_permissions: Known::Unsupported,
        url: Known::Unsupported,
        questions: Vec::new(),
    }
}

fn command_options(params: &Value) -> Vec<DecisionOption> {
    let mut v = vec![
        opt("accept", "許可（今回のみ）", DecisionEffect::Allow, DecisionScope::Once, None),
        opt("acceptForSession", "許可（このセッション中）", DecisionEffect::Allow, DecisionScope::Session, None),
    ];
    if params.get("proposedExecpolicyAmendment").is_some_and(|x| !x.is_null()) {
        v.push(opt("execpolicy", "許可し、同種のコマンドを今後も許可", DecisionEffect::Allow, DecisionScope::Persistent, Some("実行ポリシーに規則を追加します")));
    }
    if let Some(a) = params.get("proposedNetworkPolicyAmendments").and_then(Value::as_array) {
        for (i, am) in a.iter().enumerate() {
            let host = s(am, "host").unwrap_or("?");
            let action = s(am, "action").unwrap_or("?");
            let effect = if action == "deny" { DecisionEffect::Deny } else { DecisionEffect::Allow };
            v.push(opt(&format!("network:{i}"), &format!("ネットワーク規則を追加（{host}: {action}）"), effect, DecisionScope::Persistent, None));
        }
    }
    v.push(opt("decline", "拒否", DecisionEffect::Deny, DecisionScope::Once, None));
    v.push(opt("cancel", "拒否して中止", DecisionEffect::Cancel, DecisionScope::Once, Some("このturnも止めます")));
    v
}

fn file_change_options() -> Vec<DecisionOption> {
    vec![
        opt("accept", "許可（今回のみ）", DecisionEffect::Allow, DecisionScope::Once, None),
        opt("acceptForSession", "許可（このセッション中）", DecisionEffect::Allow, DecisionScope::Session, None),
        opt("decline", "拒否", DecisionEffect::Deny, DecisionScope::Once, None),
        opt("cancel", "拒否して中止", DecisionEffect::Cancel, DecisionScope::Once, Some("このturnも止めます")),
    ]
}

fn permissions_options() -> Vec<DecisionOption> {
    vec![
        opt("grantTurn", "許可（このturnのみ）", DecisionEffect::Allow, DecisionScope::Once, None),
        opt("grantSession", "許可（このセッション中）", DecisionEffect::Allow, DecisionScope::Session, None),
        opt("deny", "拒否", DecisionEffect::Deny, DecisionScope::Once, Some("追加権限を付与しません")),
    ]
}

fn elicitation_options(params: &Value) -> Vec<DecisionOption> {
    let mut v = Vec::new();
    // formモードは入力内容の組立てが未対応のため、acceptを提示しない（空の内容で承諾しない）。
    if s(params, "mode") == Some("url") {
        v.push(opt("accept", "承諾", DecisionEffect::Allow, DecisionScope::Once, None));
    }
    v.push(opt("decline", "辞退", DecisionEffect::Deny, DecisionScope::Once, None));
    v.push(opt("cancel", "キャンセル", DecisionEffect::Cancel, DecisionScope::Once, None));
    v
}

fn questions(params: &Value) -> Vec<Question> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|q| {
                    let id = s(q, "id")?.to_string();
                    let options: Vec<DecisionOption> = q
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|o| {
                            o.iter()
                                .filter_map(|x| {
                                    let label = s(x, "label")?;
                                    Some(opt(label, label, DecisionEffect::Other, DecisionScope::Once, s(x, "description").filter(|d| !d.is_empty())))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let is_other = q.get("isOther").and_then(Value::as_bool).unwrap_or(false);
                    Some(Question {
                        id,
                        header: s(q, "header").filter(|h| !h.is_empty()).map(str::to_string),
                        text: s(q, "question").unwrap_or("").to_string(),
                        allows_free_text: options.is_empty() || is_other,
                        options,
                        secret: q.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// サーバー要求を共通イベントへ。回答できない要求は `Unrecognized`（自動回答しない）。
pub fn server_request_to_event(key: &RequestKey, method: &str, p: &Value, ctx: &EventCtx) -> RequestConversion {
    let not_answerable = |note: &str| RequestConversion { event: unrecognized(method, note), stored: None };
    let (kind, options, detail) = match method {
        "item/commandExecution/requestApproval" => {
            let mut d = base_detail("コマンド実行の承認");
            d.reason = known_opt_str(s(p, "reason"));
            d.command = known_opt_str(s(p, "command"));
            d.cwd = known_opt_str(s(p, "cwd"));
            if let Some(h) = p.get("networkApprovalContext").and_then(|n| s(n, "host")) {
                d.extra_permissions = Known::direct(format!("ネットワーク接続: {h}"));
            }
            (RequestKind::CommandApproval, command_options(p), d)
        }
        "item/fileChange/requestApproval" => {
            let mut d = base_detail("ファイル変更の承認");
            d.reason = known_opt_str(s(p, "reason"));
            // 対象ファイルはitem側にある。このparamsだけでは分からない。
            d.files = Known::NotFetched;
            if let Some(root) = s(p, "grantRoot") {
                d.extra_permissions = Known::direct(format!("書き込み許可の範囲: {root}"));
            }
            (RequestKind::FileChangeApproval, file_change_options(), d)
        }
        "item/permissions/requestApproval" => {
            let mut d = base_detail("追加権限の承認");
            d.reason = known_opt_str(s(p, "reason"));
            d.cwd = known_opt_str(s(p, "cwd"));
            d.extra_permissions = match p.get("permissions") {
                Some(x) if !x.is_null() => Known::direct(x.to_string()),
                _ => Known::Missing,
            };
            (RequestKind::PermissionsApproval, permissions_options(), d)
        }
        "item/tool/requestUserInput" => {
            let mut d = base_detail("質問");
            d.questions = questions(p);
            (RequestKind::UserInput, Vec::new(), d)
        }
        "mcpServer/elicitation/request" => {
            let server = s(p, "serverName").unwrap_or("?");
            let mut d = base_detail(s(p, "message").unwrap_or("MCPサーバーからの入力要求"));
            d.reason = Known::direct(format!("MCPサーバー: {server}"));
            if s(p, "mode") == Some("url") {
                d.url = known_opt_str(s(p, "url"));
            } else {
                d.reason = Known::direct(format!("MCPサーバー: {server}（入力フォームは未対応。辞退またはキャンセルのみ可能）"));
            }
            (RequestKind::ToolElicitation, elicitation_options(p), d)
        }
        // 未対応の要求: 自動応答せず表示し、拒否だけ選べる（ユーザー操作時のみ [`build_reply`] がエラー応答を作る）。
        _ => {
            let d = base_detail(&format!("未対応のサーバー要求: {method}"));
            let reject = opt(REJECT_OPTION_ID, "拒否（エラー応答）", DecisionEffect::Deny, DecisionScope::Once, Some("AgentDockが未対応の要求です"));
            (RequestKind::Other { raw: method.to_string() }, vec![reject], d)
        }
    };
    let Some(thread_id) = s(p, "threadId") else {
        return not_answerable("server request without threadId; left unanswered");
    };
    let agent = agent_key(thread_id);
    let request = PendingRequest {
        key: key.clone(),
        chat: (ctx.chat_of)(&agent),
        agent,
        turn: s(p, "turnId").map(ext),
        item: s(p, "itemId").map(ext),
        kind,
        detail,
        options,
        state: RequestState::Pending,
        received_at: ctx.now,
    };
    RequestConversion { event: BackendEvent::RequestOpened { request }, stored: Some(StoredRequest { method: method.to_string(), params: p.clone() }) }
}

// ───────────────────────────── 回答 → 応答JSON ─────────────────────────────

fn protocol(msg: impl Into<String>) -> BackendError {
    BackendError::Protocol { message: msg.into() }
}

/// 未対応の要求に提示する唯一の選択肢ID。
pub const REJECT_OPTION_ID: &str = "reject";

/// サーバー要求への返信。
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Result(Value),
    Error { code: i64, message: String },
}

fn is_handled_request(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
    )
}

/// 回答から返信を作る。未対応の要求は「拒否」のときだけエラー応答（JSON-RPC -32601）。
pub fn build_reply(stored: &StoredRequest, answer: &RequestAnswer) -> Result<Reply, BackendError> {
    if is_handled_request(&stored.method) {
        return build_response(stored, answer).map(Reply::Result);
    }
    match answer {
        RequestAnswer::Decision { option_id } if option_id == REJECT_OPTION_ID => {
            Ok(Reply::Error { code: -32601, message: "request not supported by AgentDock; rejected by user".into() })
        }
        _ => Err(protocol("only 'reject' is available for an unsupported server request")),
    }
}

/// 回答のJSONを作る。選択肢に無いIDや未回答は `Protocol` で拒否する（何も送らない）。
pub fn build_response(stored: &StoredRequest, answer: &RequestAnswer) -> Result<Value, BackendError> {
    let p = &stored.params;
    match (stored.method.as_str(), answer) {
        ("item/commandExecution/requestApproval", RequestAnswer::Decision { option_id }) => {
            if !command_options(p).iter().any(|o| &o.id == option_id) {
                return Err(protocol(format!("option {option_id} was not offered")));
            }
            let decision = match option_id.as_str() {
                "execpolicy" => json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": p["proposedExecpolicyAmendment"].clone()}}),
                id if id.starts_with("network:") => {
                    let i: usize = id["network:".len()..].parse().map_err(|_| protocol("bad network option"))?;
                    let am = p
                        .get("proposedNetworkPolicyAmendments")
                        .and_then(|a| a.get(i))
                        .ok_or_else(|| protocol("network amendment not found"))?;
                    json!({"applyNetworkPolicyAmendment": {"network_policy_amendment": am.clone()}})
                }
                other => Value::String(other.to_string()),
            };
            Ok(json!({ "decision": decision }))
        }
        ("item/fileChange/requestApproval", RequestAnswer::Decision { option_id }) => {
            if !file_change_options().iter().any(|o| &o.id == option_id) {
                return Err(protocol(format!("option {option_id} was not offered")));
            }
            Ok(json!({ "decision": option_id }))
        }
        ("item/permissions/requestApproval", RequestAnswer::Decision { option_id }) => match option_id.as_str() {
            "grantTurn" | "grantSession" => {
                // 要求された権限をそのまま付与（nullの項目は含めない）。
                let mut granted = serde_json::Map::new();
                if let Some(obj) = p.get("permissions").and_then(Value::as_object) {
                    for (k, v) in obj {
                        if !v.is_null() {
                            granted.insert(k.clone(), v.clone());
                        }
                    }
                }
                let scope = if option_id == "grantSession" { "session" } else { "turn" };
                Ok(json!({"permissions": Value::Object(granted), "scope": scope}))
            }
            "deny" => Ok(json!({"permissions": {}, "scope": "turn"})),
            other => Err(protocol(format!("option {other} was not offered"))),
        },
        ("mcpServer/elicitation/request", RequestAnswer::Decision { option_id }) => {
            if !elicitation_options(p).iter().any(|o| &o.id == option_id) {
                return Err(protocol(format!("option {option_id} was not offered")));
            }
            Ok(json!({"action": option_id, "content": Value::Null, "_meta": Value::Null}))
        }
        ("item/tool/requestUserInput", RequestAnswer::Answers { answers }) => {
            let qs = questions(p);
            let mut out = serde_json::Map::new();
            for a in answers {
                let q = qs.iter().find(|q| q.id == a.question_id).ok_or_else(|| protocol(format!("unknown question {}", a.question_id)))?;
                let text = match (&a.text, &a.option_id) {
                    (Some(t), _) => t.clone(),
                    (None, Some(o)) => {
                        if !q.options.iter().any(|x| &x.id == o) {
                            return Err(protocol(format!("option {o} not in question {}", q.id)));
                        }
                        o.clone()
                    }
                    (None, None) => return Err(protocol(format!("empty answer for {}", q.id))),
                };
                out.insert(q.id.clone(), json!({ "answers": [text] }));
            }
            if out.is_empty() {
                return Err(protocol("no answers"));
            }
            Ok(json!({ "answers": Value::Object(out) }))
        }
        (m, _) => Err(protocol(format!("answer kind does not match request {m}"))),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn accepted_speed_distinguishes_absent_standard_and_named() {
        use super::accepted_speed;
        use serde_json::json;
        assert_eq!(accepted_speed(None), None);
        assert_eq!(accepted_speed(Some(&json!(null))), Some(None));
        assert_eq!(accepted_speed(Some(&json!("default"))), Some(None));
        assert_eq!(accepted_speed(Some(&json!("priority"))), Some(Some("priority".to_string())));
        assert_eq!(accepted_speed(Some(&json!(3))), None);
    }

    use super::*;

    fn run(method: &str, params: Value) -> Vec<BackendEvent> {
        let src = SourceId("s".into());
        let chat_of = |a: &AgentKey| ChatKey { backend: BackendKind::Codex, id: a.id.clone() };
        let ctx = EventCtx { source: &src, now: UnixMillis(10), chat_of: &chat_of };
        notification_to_events(method, &params, &ctx)
    }

    fn req(method: &str, params: Value) -> RequestConversion {
        let src = SourceId("s".into());
        let chat_of = |a: &AgentKey| ChatKey { backend: BackendKind::Codex, id: ExternalId(format!("chat-of-{}", a.id.0)) };
        let ctx = EventCtx { source: &src, now: UnixMillis(10), chat_of: &chat_of };
        let key = RequestKey { backend: BackendKind::Codex, source: src.clone(), request_id: ExternalId("n:1".into()) };
        server_request_to_event(&key, method, &params, &ctx)
    }

    #[test]
    fn unknown_method_is_kept_as_unrecognized() {
        let ev = run("future/thing", json!({"a": 1}));
        assert!(matches!(&ev[0], BackendEvent::Unrecognized { raw_label, .. } if raw_label == "future/thing"));
    }

    #[test]
    fn turn_diff_and_file_changes_are_known_unmapped_and_only_the_activity_remains() {
        // turn集約diffは意図的に写像しない既知の通知（空）。形が崩れていても警告にしない。
        assert!(run("turn/diff/updated", json!({"threadId": "a", "turnId": "t1", "diff": "diff --git a/x b/x"})).is_empty());
        assert!(run("turn/diff/updated", json!({"threadId": "a"})).is_empty());
        // 完了した fileChange item は、チャット内の活動表示と成果物の候補だけになる（変更の観測イベントは出さない）。
        let item = json!({"type": "fileChange", "id": "i1", "status": "completed", "changes": [{"path": "x.rs", "kind": {"type": "add"}, "diff": "fn x() {}"}]});
        let ev = run("item/completed", json!({"threadId": "a", "turnId": "t1", "item": item}));
        assert!(ev.iter().any(|e| matches!(e, BackendEvent::Activity { activity } if activity.key.item_id.0 == "i1")));
        assert!(ev.iter().any(|e| matches!(e, BackendEvent::ArtifactObserved { .. })));
        assert!(ev.iter().all(|e| matches!(e, BackendEvent::Activity { .. } | BackendEvent::ArtifactObserved { .. })));
    }

    #[test]
    fn known_noise_is_not_surfaced_but_warnings_are() {
        assert!(run("thread/tokenUsage/updated", json!({})).is_empty());
        assert!(run("account/rateLimits/updated", json!({})).is_empty());
        let ev = run("configWarning", json!({"message": "bad config"}));
        assert!(matches!(&ev[0], BackendEvent::Unrecognized { note: Some(n), .. } if n == "bad config"));
    }

    #[test]
    fn status_changed_notification_is_live_and_not_done() {
        let ev = run("thread/status/changed", json!({"threadId": "a", "status": {"type": "active", "activeFlags": ["waitingOnApproval"]}}));
        match &ev[0] {
            BackendEvent::AgentStatus { agent, status } => {
                assert_eq!(agent.id.0, "a");
                assert_eq!(status.state, AgentState::Waiting);
                assert_eq!(status.evidence.source, EvidenceSource::LiveEvent);
                assert_eq!(freshness_for(status.evidence.source), Freshness::Live);
            }
            e => panic!("{e:?}"),
        }
        let ev = run("thread/status/changed", json!({"threadId": "a", "status": {"type": "notLoaded"}}));
        assert!(matches!(&ev[0], BackendEvent::AgentStatus { status, .. } if status.state == AgentState::Unknown));
    }

    #[test]
    fn turn_completed_makes_terminal_event_and_turn_scoped_status() {
        let ev = run("turn/completed", json!({"threadId": "a", "turn": {"id": "t1", "status": "interrupted", "items": [], "completedAt": 7}}));
        assert!(matches!(&ev[0], BackendEvent::TurnEnded { end: TurnEnd::Interrupted, evidence, .. } if evidence.source_time == Some(UnixMillis(7000))));
        assert!(matches!(&ev[1], BackendEvent::AgentStatus { status, .. } if status.state == AgentState::Interrupted && status.turn == Some(ExternalId("t1".into()))));
        // 終端でないstatusのcompletedは終端を作らない。
        let ev = run("turn/completed", json!({"threadId": "a", "turn": {"id": "t1", "status": "inProgress", "items": []}}));
        assert!(matches!(&ev[0], BackendEvent::Unrecognized { .. }));
        assert_eq!(ev.len(), 1);
        let ev = run("turn/started", json!({"threadId": "a", "turn": {"id": "t2", "status": "inProgress", "items": []}}));
        assert!(matches!(&ev[0], BackendEvent::TurnStarted { .. }));
        assert!(matches!(&ev[1], BackendEvent::AgentStatus { status, .. } if status.state == AgentState::Running));
    }

    #[test]
    fn thread_started_for_child_discovers_with_explicit_parent() {
        let ev = run("thread/started", json!({"thread": {"id": "c", "parentThreadId": "p", "agentRole": "worker", "status": {"type": "idle"}, "turns": []}}));
        match &ev[0] {
            BackendEvent::AgentDiscovered { agent } => {
                assert_eq!(agent.parent, ParentLink::Explicit { parent: agent_key("p") });
                assert_eq!(agent.chat.id.0, "p");
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(&ev[1], BackendEvent::AgentStatus { .. }));
    }

    #[test]
    fn item_notifications_and_delta() {
        let ev = run("item/started", json!({"threadId": "a", "turnId": "t", "startedAtMs": 99, "item": {"type": "commandExecution", "id": "i", "command": "ls", "status": "inProgress"}}));
        match &ev[0] {
            BackendEvent::Activity { activity } => {
                assert_eq!(activity.phase, ActivityPhase::InProgress);
                assert_eq!(activity.evidence.source_time, Some(UnixMillis(99)));
            }
            e => panic!("{e:?}"),
        }
        let ev = run("item/completed", json!({"threadId": "a", "turnId": "t", "completedAtMs": 1, "item": {"type": "agentMessage", "id": "m", "text": "hi"}}));
        assert!(matches!(&ev[0], BackendEvent::Activity { activity } if activity.phase == ActivityPhase::Completed));
        let ev = run("item/agentMessage/delta", json!({"threadId": "a", "turnId": "t", "itemId": "m", "delta": "x"}));
        assert!(matches!(&ev[0], BackendEvent::ActivityDelta { delta, .. } if delta == "x"));
        let ev = run("item/started", json!({"threadId": "a"}));
        assert!(matches!(&ev[0], BackendEvent::Unrecognized { .. }));
    }

    #[test]
    fn resolved_notification_keeps_id_type() {
        let ev = run("serverRequest/resolved", json!({"threadId": "a", "requestId": 5}));
        assert!(matches!(&ev[0], BackendEvent::RequestResolved { request, .. } if request.request_id.0 == "n:5" && request.source.0 == "s"));
        let ev = run("serverRequest/resolved", json!({"threadId": "a", "requestId": "5"}));
        assert!(matches!(&ev[0], BackendEvent::RequestResolved { request, .. } if request.request_id.0 == "s:5"));
    }

    #[test]
    fn single_error_notification_does_not_fail_the_agent() {
        let ev = run("error", json!({"error": {"message": "boom"}, "willRetry": true, "threadId": "a", "turnId": "t"}));
        assert!(matches!(&ev[0], BackendEvent::Unrecognized { .. }));
        assert_eq!(ev.len(), 1);
    }

    #[test]
    fn command_approval_options_and_responses() {
        let params = json!({"threadId": "a", "turnId": "t", "itemId": "i", "command": "rm x", "cwd": "C:/w", "reason": "r",
            "proposedExecpolicyAmendment": ["rm", "x"], "proposedNetworkPolicyAmendments": [{"host": "h", "action": "allow"}]});
        let c = req("item/commandExecution/requestApproval", params);
        let BackendEvent::RequestOpened { request } = &c.event else { panic!() };
        assert_eq!(request.chat.id.0, "chat-of-a");
        assert_eq!(request.detail.command.value().map(String::as_str), Some("rm x"));
        let ids: Vec<_> = request.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["accept", "acceptForSession", "execpolicy", "network:0", "decline", "cancel"]);
        let st = c.stored.unwrap();
        assert_eq!(build_response(&st, &RequestAnswer::Decision { option_id: "acceptForSession".into() }).unwrap(), json!({"decision": "acceptForSession"}));
        assert_eq!(
            build_response(&st, &RequestAnswer::Decision { option_id: "execpolicy".into() }).unwrap(),
            json!({"decision": {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["rm", "x"]}}})
        );
        assert_eq!(
            build_response(&st, &RequestAnswer::Decision { option_id: "network:0".into() }).unwrap(),
            json!({"decision": {"applyNetworkPolicyAmendment": {"network_policy_amendment": {"host": "h", "action": "allow"}}}})
        );
        // 提示していない選択肢・種別違いは送らない。
        assert!(build_response(&st, &RequestAnswer::Decision { option_id: "network:9".into() }).is_err());
        assert!(build_response(&st, &RequestAnswer::Decision { option_id: "bogus".into() }).is_err());
        assert!(build_response(&st, &RequestAnswer::Answers { answers: vec![] }).is_err());
    }

    #[test]
    fn command_approval_without_amendments_hides_persistent_options() {
        let c = req("item/commandExecution/requestApproval", json!({"threadId": "a", "turnId": "t", "itemId": "i"}));
        let BackendEvent::RequestOpened { request } = &c.event else { panic!() };
        assert!(request.options.iter().all(|o| o.scope != DecisionScope::Persistent));
        assert_eq!(request.detail.command, Known::Missing);
    }

    #[test]
    fn permissions_response_drops_null_fields() {
        let c = req("item/permissions/requestApproval", json!({"threadId": "a", "turnId": "t", "itemId": "i", "cwd": "C:/w", "reason": null,
            "permissions": {"network": {"enabled": true}, "fileSystem": null}}));
        let st = c.stored.unwrap();
        assert_eq!(
            build_response(&st, &RequestAnswer::Decision { option_id: "grantSession".into() }).unwrap(),
            json!({"permissions": {"network": {"enabled": true}}, "scope": "session"})
        );
        assert_eq!(build_response(&st, &RequestAnswer::Decision { option_id: "deny".into() }).unwrap(), json!({"permissions": {}, "scope": "turn"}));
    }

    #[test]
    fn user_input_questions_and_answers() {
        let c = req("item/tool/requestUserInput", json!({"threadId": "a", "turnId": "t", "itemId": "i", "isBlocking": true, "autoResolutionMs": null,
            "questions": [{"id": "q1", "header": "H", "question": "Pick", "isOther": false, "isSecret": true,
                           "options": [{"label": "A", "description": "da"}, {"label": "B", "description": ""}]},
                          {"id": "q2", "header": "", "question": "Free", "isOther": false, "isSecret": false, "options": null}]}));
        let BackendEvent::RequestOpened { request } = &c.event else { panic!() };
        let qs = &request.detail.questions;
        assert!(qs[0].secret && !qs[0].allows_free_text && qs[0].options.len() == 2);
        assert!(qs[1].allows_free_text && qs[1].header.is_none());
        let st = c.stored.unwrap();
        let ans = RequestAnswer::Answers {
            answers: vec![
                QuestionAnswer { question_id: "q1".into(), option_id: Some("B".into()), text: None },
                QuestionAnswer { question_id: "q2".into(), option_id: None, text: Some("hello".into()) },
            ],
        };
        assert_eq!(build_response(&st, &ans).unwrap(), json!({"answers": {"q1": {"answers": ["B"]}, "q2": {"answers": ["hello"]}}}));
        let bad = RequestAnswer::Answers { answers: vec![QuestionAnswer { question_id: "q1".into(), option_id: Some("Z".into()), text: None }] };
        assert!(build_response(&st, &bad).is_err());
        let empty = RequestAnswer::Answers { answers: vec![QuestionAnswer { question_id: "q2".into(), option_id: None, text: None }] };
        assert!(build_response(&st, &empty).is_err());
    }

    #[test]
    fn elicitation_form_offers_no_accept() {
        let c = req("mcpServer/elicitation/request", json!({"threadId": "a", "turnId": null, "serverName": "srv", "mode": "form", "message": "need", "requestedSchema": {}}));
        let BackendEvent::RequestOpened { request } = &c.event else { panic!() };
        assert!(request.options.iter().all(|o| o.id != "accept"));
        assert!(request.turn.is_none());
        let st = c.stored.unwrap();
        assert!(build_response(&st, &RequestAnswer::Decision { option_id: "accept".into() }).is_err());
        assert_eq!(build_response(&st, &RequestAnswer::Decision { option_id: "decline".into() }).unwrap()["action"], "decline");
        let c = req("mcpServer/elicitation/request", json!({"threadId": "a", "serverName": "srv", "mode": "url", "message": "m", "url": "http://x", "elicitationId": "e"}));
        let BackendEvent::RequestOpened { request } = &c.event else { panic!() };
        assert!(request.options.iter().any(|o| o.id == "accept"));
        assert_eq!(request.detail.url.value().map(String::as_str), Some("http://x"));
    }

    #[test]
    fn unhandled_server_request_is_shown_as_other_with_reject_only_and_never_auto_answered() {
        let c = req("item/tool/call", json!({"threadId": "a", "turnId": "t", "callId": "c", "tool": "x"}));
        let BackendEvent::RequestOpened { request } = &c.event else { panic!("{:?}", c.event) };
        assert!(matches!(&request.kind, RequestKind::Other { raw } if raw == "item/tool/call"));
        assert_eq!(request.options.len(), 1);
        assert_eq!(request.options[0].id, REJECT_OPTION_ID);
        assert_eq!(request.state, RequestState::Pending);
        let st = c.stored.unwrap();
        assert!(matches!(build_reply(&st, &RequestAnswer::Decision { option_id: REJECT_OPTION_ID.into() }), Ok(Reply::Error { code: -32601, .. })));
        assert!(build_reply(&st, &RequestAnswer::Decision { option_id: "accept".into() }).is_err());
        assert!(build_reply(&st, &RequestAnswer::Answers { answers: vec![] }).is_err());
        // 対応済みの要求は従来どおり結果応答。
        let c = req("item/fileChange/requestApproval", json!({"threadId": "a", "turnId": "t", "itemId": "i"}));
        let r = build_reply(&c.stored.unwrap(), &RequestAnswer::Decision { option_id: "accept".into() }).unwrap();
        assert_eq!(r, Reply::Result(json!({"decision": "accept"})));
    }

    #[test]
    fn unhandled_server_request_without_thread_is_unrecognized_and_not_stored() {
        let c = req("account/chatgptAuthTokens/refresh", json!({}));
        assert!(matches!(c.event, BackendEvent::Unrecognized { .. }));
        assert!(c.stored.is_none());
        let c = req("item/fileChange/requestApproval", json!({"turnId": "t"}));
        assert!(matches!(c.event, BackendEvent::Unrecognized { .. }));
    }

    #[test]
    fn thread_closed_keeps_state_and_only_drops_freshness() {
        let ev = run("thread/closed", json!({"threadId": "a"}));
        assert_eq!(ev.len(), 1);
        assert!(matches!(&ev[0], BackendEvent::Gap { scope: GapScope::Agent { agent }, .. } if agent.id.0 == "a"));
    }

    #[test]
    fn system_error_status_is_unknown_with_a_warning() {
        let ev = run("thread/status/changed", json!({"threadId": "a", "status": {"type": "systemError"}}));
        assert!(matches!(&ev[0], BackendEvent::AgentStatus { status, .. } if status.state == AgentState::Unknown && status.raw.label == "systemError"));
        assert!(matches!(&ev[1], BackendEvent::Unrecognized { .. }));
    }
}
