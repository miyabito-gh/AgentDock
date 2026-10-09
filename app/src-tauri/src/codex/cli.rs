//! `codex.exe` のCLI補助の実行部品（段階③ DESIGN_P3 §0.1・§2.3）。
//!
//! 許可したサブコマンド（[`CliCommand`]）だけを実行し、任意のコマンドは受け付けない。引数は配列（シェルなし）、
//! コンソール窓なし、タイムアウトと出力上限あり、実行はアプリ内で直列化する。
//! 呼出しはユーザーの明示操作（確認済み）からだけ。`~/.codex` を変えうる操作（plugin・mcpの追加削除）の
//! 前後の照合・記録は呼出し側（`host/extensions.rs`、P3-5）が行う。この部品は実行と結果の取得だけを担う。
//! 引数の形は 0.160.0 の `--help` で確認した（出力の解析は各タスクで実測してから作る）。

use std::time::Duration;

use crate::exec::{run_bounded, RunError, RunSpec};
use crate::backend::model::Known;
use crate::backend::parity::{CloudRunStatus, CloudSubmitOutcome, CloudTaskInfo};

pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// 管理系（plugin・mcp・cloud）の既定のタイムアウト。
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    PluginAdd { plugin: String },
    PluginRemove { plugin: String },
    /// stdioサーバーの追加。`command` の先頭が起動するプログラム。
    McpAddStdio { name: String, command: Vec<String> },
    McpRemove { name: String },
    CloudExec { env_id: String, branch: Option<String>, prompt: String },
    CloudList,
    CloudStatus { task_id: String },
    CloudDiff { task_id: String },
    CloudApply { task_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    /// 引数が許可の形ではない（実行していない）。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("cannot start codex: {0}")]
    Unavailable(String),
    /// 時間内に終わらなかった。コマンドの結果は不明（再実行しない。状態を読み取りで確認する）。
    #[error("timed out")]
    Timeout,
    #[error("output exceeded {0} bytes")]
    OutputTooLarge(usize),
    #[error("io: {0}")]
    Io(String),
}

impl From<RunError> for CliError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::Spawn(m) => CliError::Unavailable(m),
            RunError::Timeout => CliError::Timeout,
            RunError::OutputTooLarge(n) => CliError::OutputTooLarge(n),
            RunError::Io(m) => CliError::Io(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// 識別子（プラグイン・サーバー名・タスクID・環境ID・ブランチ）として安全か。先頭が `-` でなく、空白・制御文字を含まない。
fn valid_token(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn check(ok: bool, what: &str) -> Result<(), CliError> {
    if ok {
        Ok(())
    } else {
        Err(CliError::InvalidArgument(what.to_string()))
    }
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

impl CliCommand {
    /// 引数の配列（`codex` の後ろ）。自由な本文（依頼文・サーバーの起動コマンド）は `--` の後ろに置く。
    pub fn args(&self) -> Result<Vec<String>, CliError> {
        Ok(match self {
            CliCommand::PluginAdd { plugin } => {
                check(valid_token(plugin), "plugin")?;
                let mut a = strings(&["plugin", "add"]);
                a.push(plugin.clone());
                a
            }
            CliCommand::PluginRemove { plugin } => {
                check(valid_token(plugin), "plugin")?;
                let mut a = strings(&["plugin", "remove"]);
                a.push(plugin.clone());
                a
            }
            CliCommand::McpAddStdio { name, command } => {
                check(valid_token(name), "name")?;
                check(!command.is_empty() && command.iter().all(|c| !c.is_empty() && !c.contains('\0')), "command")?;
                let mut a = strings(&["mcp", "add"]);
                a.push(name.clone());
                a.push("--".into());
                a.extend(command.iter().cloned());
                a
            }
            CliCommand::McpRemove { name } => {
                check(valid_token(name), "name")?;
                let mut a = strings(&["mcp", "remove"]);
                a.push(name.clone());
                a
            }
            CliCommand::CloudExec { env_id, branch, prompt } => {
                check(valid_token(env_id), "env")?;
                check(!prompt.trim().is_empty() && !prompt.contains('\0'), "prompt")?;
                let mut a = strings(&["cloud", "exec", "--env"]);
                a.push(env_id.clone());
                if let Some(b) = branch {
                    check(valid_token(b), "branch")?;
                    a.push("--branch".into());
                    a.push(b.clone());
                }
                a.push("--".into());
                a.push(prompt.clone());
                a
            }
            CliCommand::CloudList => strings(&["cloud", "list", "--json"]),
            CliCommand::CloudStatus { task_id } => {
                check(valid_token(task_id), "task")?;
                let mut a = strings(&["cloud", "status"]);
                a.push(task_id.clone());
                a
            }
            CliCommand::CloudDiff { task_id } => {
                check(valid_token(task_id), "task")?;
                let mut a = strings(&["cloud", "diff"]);
                a.push(task_id.clone());
                a
            }
            CliCommand::CloudApply { task_id } => {
                check(valid_token(task_id), "task")?;
                let mut a = strings(&["cloud", "apply"]);
                a.push(task_id.clone());
                a
            }
        })
    }
}

/// CLI補助の実行口（`codex.exe` は `settings.executables[codex]`）。実行は直列化する。
pub struct CodexCli {
    exe: String,
    lock: tokio::sync::Mutex<()>,
}

impl CodexCli {
    pub fn new(exe: impl Into<String>) -> Self {
        CodexCli { exe: exe.into(), lock: tokio::sync::Mutex::new(()) }
    }

    /// `cwd` は作業フォルダが必要な操作（apply等）だけ指定する。
    pub async fn run(&self, command: &CliCommand, cwd: Option<&std::path::Path>) -> Result<CliOutput, CliError> {
        let args = command.args()?;
        let _serial = self.lock.lock().await;
        let out = run_bounded(RunSpec { program: &self.exe, args: &args, cwd, env: &[], env_remove: &[], timeout: TIMEOUT, stdout_limit: OUTPUT_LIMIT, stdin: None }).await?;
        Ok(CliOutput { exit_code: out.exit_code, stdout: out.stdout, stderr: out.stderr })
    }
}

// ───────────────────────────── 出力の要約・設定ファイルのハッシュ（P3-5） ─────────────────────────────

/// 想定形式でない出力を原文のまま見せるときの、先頭の文字数。
pub const RAW_HEAD_CHARS: usize = 1000;
/// 要約として使う1行の最大文字数。
const SUMMARY_LINE_MAX: usize = 200;

/// 出力の要約。想定形式なら1行の要約、そうでなければ要約せず原文の先頭（`is_raw`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSummary {
    pub text: String,
    pub is_raw: bool,
}

pub fn raw_head(stdout: &str, stderr: &str) -> OutputSummary {
    let mut all = stdout.trim().to_string();
    if !stderr.trim().is_empty() {
        if !all.is_empty() {
            all.push('\n');
        }
        all.push_str("[stderr] ");
        all.push_str(stderr.trim());
    }
    let mut text: String = all.chars().take(RAW_HEAD_CHARS).collect();
    if all.chars().count() > RAW_HEAD_CHARS {
        text.push('…');
    }
    if text.is_empty() {
        text = "（出力なし）".to_string();
    }
    OutputSummary { text, is_raw: true }
}

/// plugin・mcp の管理コマンドの出力を要約する。想定した形式（0.160.0 の表示文）に当てはまるときだけ1行にし、
/// 当てはまらなければ要約せず原文の先頭を返す（推測で成功・失敗を言わない）。
pub fn summarize_output(command: &CliCommand, stdout: &str, stderr: &str) -> OutputSummary {
    let first_line = stdout.lines().map(str::trim).find(|l| !l.is_empty());
    let expected = match (command, first_line) {
        (CliCommand::McpAddStdio { name, .. }, Some(l)) => l == format!("Added global MCP server '{name}'."),
        (CliCommand::McpRemove { name }, Some(l)) => l == format!("Removed global MCP server '{name}'.") || l == format!("No MCP server named '{name}' found."),
        // plugin はまだ実測していない。選択子の名前部分を含む1行があるときだけ要約として使う。
        (CliCommand::PluginAdd { plugin } | CliCommand::PluginRemove { plugin }, Some(l)) => {
            let base = plugin.split('@').next().unwrap_or(plugin);
            !base.is_empty() && l.contains(base)
        }
        _ => false,
    };
    match first_line {
        Some(l) if expected && l.chars().count() <= SUMMARY_LINE_MAX => OutputSummary { text: l.to_string(), is_raw: false },
        _ => raw_head(stdout, stderr),
    }
}

/// `CODEX_HOME`（なければユーザーの `.codex`）の設定ファイル。読むだけで、書かない。
pub fn config_toml_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()).map(|h| std::path::PathBuf::from(h).join(".codex")))?;
    Some(home.join("config.toml"))
}

/// 設定ファイルのハッシュ（内容は返さない）。無ければ `Missing`（取得したが存在しない）、読めなければ `NotFetched`。
pub fn config_toml_hash() -> crate::backend::model::Known<String> {
    use crate::backend::model::Known;
    let Some(path) = config_toml_path() else { return Known::NotFetched };
    match std::fs::read(&path) {
        Ok(bytes) => Known::direct(crate::rules::revert::sha256_hex(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Known::Missing,
        Err(_) => Known::NotFetched,
    }
}


// ───────────────────────────── クラウド委任の出力の解析（P3-6） ─────────────────────────────
//
// `codex cloud` は experimental で、出力形式を実機で確認していない（exec/apply は実行していない）。
// ここの解析は想定形式のときだけ値を取り、当てはまらなければ要約せず原文の先頭を見せる（推測で成功・失敗を言わない）。

/// タスクIDとして安全な文字だけか（英数・`_`・`-`）。
fn is_task_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `cloud exec` の標準出力からタスクIDを取る。タスクのURL（`…/tasks/<id>`）か `task_…` 形式の語だけを受け付ける。
pub fn parse_cloud_task_id(stdout: &str) -> Option<String> {
    for raw in stdout.split_whitespace() {
        let tok = raw.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '<' | '>' | ',' | ';' | '.'));
        if let Some(pos) = tok.find("/tasks/") {
            let rest = &tok[pos + "/tasks/".len()..];
            let id = rest.split(['?', '#', '/']).next().unwrap_or("");
            if is_task_id(id) {
                return Some(id.to_string());
            }
        } else if tok.starts_with("task_") && is_task_id(tok) {
            return Some(tok.to_string());
        }
    }
    None
}

/// `cloud list --json` の出力。`{"tasks":[…]}` か配列で、各項目に文字列の `id` があるときだけ解釈する。
/// 状態語（`status`）・題名・更新時刻は原文のまま持つ（無ければ未取得）。形が違えば None（呼び出し側が原文を見せる）。
pub fn parse_cloud_list(stdout: &str) -> Option<Vec<CloudTaskInfo>> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let items = match &v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(o) => o.get("tasks")?.as_array()?,
        _ => return None,
    };
    let text = |item: &serde_json::Value, keys: &[&str]| -> Known<String> {
        keys.iter()
            .find_map(|k| item.get(*k).and_then(|x| x.as_str()))
            .map(|s| Known::direct(s.to_string()))
            .unwrap_or(Known::NotFetched)
    };
    items
        .iter()
        .map(|item| {
            let id = item.get("id")?.as_str()?;
            Some(CloudTaskInfo {
                task_id: id.to_string(),
                state_text: text(item, &["status", "state"]),
                title: text(item, &["title"]),
                updated_text: text(item, &["updated_at", "updatedAt"]),
            })
        })
        .collect()
}

/// サブコマンド自体がない版（clapの「unrecognized subcommand」）か。
pub fn is_missing_subcommand(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("unrecognized subcommand")
}

/// `cloud exec` の実行結果を、送信の結果へ分類する（純粋）。
/// - 起動できない・引数不正: 送られていない（Rejected）。
/// - 時間切れ・異常終了・出力過大: 送られたか分からない（Unknown。再送せず一覧で照合）。
/// - 終了コード0でタスクIDあり: Submitted。0でもIDなしは Unknown（原文の先頭を添える）。非0: Rejected。
pub fn classify_cloud_exec(run: &Result<CliOutput, CliError>) -> CloudSubmitOutcome {
    match run {
        Err(CliError::Unavailable(m)) => CloudSubmitOutcome::Rejected { message: format!("codex を起動できなかったため、送っていません（{m}）") },
        Err(CliError::InvalidArgument(m)) => CloudSubmitOutcome::Rejected { message: format!("引数が正しくないため、送っていません（{m}）") },
        Err(CliError::Timeout) => CloudSubmitOutcome::Unknown { message: "時間内に終わらず、送られたか分かりません".into() },
        Err(CliError::OutputTooLarge(n)) => CloudSubmitOutcome::Unknown { message: format!("出力が {n} バイトを超え、送られたか分かりません") },
        Err(CliError::Io(m)) => CloudSubmitOutcome::Unknown { message: format!("実行中に問題が起き、送られたか分かりません（{m}）") },
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            match out.exit_code {
                Some(0) => match parse_cloud_task_id(&stdout) {
                    Some(task_id) => CloudSubmitOutcome::Submitted { task_id },
                    None => CloudSubmitOutcome::Unknown { message: format!("終了コード0でしたが、出力からタスクIDを取れませんでした。出力の先頭: {}", raw_head(&stdout, &stderr).text) },
                },
                Some(n) => CloudSubmitOutcome::Rejected { message: format!("終了コード {n}: {}", raw_head(&stdout, &stderr).text) },
                None => CloudSubmitOutcome::Unknown { message: "終了コードを取得できず（異常終了）、送られたか分かりません".into() },
            }
        }
    }
}

/// status・diff・apply の出力を、終了の観測と表示用の文にする（純粋）。`full` は標準出力の全文（diff用）、でなければ原文の先頭。
pub fn cloud_text_output(out: &CliOutput, full: bool) -> (CloudRunStatus, String) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let status = match out.exit_code {
        Some(0) => CloudRunStatus::ExitedZero,
        Some(code) => CloudRunStatus::ExitedNonZero { code },
        None => CloudRunStatus::Unconfirmed { reason: "終了コードを取得できませんでした（異常終了）".into() },
    };
    let text = if full && out.exit_code == Some(0) && !stdout.trim().is_empty() { stdout.trim_end().to_string() } else { raw_head(&stdout, &stderr).text };
    (status, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_text_goes_after_double_dash_and_options_cannot_be_injected() {
        let a = CliCommand::CloudExec { env_id: "env1".into(), branch: Some("main".into()), prompt: "--version fix it".into() }.args().unwrap();
        assert_eq!(a, ["cloud", "exec", "--env", "env1", "--branch", "main", "--", "--version fix it"]);
        assert!(CliCommand::CloudExec { env_id: "--env".into(), branch: None, prompt: "x".into() }.args().is_err());
        assert!(CliCommand::PluginAdd { plugin: "-x".into() }.args().is_err());
        assert!(CliCommand::McpRemove { name: "a b".into() }.args().is_err());
        assert!(CliCommand::McpAddStdio { name: "s".into(), command: vec![] }.args().is_err());
    }

    fn mcp_add() -> CliCommand {
        CliCommand::McpAddStdio { name: "srv".into(), command: vec!["npx".into()] }
    }

    #[test]
    fn expected_output_is_summarized_and_unexpected_output_is_shown_raw() {
        let s = summarize_output(&mcp_add(), "Added global MCP server 'srv'.\n", "");
        assert_eq!(s, OutputSummary { text: "Added global MCP server 'srv'.".into(), is_raw: false });
        // 想定外の文面・別のサーバー名は要約せず原文。
        let s = summarize_output(&mcp_add(), "Added global MCP server 'other'.\n", "");
        assert!(s.is_raw && s.text.contains("other"));
        let s = summarize_output(&mcp_add(), "", "boom");
        assert_eq!(s, OutputSummary { text: "[stderr] boom".into(), is_raw: true });
        assert_eq!(summarize_output(&mcp_add(), "", "").text, "（出力なし）");
        let p = CliCommand::PluginAdd { plugin: "sample@debug".into() };
        assert!(!summarize_output(&p, "Installed sample from debug\n", "").is_raw);
        assert!(summarize_output(&p, "{\"x\":1}", "").is_raw);
    }

    #[test]
    fn raw_output_is_cut_to_the_head() {
        let long = "x".repeat(RAW_HEAD_CHARS + 50);
        let s = summarize_output(&mcp_add(), &long, "");
        assert!(s.is_raw);
        assert_eq!(s.text.chars().count(), RAW_HEAD_CHARS + 1);
    }

    fn out(code: Option<i32>, stdout: &str, stderr: &str) -> Result<CliOutput, CliError> {
        Ok(CliOutput { exit_code: code, stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() })
    }

    #[test]
    fn cloud_task_id_is_taken_only_from_a_task_url_or_task_word() {
        assert_eq!(parse_cloud_task_id("https://chatgpt.com/codex/tasks/task_e_abc123?x=1\n").as_deref(), Some("task_e_abc123"));
        assert_eq!(parse_cloud_task_id("Submitted task_e_9f.").as_deref(), Some("task_e_9f"));
        assert_eq!(parse_cloud_task_id("done\nok"), None);
        assert_eq!(parse_cloud_task_id("https://example.com/tasks/"), None);
    }

    #[test]
    fn cloud_exec_is_classified_into_submitted_rejected_or_unknown() {
        assert_eq!(classify_cloud_exec(&out(Some(0), "https://x/tasks/task_1\n", "")), CloudSubmitOutcome::Submitted { task_id: "task_1".into() });
        assert!(matches!(classify_cloud_exec(&out(Some(0), "ok\n", "")), CloudSubmitOutcome::Unknown { .. }));
        assert!(matches!(classify_cloud_exec(&out(Some(1), "", "bad env")), CloudSubmitOutcome::Rejected { .. }));
        assert!(matches!(classify_cloud_exec(&out(None, "", "")), CloudSubmitOutcome::Unknown { .. }));
        assert!(matches!(classify_cloud_exec(&Err(CliError::Timeout)), CloudSubmitOutcome::Unknown { .. }));
        assert!(matches!(classify_cloud_exec(&Err(CliError::Io("x".into()))), CloudSubmitOutcome::Unknown { .. }));
        assert!(matches!(classify_cloud_exec(&Err(CliError::Unavailable("x".into()))), CloudSubmitOutcome::Rejected { .. }));
    }

    #[test]
    fn cloud_list_json_is_parsed_only_when_the_shape_is_expected() {
        let l = parse_cloud_list(r#"{"tasks":[{"id":"task_a","status":"READY","title":"Fix"},{"id":"task_b"}],"cursor":null}"#).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].state_text, Known::direct("READY".to_string()));
        assert_eq!(l[1].state_text, Known::NotFetched);
        assert!(parse_cloud_list(r#"[{"id":"t"}]"#).is_some());
        assert!(parse_cloud_list(r#"{"tasks":[{"title":"no id"}]}"#).is_none());
        assert!(parse_cloud_list("not json").is_none());
        assert!(parse_cloud_list("{}").is_none());
    }

    #[test]
    fn missing_cloud_subcommand_and_text_output() {
        assert!(is_missing_subcommand("error: unrecognized subcommand 'cloud'"));
        assert!(!is_missing_subcommand("error: env not found"));
        let o = CliOutput { exit_code: Some(0), stdout: b"diff --git a b\n".to_vec(), stderr: vec![] };
        assert_eq!(cloud_text_output(&o, true), (CloudRunStatus::ExitedZero, "diff --git a b".to_string()));
        let bad = CliOutput { exit_code: Some(2), stdout: vec![], stderr: b"nope".to_vec() };
        assert_eq!(cloud_text_output(&bad, true), (CloudRunStatus::ExitedNonZero { code: 2 }, "[stderr] nope".to_string()));
    }
}
