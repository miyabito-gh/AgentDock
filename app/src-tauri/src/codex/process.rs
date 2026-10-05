//! codex.exeの起動・版確認・initialize握手（要件§3.1）。
//!
//! 起動失敗は [`LaunchError`]（agentの作業失敗とは別物）。成功前にagentを作らない。
//! 子プロセスの終了は「接続の終了」であり、実行中作業の停止確認ではない。

use super::rpc::{RpcClient, RpcEvent};
use crate::backend::backend::BackendError;
use crate::backend::model::{LaunchFailure, SourceId, VersionCheck};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// 対象版（要件§1.2）。
pub const TARGET_VERSION: &str = "0.160.0";
const STDERR_KEEP_LINES: usize = 500;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct LaunchError {
    pub kind: LaunchFailure,
    pub message: String,
}

impl LaunchError {
    fn new(kind: LaunchFailure, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

fn command(path: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(path);
    cmd.args(args);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// `codex.exe --version` の出力から版文字列（x.y.z[-suffix]）を取り出す。
pub fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.matches('.').count() >= 2)
        .map(|t| t.to_string())
}

/// 版を判定する。取得できなければ `Unknown`（`Match`にしない）。`expected`が空なら[`TARGET_VERSION`]。
pub fn classify_version(output: &str, expected: Option<&str>) -> VersionCheck {
    let expected = expected.unwrap_or(TARGET_VERSION);
    match parse_version(output) {
        Some(v) if v == expected => VersionCheck::Match { version: v },
        Some(v) => VersionCheck::Mismatch { expected: expected.to_string(), actual: v },
        None => VersionCheck::Unknown { message: format!("cannot parse version from {:?}", output.trim()) },
    }
}

/// `--version` を実行して版を確認する。起動不能は `LaunchError`、版違いは `Ok(Mismatch)`（警告のみ）。
pub async fn check_version(path: &str, expected: Option<&str>) -> Result<VersionCheck, LaunchError> {
    let mut cmd = command(path, &["--version"]);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = match tokio::time::timeout(VERSION_TIMEOUT, cmd.output()).await {
        Err(_) => return Err(LaunchError::new(LaunchFailure::VersionCheckFailed, "--version timed out")),
        Ok(Err(e)) => return Err(spawn_error(e)),
        Ok(Ok(o)) => o,
    };
    if !out.status.success() {
        return Err(LaunchError::new(LaunchFailure::VersionCheckFailed, format!("--version exited with {}", out.status)));
    }
    Ok(classify_version(&String::from_utf8_lossy(&out.stdout), expected))
}

fn spawn_error(e: std::io::Error) -> LaunchError {
    let kind = match e.kind() {
        std::io::ErrorKind::NotFound => LaunchFailure::NotFound,
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied => LaunchFailure::InvalidPath,
        _ => LaunchFailure::SpawnFailed,
    };
    LaunchError::new(kind, e.to_string())
}

/// 起動済みのApp Server接続。
pub struct CodexProcess {
    pub client: RpcClient,
    pub pid: Option<u32>,
    /// `initialize` 応答（userAgent・codexHome・platform等）。
    pub init_response: Value,
    stderr: Arc<Mutex<VecDeque<String>>>,
    child: Child,
}

impl CodexProcess {
    /// `codex app-server`（stdio）を起動し、initializeとinitializedまで行う。
    /// 握手失敗時は子プロセスを終了して `HandshakeFailed` を返す。
    /// 戻りの `Receiver` は通知・サーバー要求・切断の受け口（単一消費者）。
    pub async fn launch(
        path: &str,
        source: SourceId,
        enable_experimental: bool,
        event_capacity: usize,
    ) -> Result<(CodexProcess, mpsc::Receiver<RpcEvent>), LaunchError> {
        let mut cmd = command(path, &["app-server"]);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(spawn_error)?;
        let pid = child.id();
        let (stdin, stdout, stderr) = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(i), Some(o), Some(e)) => (i, o, e),
            _ => return Err(LaunchError::new(LaunchFailure::SpawnFailed, "stdio pipes unavailable")),
        };

        // stderrは別taskで収集（直近のみ保持。本文は画面・保存へ自動転送しない）。
        let buf = Arc::new(Mutex::new(VecDeque::new()));
        let buf2 = buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut b = buf2.lock().unwrap();
                if b.len() >= STDERR_KEEP_LINES {
                    b.pop_front();
                }
                b.push_back(l);
            }
        });

        let (client, rx) = RpcClient::start(stdout, stdin, source, event_capacity);
        let mut this = CodexProcess { client, pid, init_response: Value::Null, stderr: buf, child };
        match this.handshake(enable_experimental).await {
            Ok(resp) => {
                this.init_response = resp;
                Ok((this, rx))
            }
            Err(e) => {
                let _ = this.child.start_kill();
                let _ = this.child.wait().await;
                Err(LaunchError::new(LaunchFailure::HandshakeFailed, e.to_string()))
            }
        }
    }

    async fn handshake(&self, enable_experimental: bool) -> Result<Value, BackendError> {
        let params = json!({
            "clientInfo": { "name": "agentdock", "title": "AgentDock", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": enable_experimental, "requestAttestation": false },
        });
        let resp = self.client.request("initialize", params, Some(INIT_TIMEOUT)).await?;
        self.client.notify("initialized", Some(json!({}))).await?;
        Ok(resp)
    }

    /// 直近のstderr行（診断用）。
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr.lock().unwrap().iter().cloned().collect()
    }

    /// 接続を閉じる。stdinを閉じて`grace`待ち、残っていれば終了させる。
    /// 実行中作業の停止確認ではない（停止の照合は別経路）。
    pub async fn shutdown(&mut self, grace: Duration) {
        self.client.close_writer().await;
        if tokio::time::timeout(grace, self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_output() {
        assert_eq!(parse_version("codex-cli 0.160.0\n").as_deref(), Some("0.160.0"));
        assert_eq!(parse_version("codex-cli 0.159.0-alpha.3").as_deref(), Some("0.159.0-alpha.3"));
        assert_eq!(parse_version("garbage"), None);
    }

    #[test]
    fn classifies_version() {
        assert!(matches!(classify_version("codex-cli 0.160.0", None), VersionCheck::Match { .. }));
        assert!(matches!(classify_version("codex-cli 0.159.0-alpha.3", None), VersionCheck::Mismatch { .. }));
        assert!(matches!(classify_version("", None), VersionCheck::Unknown { .. }));
    }
}
