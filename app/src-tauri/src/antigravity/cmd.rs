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

/// 本文が `-` で始まると、agy は（`-p=` 形式でも）フラグ・誤指定として扱い、依頼を実行しない（実機で確認:
/// `-p "--version"` は版を表示するだけ）。先頭に空白を1つ足して本文として渡す。内容は変えない。
fn safe_prompt(text: &str) -> String {
    if text.starts_with('-') {
        format!(" {text}")
    } else {
        text.to_string()
    }
}

/// 1ターン分の `agy` の引数（実行ファイル名は含めない）。
pub fn turn_args(l: &TurnLaunch) -> Vec<String> {
    let mut a: Vec<String> = vec!["-p".into(), safe_prompt(l.text), "--output-format".into(), "stream-json".into(), "--model".into(), l.model.into()];
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

// ───────────────────────────── コマンドライン長 ─────────────────────────────

/// Windows の `CreateProcess` に渡せるコマンドライン全体の長さの上限（UTF-16 の単位数。仕様は 32,767 で、末尾の NUL を除くと 32,766）。
/// agy は本文を引数（`-p <本文>`）で受け取るため、本文が長いとこの上限で起動できない（OS エラー 206）。
pub const WINDOWS_COMMAND_LINE_MAX_UNITS: usize = 32_766;

/// Rust の標準ライブラリが Windows で1つの引数をコマンドラインへ組み立てたときの UTF-16 の単位数（引用符・バックスラッシュの重ね書きを含む）。
fn quoted_units(arg: &str, force_quotes: bool) -> usize {
    let quote = force_quotes || arg.is_empty() || arg.contains([' ', '\t']);
    let mut n = if quote { 2 } else { 0 };
    let mut backslashes = 0usize;
    for u in arg.encode_utf16() {
        if u == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if u == u16::from(b'"') {
                // 引用符の前には、直前のバックスラッシュの2倍＋エスケープの1つが付く。
                n += backslashes + 1;
            }
            backslashes = 0;
        }
        n += 1;
    }
    if quote {
        n += backslashes;
    }
    n
}

/// 実行ファイルと引数から組み立てられるコマンドライン全体の長さ（UTF-16 の単位数）。実行ファイル名は常に引用符で囲まれる。
pub fn command_line_units(exe: &str, args: &[String]) -> usize {
    quoted_units(exe, true) + args.iter().map(|a| 1 + quoted_units(a, false)).sum::<usize>()
}

/// 本文が長すぎて agy を起動できない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLineTooLong {
    /// 本文の字数。
    pub text_chars: usize,
    /// この送信（モデル・権限・作業フォルダ等の引数を含む）で渡せる本文のおよその字数。
    pub room_chars: usize,
}

impl CommandLineTooLong {
    /// 利用者向けの理由。何を・なぜ・どうすればよいかを示す。
    pub fn message(&self) -> String {
        format!(
            "本文が長すぎるため送っていません（{}字）。Antigravity は本文をコマンドラインで受け取り、Windows の長さの上限があるため、いまの設定では約{}字までです。本文を短くするか、分けて送ってください。",
            self.text_chars, self.room_chars
        )
    }
}

/// 起動する前に、コマンドラインが上限に収まるか調べる。超えるなら何も起動せず理由を返す。
pub fn check_command_line(exe: &str, l: &TurnLaunch) -> Result<(), CommandLineTooLong> {
    let used = command_line_units(exe, &turn_args(l));
    if used <= WINDOWS_COMMAND_LINE_MAX_UNITS {
        return Ok(());
    }
    // 本文を空にしたときの長さ（空の本文も引用符の2字は数える）との差が、本文に使える量。
    let empty = command_line_units(exe, &turn_args(&TurnLaunch { text: "", ..*l }));
    let room = (WINDOWS_COMMAND_LINE_MAX_UNITS + 2).saturating_sub(empty);
    Err(CommandLineTooLong { text_chars: l.text.chars().count(), room_chars: room / 100 * 100 })
}

/// 受付・キューが登録の前に使う早期の検査。送り先の実行ファイル・モデル・追加フォルダを知らないので、最も短い構成で見積もる
/// （この検査を通らない本文は、実際の構成でも必ず送れない。通っても、実際の送信側で改めて検査される）。
pub fn precheck_text(text: &str) -> Result<(), CommandLineTooLong> {
    check_command_line("agy", &TurnLaunch { text, model: "gemini-x", conversation_id: None, permission: PermissionPreset::ReadOnly, extra_dirs: &[] })
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
        // 先頭が '-' の本文は、フラグと誤認されないよう先頭に空白を足す（それ以外は変えない）。
        assert_eq!(safe_prompt("--version"), " --version");
        assert_eq!(safe_prompt("- 箇条書き"), " - 箇条書き");
        assert_eq!(safe_prompt("本文 -x"), "本文 -x");
        assert!(!first.iter().any(|a| a == "--conversation" || a == "--add-dir"));
        let later = turn_args(&TurnLaunch { text: "again", model: "gemini-x", conversation_id: Some("9db8"), permission: PermissionPreset::FullAccess, extra_dirs: &extra });
        assert_eq!(
            later,
            ["-p", "again", "--output-format", "stream-json", "--model", "gemini-x", "--conversation", "9db8", "--dangerously-skip-permissions", "--add-dir", "C:\\b", "--add-dir", "C:\\c"]
        );
    }

    fn launch(text: &str) -> TurnLaunch<'_> {
        TurnLaunch { text, model: "gemini-x", conversation_id: None, permission: PermissionPreset::ReadOnly, extra_dirs: &[] }
    }

    #[test]
    fn command_line_units_count_quoting_the_way_the_standard_library_builds_it() {
        let n = |args: &[&str]| command_line_units("agy", &args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        // 実行ファイル名は常に引用符つき（"agy" = 5字）。引数ごとに区切りの空白が1つ。
        assert_eq!(n(&[]), 5);
        assert_eq!(n(&["-p"]), 5 + 1 + 2);
        // 空白を含むと引用符で囲まれる（+2）。空の引数も同様。
        assert_eq!(n(&["a b"]), 5 + 1 + 3 + 2);
        assert_eq!(n(&[""]), 5 + 1 + 2);
        // 引用符は `\"` になる（+1）。引用符の前のバックスラッシュは2倍になる。
        assert_eq!(n(&["a\"b"]), 5 + 1 + 3 + 1);
        assert_eq!(n(&["a\\\"b"]), 5 + 1 + 4 + 2);
        // 引用符で囲まれた引数の末尾のバックスラッシュは、閉じ引用符の前で2倍になる。
        assert_eq!(n(&["a b\\"]), 5 + 1 + 4 + 2 + 1);
        // UTF-16 の単位で数える（BMP 外の文字は2単位、日本語は1単位）。
        assert_eq!(n(&["あ"]), 5 + 1 + 1);
        assert_eq!(n(&["😀"]), 5 + 1 + 2);
    }

    #[test]
    fn a_text_within_the_limit_passes_and_one_unit_over_is_refused_with_the_room_left() {
        let exe = "C:\\Users\\x\\AppData\\Local\\agy\\bin\\agy.exe";
        // 本文以外の引数を含め、ちょうど上限に収まる長さ（空白のない本文＝引用符なし）。
        let base = command_line_units(exe, &turn_args(&launch("")));
        let fit = WINDOWS_COMMAND_LINE_MAX_UNITS - (base - 2); // 空の本文の引用符2字を除く
        let ok = "a".repeat(fit);
        assert_eq!(command_line_units(exe, &turn_args(&launch(&ok))), WINDOWS_COMMAND_LINE_MAX_UNITS);
        assert_eq!(check_command_line(exe, &launch(&ok)), Ok(()));
        let over = "a".repeat(fit + 1);
        let e = check_command_line(exe, &launch(&over)).unwrap_err();
        assert_eq!(e.text_chars, fit + 1);
        // 渡せる量の見積もりは百字単位に切り捨てる。上限に収まる最大の本文（fit）以下で、百字以内の差。
        assert!(e.room_chars <= fit && fit - e.room_chars < 100, "room={} fit={fit}", e.room_chars);
        let m = e.message();
        assert!(m.contains("本文が長すぎるため送っていません") && m.contains(&format!("{}字", fit + 1)) && m.contains(&format!("約{}字", e.room_chars)), "{m}");
        assert!(!m.contains("os error"));
    }

    #[test]
    fn the_early_check_is_the_loosest_one_and_a_bigger_launch_is_refused_sooner() {
        // 早期の検査は最も短い構成で見積もるので、早期の検査で止まる本文は実際の構成でも必ず止まる。
        let long_path = "C:\\Users\\someone\\AppData\\Local\\agy\\bin\\agy.exe";
        let dirs = vec!["C:\\work\\one".to_string(), "C:\\work\\two".to_string()];
        let text = "あ".repeat(WINDOWS_COMMAND_LINE_MAX_UNITS - 80);
        assert!(precheck_text(&text).is_ok());
        let real = TurnLaunch { text: &text, model: "gemini-3.8-flash-medium", conversation_id: Some("9db8c0de-1111-2222-3333-444455556666"), permission: PermissionPreset::FullAccess, extra_dirs: &dirs };
        assert!(check_command_line(long_path, &real).is_err(), "実際の構成では収まらない");
        assert!(precheck_text(&"あ".repeat(WINDOWS_COMMAND_LINE_MAX_UNITS)).is_err());
        assert!(precheck_text("短い依頼").is_ok());
    }

    /// 上限が実際の OS の挙動と一致する（Windows）。計算上ちょうど上限の引数は起動でき、1つ超えると OS エラー 206 で起動できない。
    /// 起動するのはこのテスト自身の実行ファイル（`--list` と存在しない名前の絞り込みだけで、何も実行せず終わる）。
    #[cfg(windows)]
    #[test]
    fn the_computed_limit_matches_what_windows_actually_accepts() {
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let args_at = |units: usize| -> Vec<String> {
            // 1つ目の引数 `--list`（区切りの空白を含め 7）と、区切りの空白1つ＋ a の繰り返し。
            let n = units - command_line_units(&exe, &[]) - 7 - 1;
            vec!["--list".to_string(), "a".repeat(n)]
        };
        let fit = args_at(WINDOWS_COMMAND_LINE_MAX_UNITS);
        assert_eq!(command_line_units(&exe, &fit), WINDOWS_COMMAND_LINE_MAX_UNITS);
        let ok = std::process::Command::new(&exe).args(&fit).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        assert!(ok.is_ok(), "上限ちょうどは起動できる: {ok:?}");
        let over = args_at(WINDOWS_COMMAND_LINE_MAX_UNITS + 1);
        let err = std::process::Command::new(&exe).args(&over).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap_err();
        assert_eq!(err.raw_os_error(), Some(206), "上限を1つ超えると起動できない: {err}");
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
