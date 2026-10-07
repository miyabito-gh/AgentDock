//! `codex.exe` のCLI補助の実行部品（段階③ DESIGN_P3 §0.1・§2.3）。
//!
//! 許可したサブコマンド（[`CliCommand`]）だけを実行し、任意のコマンドは受け付けない。引数は配列（シェルなし）、
//! コンソール窓なし、タイムアウトと出力上限あり、実行はアプリ内で直列化する。
//! 呼出しはユーザーの明示操作（確認済み）からだけ。`~/.codex` を変えうる操作（plugin・mcpの追加削除）の
//! 前後の照合・記録は呼出し側（`host/extensions.rs`、P3-5）が行う。この部品は実行と結果の取得だけを担う。
//! 引数の形は 0.160.0 の `--help` で確認した（出力の解析は各タスクで実測してから作る）。

use std::time::Duration;

use crate::exec::{run_bounded, RunError, RunSpec};

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
        let out = run_bounded(RunSpec { program: &self.exe, args: &args, cwd, env: &[], timeout: TIMEOUT, stdout_limit: OUTPUT_LIMIT }).await?;
        Ok(CliOutput { exit_code: out.exit_code, stdout: out.stdout, stderr: out.stderr })
    }
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
}
