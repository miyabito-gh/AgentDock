//! 実機確認用の診断ログ（`<app data>/diag/diag.log`、サイズ上限つきローテート）。
//!
//! 記録するのは、メソッド名・JSONの**キー構造**（値は型名のみ）・件数・source値・版情報だけ。
//! 会話本文・自由文・認証情報・要求本文は記録しない（文字列の値は `str` と書くだけ）。

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// ログファイルの上限。超えたら `diag.log.1` へ退避し（古い退避は捨てる）、新しく始める。
pub const MAX_BYTES: u64 = 512 * 1024;
const MAX_DEPTH: usize = 5;
const MAX_KEYS: usize = 40;

struct Sink {
    path: PathBuf,
    lock: Mutex<()>,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// アプリデータ直下に `diag` を作る。失敗しても動作は続ける（ログなし）。
pub fn init(app_data_dir: &std::path::Path) {
    let dir = app_data_dir.join("diag");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = SINK.set(Sink { path: dir.join("diag.log"), lock: Mutex::new(()) });
    }
}

pub fn log_path() -> Option<PathBuf> {
    SINK.get().map(|s| s.path.clone())
}

pub fn rotate_needed(current_len: u64, incoming: u64) -> bool {
    current_len > 0 && current_len + incoming > MAX_BYTES
}

pub fn log(category: &str, message: &str) {
    let Some(s) = SINK.get() else { return };
    let _g = s.lock.lock().unwrap();
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let line = format!("{ms} [{category}] {message}\n");
    let len = std::fs::metadata(&s.path).map(|m| m.len()).unwrap_or(0);
    if rotate_needed(len, line.len() as u64) {
        let old = s.path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&s.path, &old);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&s.path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// JSONのキー構造だけを文字列にする。値は型名（str/num/bool/null）に置き換える。
pub fn shape(v: &Value) -> String {
    shape_at(v, 0)
}

fn shape_at(v: &Value, depth: usize) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "num".into(),
        Value::String(_) => "str".into(),
        Value::Array(a) => match a.first() {
            None => "[]".into(),
            Some(f) if depth < MAX_DEPTH => format!("[{}x {}]", a.len(), shape_at(f, depth + 1)),
            Some(_) => format!("[{}x ..]", a.len()),
        },
        Value::Object(o) => {
            if depth >= MAX_DEPTH {
                return "{..}".into();
            }
            let mut parts: Vec<String> = o.iter().take(MAX_KEYS).map(|(k, v)| format!("{k}:{}", shape_at(v, depth + 1))).collect();
            if o.len() > MAX_KEYS {
                parts.push("..".into());
            }
            format!("{{{}}}", parts.join(","))
        }
    }
}


/// thread.source の種別名だけ（ID・ニックネーム・任意文字列は記録しない）。customはCLIENT_NAMEとの一致だけ。
pub fn source_kind(src: Option<&Value>) -> String {
    const KNOWN: [&str; 6] = ["cli", "vscode", "exec", "appServer", "mcp", "unknown"];
    match src {
        None | Some(Value::Null) => "none".into(),
        Some(Value::String(s)) if KNOWN.contains(&s.as_str()) => s.clone(),
        Some(Value::String(_)) => "string(other)".into(),
        Some(Value::Object(o)) => {
            if let Some(c) = o.get("custom") {
                return format!("custom(matchesClient={})", c.as_str() == Some(crate::codex::process::CLIENT_NAME));
            }
            match o.get("subAgent") {
                Some(Value::String(s)) if ["review", "compact", "other", "memory_consolidation"].contains(&s.as_str()) => format!("subAgent/{s}"),
                Some(Value::Object(sub)) => match sub.keys().next().map(String::as_str) {
                    Some(k @ ("thread_spawn" | "review" | "compact" | "other")) => format!("subAgent/{k}"),
                    _ => "subAgent/other".into(),
                },
                Some(_) => "subAgent".into(),
                None => "object(other)".into(),
            }
        }
        Some(_) => "other".into(),
    }
}

/// userAgent から `<製品名>/<数値版>` の部分だけ。取れなければ `unparsed`。
pub fn user_agent_version(ua: Option<&str>) -> String {
    let Some(ua) = ua else { return "unparsed".into() };
    for tok in ua.split_whitespace() {
        let Some((name, ver)) = tok.split_once('/') else { continue };
        let name_ok = !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
        let ver_ok = ver.chars().next().is_some_and(|c| c.is_ascii_digit())
            && ver.matches('.').count() >= 1
            && ver.len() <= 24
            && ver.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if name_ok && ver_ok {
            return format!("{name}/{ver}");
        }
    }
    "unparsed".into()
}

/// エラーの種別だけ（メッセージ本文・パスは記録しない）。
pub fn error_kind(e: &crate::backend::backend::BackendError) -> String {
    use crate::backend::backend::BackendError as B;
    match e {
        B::NotConnected => "NotConnected".into(),
        B::Unsupported { .. } => "Unsupported".into(),
        B::Rejected { code, .. } => format!("Rejected(code={})", code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())),
        B::OutcomeUnknown { .. } => "OutcomeUnknown".into(),
        B::Protocol { .. } => "Protocol".into(),
        B::Io { .. } => "Io".into(),
    }
}

/// 版確認の結果の種別と、一致した版だけ（失敗時のメッセージは記録しない）。
pub fn version_check_kind(v: &crate::backend::model::VersionCheck) -> String {
    use crate::backend::model::VersionCheck as V;
    match v {
        V::Match { version } => format!("match({version})"),
        V::Mismatch { actual, .. } => format!("mismatch({})", actual.chars().take(24).collect::<String>()),
        V::Unknown { .. } => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shape_keeps_keys_and_drops_values() {
        let v = json!({"threadId": "secret-id", "item": {"type": "commandExecution", "command": "rm -rf secret", "exitCode": 0, "tags": ["a", "b"]}, "x": null});
        let s = shape(&v);
        assert!(s.contains("threadId:str") && s.contains("exitCode:num") && s.contains("tags:[2x str]") && s.contains("x:null"));
        assert!(!s.contains("secret") && !s.contains("rm -rf") && !s.contains("commandExecution"));
    }

    #[test]
    fn source_and_user_agent_are_reduced_to_kinds() {
        assert_eq!(source_kind(Some(&json!("vscode"))), "vscode");
        assert_eq!(source_kind(Some(&json!({"custom": "agentdock"}))), "custom(matchesClient=true)");
        assert_eq!(source_kind(Some(&json!({"custom": "someone-secret"}))), "custom(matchesClient=false)");
        assert_eq!(source_kind(Some(&json!({"subAgent": {"thread_spawn": {"parent_thread_id": "p1", "agent_nickname": "Euler"}}}))), "subAgent/thread_spawn");
        assert_eq!(source_kind(Some(&json!("weird-user-string"))), "string(other)");
        assert_eq!(user_agent_version(Some("codex_cli_rs/0.160.0 (Windows 10; x86_64) vscode/1.2")), "codex_cli_rs/0.160.0");
        assert_eq!(user_agent_version(Some("no version here C:/Users/me")), "unparsed");
        assert_eq!(user_agent_version(None), "unparsed");
    }

    #[test]
    fn shape_is_depth_limited() {
        let v = json!({"a": {"b": {"c": {"d": {"e": {"f": {"g": 1}}}}}}});
        assert!(shape(&v).contains("{..}"));
    }

    #[test]
    fn rotation_threshold() {
        assert!(!rotate_needed(0, MAX_BYTES + 1), "an empty file is never rotated");
        assert!(!rotate_needed(100, 100));
        assert!(rotate_needed(MAX_BYTES - 10, 11));
    }
}
