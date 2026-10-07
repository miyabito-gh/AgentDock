//! 外部コマンドの実行部品（`gitops` と `codex::cli` が共有）。段階③ DESIGN_P3 §2.3。
//!
//! 引数は配列で渡す（シェルを経由しない）。コンソール窓を出さない（`CREATE_NO_WINDOW`）。
//! タイムアウトと出力上限を必ず持つ。呼出し側は許可した操作の列挙型だけを公開し、任意のコマンドを受け付けない。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 標準エラーの保持上限（診断表示用。超えた分は捨てる）。
const STDERR_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("cannot start: {0}")]
    Spawn(String),
    /// 時間内に終わらなかった（プロセスは終了させた）。コマンドの結果は不明。
    #[error("timed out")]
    Timeout,
    /// 標準出力が上限を超えた（プロセスは終了させた）。
    #[error("output exceeded {0} bytes")]
    OutputTooLarge(usize),
    #[error("io: {0}")]
    Io(String),
}

/// 実行の指定。`env` は追加する環境変数。
pub struct RunSpec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(&'a str, &'a str)],
    pub timeout: Duration,
    pub stdout_limit: usize,
}

async fn read_capped<R: AsyncRead + Unpin>(mut r: R, limit: usize) -> Result<Vec<u8>, usize> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = r.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return Ok(buf);
        }
        if buf.len() + n > limit {
            return Err(limit);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn read_truncated<R: AsyncRead + Unpin>(mut r: R, keep: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = r.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return buf;
        }
        // 読み続ける（詰まらせない）が、保持は上限まで。
        let room = keep.saturating_sub(buf.len());
        buf.extend_from_slice(&chunk[..n.min(room)]);
    }
}

/// 実行して終わるまで待つ。タイムアウト・出力超過ではプロセスを終了させる（`kill_on_drop`）。
pub async fn run_bounded(spec: RunSpec<'_>) -> Result<RunOutput, RunError> {
    let mut cmd = Command::new(spec.program);
    cmd.args(spec.args);
    if let Some(cwd) = spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in spec.env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| RunError::Spawn(e.to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| RunError::Io("no stdout".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| RunError::Io("no stderr".into()))?;
    let limit = spec.stdout_limit;
    let err_task = tokio::spawn(read_truncated(stderr, STDERR_LIMIT));
    let work = async {
        match read_capped(stdout, limit).await {
            Err(l) => {
                let _ = child.kill().await;
                Err(RunError::OutputTooLarge(l))
            }
            Ok(out) => {
                let status = child.wait().await.map_err(|e| RunError::Io(e.to_string()))?;
                let err = err_task.await.unwrap_or_default();
                Ok(RunOutput { exit_code: status.code(), stdout: out, stderr: err })
            }
        }
    };
    match tokio::time::timeout(spec.timeout, work).await {
        Ok(r) => r,
        Err(_) => Err(RunError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn shell_echo(text: &str) -> (String, Vec<String>) {
        ("cmd".to_string(), vec!["/C".into(), format!("echo {text}")])
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn runs_with_args_array_and_captures_output() {
        let (program, args) = shell_echo("hello");
        let out = run_bounded(RunSpec { program: &program, args: &args, cwd: None, env: &[], timeout: Duration::from_secs(10), stdout_limit: 1024 }).await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn output_over_limit_is_an_error_not_truncation() {
        let (program, args) = shell_echo("0123456789");
        let r = run_bounded(RunSpec { program: &program, args: &args, cwd: None, env: &[], timeout: Duration::from_secs(10), stdout_limit: 4 }).await;
        assert_eq!(r, Err(RunError::OutputTooLarge(4)));
    }

    #[tokio::test]
    async fn missing_program_is_a_spawn_error() {
        let r = run_bounded(RunSpec { program: "agentdock-no-such-program", args: &[], cwd: None, env: &[], timeout: Duration::from_secs(5), stdout_limit: 16 }).await;
        assert!(matches!(r, Err(RunError::Spawn(_))));
    }
}
