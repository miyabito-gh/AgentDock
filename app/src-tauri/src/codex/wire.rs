//! Codex App Server（0.160.0 stable schema）のうち、アダプターが読む項目だけの受信用型。
//!
//! 規則:
//! - この型はcodexモジュール内に閉じる。UI・保存形式・`backend`モジュールへ出さない。
//! - 想定外の値で止めない。未知のstatus・型は `Other(String)` に落とし、呼び出し側が unknown 扱いにする。
//! - 欠けた項目は `Option` / `default`。0や空文字で補わない（空のVec・Noneのまま渡す）。

use serde::Deserialize;
use serde_json::Value;

/// `Thread.status`（`ThreadStatus`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active { flags: Vec<String> },
    /// 未知の型、または status 自体が無い。
    Other(String),
}

impl WireStatus {
    pub fn from_value(v: Option<&Value>) -> WireStatus {
        let Some(v) = v else { return WireStatus::Other("missing".into()) };
        match v.get("type").and_then(Value::as_str) {
            Some("notLoaded") => WireStatus::NotLoaded,
            Some("idle") => WireStatus::Idle,
            Some("systemError") => WireStatus::SystemError,
            Some("active") => WireStatus::Active {
                flags: v
                    .get("activeFlags")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            },
            Some(other) => WireStatus::Other(other.to_string()),
            None => WireStatus::Other("missing".into()),
        }
    }
}

/// `Turn.status`（`TurnStatus`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireTurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
    Other(String),
}

impl WireTurnStatus {
    pub fn parse(s: Option<&str>) -> WireTurnStatus {
        match s {
            Some("completed") => WireTurnStatus::Completed,
            Some("interrupted") => WireTurnStatus::Interrupted,
            Some("failed") => WireTurnStatus::Failed,
            Some("inProgress") => WireTurnStatus::InProgress,
            Some(o) => WireTurnStatus::Other(o.to_string()),
            None => WireTurnStatus::Other("missing".into()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WireTurnError {
    pub message: Option<String>,
}

/// `Turn`。`items` は個々のitem（種類が多いのでJSONのまま保持）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WireTurnRaw {
    pub id: String,
    pub items: Vec<Value>,
    /// "notLoaded" | "summary" | "full"。無ければ不明。
    pub items_view: Option<String>,
    pub status: Option<String>,
    pub error: Option<WireTurnError>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct WireTurn {
    pub raw: WireTurnRaw,
    pub status: WireTurnStatus,
}

impl WireTurn {
    pub fn from_value(v: &Value) -> Result<WireTurn, String> {
        let raw: WireTurnRaw = serde_json::from_value(v.clone()).map_err(|e| format!("turn: {e}"))?;
        if raw.id.is_empty() {
            return Err("turn: missing id".into());
        }
        let status = WireTurnStatus::parse(raw.status.as_deref());
        Ok(WireTurn { raw, status })
    }

    pub fn id(&self) -> &str {
        &self.raw.id
    }

    /// items が全件ロード済みと確認できるか。確認できなければ部分取得として扱う。
    pub fn items_complete(&self) -> bool {
        self.raw.items_view.as_deref() == Some("full")
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct WireThreadRaw {
    id: String,
    session_id: Option<String>,
    forked_from_id: Option<String>,
    parent_thread_id: Option<String>,
    preview: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
    source: Option<Value>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    turns: Vec<Value>,
}

/// `Thread`。
#[derive(Debug, Clone)]
pub struct WireThread {
    pub id: String,
    pub session_id: Option<String>,
    pub forked_from_id: Option<String>,
    pub parent_thread_id: Option<String>,
    pub preview: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub source: Option<Value>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub status: WireStatus,
    pub turns: Vec<WireTurn>,
}

impl WireThread {
    pub fn from_value(v: &Value) -> Result<WireThread, String> {
        let raw: WireThreadRaw = serde_json::from_value(v.clone()).map_err(|e| format!("thread: {e}"))?;
        if raw.id.is_empty() {
            return Err("thread: missing id".into());
        }
        let turns = raw.turns.iter().map(WireTurn::from_value).collect::<Result<Vec<_>, _>>()?;
        Ok(WireThread {
            status: WireStatus::from_value(v.get("status")),
            id: raw.id,
            session_id: raw.session_id,
            forked_from_id: raw.forked_from_id,
            parent_thread_id: raw.parent_thread_id,
            preview: raw.preview,
            name: raw.name,
            cwd: raw.cwd,
            created_at: raw.created_at,
            updated_at: raw.updated_at,
            source: raw.source,
            agent_nickname: raw.agent_nickname,
            agent_role: raw.agent_role,
            turns,
        })
    }

    /// 最新のturn。`startedAt` が null のturn（作成直後で時刻未設定）は最新扱い、同値は配列の後ろを優先。
    /// 配列順の保証をschemaで確認できないため、時刻と配列順を併用する。
    pub fn latest_turn(&self) -> Option<&WireTurn> {
        self.turns.iter().enumerate().max_by_key(|(i, t)| (t.raw.started_at.unwrap_or(i64::MAX), *i)).map(|(_, t)| t)
    }

    /// `source` が `{"subAgent": ...}` か。
    pub fn is_subagent_source(&self) -> bool {
        self.source.as_ref().and_then(|s| s.get("subAgent")).is_some()
    }

    /// `source.subAgent.thread_spawn.parent_thread_id`（明示されている場合のみ）。
    pub fn source_parent_thread_id(&self) -> Option<String> {
        self.source
            .as_ref()?
            .get("subAgent")?
            .get("thread_spawn")?
            .get("parent_thread_id")?
            .as_str()
            .map(str::to_string)
    }
}
