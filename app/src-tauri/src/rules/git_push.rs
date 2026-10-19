//! アプリからのプッシュの純粋ロジック（DESIGN_ORG_INTAKE §3）。I/Oをしない。
//!
//! - 送れない理由の判定、送る refspec の検査（強制系・削除・全ブランチ・タグは作れない）
//! - `git push --porcelain` の結果の分類（結果が分からなければ `Unknown`。成功に変換しない）
//! - URL の資格情報の伏せ字

use crate::backend::git_ops::*;

/// AgentDockが作業用に使うブランチの接頭辞（利用者のブランチではない）。
pub const INTERNAL_BRANCH_PREFIX: &str = "agentdock/";

/// プッシュ前に集めた事実。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushFacts {
    /// Git を読めた。
    pub readable: bool,
    /// 現在のブランチ名（detached なら `HEAD`）。
    pub branch: Option<String>,
    pub remote_count: usize,
    /// 送るコミットの総数。
    pub commit_total: u32,
}

/// 送れない理由。送れるなら None。
pub fn push_blocker(f: &PushFacts) -> Option<PushBlocker> {
    if !f.readable {
        return Some(PushBlocker::Unavailable);
    }
    match f.branch.as_deref() {
        None | Some("") => return Some(PushBlocker::Unavailable),
        Some("HEAD") => return Some(PushBlocker::Detached),
        Some(b) if b.starts_with(INTERNAL_BRANCH_PREFIX) => return Some(PushBlocker::InternalBranch),
        _ => {}
    }
    if f.remote_count == 0 {
        return Some(PushBlocker::NoRemote);
    }
    if f.commit_total == 0 {
        return Some(PushBlocker::NothingToPush);
    }
    None
}

pub fn blocker_message(b: PushBlocker) -> &'static str {
    match b {
        PushBlocker::NoRemote => "リモートがありません。GitHub 等でリポジトリを作り、`git remote add origin <URL>` を実行してから、もう一度開いてください（AgentDock はリモートを追加しません）",
        PushBlocker::Detached => "ブランチを指していない状態（detached HEAD）のため、プッシュできません",
        PushBlocker::InternalBranch => "AgentDock の作業用ブランチ（agentdock/…）は、プッシュできません。取り込み先の自分のブランチを開いてください",
        PushBlocker::NothingToPush => "送るコミットがありません",
        PushBlocker::Unavailable => "Git の状態を確認できないため、プッシュできません",
    }
}

fn valid_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// プッシュ先のリモート名として安全か（設定済みのリモート名だけ。URL・オプションを受けない）。
pub fn valid_remote_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 送るブランチ名として安全か（`agentdock/` で始まるものは断る）。
pub fn valid_push_branch(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.starts_with('+')
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.ends_with(".lock")
        && !s.ends_with('.')
        && !s.contains("..")
        && !s.contains("//")
        && !s.contains("@{")
        && !s.starts_with(INTERNAL_BRANCH_PREFIX)
        && s != "HEAD"
        && !s.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

/// 送る refspec（`<コミット>:refs/heads/<ブランチ>`）。先頭が `+`（強制）になる形・`:` 単独（削除）・コミットID以外の元は作れない。
pub fn push_refspec(sha: &str, branch: &str) -> Result<String, String> {
    if !valid_sha(sha) {
        return Err("sha".to_string());
    }
    if !valid_push_branch(branch) {
        return Err("branch".to_string());
    }
    Ok(format!("{sha}:refs/heads/{branch}"))
}

// ───────────────────────────── 結果の分類 ─────────────────────────────

/// `git push --porcelain` の結果から分類する。`timed_out` は時間切れ（出力は途中）。
pub fn classify_push(timed_out: bool, exit_code: Option<i32>, stdout: &str, stderr: &str, sent_sha: &str) -> PushResult {
    if timed_out {
        return PushResult::Unknown { message: "時間内に終わらなかったため、送れたか確認できません".to_string() };
    }
    let Some(code) = exit_code else {
        return PushResult::Unknown { message: "gitが異常終了したため、送れたか確認できません".to_string() };
    };
    // porcelain の行: `<flag>\t<from>:<to>\t<summary> (<reason>)`
    let lines: Vec<(char, &str)> = stdout.lines().filter_map(|l| l.split_once('\t').and_then(|(flag, rest)| flag.chars().next().filter(|_| flag.chars().count() == 1).map(|c| (c, rest)))).collect();
    if code == 0 {
        // 想定した形（通常の送信・新規・最新）だけを成功にする。強制・削除の行や、読めない出力は成功にしない。
        let ok = !lines.is_empty() && lines.iter().all(|(c, _)| matches!(c, ' ' | '*' | '='));
        return if ok {
            PushResult::Pushed { sha: sent_sha.to_string(), upstream_note: None }
        } else {
            PushResult::Unknown { message: "gitは正常終了しましたが、結果を読み取れないため、送れたか確認できません".to_string() }
        };
    }
    let lower = stderr.to_lowercase();
    for (c, rest) in &lines {
        if *c != '!' {
            continue;
        }
        let r = rest.to_lowercase();
        if r.contains("non-fast-forward") || r.contains("fetch first") || r.contains("stale info") {
            return PushResult::Rejected { reason: PushRejectReason::NonFastForward };
        }
        if r.contains("hook declined") {
            return PushResult::Rejected { reason: PushRejectReason::HookDeclined };
        }
    }
    if AUTH_MARKERS.iter().any(|m| lower.contains(m)) {
        return PushResult::AuthFailed;
    }
    if NETWORK_MARKERS.iter().any(|m| lower.contains(m)) {
        return PushResult::NetworkError;
    }
    let rejected_line = lines.iter().find(|(c, _)| *c == '!').map(|(_, r)| r.to_string());
    let text = rejected_line.or_else(|| stderr.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)).unwrap_or_else(|| format!("gitが終了コード {code} で終わりました"));
    PushResult::Rejected { reason: PushRejectReason::Other { text: text.chars().take(300).collect() } }
}

const AUTH_MARKERS: &[&str] = &[
    "authentication failed",
    "could not read username",
    "could not read password",
    "terminal prompts disabled",
    "permission denied (publickey",
    "invalid username or password",
    "returned error: 401",
    "returned error: 403",
    "http 401",
    "http 403",
    "access denied",
    "bad credentials",
];

const NETWORK_MARKERS: &[&str] = &[
    "could not resolve host",
    "failed to connect",
    "unable to connect",
    "could not connect",
    "connection timed out",
    "connection refused",
    "connection reset",
    "network is unreachable",
    "operation timed out",
    "unable to access",
    "ssl connect error",
    "the remote end hung up",
];

// ───────────────────────────── URL の伏せ字 ─────────────────────────────

/// `scheme://user:token@host/path` の `user:token@` を伏せる。`user@host:path`（ssh の短い形）はそのまま。
pub fn mask_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    match authority.rfind('@') {
        Some(i) => format!("{scheme}://{}{tail}", &authority[i + 1..]),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(branch: &str, remotes: usize, commits: u32) -> PushFacts {
        PushFacts { readable: true, branch: Some(branch.to_string()), remote_count: remotes, commit_total: commits }
    }

    #[test]
    fn blockers_are_checked_in_order() {
        assert_eq!(push_blocker(&facts("main", 1, 2)), None);
        assert_eq!(push_blocker(&PushFacts { readable: false, ..facts("main", 1, 2) }), Some(PushBlocker::Unavailable));
        assert_eq!(push_blocker(&PushFacts { branch: None, ..facts("main", 1, 2) }), Some(PushBlocker::Unavailable));
        assert_eq!(push_blocker(&facts("HEAD", 1, 2)), Some(PushBlocker::Detached));
        assert_eq!(push_blocker(&facts("agentdock/org/wf/t1", 1, 2)), Some(PushBlocker::InternalBranch));
        assert_eq!(push_blocker(&facts("main", 0, 2)), Some(PushBlocker::NoRemote));
        assert_eq!(push_blocker(&facts("main", 1, 0)), Some(PushBlocker::NothingToPush));
        assert!(blocker_message(PushBlocker::NoRemote).contains("git remote add origin"));
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn refspec_is_commit_to_a_plain_branch_only() {
        assert_eq!(push_refspec(SHA, "main").unwrap(), format!("{SHA}:refs/heads/main"));
        assert_eq!(push_refspec(SHA, "feature/x").unwrap(), format!("{SHA}:refs/heads/feature/x"));
        // force / delete / sources other than a full commit id
        assert!(push_refspec(&format!("+{SHA}"), "main").is_err());
        assert!(push_refspec("", "main").is_err());
        assert!(push_refspec("HEAD", "main").is_err());
        assert!(push_refspec("main", "main").is_err());
        assert!(push_refspec(&SHA[..39], "main").is_err());
        // destination tricks
        for bad in ["", "-f", "a:b", "+main", ":", "a b", "a..b", "x.lock", "/x", "x/", "HEAD", "agentdock/org/x", "a\\b", "a*", "a@{1}"] {
            assert!(push_refspec(SHA, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn remote_names_cannot_be_urls_or_options() {
        assert!(valid_remote_name("origin"));
        assert!(valid_remote_name("my-fork.2"));
        for bad in ["", "-f", "--mirror", "https://x/y", "a b", "a:b", "a/b"] {
            assert!(!valid_remote_name(bad), "{bad}");
        }
    }

    #[test]
    fn porcelain_success_lines_are_pushed_and_others_are_not() {
        let ok = "To https://github.com/u/r.git\n \trefs/heads/main:refs/heads/main\tabc..def\nDone\n";
        assert_eq!(classify_push(false, Some(0), ok, "", SHA), PushResult::Pushed { sha: SHA.into(), upstream_note: None });
        let new = "To x\n*\trefs/heads/main:refs/heads/main\t[new branch]\nDone\n";
        assert!(matches!(classify_push(false, Some(0), new, "", SHA), PushResult::Pushed { .. }));
        let same = "To x\n=\trefs/heads/main:refs/heads/main\t[up to date]\nDone\n";
        assert!(matches!(classify_push(false, Some(0), same, "", SHA), PushResult::Pushed { .. }));
        // forced or deleted lines, empty or unreadable output never count as a clean success
        let forced = "To x\n+\trefs/heads/main:refs/heads/main\tabc...def (forced update)\nDone\n";
        assert!(matches!(classify_push(false, Some(0), forced, "", SHA), PushResult::Unknown { .. }));
        assert!(matches!(classify_push(false, Some(0), "", "", SHA), PushResult::Unknown { .. }));
        assert!(matches!(classify_push(false, Some(0), "garbled", "", SHA), PushResult::Unknown { .. }));
    }

    #[test]
    fn rejections_are_classified_by_reason() {
        let nff = "To x\n!\trefs/heads/main:refs/heads/main\t[rejected] (non-fast-forward)\nDone\n";
        assert_eq!(classify_push(false, Some(1), nff, "error: failed to push some refs", SHA), PushResult::Rejected { reason: PushRejectReason::NonFastForward });
        let fetch_first = "To x\n!\trefs/heads/main:refs/heads/main\t[rejected] (fetch first)\nDone\n";
        assert_eq!(classify_push(false, Some(1), fetch_first, "", SHA), PushResult::Rejected { reason: PushRejectReason::NonFastForward });
        let hook = "To x\n!\trefs/heads/main:refs/heads/main\t[remote rejected] (pre-receive hook declined)\nDone\n";
        assert_eq!(classify_push(false, Some(1), hook, "", SHA), PushResult::Rejected { reason: PushRejectReason::HookDeclined });
        let other = "To x\n!\trefs/heads/main:refs/heads/main\t[remote rejected] (protected branch)\nDone\n";
        assert!(matches!(classify_push(false, Some(1), other, "", SHA), PushResult::Rejected { reason: PushRejectReason::Other { text } } if text.contains("protected branch")));
        // a failing pre-push hook has no porcelain line: shown with the first stderr line
        assert!(matches!(classify_push(false, Some(1), "", "\nhook says no\nerror: failed", SHA), PushResult::Rejected { reason: PushRejectReason::Other { text } } if text == "hook says no"));
    }

    #[test]
    fn auth_and_network_failures_are_told_apart_and_unknown_is_never_success() {
        let auth = "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/u/r.git/'";
        assert_eq!(classify_push(false, Some(128), "", auth, SHA), PushResult::AuthFailed);
        assert_eq!(classify_push(false, Some(128), "", "fatal: could not read Username for 'https://github.com': terminal prompts disabled", SHA), PushResult::AuthFailed);
        assert_eq!(classify_push(false, Some(128), "", "fatal: unable to access 'https://x/': The requested URL returned error: 403", SHA), PushResult::AuthFailed);
        assert_eq!(classify_push(false, Some(128), "", "fatal: unable to access 'https://x/': Could not resolve host: github.com", SHA), PushResult::NetworkError);
        assert!(matches!(classify_push(true, None, "", "", SHA), PushResult::Unknown { .. }));
        assert!(matches!(classify_push(false, None, "", "", SHA), PushResult::Unknown { .. }));
        assert!(matches!(classify_push(false, Some(128), "", "", SHA), PushResult::Rejected { reason: PushRejectReason::Other { .. } }));
    }

    #[test]
    fn mask_url_drops_credentials_only() {
        assert_eq!(mask_url("https://user:tok3n@github.com/u/r.git"), "https://github.com/u/r.git");
        assert_eq!(mask_url("https://tok3n@github.com/u/r.git"), "https://github.com/u/r.git");
        assert_eq!(mask_url("https://github.com/u/r.git"), "https://github.com/u/r.git");
        assert_eq!(mask_url("git@github.com:u/r.git"), "git@github.com:u/r.git");
        assert_eq!(mask_url("ssh://git@host:22/u/r.git"), "ssh://host:22/u/r.git");
        assert_eq!(mask_url("C:/repos/r.git"), "C:/repos/r.git");
        // an at-sign in the path is not credentials
        assert_eq!(mask_url("https://host/u/r@x.git"), "https://host/u/r@x.git");
    }
}
