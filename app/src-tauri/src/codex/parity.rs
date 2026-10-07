//! `ParityOps` のCodex実装（段階③、`app/DESIGN_P3.md` §2.2・§3）。
//!
//! 操作ごとの節に分けてある。P3-1〜P3-6 は自分の節のメソッドだけを実装し、`DECLARED` の `implemented` を真にする。
//! Codex固有の変換（レビュー対象・協調モード・速度ID・MCP接続状態など）はこのファイルと `cli.rs` に閉じ込める。
//! 実装していない操作は、能力宣言も呼出しも `Unsupported`（成功を装わない）。

use super::adapter::{CodexBackend, READ_TIMEOUT, WRITE_TIMEOUT};
use super::convert::chat_key;
use super::parity_table;
use crate::backend::backend::*;
use crate::backend::model::*;
use crate::backend::parity::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// 状態表示の読取り1件あたりの待ち時間（取れなければその項目は未取得）。
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// 宣言の1行。`schema_support` はschema上の有無（experimentalなら `Experimental`）、`implemented` はAgentDock側の実装有無。
struct Declared {
    op: ParityOp,
    route: OpRoute,
    schema_support: Support,
    deprecated: bool,
    implemented: bool,
}

const fn d(op: ParityOp, route: OpRoute, schema_support: Support) -> Declared {
    Declared { op, route, schema_support, deprecated: false, implemented: false }
}

/// 実装済みの宣言（`implemented` が真）。
const fn done(op: ParityOp, route: OpRoute, schema_support: Support) -> Declared {
    Declared { op, route, schema_support, deprecated: false, implemented: true }
}

/// 0.160.0 のschemaと `DESIGN_P3.md` §1 に基づく宣言。実装したタスクが `implemented` を真にする。
const DECLARED: [Declared; 20] = [
    // P3-1: 変更の報告（turn集約diff・fileChange item）の観測と、AgentDock管理の差分表示・戻す操作。確認状況は未確認のまま。
    done(ParityOp::ChangeList, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::RevertChanges, OpRoute::AppManaged, Support::Supported),
    // P3-2: レビュー（inline。別チャットは thread/start＋inline）・分岐・圧縮。確認状況は未確認のまま。
    done(ParityOp::CodeReview, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::ReviewToNewChat, OpRoute::BackendApi, Support::Supported),
    // P3-3: 計画／実行（experimental）・Goal・状態表示・速度・memories（experimental）。確認状況は未確認のまま。
    done(ParityOp::WorkMode, OpRoute::BackendApi, Support::Experimental),
    done(ParityOp::Fork, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::Compact, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::ReferenceChat, OpRoute::AppManaged, Support::Supported),
    done(ParityOp::Goal, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::SideChat, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::Skills, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::InstructionFiles, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::ToolServers, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::Extensions, OpRoute::CliHelper, Support::Supported),
    d(ParityOp::CloudDelegation, OpRoute::CliHelper, Support::Experimental),
    d(ParityOp::Worktree, OpRoute::AppManaged, Support::Supported),
    done(ParityOp::BackendStatus, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::SpeedTier, OpRoute::BackendApi, Support::Supported),
    // schema注記: 常に効果なし。操作は置かない。
    Declared { deprecated: true, ..d(ParityOp::Personality, OpRoute::BackendApi, Support::Unsupported) },
    done(ParityOp::Memory, OpRoute::BackendApi, Support::Experimental),
];

/// 操作ごとの能力。`version` は接続先の実際の版（不明なら None＝確認状況はすべて未確認）。
/// `experimental` は experimental API を有効にして接続したか。実装済みでも experimental 前提の操作は、無効なら `Experimental`（使えない）。
/// 未実装の操作は `Unsupported`（理由を note に書く）。
pub fn op_capabilities(version: Option<&str>, experimental: bool) -> Vec<OpCapability> {
    DECLARED
        .iter()
        .map(|x| {
            let (support, note) = if x.deprecated {
                (Support::Unsupported, Some("非推奨（このCodex版では選べない）".to_string()))
            } else if !x.implemented {
                (Support::Unsupported, Some("AgentDockでは未実装".to_string()))
            } else if x.schema_support == Support::Experimental {
                if experimental {
                    (Support::Supported, None)
                } else {
                    (Support::Experimental, Some("experimental API が無効のため使えません".to_string()))
                }
            } else {
                (x.schema_support, None)
            };
            OpCapability { op: x.op, support, route: x.route, verification: parity_table::verification(version, x.op), deprecated: x.deprecated, note }
        })
        .collect()
}

#[async_trait]
impl ParityOps for CodexBackend {
    // ── 分岐・side相談（P3-2・P3-4） ──
    /// 履歴は取り寄せない（`excludeTurns`）。応答の新しい会話のIDだけを返し、内容の反映は呼び出し側が読取りで行う。
    /// 応答はあったが新しい会話を特定できないときは、受理不明として返す（再送しない）。
    async fn fork_chat(&self, chat: ChatKey, params: ForkParams, _confirmed: &UserConfirmed) -> BackendResult<ForkOutcome> {
        match self.call("thread/fork", fork_params_json(&chat.id.0, &params), WRITE_TIMEOUT).await {
            Ok(r) => Ok(match forked_thread_id(&r) {
                Some(id) => ForkOutcome { ack: OpAck::Accepted, chat: Some(chat_key(id)) },
                None => ForkOutcome { ack: OpAck::Unknown { message: "分岐の応答から、新しい会話を特定できませんでした".into() }, chat: None },
            }),
            Err(BackendError::Rejected { message, .. }) => Ok(ForkOutcome { ack: OpAck::Rejected { message }, chat: None }),
            Err(BackendError::OutcomeUnknown { message }) => Ok(ForkOutcome { ack: OpAck::Unknown { message }, chat: None }),
            Err(e) => Err(e),
        }
    }

    // ── レビュー・圧縮（P3-2） ──
    /// 常に現在の会話で実行する（`delivery: inline`）。`detached` は非推奨なので使わない（別チャットは呼び出し側が thread/start してから呼ぶ）。
    async fn start_review(&self, chat: ChatKey, target: ReviewTarget, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        ack_of(self.call("review/start", review_params_json(&chat.id.0, &target), WRITE_TIMEOUT).await)
    }
    /// 応答は空（受付のみ）。圧縮の完了は、圧縮の記録（item）を観測したときだけ表示する。
    async fn compact(&self, chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        ack_of(self.call("thread/compact/start", json!({"threadId": chat.id.0}), WRITE_TIMEOUT).await)
    }

    // ── Goal（P3-3） ──
    /// 読取り。表示のためにresumeしない（ロードされていないスレッドは Codex が拒否することがあり、その場合は呼び出し側が「未取得」にする）。
    /// 目標が設定されていないことは `Known::Missing`（取得したが値がない）で表す。
    async fn get_goal(&self, chat: ChatKey) -> BackendResult<Known<Goal>> {
        let r = self.call("thread/goal/get", json!({"threadId": chat.id.0}), READ_TIMEOUT).await?;
        match r.get("goal") {
            None | Some(Value::Null) => Ok(Known::Missing),
            Some(g) => goal_from_wire(g).map(Known::direct).ok_or_else(|| BackendError::Protocol { message: "thread/goal/get: unexpected goal shape".into() }),
        }
    }
    async fn set_goal(&self, chat: ChatKey, update: GoalUpdate, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        let params = goal_set_params(&chat.id.0, &update)?;
        ack_of(self.call("thread/goal/set", params, WRITE_TIMEOUT).await)
    }
    async fn clear_goal(&self, chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        ack_of(self.call("thread/goal/clear", json!({"threadId": chat.id.0}), WRITE_TIMEOUT).await)
    }

    // ── 計画／実行の切替（P3-3） ──
    async fn list_work_modes(&self) -> BackendResult<Vec<WorkModeInfo>> {
        let r = self.call_experimental(ParityOp::WorkMode, "collaborationMode/list", json!({}), READ_TIMEOUT).await?;
        Ok(work_modes_of(&r))
    }

    // ── Skills・指示ファイル（P3-4） ──
    async fn list_skills(&self, _cwd: String, _force_reload: bool) -> BackendResult<Vec<SkillInfo>> {
        unsupported(ParityOp::Skills)
    }
    async fn instruction_sources(&self, _chat: ChatKey) -> BackendResult<Known<Vec<String>>> {
        unsupported(ParityOp::InstructionFiles)
    }

    // ── ツールサーバー・拡張（P3-5） ──
    async fn list_tool_servers(&self) -> BackendResult<Vec<ToolServerView>> {
        unsupported(ParityOp::ToolServers)
    }
    async fn login_tool_server(&self, _name: String, _confirmed: &UserConfirmed) -> BackendResult<String> {
        unsupported(ParityOp::ToolServers)
    }
    async fn reload_tool_servers(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::ToolServers)
    }
    async fn list_extensions(&self) -> BackendResult<Vec<ExtensionView>> {
        unsupported(ParityOp::Extensions)
    }
    async fn manage_extension(&self, _op: ExtensionOp, _confirmed: &UserConfirmed) -> BackendResult<ExtensionOpResult> {
        unsupported(ParityOp::Extensions)
    }

    // ── クラウド委任（P3-6） ──
    async fn cloud_submit(&self, _request: CloudSubmit, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_list(&self) -> BackendResult<Vec<CloudTaskInfo>> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_diff(&self, _task_id: String) -> BackendResult<String> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_apply(&self, _task_id: String, _cwd: String, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CloudDelegation)
    }

    // ── 状態・速度・memories（P3-3） ──
    /// すべて読取り。取れない項目は未取得のまま（0・空文字で代用しない）。アカウントのメール・トークン類は読まない。
    async fn backend_status(&self) -> BackendResult<BackendStatus> {
        let experimental = self.experimental_enabled()?;
        let (executable, version) = self.launched_info();
        let (account_kind, plan) = match self.call("account/read", json!({}), STATUS_TIMEOUT).await {
            Ok(v) => account_of(&v),
            Err(_) => (Known::NotFetched, Known::NotFetched),
        };
        let rate_limits = match self.call("account/rateLimits/read", json!({}), STATUS_TIMEOUT).await {
            Ok(v) => rate_limits_of(&v),
            Err(_) => Known::NotFetched,
        };
        let usage = match self.call("account/usage/read", json!({}), STATUS_TIMEOUT).await {
            Ok(v) => usage_of(&v),
            Err(_) => Known::NotFetched,
        };
        Ok(BackendStatus {
            version: version.map(Known::direct).unwrap_or(Known::NotFetched),
            executable: executable.map(Known::direct).unwrap_or(Known::NotFetched),
            experimental_enabled: Known::direct(experimental),
            account_kind,
            plan,
            rate_limits,
            usage,
            warnings: self.config_warnings(),
        })
    }
    async fn memory_status(&self) -> BackendResult<MemoryStatus> {
        let r = self.call_experimental(ParityOp::Memory, "memory/status", json!({}), READ_TIMEOUT).await?;
        Ok(MemoryStatus { summary: memory_summary(&r) })
    }
    async fn set_memory_mode(&self, chat: ChatKey, mode: String, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        if !matches!(mode.as_str(), "enabled" | "disabled") {
            return Err(BackendError::Rejected { code: None, message: format!("未知のmemoriesの設定値です: {mode}") });
        }
        ack_of(self.call_experimental(ParityOp::Memory, "thread/memoryMode/set", json!({"threadId": chat.id.0, "mode": mode}), WRITE_TIMEOUT).await)
    }
    /// 記憶データを消す。呼ぶのは影響を表示して確認した後だけ（ホストが確認済みの印を要求する）。
    async fn reset_memory(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        ack_of(self.call_experimental(ParityOp::Memory, "memory/reset", json!({}), WRITE_TIMEOUT).await)
    }
}

impl CodexBackend {
    /// experimental API の呼出し。無効なら送らず非対応。拒否されたら能力を非対応へ降格して出し直す。
    async fn call_experimental(&self, op: ParityOp, method: &str, params: Value, timeout: Duration) -> BackendResult<Value> {
        if !self.experimental_enabled()? || !self.op_supported(op) {
            return unsupported(op);
        }
        let r = self.call(method, params, timeout).await;
        if let Err(e) = &r {
            if is_experimental_rejection(e) {
                self.degrade_op(op, "experimental API の利用を Codex が拒否しました");
            }
        }
        r
    }
}

// ───────────────────────────── 変換（純粋） ─────────────────────────────

/// `review/start.target`。
pub fn review_target_json(t: &ReviewTarget) -> Value {
    match t {
        ReviewTarget::UncommittedChanges => json!({"type": "uncommittedChanges"}),
        ReviewTarget::BaseBranch { branch } => json!({"type": "baseBranch", "branch": branch}),
        ReviewTarget::Commit { sha, title } => json!({"type": "commit", "sha": sha, "title": title}),
        ReviewTarget::Custom { instructions } => json!({"type": "custom", "instructions": instructions}),
    }
}

/// `review/start` の引数。`delivery` は常に inline（`detached` は非推奨）。
pub fn review_params_json(thread_id: &str, t: &ReviewTarget) -> Value {
    json!({"threadId": thread_id, "target": review_target_json(t), "delivery": "inline"})
}

/// `thread/fork` の引数。履歴の取り寄せはしない（`excludeTurns`）。読取り専用の指定は sandbox で表す。
pub fn fork_params_json(thread_id: &str, p: &ForkParams) -> Value {
    let mut v = json!({"threadId": thread_id, "excludeTurns": true, "ephemeral": p.ephemeral});
    if let Some(t) = &p.through_turn {
        v["lastTurnId"] = json!(t.0);
    }
    if p.read_only {
        v["sandbox"] = json!("read-only");
    }
    v
}

/// `thread/fork` の応答の、新しい会話のID。
pub fn forked_thread_id(r: &Value) -> Option<&str> {
    r.get("thread")?.get("id")?.as_str().filter(|s| !s.is_empty())
}

/// 状態を変える要求の結果を `OpAck` にする。明示拒否は `Rejected`、応答なしは `Unknown`（再送しない）。接続・非対応はエラーのまま。
pub fn ack_of(r: BackendResult<Value>) -> BackendResult<OpAck> {
    match r {
        Ok(_) => Ok(OpAck::Accepted),
        Err(BackendError::Rejected { message, .. }) => Ok(OpAck::Rejected { message }),
        Err(BackendError::OutcomeUnknown { message }) => Ok(OpAck::Unknown { message }),
        Err(e) => Err(e),
    }
}

/// experimental API を使えなかったことを示す拒否か（メッセージに experimental を含む）。
pub fn is_experimental_rejection(e: &BackendError) -> bool {
    matches!(e, BackendError::Rejected { message, .. } if message.to_ascii_lowercase().contains("experimental"))
}

pub fn work_mode_from_wire(s: &str) -> Option<WorkMode> {
    match s {
        "plan" => Some(WorkMode::Plan),
        "default" => Some(WorkMode::Default),
        _ => None,
    }
}

fn work_mode_to_wire(m: WorkMode) -> &'static str {
    match m {
        WorkMode::Plan => "plan",
        WorkMode::Default => "default",
    }
}

/// `turn/start.collaborationMode`。設定のモデルが必須なので、特定できなければ何も送らず拒否する（推測で補わない）。
/// 開発者向け指示は null（そのモードの組込み指示を使う）。
pub fn collaboration_mode_json(mode: WorkMode, model: Option<&ModelChoice>) -> BackendResult<Value> {
    let Some(m) = model else {
        return Err(BackendError::Rejected { code: None, message: "計画／実行を送るには、モデルを特定できる必要があります。モデルを選択してください".into() });
    };
    Ok(json!({
        "mode": work_mode_to_wire(mode),
        "settings": {"model": m.model, "reasoning_effort": m.effort, "developer_instructions": null},
    }))
}

/// `collaborationMode/list` → 計画／実行の選択肢（名前はバックエンドが示したまま。知らないモード・重複は除く）。
pub fn work_modes_of(r: &Value) -> Vec<WorkModeInfo> {
    let mut out: Vec<WorkModeInfo> = Vec::new();
    for m in r.get("data").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        let Some(mode) = m.get("mode").and_then(Value::as_str).and_then(work_mode_from_wire) else { continue };
        if out.iter().any(|x| x.mode == mode) {
            continue;
        }
        let label = m.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(work_mode_to_wire(mode)).to_string();
        out.push(WorkModeInfo { mode, label });
    }
    out
}

/// 目標の状態。知らない値は `Unknown`＋原文（捨てない・既知の値に丸めない）。
pub fn goal_status_from_wire(s: &str) -> GoalStatus {
    match s {
        "active" => GoalStatus::Active,
        "paused" => GoalStatus::Paused,
        "blocked" => GoalStatus::Blocked,
        "usageLimited" => GoalStatus::UsageLimited,
        "budgetLimited" => GoalStatus::BudgetLimited,
        "complete" => GoalStatus::Complete,
        other => GoalStatus::Unknown { raw: other.to_string() },
    }
}

/// 設定できる状態の外向きの値。`Unknown` は設定できない（None）。
pub fn goal_status_to_wire(s: &GoalStatus) -> Option<&'static str> {
    match s {
        GoalStatus::Active => Some("active"),
        GoalStatus::Paused => Some("paused"),
        GoalStatus::Blocked => Some("blocked"),
        GoalStatus::UsageLimited => Some("usageLimited"),
        GoalStatus::BudgetLimited => Some("budgetLimited"),
        GoalStatus::Complete => Some("complete"),
        GoalStatus::Unknown { .. } => None,
    }
}

/// 負数・数値でない・欠けた値は `Missing`（0に丸めない）。
fn known_u64(v: &Value, key: &str) -> Known<u64> {
    match v.get(key).and_then(Value::as_u64) {
        Some(n) => Known::direct(n),
        None => Known::Missing,
    }
}

/// 目標1件。目的と状態が読めなければ None（空の目標に見せない）。更新時刻は単位がschemaに示されていないので換算せず未取得とする。
pub fn goal_from_wire(g: &Value) -> Option<Goal> {
    let objective = g.get("objective").and_then(Value::as_str)?.to_string();
    let status = goal_status_from_wire(g.get("status").and_then(Value::as_str)?);
    Some(Goal {
        objective,
        status,
        token_budget: known_u64(g, "tokenBudget"),
        tokens_used: known_u64(g, "tokensUsed"),
        time_used_secs: known_u64(g, "timeUsedSeconds"),
        updated_at: Known::NotFetched,
    })
}

/// `thread/goal/set` の引数。何も変えない更新・設定できない状態は、送らずに拒否する。
pub fn goal_set_params(thread_id: &str, u: &GoalUpdate) -> BackendResult<Value> {
    let reject = |m: &str| BackendError::Rejected { code: None, message: m.into() };
    let mut p = json!({"threadId": thread_id});
    let mut any = false;
    if let Some(o) = &u.objective {
        if o.trim().is_empty() {
            return Err(reject("目標が空です"));
        }
        p["objective"] = json!(o);
        any = true;
    }
    if let Some(s) = &u.status {
        let Some(w) = goal_status_to_wire(s) else { return Err(reject("この状態は設定できません")) };
        p["status"] = json!(w);
        any = true;
    }
    if let Some(b) = u.token_budget {
        p["tokenBudget"] = json!(b);
        any = true;
    }
    if !any {
        return Err(reject("変更する項目がありません"));
    }
    Ok(p)
}

/// `account/read` → (種類, プラン)。アカウントがない・プランが示されないときは `Missing`。メール等は読まない。
pub fn account_of(v: &Value) -> (Known<String>, Known<String>) {
    let Some(a) = v.get("account").filter(|a| !a.is_null()) else { return (Known::Missing, Known::Missing) };
    let kind = a.get("type").and_then(Value::as_str).map(|s| Known::direct(s.to_string())).unwrap_or(Known::Missing);
    let plan = a.get("planType").and_then(Value::as_str).map(|s| Known::direct(s.to_string())).unwrap_or(Known::Missing);
    (kind, plan)
}

fn window_of(w: &Value, role: LimitWindowRole) -> Option<RateLimitWindowView> {
    let used = w.get("usedPercent").and_then(Value::as_f64)?;
    let minutes = match w.get("windowDurationMins").and_then(Value::as_u64) {
        Some(m) => Known::direct(m as u32),
        None => Known::Missing,
    };
    Some(RateLimitWindowView { role, used_percent: used, window_minutes: minutes })
}

fn limit_of(s: &Value, fallback_name: Option<&str>) -> RateLimitView {
    let name = s
        .get("limitName")
        .and_then(Value::as_str)
        .or_else(|| s.get("limitId").and_then(Value::as_str))
        .or(fallback_name)
        .map(|n| Known::direct(n.to_string()))
        .unwrap_or(Known::Missing);
    let mut windows = Vec::new();
    for (key, role) in [("primary", LimitWindowRole::Primary), ("secondary", LimitWindowRole::Secondary)] {
        if let Some(w) = s.get(key).filter(|w| !w.is_null()).and_then(|w| window_of(w, role)) {
            windows.push(w);
        }
    }
    RateLimitView { name, windows }
}

/// `account/rateLimits/read` → 利用上限の一覧。複数枠の表（`rateLimitsByLimitId`）があればそれ、なければ従来の1枠。使用率は換算しない。
pub fn rate_limits_of(v: &Value) -> Known<Vec<RateLimitView>> {
    if let Some(map) = v.get("rateLimitsByLimitId").and_then(Value::as_object).filter(|m| !m.is_empty()) {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        return Known::direct(keys.into_iter().map(|k| limit_of(&map[k], Some(k))).collect());
    }
    match v.get("rateLimits").filter(|s| !s.is_null()) {
        Some(s) => Known::direct(vec![limit_of(s, None)]),
        None => Known::Missing,
    }
}

/// `account/usage/read` → 累計の使用量（表示だけ）。
pub fn usage_of(v: &Value) -> Known<UsageSummary> {
    match v.get("summary").filter(|s| s.is_object()) {
        Some(s) => Known::direct(UsageSummary { lifetime_tokens: known_u64(s, "lifetimeTokens"), peak_daily_tokens: known_u64(s, "peakDailyTokens") }),
        None => Known::Missing,
    }
}

/// `memory/status` の表示用の要約（値はそのまま。読めない項目は触れない）。
pub fn memory_summary(v: &Value) -> Known<String> {
    let threads = v.get("v2ConsolidatedThreads").and_then(Value::as_u64);
    let ready = v.get("v2Ready").and_then(Value::as_bool);
    match (threads, ready) {
        (None, None) => Known::Missing,
        (t, r) => {
            let t = t.map(|n| format!("統合済みスレッド {n} 件")).unwrap_or_else(|| "統合済みスレッド数は未取得".into());
            let r = match r {
                Some(true) => "準備完了",
                Some(false) => "準備中または未完了",
                None => "準備状況は未取得",
            };
            Known::direct(format!("{t}／{r}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_operations_are_unsupported_never_supported() {
        let caps = op_capabilities(Some("0.160.0"), true);
        assert_eq!(caps.len(), ParityOp::ALL.len());
        // 実装済みの操作（P3-1: 変更の一覧・戻す、P3-2: レビュー・分岐・圧縮、P3-3: 計画／実行・Goal・状態・速度・memories）だけが対応。確認状況は未確認のまま。
        let implemented = [
            ParityOp::ChangeList,
            ParityOp::RevertChanges,
            ParityOp::CodeReview,
            ParityOp::ReviewToNewChat,
            ParityOp::Fork,
            ParityOp::Compact,
            ParityOp::WorkMode,
            ParityOp::Goal,
            ParityOp::BackendStatus,
            ParityOp::SpeedTier,
            ParityOp::Memory,
        ];
        for c in &caps {
            if implemented.contains(&c.op) {
                assert_eq!(c.support, Support::Supported, "{:?}", c.op);
                assert_eq!(c.verification, Verification::Unverified, "{:?}", c.op);
                assert!(c.note.is_none());
            } else {
                assert_eq!(c.support, Support::Unsupported, "{:?}", c.op);
                assert!(c.note.is_some());
            }
        }
        let personality = caps.iter().find(|c| c.op == ParityOp::Personality).unwrap();
        assert!(personality.deprecated);
    }

    #[test]
    fn experimental_dependent_ops_are_not_usable_when_experimental_is_off() {
        let caps = op_capabilities(Some("0.160.0"), false);
        for op in [ParityOp::WorkMode, ParityOp::Memory] {
            let c = caps.iter().find(|c| c.op == op).unwrap();
            assert_eq!(c.support, Support::Experimental, "{op:?}");
            assert!(c.note.is_some());
        }
        // 安定APIの操作は影響を受けない。
        assert_eq!(caps.iter().find(|c| c.op == ParityOp::Goal).unwrap().support, Support::Supported);
    }

    #[test]
    fn goal_status_maps_known_values_and_keeps_unknown_raw() {
        assert_eq!(goal_status_from_wire("active"), GoalStatus::Active);
        assert_eq!(goal_status_from_wire("usageLimited"), GoalStatus::UsageLimited);
        assert_eq!(goal_status_from_wire("budgetLimited"), GoalStatus::BudgetLimited);
        assert_eq!(goal_status_from_wire("complete"), GoalStatus::Complete);
        // 未知の値は既知の値に丸めず、原文を残す。
        assert_eq!(goal_status_from_wire("someFutureState"), GoalStatus::Unknown { raw: "someFutureState".into() });
        assert_eq!(goal_status_to_wire(&GoalStatus::Paused), Some("paused"));
        assert_eq!(goal_status_to_wire(&GoalStatus::Unknown { raw: "x".into() }), None);
    }

    #[test]
    fn goal_from_wire_requires_objective_and_status_and_never_invents_numbers() {
        let g = json!({"threadId": "t", "objective": "ship", "status": "weird", "tokenBudget": null, "tokensUsed": 12, "timeUsedSeconds": -1, "createdAt": 1, "updatedAt": 2});
        let goal = goal_from_wire(&g).unwrap();
        assert_eq!(goal.status, GoalStatus::Unknown { raw: "weird".into() });
        assert_eq!(goal.token_budget, Known::Missing);
        assert_eq!(goal.tokens_used, Known::direct(12));
        assert_eq!(goal.time_used_secs, Known::Missing, "negative is not rounded to zero");
        assert_eq!(goal.updated_at, Known::NotFetched, "unit is not documented, so it is not converted");
        assert!(goal_from_wire(&json!({"objective": "x"})).is_none());
        assert!(goal_from_wire(&json!({"status": "active"})).is_none());
    }

    #[test]
    fn goal_set_params_reject_empty_unknown_and_noop_updates() {
        let upd = |o: Option<&str>, s: Option<GoalStatus>, b: Option<u64>| GoalUpdate { objective: o.map(str::to_string), status: s, token_budget: b };
        assert!(goal_set_params("t", &upd(None, None, None)).is_err());
        assert!(goal_set_params("t", &upd(Some("  "), None, None)).is_err());
        assert!(goal_set_params("t", &upd(None, Some(GoalStatus::Unknown { raw: "x".into() }), None)).is_err());
        let p = goal_set_params("t", &upd(Some("do it"), Some(GoalStatus::Paused), Some(100))).unwrap();
        assert_eq!(p, json!({"threadId": "t", "objective": "do it", "status": "paused", "tokenBudget": 100}));
    }

    #[test]
    fn collaboration_mode_needs_a_known_model() {
        assert!(collaboration_mode_json(WorkMode::Plan, None).is_err());
        let m = ModelChoice { model: "m".into(), effort: Some("low".into()), speed_tier: Some("fast".into()) };
        let v = collaboration_mode_json(WorkMode::Plan, Some(&m)).unwrap();
        assert_eq!(v, json!({"mode": "plan", "settings": {"model": "m", "reasoning_effort": "low", "developer_instructions": null}}));
        assert_eq!(work_mode_from_wire("default"), Some(WorkMode::Default));
        assert_eq!(work_mode_from_wire("other"), None);
    }

    #[test]
    fn ack_distinguishes_rejected_unknown_and_connection_errors() {
        assert_eq!(ack_of(Ok(json!({}))).unwrap(), OpAck::Accepted);
        assert_eq!(ack_of(Err(BackendError::Rejected { code: None, message: "no".into() })).unwrap(), OpAck::Rejected { message: "no".into() });
        assert_eq!(ack_of(Err(BackendError::OutcomeUnknown { message: "t".into() })).unwrap(), OpAck::Unknown { message: "t".into() });
        assert!(matches!(ack_of(Err(BackendError::NotConnected)), Err(BackendError::NotConnected)));
    }

    #[test]
    fn experimental_rejection_is_detected_from_the_message_only_for_rejections() {
        let rej = |m: &str| BackendError::Rejected { code: None, message: m.into() };
        assert!(is_experimental_rejection(&rej("turn/start.collaborationMode requires experimentalApi capability")));
        assert!(!is_experimental_rejection(&rej("invalid params")));
        assert!(!is_experimental_rejection(&BackendError::OutcomeUnknown { message: "experimental".into() }));
    }

    #[test]
    fn status_parsers_leave_unreadable_items_unfetched_or_missing() {
        assert_eq!(account_of(&json!({"account": null, "requiresOpenaiAuth": true})), (Known::Missing, Known::Missing));
        let (k, p) = account_of(&json!({"account": {"type": "chatgpt", "email": "a@b", "planType": "plus"}}));
        assert_eq!((k, p), (Known::direct("chatgpt".to_string()), Known::direct("plus".to_string())));
        let rl = rate_limits_of(&json!({"rateLimits": {"limitId": "codex", "primary": {"usedPercent": 12.5, "windowDurationMins": 300, "resetsAt": 1}, "secondary": null}}));
        let Known::Value { value, .. } = rl else { panic!("expected value") };
        assert_eq!(value.len(), 1);
        assert_eq!(value[0].windows.len(), 1);
        assert_eq!(value[0].windows[0].used_percent, 12.5);
        assert_eq!(rate_limits_of(&json!({})), Known::Missing);
        assert_eq!(usage_of(&json!({})), Known::Missing);
        assert_eq!(memory_summary(&json!({})), Known::Missing);
        assert_eq!(memory_summary(&json!({"v2ConsolidatedThreads": 3, "v2Ready": true})), Known::direct("統合済みスレッド 3 件／準備完了".to_string()));
    }

    #[test]
    fn review_params_are_always_inline_and_never_detached() {
        let ids = |t: ReviewTarget| review_params_json("th", &t);
        let v = ids(ReviewTarget::UncommittedChanges);
        assert_eq!(v, json!({"threadId": "th", "target": {"type": "uncommittedChanges"}, "delivery": "inline"}));
        assert_eq!(ids(ReviewTarget::BaseBranch { branch: "main".into() })["target"], json!({"type": "baseBranch", "branch": "main"}));
        assert_eq!(ids(ReviewTarget::Commit { sha: "abc1234".into(), title: None })["target"], json!({"type": "commit", "sha": "abc1234", "title": null}));
        assert_eq!(ids(ReviewTarget::Custom { instructions: "x".into() })["target"], json!({"type": "custom", "instructions": "x"}));
        assert!(!v.to_string().contains("detached"));
    }

    #[test]
    fn fork_params_exclude_turns_and_carry_the_terminal_turn_and_read_only() {
        let plain = fork_params_json("th", &ForkParams { through_turn: None, ephemeral: false, read_only: false });
        assert_eq!(plain, json!({"threadId": "th", "excludeTurns": true, "ephemeral": false}));
        let full = fork_params_json("th", &ForkParams { through_turn: Some(ExternalId("t9".into())), ephemeral: true, read_only: true });
        assert_eq!(full, json!({"threadId": "th", "excludeTurns": true, "ephemeral": true, "lastTurnId": "t9", "sandbox": "read-only"}));
        assert_eq!(forked_thread_id(&json!({"thread": {"id": "n1"}})), Some("n1"));
        assert_eq!(forked_thread_id(&json!({"thread": {"id": ""}})), None);
        assert_eq!(forked_thread_id(&json!({})), None);
    }

    #[test]
    fn work_modes_skip_unknown_and_duplicate_presets() {
        let r = json!({"data": [{"name": "Plan", "mode": "plan"}, {"name": "x", "mode": null}, {"name": "Default", "mode": "default"}, {"name": "Plan2", "mode": "plan"}, {"name": "?", "mode": "future"}]});
        let v = work_modes_of(&r);
        assert_eq!(v.iter().map(|m| (m.mode, m.label.as_str())).collect::<Vec<_>>(), vec![(WorkMode::Plan, "Plan"), (WorkMode::Default, "Default")]);
    }
}
