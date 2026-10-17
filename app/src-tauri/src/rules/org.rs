//! 組織実行（P-ORG-4c）の純粋ロジック: sandbox健全性チェックの判定と、config.toml の trust 追記の分類。
//!
//! どちらも読むだけ。config.toml は本文を受け取って比べるだけで、書かない・保存しない・ログに出さない。
//! 既存の `judge_config`／`ConfigCompare`（CLI管理操作用）には触れない。

use crate::backend::backend::BackendError;
use crate::backend::model::Known;
use crate::backend::parity::SandboxCheck;

// ───────────────────────────── sandbox健全性チェックの判定 ─────────────────────────────

/// 健全性チェックの結果。`Unhealthy` はワークフローを `Halted{SandboxUnhealthy}`（要対応）で止める根拠。
/// 自動再試行・自動回避はしない（利用者の「再開」でだけ出し直す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxVerdict {
    Healthy,
    /// 失敗・応答なし・非対応。成功とは扱わない。`reason` は診断用（標準エラー本文は含めない）。
    Unhealthy { reason: String },
}

/// 失敗時の案内文（DESIGN_ORG_P4 §5.2。ワークフロー画面の帯に出す）。
pub const SANDBOX_UNHEALTHY_NOTICE: &str = "Codex の sandbox を起動できないため、組織実行を始めていません。\n原因の候補: Codex のプラグイン（node_repl／computer-use）と sandbox の更新が競合している（os error 32）。詳しくは %USERPROFILE%\\.codex\\.sandbox\\ のログを確認してください。\n回避: 設定 →「Codex」→「sandbox の競合を回避する」をオンにし、AgentDock を再起動してから「再開」。";

/// `ParityOps::sandbox_check` の結果を判定する。応答なし（`OutcomeUnknown`）・拒否・非対応はすべて `Unhealthy`。
pub fn judge_sandbox(result: &Result<SandboxCheck, BackendError>) -> SandboxVerdict {
    let check = match result {
        Ok(c) => c,
        Err(BackendError::Unsupported { .. }) => return unhealthy("このバックエンドでは sandbox の確認ができません".to_string()),
        Err(BackendError::OutcomeUnknown { .. }) => return unhealthy("確認コマンドの結果が得られませんでした（応答なし）".to_string()),
        Err(e) => return unhealthy(format!("確認を実行できませんでした: {e}")),
    };
    match &check.readiness {
        Known::Value { value, .. } if value == "ready" => {}
        Known::Value { value, .. } => return unhealthy(format!("sandbox の準備状態が ready ではありません（{value}）")),
        _ => return unhealthy("sandbox の準備状態を読み取れませんでした".to_string()),
    }
    match &check.exit_code {
        Known::Value { value: 0, .. } => SandboxVerdict::Healthy,
        Known::Value { value, .. } => unhealthy(format!("確認コマンドが終了コード {value} で失敗しました")),
        _ => unhealthy("確認コマンドの終了コードを読み取れませんでした".to_string()),
    }
}

fn unhealthy(reason: String) -> SandboxVerdict {
    SandboxVerdict::Unhealthy { reason }
}

// ───────────────────────────── trust 追記の分類（J2-A） ─────────────────────────────

/// config.toml の前後の比較結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustCompare {
    Unchanged,
    /// 追加された行が `[projects.'<path>']` 見出しと `trust_level = "trusted"` だけ（削除・変更なし）。
    CodexTrustAppended { projects: Vec<String> },
    /// それ以外の変化がある（Codex本体・他の操作・利用者の編集の可能性。AgentDock は書いていない）。値は返さない。
    Other { changed_lines: usize },
    /// 前後どちらかを読めなかった。一致とは扱わない。
    Unverified { reason: String },
}

/// 前後の config.toml 本文を比べ、「Codex が trust を追記しただけ」かを分類する。
/// 行の多重集合の差で判定する（空行は無視、前後の空白は無視）。見出しは `[projects."…"]`・`[projects.'…']` の両方を受ける。
/// 追記と認めるのは、見出しと `trust_level = "trusted"` が同数で1件以上あり、ほかの追加・削除がないときだけ。
pub fn classify_trust_append(before: Option<&str>, after: Option<&str>) -> TrustCompare {
    let (Some(before), Some(after)) = (before, after) else {
        return TrustCompare::Unverified { reason: "config.toml を読み取れなかったため、前後を比べられません".to_string() };
    };
    let mut removed = lines_of(before);
    let mut added = Vec::new();
    for line in lines_of(after) {
        match removed.iter().position(|l| *l == line) {
            Some(i) => {
                removed.swap_remove(i);
            }
            None => added.push(line),
        }
    }
    if added.is_empty() && removed.is_empty() {
        return TrustCompare::Unchanged;
    }
    let other = |n: usize| TrustCompare::Other { changed_lines: n };
    if !removed.is_empty() {
        return other(added.len() + removed.len());
    }
    let mut projects = Vec::new();
    let mut trust_lines = 0usize;
    for line in &added {
        if let Some(p) = project_heading(line) {
            projects.push(p);
        } else if is_trusted_line(line) {
            trust_lines += 1;
        } else {
            return other(added.len());
        }
    }
    if projects.is_empty() || projects.len() != trust_lines {
        return other(added.len());
    }
    TrustCompare::CodexTrustAppended { projects }
}

/// 空行を除き、前後の空白を落とした行。
fn lines_of(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// `[projects."…"]`／`[projects.'…']` の見出しなら、中のパスを返す（基本文字列の `\\` は `\` に戻す）。
fn project_heading(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    let rest = inner.strip_prefix("projects.")?.trim();
    if let Some(p) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Some(p.replace("\\\\", "\\"));
    }
    rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')).map(str::to_string)
}

/// `trust_level = "trusted"`（空白・引用符の種類は問わない）。コメント付きは別の行として扱う（一致しない）。
fn is_trusted_line(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "trust_level=\"trusted\"" || compact == "trust_level='trusted'"
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "model = \"gpt-5\"\n\n[projects.'C:\\work\\a']\ntrust_level = \"trusted\"\n";

    fn check(readiness: Known<String>, exit: Known<i32>) -> SandboxCheck {
        SandboxCheck { readiness, exit_code: exit, stderr_head: Known::NotFetched }
    }

    #[test]
    fn trust_appended_only() {
        let after = format!("{BASE}\n[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::CodexTrustAppended { projects: vec!["C:\\work\\b".to_string()] });
        // 基本文字列の見出し（\\ は \ に戻る）、同一内容は Unchanged。
        let after = format!("{BASE}[projects.\"C:\\\\work\\\\c\"]\ntrust_level = \"trusted\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::CodexTrustAppended { projects: vec!["C:\\work\\c".to_string()] });
        assert_eq!(classify_trust_append(Some(BASE), Some(BASE)), TrustCompare::Unchanged);
    }

    #[test]
    fn heading_without_trust_value_is_other() {
        let after = format!("{BASE}[projects.'C:\\work\\b']\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { changed_lines: 1 });
        // 値だけの追加も追記とは認めない。
        let after = format!("{BASE}trust_level = \"trusted\"\n");
        assert!(matches!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { .. }));
    }

    #[test]
    fn other_key_changes_mixed_in_are_other() {
        let after = format!("{BASE}[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\napproval_policy = \"never\"\n");
        assert_eq!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { changed_lines: 3 });
        // untrusted は追記と認めない。
        let after = format!("{BASE}[projects.'C:\\work\\b']\ntrust_level = \"untrusted\"\n");
        assert!(matches!(classify_trust_append(Some(BASE), Some(&after)), TrustCompare::Other { .. }));
    }

    #[test]
    fn any_removal_is_other_even_with_an_append() {
        let after = "[projects.'C:\\work\\b']\ntrust_level = \"trusted\"\n";
        match classify_trust_append(Some(BASE), Some(after)) {
            TrustCompare::Other { changed_lines } => assert!(changed_lines >= 3),
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn unreadable_side_is_unverified_not_unchanged() {
        assert!(matches!(classify_trust_append(None, Some(BASE)), TrustCompare::Unverified { .. }));
        assert!(matches!(classify_trust_append(Some(BASE), None), TrustCompare::Unverified { .. }));
        assert!(matches!(classify_trust_append(None, None), TrustCompare::Unverified { .. }));
    }

    #[test]
    fn whitespace_and_blank_lines_are_ignored() {
        let after = "model = \"gpt-5\"\n\n\n  [projects.'C:\\work\\a']  \n  trust_level = \"trusted\"  \n";
        assert_eq!(classify_trust_append(Some(BASE), Some(after)), TrustCompare::Unchanged);
    }

    #[test]
    fn sandbox_healthy_only_when_ready_and_exit_zero() {
        let ok = check(Known::direct("ready".to_string()), Known::direct(0));
        assert_eq!(judge_sandbox(&Ok(ok)), SandboxVerdict::Healthy);
    }

    #[test]
    fn sandbox_failures_are_unhealthy_and_never_healthy() {
        let not_ready = check(Known::direct("updateRequired".to_string()), Known::NotFetched);
        assert!(matches!(judge_sandbox(&Ok(not_ready)), SandboxVerdict::Unhealthy { .. }));
        let bad_exit = check(Known::direct("ready".to_string()), Known::direct(1));
        assert!(matches!(judge_sandbox(&Ok(bad_exit)), SandboxVerdict::Unhealthy { .. }));
        // 終了コードを読めない（0 で代用しない）。
        let missing = check(Known::direct("ready".to_string()), Known::Missing);
        assert!(matches!(judge_sandbox(&Ok(missing)), SandboxVerdict::Unhealthy { .. }));
        let unreadable = check(Known::Missing, Known::NotFetched);
        assert!(matches!(judge_sandbox(&Ok(unreadable)), SandboxVerdict::Unhealthy { .. }));
        for e in [
            BackendError::OutcomeUnknown { message: "timeout".into() },
            BackendError::Unsupported { capability: "sandboxCheck".into() },
            BackendError::Rejected { code: None, message: "x".into() },
            BackendError::NotConnected,
        ] {
            assert!(matches!(judge_sandbox(&Err(e)), SandboxVerdict::Unhealthy { .. }));
        }
    }
}
