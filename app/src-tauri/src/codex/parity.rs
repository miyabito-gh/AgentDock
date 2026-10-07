//! `ParityOps` のCodex実装（段階③、`app/DESIGN_P3.md` §2.2・§3）。
//!
//! 操作ごとの節に分けてある。P3-1〜P3-6 は自分の節のメソッドだけを実装し、`DECLARED` の `implemented` を真にする。
//! Codex固有の変換（レビュー対象・協調モード・速度ID・MCP接続状態など）はこのファイルと `cli.rs` に閉じ込める。
//! 実装していない操作は、能力宣言も呼出しも `Unsupported`（成功を装わない）。

use super::adapter::{CodexBackend, READ_TIMEOUT, WRITE_TIMEOUT};
use super::cli::{config_toml_hash, summarize_output, CliCommand, CliError, OutputSummary};
use super::convert::chat_key;
use super::parity_table;
use crate::backend::backend::*;
use crate::backend::model::*;
use crate::backend::parity::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
    // P3-4: 過去会話の参考指定（AgentDock管理・履歴の読取りだけ）、side相談（一時fork）、Skills、指示ファイル。確認状況は未確認（Skillsの明示呼出しだけ実測済み）。
    done(ParityOp::ReferenceChat, OpRoute::AppManaged, Support::Supported),
    done(ParityOp::Goal, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::SideChat, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::Skills, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::InstructionFiles, OpRoute::BackendApi, Support::Supported),
    // P3-5: MCP（App Server）とPlugins（一覧は読取り、導入・削除はCLI補助）。確認状況は未確認のまま。
    done(ParityOp::ToolServers, OpRoute::BackendApi, Support::Supported),
    done(ParityOp::Extensions, OpRoute::CliHelper, Support::Supported),
    d(ParityOp::CloudDelegation, OpRoute::CliHelper, Support::Experimental),
    // P3-7: Git worktree（AgentDock管理）。確認状況は未確認のまま。
    done(ParityOp::Worktree, OpRoute::AppManaged, Support::Supported),
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
                Some(id) => {
                    let forked = chat_key(id);
                    // 分岐先の読み込まれた指示ファイル（応答が示していれば覚える。後で確認画面に出す）。
                    self.note_instruction_sources(&forked, &r);
                    ForkOutcome { ack: OpAck::Accepted, chat: Some(forked) }
                }
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
    /// 作業フォルダのSkillを一覧する（読取りのみ。`forceReload` は明示操作のときだけ）。
    async fn list_skills(&self, cwd: String, force_reload: bool) -> BackendResult<SkillList> {
        let r = self.call("skills/list", json!({"cwds": [cwd], "forceReload": force_reload}), READ_TIMEOUT).await?;
        skill_list_of(&r).ok_or_else(|| BackendError::Protocol { message: "skills/list: unexpected response shape".into() })
    }
    /// この接続で、start・resume・forkの応答が示した指示ファイル。読むためにresumeしないので、まだ示されていなければ未取得。
    async fn instruction_sources(&self, chat: ChatKey) -> BackendResult<Known<Vec<String>>> {
        Ok(match self.instruction_sources_seen(&chat) {
            Some(list) => Known::direct(list),
            None => Known::NotFetched,
        })
    }

    // ── ツールサーバー・拡張（P3-5） ──
    /// 読取りのみ。接続状態は「いまのApp Server側の実行状態」で、既存の会話に反映済みという意味ではない。
    async fn list_tool_servers(&self) -> BackendResult<Vec<ToolServerView>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_STATUS_PAGES {
            let mut params = json!({"detail": "toolsAndAuthOnly"});
            if let Some(c) = &cursor {
                params["cursor"] = json!(c);
            }
            let r = self.call("mcpServerStatus/list", params, READ_TIMEOUT).await?;
            let data = r.get("data").and_then(Value::as_array).ok_or_else(|| BackendError::Protocol { message: "mcpServerStatus/list: unexpected response shape".into() })?;
            for v in data {
                out.push(tool_server_from_wire(v).ok_or_else(|| BackendError::Protocol { message: "mcpServerStatus/list: unexpected server shape".into() })?);
            }
            match r.get("nextCursor").and_then(Value::as_str) {
                Some(c) => cursor = Some(c.to_string()),
                None => return Ok(out),
            }
        }
        // ページが多すぎて読み切れなかった。一部だけを全体として見せない。
        Err(BackendError::Protocol { message: "mcpServerStatus/list: too many pages".into() })
    }
    /// 認可URLを返す。開くのはUIの明示クリックで、URLはログ・履歴に残さない。
    async fn login_tool_server(&self, name: String, _confirmed: &UserConfirmed) -> BackendResult<String> {
        let r = self.call("mcpServer/oauth/login", json!({"name": name}), WRITE_TIMEOUT).await?;
        r.get("authorizationUrl")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| BackendError::Protocol { message: "mcpServer/oauth/login: no authorization url".into() })
    }
    /// ユーザーのボタンからだけ呼ぶ。受付は「設定を読み直す要求が受け付けられた」だけで、既存会話への反映は確認できない。
    async fn reload_tool_servers(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        ack_of(self.call("config/mcpServer/reload", json!({}), WRITE_TIMEOUT).await)
    }
    /// 読取りのみ（`plugin/installed`）。管理系のApp Server API（install・uninstall）は使わない。
    async fn list_extensions(&self) -> BackendResult<Vec<ExtensionView>> {
        let r = self.call("plugin/installed", json!({}), READ_TIMEOUT).await?;
        extension_views_of(&r).ok_or_else(|| BackendError::Protocol { message: "plugin/installed: unexpected response shape".into() })
    }
    /// 導入・削除（CLI補助）。設定を前後で読み直して照合する。自動rollbackはしない。呼び出し側が確認済みで、アプリ内で直列化する。
    /// CLIが終わらない・異常終了のときは `ResultUnconfirmed`（成功にも失敗にもしない）。
    async fn manage_extension(&self, op: ExtensionOp, _confirmed: &UserConfirmed) -> BackendResult<ExtensionOpResult> {
        let (command, scope) = cli_command_of(&op).map_err(|m| BackendError::Rejected { code: None, message: m })?;
        self.client()?;
        let Some(cli) = self.cli() else { return Err(BackendError::NotConnected) };
        let _serial = self.ext_lock().await;
        let hash_before = config_toml_hash();
        let before = self.read_config().await;
        let run = cli.run(&command, None).await;
        if let Err(CliError::InvalidArgument(m)) = &run {
            return Err(BackendError::Rejected { code: None, message: format!("引数が正しくありません: {m}") });
        }
        let hash_after = config_toml_hash();
        let after = self.read_config().await;
        let compare = if matches!(run, Err(CliError::Unavailable(_))) {
            ConfigCompare::Unverified { reason: "CLIを実行していないため照合していません".into() }
        } else {
            judge_config(before.as_ref(), after.as_ref(), &hash_before, &hash_after, &scope)
        };
        let (status, exit_code, summary) = match run {
            Ok(out) => {
                let summary = summarize_output(&command, &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr));
                match out.exit_code {
                    Some(0) => (ExtensionOpStatus::ExitedZero, Known::direct(0), summary),
                    Some(n) => (ExtensionOpStatus::ExitedNonZero, Known::direct(n), summary),
                    None => (ExtensionOpStatus::ResultUnconfirmed { reason: "終了コードを取得できませんでした（異常終了）".into() }, Known::NotFetched, summary),
                }
            }
            Err(CliError::Unavailable(m)) => {
                let reason = format!("codex を起動できませんでした: {m}");
                (ExtensionOpStatus::NotRun { reason: reason.clone() }, Known::NotFetched, OutputSummary { text: reason, is_raw: false })
            }
            Err(e) => {
                let reason = match e {
                    CliError::Timeout => "時間内に終わりませんでした".to_string(),
                    CliError::OutputTooLarge(n) => format!("出力が {n} バイトを超えました"),
                    other => format!("実行中に問題が起きました: {other}"),
                };
                (ExtensionOpStatus::ResultUnconfirmed { reason: reason.clone() }, Known::NotFetched, OutputSummary { text: reason, is_raw: false })
            }
        };
        Ok(ExtensionOpResult {
            status,
            exit_code,
            config_compare: compare,
            config_hash_before: hash_before,
            config_hash_after: hash_after,
            output_summary: summary.text,
            summary_is_raw: summary.is_raw,
        })
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
    /// `config/read` の `config`（読めなければ None。照合は「照合できなかった」になる）。
    async fn read_config(&self) -> Option<Value> {
        let r = self.call("config/read", json!({}), READ_TIMEOUT).await.ok()?;
        r.get("config").filter(|c| c.is_object()).cloned()
    }

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

/// `skills/list` の応答 → Skill一覧。複数の作業フォルダの項目を連結する（同じ定義ファイルは1件）。
/// 名前・定義ファイルのパスが読めないSkillは捨てずに errors へ出す。説明・scope・有効は欠けたら未取得（空・既定値にしない）。
pub fn skill_list_of(r: &Value) -> Option<SkillList> {
    let mut out = SkillList::default();
    for entry in r.get("data")?.as_array()? {
        for s in entry.get("skills").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let name = s.get("name").and_then(Value::as_str).filter(|n| !n.is_empty());
            let path = s.get("path").and_then(Value::as_str).filter(|p| !p.is_empty());
            let (Some(name), Some(path)) = (name, path) else {
                out.errors.push("名前または定義ファイルの場所を読めないSkillがありました".into());
                continue;
            };
            if out.skills.iter().any(|x| x.path == path) {
                continue;
            }
            out.skills.push(SkillInfo {
                name: name.to_string(),
                description: s.get("description").and_then(Value::as_str).map(|d| Known::direct(d.to_string())).unwrap_or(Known::NotFetched),
                scope: s.get("scope").and_then(Value::as_str).map(|d| Known::direct(d.to_string())).unwrap_or(Known::NotFetched),
                enabled: s.get("enabled").and_then(Value::as_bool).map(Known::direct).unwrap_or(Known::NotFetched),
                path: path.to_string(),
                errors: Vec::new(),
            });
        }
        for e in entry.get("errors").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let path = e.get("path").and_then(Value::as_str).unwrap_or("（場所不明）");
            let message = e.get("message").and_then(Value::as_str).unwrap_or("（内容不明）");
            out.errors.push(format!("{path}: {message}"));
        }
    }
    Some(out)
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

// ───────────────────────────── ツールサーバー・拡張の変換（純粋、P3-5） ─────────────────────────────

const MAX_STATUS_PAGES: usize = 20;
/// 照合結果に載せる項目名の上限（超えた分は件数で示す）。
const MAX_COMPARE_KEYS: usize = 20;

pub fn tool_connection_from_wire(v: Option<&str>) -> ToolServerConnection {
    match v {
        Some("notStarted") => ToolServerConnection::NotStarted,
        Some("starting") => ToolServerConnection::Starting,
        Some("connected") => ToolServerConnection::Connected,
        Some("authenticationRequired") => ToolServerConnection::AuthRequired,
        Some("failed") => ToolServerConnection::Failed,
        Some("cancelled") => ToolServerConnection::Cancelled,
        Some("disabled") => ToolServerConnection::Disabled,
        _ => ToolServerConnection::Unknown,
    }
}

/// 起動状態の通知（`mcpServer/startupStatus/updated.status`）の写像。
pub fn tool_startup_state_from_wire(v: &str) -> ToolServerConnection {
    match v {
        "starting" => ToolServerConnection::Starting,
        "ready" => ToolServerConnection::Connected,
        "failed" => ToolServerConnection::Failed,
        "cancelled" => ToolServerConnection::Cancelled,
        _ => ToolServerConnection::Unknown,
    }
}

fn tool_auth_from_wire(v: Option<&str>) -> ToolServerAuth {
    match v {
        Some("notLoggedIn") => ToolServerAuth::NotLoggedIn,
        Some("bearerToken") => ToolServerAuth::Bearer,
        Some("oAuth") => ToolServerAuth::OAuth,
        Some("unsupported") => ToolServerAuth::Unsupported,
        _ => ToolServerAuth::Unknown,
    }
}

/// `McpServerStatus` 1件。名前がなければ None。ツール数は、一覧の取得に失敗していない（`toolsError` なし）ときだけ値にする。
pub fn tool_server_from_wire(v: &Value) -> Option<ToolServerView> {
    let name = v.get("name").and_then(Value::as_str)?.to_string();
    let tools_error = v.get("toolsError").and_then(Value::as_str).map(str::to_string);
    let tool_count = match (&tools_error, v.get("tools").and_then(Value::as_object)) {
        (None, Some(t)) => Known::direct(t.len() as u32),
        _ => Known::NotFetched,
    };
    Some(ToolServerView {
        name,
        connection: tool_connection_from_wire(v.get("runtimeStatus").and_then(Value::as_str)),
        auth: tool_auth_from_wire(v.get("authStatus").and_then(Value::as_str)),
        tool_count,
        tools_error,
    })
}

/// `plugin/installed` の応答。形が違えば None。cache・会話への提示は取る手段がないので未取得のまま。
pub fn extension_views_of(r: &Value) -> Option<Vec<ExtensionView>> {
    let mut out = Vec::new();
    for m in r.get("marketplaces")?.as_array()? {
        for p in m.get("plugins")?.as_array()? {
            let id = p.get("id").and_then(Value::as_str)?;
            let name = p.get("name").and_then(Value::as_str).unwrap_or(id);
            out.push(ExtensionView {
                id: id.to_string(),
                name: name.to_string(),
                installed: p.get("installed").and_then(Value::as_bool).map(Known::direct).unwrap_or(Known::NotFetched),
                enabled_in_config: p.get("enabled").and_then(Value::as_bool).map(Known::direct).unwrap_or(Known::NotFetched),
                cache_present: Known::NotFetched,
                advertised_in_chat: Known::NotFetched,
            });
        }
    }
    Some(out)
}

/// 管理操作が変えてよい設定の範囲（`top` の直下の `name`。pluginは `name@提供元` も含む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedScope {
    pub top: &'static str,
    pub name: String,
}

impl ExpectedScope {
    fn contains(&self, path: &[String]) -> bool {
        if path.first().map(String::as_str) != Some(self.top) {
            return false;
        }
        match path.get(1) {
            // 親の表そのものの出現・消滅（`plugins` が空になる等）。
            None => true,
            Some(seg) => seg == &self.name || (self.top == "plugins" && seg.starts_with(&format!("{}@", self.name))),
        }
    }
}

/// 操作を許可済みのCLIコマンドと期待範囲へ。
pub fn cli_command_of(op: &ExtensionOp) -> Result<(CliCommand, ExpectedScope), String> {
    let plugin_name = |id: &str| id.split('@').next().unwrap_or(id).to_string();
    Ok(match op {
        ExtensionOp::Install { id } => (CliCommand::PluginAdd { plugin: id.clone() }, ExpectedScope { top: "plugins", name: plugin_name(id) }),
        ExtensionOp::Remove { id } => (CliCommand::PluginRemove { plugin: id.clone() }, ExpectedScope { top: "plugins", name: plugin_name(id) }),
        ExtensionOp::AddToolServer { name, command } => {
            (CliCommand::McpAddStdio { name: name.clone(), command: command.clone() }, ExpectedScope { top: "mcp_servers", name: name.clone() })
        }
        ExtensionOp::RemoveToolServer { name } => (CliCommand::McpRemove { name: name.clone() }, ExpectedScope { top: "mcp_servers", name: name.clone() }),
    })
}

fn flatten(v: &Value, path: &mut Vec<String>, out: &mut BTreeMap<Vec<String>, Value>) {
    match v {
        Value::Object(m) if !m.is_empty() => {
            for (k, x) in m {
                path.push(k.clone());
                flatten(x, path, out);
                path.pop();
            }
        }
        // 空の設定そのもの（根が空の表）は項目ではない。
        Value::Object(_) if path.is_empty() => {}
        leaf => {
            out.insert(path.clone(), leaf.clone());
        }
    }
}

/// 前後の設定で値が違う（片方にしかない）項目のパス。値は返さない。
pub fn config_diff_paths(before: &Value, after: &Value) -> Vec<Vec<String>> {
    let (mut b, mut a) = (BTreeMap::new(), BTreeMap::new());
    flatten(before, &mut Vec::new(), &mut b);
    flatten(after, &mut Vec::new(), &mut a);
    let mut keys: Vec<&Vec<String>> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().filter(|k| b.get(*k) != a.get(*k)).cloned().collect()
}

fn hash_changed(a: &Known<String>, b: &Known<String>) -> Option<bool> {
    match (a, b) {
        (Known::Value { value: x, .. }, Known::Value { value: y, .. }) => Some(x != y),
        (Known::Missing, Known::Missing) => Some(false),
        (Known::Missing, Known::Value { .. }) | (Known::Value { .. }, Known::Missing) => Some(true),
        _ => None,
    }
}

fn join_keys(paths: &[Vec<String>]) -> Vec<String> {
    let mut keys: Vec<String> = paths.iter().take(MAX_COMPARE_KEYS).map(|p| p.join(".")).collect();
    if paths.len() > MAX_COMPARE_KEYS {
        keys.push(format!("ほか{}件", paths.len() - MAX_COMPARE_KEYS));
    }
    keys
}

/// 管理操作の前後の設定の照合（純粋）。読めなかったときは一致と扱わない。値は比べるだけで、結果には項目名しか入れない。
/// - 読み取った設定が同じで、設定ファイルのハッシュも同じ（または比べられない）→ 変化なし。
/// - 読み取った設定が同じなのにファイルのハッシュが違う → 想定外（読み取りに現れない変更）。
/// - 設定の読取りだけ変わり、ファイルのハッシュが変わっていない → 想定外（操作の範囲でも）。
/// - 変化がすべて対象の範囲の下 → 想定どおり。範囲の外が混じれば想定外（その項目名を示す）。
pub fn judge_config(before: Option<&Value>, after: Option<&Value>, hash_before: &Known<String>, hash_after: &Known<String>, scope: &ExpectedScope) -> ConfigCompare {
    let (Some(b), Some(a)) = (before, after) else {
        return ConfigCompare::Unverified { reason: "設定（config/read）を前後で読み取れなかったため、変更内容を照合できません".into() };
    };
    let paths = config_diff_paths(b, a);
    let file_changed = hash_changed(hash_before, hash_after);
    if paths.is_empty() {
        return if file_changed == Some(true) {
            ConfigCompare::Unexpected { keys: vec!["config.toml（読み取った設定には現れない変更）".into()] }
        } else {
            ConfigCompare::Unchanged
        };
    }
    if file_changed == Some(false) {
        return ConfigCompare::Unexpected { keys: join_keys(&paths) };
    }
    let outside: Vec<Vec<String>> = paths.into_iter().filter(|p| !scope.contains(p)).collect();
    if outside.is_empty() {
        ConfigCompare::ChangedAsExpected
    } else {
        ConfigCompare::Unexpected { keys: join_keys(&outside) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_operations_are_unsupported_never_supported() {
        let caps = op_capabilities(Some("0.160.0"), true);
        assert_eq!(caps.len(), ParityOp::ALL.len());
        // 実装済みの操作（P3-1: 変更の一覧・戻す、P3-2: レビュー・分岐・圧縮、P3-3: 計画／実行・Goal・状態・速度・memories、P3-4: 参考指定・side・Skills・指示ファイル、P3-5: MCP・Plugins）だけが対応。
        // 確認状況は未確認のまま（Skillsの明示呼出しだけ実測済み）。
        let implemented = [
            ParityOp::Worktree,
            ParityOp::ReferenceChat,
            ParityOp::SideChat,
            ParityOp::Skills,
            ParityOp::InstructionFiles,
            ParityOp::ToolServers,
            ParityOp::Extensions,
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
                let expected = if c.op == ParityOp::Skills { Verification::Verified } else { Verification::Unverified };
                assert_eq!(c.verification, expected, "{:?}", c.op);
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
    fn skill_list_keeps_unreadable_entries_as_errors_and_never_invents_fields() {
        let r = json!({"data": [{
            "cwd": "C:/w",
            "skills": [
                {"name": "review", "description": "d", "path": "C:/s/review/SKILL.md", "scope": "repo", "enabled": true, "pluginId": null},
                {"name": "bare", "path": "C:/s/bare/SKILL.md"},
                {"name": "", "path": "C:/s/x/SKILL.md"},
                {"name": "dup", "path": "C:/s/review/SKILL.md"}
            ],
            "errors": [{"path": "C:/s/bad/SKILL.md", "message": "bad yaml"}]
        }]});
        let l = skill_list_of(&r).unwrap();
        assert_eq!(l.skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["review", "bare"], "unnamed and duplicate definitions are not listed twice");
        assert_eq!(l.skills[0].scope, Known::direct("repo".to_string()));
        assert_eq!(l.skills[1].description, Known::NotFetched);
        assert_eq!(l.skills[1].enabled, Known::NotFetched, "a missing flag is not turned into false");
        assert_eq!(l.errors.len(), 2);
        assert!(l.errors.iter().any(|e| e == "C:/s/bad/SKILL.md: bad yaml"));
        assert!(skill_list_of(&json!({})).is_none());
    }

    #[test]
    fn work_modes_skip_unknown_and_duplicate_presets() {
        let r = json!({"data": [{"name": "Plan", "mode": "plan"}, {"name": "x", "mode": null}, {"name": "Default", "mode": "default"}, {"name": "Plan2", "mode": "plan"}, {"name": "?", "mode": "future"}]});
        let v = work_modes_of(&r);
        assert_eq!(v.iter().map(|m| (m.mode, m.label.as_str())).collect::<Vec<_>>(), vec![(WorkMode::Plan, "Plan"), (WorkMode::Default, "Default")]);
    }

    fn scope_mcp(n: &str) -> ExpectedScope {
        ExpectedScope { top: "mcp_servers", name: n.into() }
    }
    fn hv(x: &str) -> Known<String> {
        Known::direct(x.to_string())
    }

    #[test]
    fn config_compare_distinguishes_unchanged_expected_unexpected_and_unverified() {
        let base = json!({"model": "m", "mcp_servers": {"a": {"command": "x"}}});
        let added = json!({"model": "m", "mcp_servers": {"a": {"command": "x"}, "srv": {"command": "npx", "args": ["y"]}}});
        let other = json!({"model": "n", "mcp_servers": {"a": {"command": "x"}, "srv": {"command": "npx"}}});
        let s = scope_mcp("srv");
        assert_eq!(judge_config(Some(&base), Some(&base), &hv("1"), &hv("1"), &s), ConfigCompare::Unchanged);
        assert_eq!(judge_config(Some(&base), Some(&added), &hv("1"), &hv("2"), &s), ConfigCompare::ChangedAsExpected);
        assert_eq!(judge_config(Some(&base), Some(&other), &hv("1"), &hv("2"), &s), ConfigCompare::Unexpected { keys: vec!["model".into()] });
        // 別のサーバーの変更は対象外。
        let touched_a = json!({"model": "m", "mcp_servers": {"a": {"command": "z"}, "srv": {"command": "npx"}}});
        assert_eq!(judge_config(Some(&base), Some(&touched_a), &hv("1"), &hv("2"), &s), ConfigCompare::Unexpected { keys: vec!["mcp_servers.a.command".into()] });
        // 読めなければ一致とは言わない。
        assert!(matches!(judge_config(None, Some(&base), &hv("1"), &hv("1"), &s), ConfigCompare::Unverified { .. }));
        assert!(matches!(judge_config(Some(&base), None, &hv("1"), &hv("1"), &s), ConfigCompare::Unverified { .. }));
    }

    #[test]
    fn config_compare_uses_the_file_hash_as_a_cross_check() {
        let base = json!({"model": "m"});
        let s = scope_mcp("srv");
        // 読み取りは同じでもファイルが変わっていれば想定外。
        assert!(matches!(judge_config(Some(&base), Some(&base), &hv("1"), &hv("2"), &s), ConfigCompare::Unexpected { .. }));
        // ハッシュを取れなかったときは、読み取りの結果だけで判断する。
        assert_eq!(judge_config(Some(&base), Some(&base), &Known::NotFetched, &hv("2"), &s), ConfigCompare::Unchanged);
        // ファイルが変わっていないのに読み取りだけ変わった場合は想定外（対象の範囲でも）。
        let added = json!({"model": "m", "mcp_servers": {"srv": {"command": "x"}}});
        assert!(matches!(judge_config(Some(&base), Some(&added), &hv("1"), &hv("1"), &s), ConfigCompare::Unexpected { .. }));
        // ファイルが新規作成された場合（前は無い）。
        assert_eq!(judge_config(Some(&json!({"model": "m"})), Some(&added), &Known::Missing, &hv("2"), &s), ConfigCompare::ChangedAsExpected);
    }

    #[test]
    fn plugin_scope_covers_name_at_marketplace() {
        let (cmd, scope) = cli_command_of(&ExtensionOp::Install { id: "sample@debug".into() }).unwrap();
        assert_eq!(cmd, CliCommand::PluginAdd { plugin: "sample@debug".into() });
        let base = json!({"plugins": {}});
        let after = json!({"plugins": {"sample@debug": {"enabled": true}}});
        assert_eq!(judge_config(Some(&base), Some(&after), &hv("1"), &hv("2"), &scope), ConfigCompare::ChangedAsExpected);
        let other = json!({"plugins": {"zzz@debug": {"enabled": true}}});
        assert!(matches!(judge_config(Some(&base), Some(&other), &hv("1"), &hv("2"), &scope), ConfigCompare::Unexpected { .. }));
    }

    #[test]
    fn tool_server_and_extension_conversion_keep_unknowns_unknown() {
        let v = json!({"name": "srv", "runtimeStatus": null, "authStatus": "oAuth", "tools": {"a": {}, "b": {}}, "toolsError": null});
        let t = tool_server_from_wire(&v).unwrap();
        assert_eq!((t.connection, t.auth, t.tool_count), (ToolServerConnection::Unknown, ToolServerAuth::OAuth, Known::direct(2)));
        let failed = json!({"name": "s2", "runtimeStatus": "authenticationRequired", "authStatus": "weird", "tools": {}, "toolsError": "boom"});
        let t = tool_server_from_wire(&failed).unwrap();
        assert_eq!((t.connection, t.auth, t.tool_count, t.tools_error.as_deref()), (ToolServerConnection::AuthRequired, ToolServerAuth::Unknown, Known::NotFetched, Some("boom")));
        assert!(tool_server_from_wire(&json!({"x": 1})).is_none());
        let r = json!({"marketplaces": [{"name": "m", "plugins": [{"id": "p@m", "name": "p", "installed": true, "enabled": false}]}]});
        let e = extension_views_of(&r).unwrap();
        assert_eq!((e[0].installed.clone(), e[0].enabled_in_config.clone(), e[0].cache_present.clone()), (Known::direct(true), Known::direct(false), Known::NotFetched));
        assert!(extension_views_of(&json!({"x": 1})).is_none());
    }
}
