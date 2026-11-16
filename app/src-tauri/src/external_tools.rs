//! Fixed argument vectors for explicitly configured local applications. No shell commands.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum OpenTarget { #[default] System, Editor, Terminal }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum EditorKind { #[default] VsCode, Cursor }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum TerminalKind { #[default] WindowsTerminal, PowerShell }

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalToolsSettings {
    pub editor: EditorKind,
    pub editor_executable: String,
    pub terminal: TerminalKind,
    pub terminal_executable: String,
}

/// 実行ファイルとして受け付ける形: ドライブ文字の絶対パス（割り当てたネットワークドライブを含む）か `\\server\share\…` の .exe。
/// ネットワーク上の実行ファイルは利用者が設定で明示したときだけ使う（設定画面で保存前に確認する）。
/// 触るのは利用者が開く操作をしたときだけで、保存時や自動の確認では触らない。`\\?\`・`\\.\` の形は受け付けない。
fn acceptable(value: &str) -> bool {
    (crate::pathref::is_drive_abs(value) || crate::pathref::is_unc(value))
        && !value.chars().any(|c| c.is_control())
        && Path::new(value).extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

fn executable(value: &str) -> Result<PathBuf, String> {
    if !acceptable(value) {
        return Err("実行ファイルは絶対パス（.exe）で指定してください".into());
    }
    let path = std::fs::canonicalize(value).map_err(|_| "設定した実行ファイルが見つかりません")?;
    if !path.is_file() {
        return Err("設定した実行ファイルが見つかりません".into());
    }
    Ok(path)
}

pub fn validate_settings(settings: &ExternalToolsSettings) -> Result<(), String> {
    // Existence is checked at launch; a moved/uninstalled app must not block unrelated settings saves.
    for value in [&settings.editor_executable, &settings.terminal_executable] {
        if !value.is_empty() && !acceptable(value) {
            return Err("外部ツールには絶対パス（.exe）を指定してください".into());
        }
    }
    Ok(())
}

fn arguments(settings: &ExternalToolsSettings, target: OpenTarget, path: &Path, line: Option<u32>) -> Result<Vec<String>, String> {
    if line.is_some_and(|n| n == 0 || n > 10_000_000) { return Err("行番号は1〜10000000で指定してください".into()); }
    let shown = crate::pathref::lexical_normalize(&path.to_string_lossy().trim_start_matches(r"\\?\"));
    match target {
        OpenTarget::Editor => Ok(vec!["--reuse-window".into(), "--goto".into(), format!("{shown}:{}", line.unwrap_or(1))]),
        OpenTarget::Terminal if line.is_some() => Err("ターミナルには行番号を指定できません".into()),
        OpenTarget::Terminal => Ok(match settings.terminal {
            TerminalKind::WindowsTerminal => vec!["-d".into(), shown],
            TerminalKind::PowerShell => vec!["-NoLogo".into(), "-NoExit".into()],
        }),
        OpenTarget::System => Err("外部ツールの対象を指定してください".into()),
    }
}

pub fn launch(settings: &ExternalToolsSettings, target: OpenTarget, path: &Path, line: Option<u32>) -> Result<(), String> {
    validate_settings(settings)?;
    if crate::pathref::is_network_path(&path.to_string_lossy()) { return Err("ネットワークパスは開きません".into()); }
    let meta = std::fs::metadata(path).map_err(|_| "対象のパスを確認できません")?;
    let exe = match target {
        OpenTarget::Editor if meta.is_file() => &settings.editor_executable,
        OpenTarget::Terminal if meta.is_dir() => &settings.terminal_executable,
        _ => return Err("エディタにはファイル、ターミナルには作業フォルダを指定してください".into()),
    };
    if exe.is_empty() { return Err("設定の「外部ツール」で実行ファイルを指定してください".into()); }
    let mut command = std::process::Command::new(executable(exe)?);
    command.args(arguments(settings, target, path, line)?);
    if target == OpenTarget::Terminal {
        command.current_dir(path);
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000010); // CREATE_NEW_CONSOLE (explicit user operation)
        }
    }
    command.spawn().map_err(|e| format!("外部ツールを起動できませんでした: {e}"))?;
    Ok(()) // Spawn confirmed; readiness of the other application is not observed.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_arguments_keep_shell_characters_in_a_single_argument() {
        let settings = ExternalToolsSettings::default();
        let path = Path::new(r"C:\work\a & $(echo bad)\file.rs");
        assert_eq!(arguments(&settings, OpenTarget::Editor, path, Some(42)).unwrap(),
            vec!["--reuse-window", "--goto", r"C:\work\a & $(echo bad)\file.rs:42"]);
        assert_eq!(arguments(&settings, OpenTarget::Terminal, Path::new(r"C:\work\a & b"), None).unwrap(),
            vec!["-d", r"C:\work\a & b"]);
        assert!(arguments(&settings, OpenTarget::Editor, path, Some(0)).is_err());
        assert!(arguments(&settings, OpenTarget::Terminal, path, Some(1)).is_err());
    }
    #[test]
    fn defaults_read_old_settings_and_invalid_executable_is_rejected() {
        assert_eq!(serde_json::from_str::<ExternalToolsSettings>("{}").unwrap(), ExternalToolsSettings::default());
        let mut settings = ExternalToolsSettings::default();
        for path in ["cmd.exe", r"C:\app.cmd", "C:\\bad\napp.exe", r"\\?\C:\app.exe", r"\\.\pipe\app.exe", r"\\?\UNC\server\share\app.exe"] {
            settings.editor_executable = path.into();
            assert!(validate_settings(&settings).is_err(), "{path}");
        }
        // 利用者が明示したネットワーク上の実行ファイル（UNC・割り当てドライブ）は受け付ける。
        for path in [r"C:\Program Files\Code\Code.exe", r"\\server\share\tools\app.exe", r"Z:\tools\app.exe"] {
            settings.editor_executable = path.into();
            assert!(validate_settings(&settings).is_ok(), "{path}");
        }
    }
}
