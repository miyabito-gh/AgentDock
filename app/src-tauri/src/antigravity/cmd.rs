//! agy（Antigravity CLI）の起動引数・実行ファイル解決・出力解析（純粋。I/Oは実行ファイルの存在確認だけ）。
//!
//! 1ターン=1プロセスの print モード（`agy -p ... --output-format stream-json`）。ACP は使わない。

use crate::backend::backend::PermissionPreset;
use crate::backend::model::{ModelInfo, VersionCheck};
use std::path::{Path, PathBuf};

/// 選べるモデルは Gemini 系だけ（claude-/gpt-oss- 等は扱わない）。
pub const MODEL_PREFIX: &str = "gemini-";
/// 一覧にあれば既定にするモデル。無ければ一覧の先頭。
pub const PREFERRED_DEFAULT_MODEL: &str = "gemini-3.8-flash-medium";
/// 実機で確認した版（major.minor）。
const CHECKED_MAJOR_MINOR: (u64, u64) = (1, 5);

pub fn is_supported_model(id: &str) -> bool {
    id.len() > MODEL_PREFIX.len() && id.starts_with(MODEL_PREFIX)
}

// ───────────────────────────── 実行ファイル ─────────────────────────────

fn is_agy_file_name(path: &Path) -> bool {
    path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).is_some_and(|n| n == "agy" || n == "agy.exe")
}

/// agy の場所。設定がファイル名 `agy`/`agy.exe` の実在パスならそれ、それ以外は無視して
/// `%LOCALAPPDATA%\agy\bin\agy.exe`、無ければ PATH 上の `agy`。
pub fn resolve_executable_in(configured: Option<&str>, local_app_data: Option<&Path>) -> String {
    if let Some(p) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let path = Path::new(p);
        if is_agy_file_name(path) && path.is_file() {
            return p.to_string();
        }
    }
    if let Some(local) = local_app_data {
        let candidate: PathBuf = local.join("agy").join("bin").join("agy.exe");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "agy".to_string()
}

pub fn resolve_executable(configured: Option<&str>) -> String {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    resolve_executable_in(configured, local.as_deref())
}

// ───────────────────────────── 版 ─────────────────────────────

/// `agy --version` の出力（`1.5.16` のように版だけ）から版文字列を取り出す。
pub fn parse_version(output: &str) -> Option<String> {
    output.split_whitespace().find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.')).map(str::to_string)
}

fn major_minor(version: &str) -> Option<(u64, u64)> {
    let mut it = version.split(['.', '-']);
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// 実機確認済みの 1.5.x なら `Checked(live)`、他は警告つきの `Mismatch`（接続は止めない）。読めなければ `Unknown`（一致にしない）。
pub fn classify_version(output: &str) -> VersionCheck {
    let Some(v) = parse_version(output) else {
        return VersionCheck::Unknown { message: format!("cannot parse version from {:?}", output.trim()) };
    };
    match major_minor(&v) {
        Some(mm) if mm == CHECKED_MAJOR_MINOR => VersionCheck::Checked { version: v, basis: "live".into() },
        Some(mm) => {
            let direction = if mm > CHECKED_MAJOR_MINOR { "newer" } else { "older" };
            VersionCheck::Mismatch { expected: format!("{}.{}.x", CHECKED_MAJOR_MINOR.0, CHECKED_MAJOR_MINOR.1), actual: v, direction: direction.into() }
        }
        None => VersionCheck::Mismatch { expected: format!("{}.{}.x", CHECKED_MAJOR_MINOR.0, CHECKED_MAJOR_MINOR.1), actual: v, direction: "unknown".into() },
    }
}

// ───────────────────────────── 起動引数 ─────────────────────────────

/// 権限の対応。読取り専用系は plan、作業領域への書込み系は accept-edits、フルアクセスは確認を全て省く。
pub fn permission_args(p: PermissionPreset) -> &'static [&'static str] {
    match p {
        PermissionPreset::ReadOnly | PermissionPreset::ReadOnlyUntrusted | PermissionPreset::ReadOnlyNoPrompt => &["--mode", "plan"],
        PermissionPreset::WorkspaceWriteOnRequest | PermissionPreset::WorkspaceWriteUntrusted => &["--mode", "accept-edits"],
        PermissionPreset::FullAccess => &["--dangerously-skip-permissions"],
    }
}

pub struct TurnLaunch<'a> {
    pub text: &'a str,
    pub model: &'a str,
    /// 2回目以降のターンで渡す agy の会話ID。
    pub conversation_id: Option<&'a str>,
    pub permission: PermissionPreset,
    /// プライマリ（プロセスの cwd）を除く追加の作業フォルダ。
    pub extra_dirs: &'a [String],
}

/// 1ターン分の `agy` の引数（実行ファイル名は含めない）。
pub fn turn_args(l: &TurnLaunch) -> Vec<String> {
    let mut a: Vec<String> = vec!["-p".into(), l.text.into(), "--output-format".into(), "stream-json".into(), "--model".into(), l.model.into()];
    if let Some(c) = l.conversation_id {
        a.push("--conversation".into());
        a.push(c.into());
    }
    a.extend(permission_args(l.permission).iter().map(|s| (*s).to_string()));
    for d in l.extra_dirs {
        a.push("--add-dir".into());
        a.push(d.clone());
    }
    a
}

// ───────────────────────────── agy models ─────────────────────────────

/// `agy models` の標準出力（「Fetching available models...」の後に `<id>\t<表示名>` が1行ずつ）から、Gemini 系だけを取り出す。
/// 既定は `gemini-3.8-flash-medium`、無ければ最初の1件。
pub fn parse_models(stdout: &str) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = Vec::new();
    for line in stdout.lines() {
        let Some((id, name)) = line.split_once('\t') else { continue };
        let (id, name) = (id.trim(), name.trim());
        if !is_supported_model(id) || id.contains(char::is_whitespace) || out.iter().any(|m| m.id == id) {
            continue;
        }
        out.push(ModelInfo {
            id: id.to_string(),
            display_name: if name.is_empty() { id.to_string() } else { name.to_string() },
            description: None,
            efforts: Vec::new(),
            default_effort: None,
            is_default: false,
            hidden: false,
            input_kinds: Vec::new(),
            speed_tiers: Vec::new(),
            default_speed_tier: None,
        });
    }
    let default = out.iter().position(|m| m.id == PREFERRED_DEFAULT_MODEL).or(if out.is_empty() { None } else { Some(0) });
    if let Some(i) = default {
        out[i].is_default = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_maps_to_agy_flags() {
        assert_eq!(permission_args(PermissionPreset::ReadOnly), ["--mode", "plan"]);
        assert_eq!(permission_args(PermissionPreset::ReadOnlyUntrusted), ["--mode", "plan"]);
        assert_eq!(permission_args(PermissionPreset::ReadOnlyNoPrompt), ["--mode", "plan"]);
        assert_eq!(permission_args(PermissionPreset::WorkspaceWriteOnRequest), ["--mode", "accept-edits"]);
        assert_eq!(permission_args(PermissionPreset::WorkspaceWriteUntrusted), ["--mode", "accept-edits"]);
        assert_eq!(permission_args(PermissionPreset::FullAccess), ["--dangerously-skip-permissions"]);
    }

    #[test]
    fn first_turn_has_no_conversation_and_later_turns_resume_it() {
        let extra = vec!["C:\\b".to_string(), "C:\\c".to_string()];
        let first = turn_args(&TurnLaunch { text: "hi", model: "gemini-x", conversation_id: None, permission: PermissionPreset::ReadOnly, extra_dirs: &[] });
        assert_eq!(first, ["-p", "hi", "--output-format", "stream-json", "--model", "gemini-x", "--mode", "plan"]);
        assert!(!first.iter().any(|a| a == "--conversation" || a == "--add-dir"));
        let later = turn_args(&TurnLaunch { text: "again", model: "gemini-x", conversation_id: Some("9db8"), permission: PermissionPreset::FullAccess, extra_dirs: &extra });
        assert_eq!(
            later,
            ["-p", "again", "--output-format", "stream-json", "--model", "gemini-x", "--conversation", "9db8", "--dangerously-skip-permissions", "--add-dir", "C:\\b", "--add-dir", "C:\\c"]
        );
    }

    #[test]
    fn only_gemini_models_are_supported() {
        assert!(is_supported_model("gemini-3.8-flash-low"));
        assert!(!is_supported_model("gemini-"));
        assert!(!is_supported_model("claude-sonnet-4-6"));
        assert!(!is_supported_model("gpt-oss-120b-medium"));
        assert!(!is_supported_model(""));
    }

    const MODELS_OUT: &str = "Fetching available models...\n\
        gemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n\
        gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)\n\
        claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n\
        gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
        gemini-3.8-flash-medium\tduplicate\n\
        not a model line\n";

    #[test]
    fn models_output_keeps_gemini_only_and_prefers_the_known_default() {
        let m = parse_models(MODELS_OUT);
        assert_eq!(m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["gemini-3.8-flash-low", "gemini-3.8-flash-medium"]);
        assert_eq!(m[1].display_name, "Gemini 3.8 Flash (Medium)");
        assert_eq!(m.iter().filter(|x| x.is_default).map(|x| x.id.as_str()).collect::<Vec<_>>(), ["gemini-3.8-flash-medium"]);
        assert!(m.iter().all(|x| x.efforts.is_empty() && x.input_kinds.is_empty() && !x.hidden));
    }

    #[test]
    fn default_falls_back_to_the_first_gemini_model() {
        let m = parse_models("Fetching available models...\ngemini-9-pro\tGemini 9 Pro\ngemini-9-flash\tGemini 9 Flash\n");
        assert_eq!(m.iter().filter(|x| x.is_default).map(|x| x.id.as_str()).collect::<Vec<_>>(), ["gemini-9-pro"]);
        assert!(parse_models("Fetching available models...\nclaude-x\tClaude\n").is_empty());
        assert!(parse_models("").is_empty());
    }

    #[test]
    fn version_is_parsed_and_classified() {
        assert_eq!(parse_version("1.5.16\n").as_deref(), Some("1.5.16"));
        assert_eq!(parse_version("agy version 1.5.16"), Some("1.5.16".to_string()));
        assert_eq!(parse_version("no version"), None);
        assert!(matches!(classify_version("1.5.16\n"), VersionCheck::Checked { version, basis } if version == "1.5.16" && basis == "live"));
        assert!(matches!(classify_version("1.6.0"), VersionCheck::Mismatch { direction, .. } if direction == "newer"));
        assert!(matches!(classify_version("1.4.9"), VersionCheck::Mismatch { direction, .. } if direction == "older"));
        assert!(matches!(classify_version("garbage"), VersionCheck::Unknown { .. }));
    }

    #[test]
    fn configured_executable_is_used_only_when_it_is_an_existing_agy_file() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-exe");
        let agy = dir.join("agy.exe");
        let other = dir.join("codex.exe");
        std::fs::write(&agy, b"").unwrap();
        std::fs::write(&other, b"").unwrap();
        assert_eq!(resolve_executable_in(Some(agy.to_str().unwrap()), None), agy.to_str().unwrap());
        // 別名の実行ファイルは無視する（Codexのパスが渡されても使わない）。
        assert_eq!(resolve_executable_in(Some(other.to_str().unwrap()), None), "agy");
        // 実在しないパスは無視する。
        assert_eq!(resolve_executable_in(Some(dir.join("nope").join("agy.exe").to_str().unwrap()), None), "agy");
        // 設定が無ければ %LOCALAPPDATA%\agy\bin\agy.exe。
        let local = dir.join("local");
        std::fs::create_dir_all(local.join("agy").join("bin")).unwrap();
        std::fs::write(local.join("agy").join("bin").join("agy.exe"), b"").unwrap();
        assert_eq!(resolve_executable_in(None, Some(&local)), local.join("agy").join("bin").join("agy.exe").to_string_lossy());
        assert_eq!(resolve_executable_in(None, Some(&dir.join("missing"))), "agy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
