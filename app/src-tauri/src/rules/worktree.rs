//! Git worktree（P3-7、`app/DESIGN_P3.md` §1 #14）の純粋ロジック。I/Oをしない。
//!
//! - `git worktree list --porcelain` の解析
//! - 置き場所のパス・ブランチ名の組立て（チャット名の安全化、短いID）と、置き場所配下の確認（字句）
//! - 削除の可否判定（AgentDockが作った記録のあるものだけ。使用中・未コミット変更・Git上の実在で止める）

use super::baseline::norm_path;

/// `git worktree list --porcelain` の1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedWorktree {
    pub path: String,
    pub head: Option<String>,
    /// `refs/heads/` を除いたブランチ名。detached・bare なら None。
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// `worktree list --porcelain` の出力を解析する。空行で区切られた塊ごとに1件。先頭がメインの作業ツリー。
/// `worktree <path>` の行がない塊は無視する（推定で補わない）。
pub fn parse_worktree_list(out: &str) -> Vec<ListedWorktree> {
    let mut v: Vec<ListedWorktree> = Vec::new();
    let mut cur: Option<ListedWorktree> = None;
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        if line.is_empty() {
            v.extend(cur.take());
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            v.extend(cur.take());
            cur = Some(ListedWorktree { path: path.to_string(), head: None, branch: None, detached: false, bare: false, locked: false, prunable: false });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "HEAD" if !rest.is_empty() => w.head = Some(rest.to_string()),
            "branch" if !rest.is_empty() => w.branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string()),
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    v.extend(cur.take());
    v
}

const SLUG_MAX_CHARS: usize = 30;

/// チャット名をブランチ名の一部として安全にする。英数字（日本語などを含む）は小文字で残し、それ以外は `-` にして連続をまとめる。
/// 空になれば `chat`。gitの参照名で問題になる文字（`.`・`@`・`/`・空白など）は残らない。
pub fn safe_branch_slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
        if out.chars().count() >= SLUG_MAX_CHARS {
            break;
        }
    }
    let t = out.trim_matches('-');
    if t.is_empty() {
        "chat".to_string()
    } else {
        t.to_string()
    }
}

/// 短いID（6桁の16進数）。同じ種からは同じ値。
pub fn short_id(seed: u64) -> String {
    // 連番に近い種でも散らばるように混ぜる（暗号用途ではない）。
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    format!("{:06x}", x & 0xFF_FFFF)
}

/// ブランチ名 `agentdock/<チャット名を安全化>-<短いID>`。
pub fn worktree_branch_name(chat_name: &str, short: &str) -> String {
    format!("agentdock/{}-{}", safe_branch_slug(chat_name), short)
}

/// フォルダ名 `<リポジトリ名>-<短いID>`（ファイル名として安全化）。
pub fn worktree_folder_name(repo_name: &str, short: &str) -> String {
    format!("{}-{}", crate::store::layout::sanitize_file_name(repo_name), short)
}

/// パスの最後の要素（リポジトリ名）。取れなければ `repo`。
pub fn repo_name_of(repo_root: &str) -> String {
    let t = repo_root.trim_end_matches(['/', '\\']);
    t.rsplit(['/', '\\']).next().filter(|s| !s.is_empty() && !s.ends_with(':')).unwrap_or("repo").to_string()
}

/// `p` が `base` の配下か（字句だけ。`..` を含むパスは配下とみなさない。大文字小文字と区切りの違いは無視）。
/// リンク・ジャンクションを越える確認は呼出し側が解決後のパスで行う。
pub fn is_under(base: &str, p: &str) -> bool {
    if std::path::Path::new(p).components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    let b = norm_path(base);
    let b = b.trim_end_matches('\\');
    let t = norm_path(p);
    !b.is_empty() && t.starts_with(&format!("{b}\\")) && t.len() > b.len() + 1
}

/// Git上の実在（一覧の照合結果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listed {
    /// 一覧にある。`main` はメインの作業ツリー。
    Yes { main: bool },
    /// 一覧を読めて、載っていない。
    No,
    /// 一覧を読めなかった（Git未導入・タイムアウトなど）。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveInput {
    /// 記録のパスが、AgentDockの置き場所の配下（字句・解決後とも）。
    pub under_root: bool,
    pub listed: Listed,
    /// パスがディスクに実在する。
    pub path_exists: bool,
    /// このworktreeを使うチャットに、作業中・停止未確認・送信待ち・受理不明がある。
    pub in_use: bool,
    /// 未コミット・未追跡の変更の件数。取得できなければ None。
    pub dirty: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveBlock {
    /// 置き場所の外（記録が書き換えられた・リンク越え）。
    OutsideRoot,
    InUse,
    /// メインの作業ツリー（消さない）。
    MainWorktree,
    /// ディスクにはあるが、Gitの一覧に載っていない（別の用途のフォルダかもしれない。消さない）。
    NotListed,
    /// Gitの状態を読めず、安全と確認できない。
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveVerdict {
    /// 通常の削除（`--force` なし）。
    Allowed,
    /// 未コミットの変更がある。止めて案内し、「変更を破棄して削除」は2段目の確認で `--force`。
    NeedsDiscard { dirty: u32 },
    /// Gitの一覧にもディスクにもない。台帳の記録だけを外す（ファイルは触らない）。
    AlreadyGone,
    Blocked(RemoveBlock),
}

/// 削除の可否。記録（AgentDockが作った）があることは呼出し側が前提にする（記録のないものは対象にしない）。
pub fn remove_verdict(i: &RemoveInput) -> RemoveVerdict {
    if !i.under_root {
        return RemoveVerdict::Blocked(RemoveBlock::OutsideRoot);
    }
    if i.in_use {
        return RemoveVerdict::Blocked(RemoveBlock::InUse);
    }
    match i.listed {
        Listed::Unknown => RemoveVerdict::Blocked(RemoveBlock::Unverified),
        Listed::Yes { main: true } => RemoveVerdict::Blocked(RemoveBlock::MainWorktree),
        Listed::No if i.path_exists => RemoveVerdict::Blocked(RemoveBlock::NotListed),
        Listed::No => RemoveVerdict::AlreadyGone,
        Listed::Yes { main: false } => match i.dirty {
            None => RemoveVerdict::Blocked(RemoveBlock::Unverified),
            Some(0) => RemoveVerdict::Allowed,
            Some(n) => RemoveVerdict::NeedsDiscard { dirty: n },
        },
    }
}

/// チャットが使っている作業の状況（このチャットが worktree の削除を止めるか）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatUse {
    pub running: bool,
    pub stop_open: bool,
    pub unknown_send: bool,
    pub pending_ops: bool,
    /// 送信待ち・送信中・受理不明・受理なし（再送待ち）のキュー項目がある。
    pub queue_pending: bool,
}

impl ChatUse {
    pub fn blocks_removal(&self) -> bool {
        self.running || self.stop_open || self.unknown_send || self.pending_ops || self.queue_pending
    }
}

/// チャットがそのworktreeを使っているか。記録で結び付けたチャット、または作業フォルダがその配下のチャット（外部の会話を含む）。
pub fn chat_uses_worktree(bound: Option<&str>, chat_cwd: Option<&str>, worktree_id: &str, worktree_path: &str) -> bool {
    if bound == Some(worktree_id) {
        return true;
    }
    let Some(cwd) = chat_cwd else { return false };
    let (c, w) = (norm_path(cwd), norm_path(worktree_path));
    let w = w.trim_end_matches('\\');
    !w.is_empty() && (c.trim_end_matches('\\') == w || c.starts_with(&format!("{w}\\")))
}

/// チャットに結び付いたworktreeの、削除確認での見え方。`Present` は台帳にある（場所つき）、`Removed` は結び付きはあるが台帳に記録がない
/// （削除済み）。結び付きがなければ None。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundWorktree {
    Present { path: String },
    Removed,
}

pub fn bound_worktree(bound: Option<&str>, records: &[(String, String)]) -> Option<BoundWorktree> {
    let id = bound?;
    Some(match records.iter().find(|(rid, _)| rid == id) {
        Some((_, path)) => BoundWorktree::Present { path: path.clone() },
        None => BoundWorktree::Removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_worktree_distinguishes_present_removed_and_unbound() {
        let recs = vec![("w1".to_string(), r"C:\wt\a".to_string())];
        assert_eq!(bound_worktree(Some("w1"), &recs), Some(BoundWorktree::Present { path: r"C:\wt\a".into() }));
        assert_eq!(bound_worktree(Some("w2"), &recs), Some(BoundWorktree::Removed));
        assert_eq!(bound_worktree(None, &recs), None);
    }

    #[test]
    fn parses_porcelain_blocks() {
        let out = "worktree C:/repo\nHEAD 1111111\nbranch refs/heads/main\n\nworktree C:/wt/repo-abc123\nHEAD 2222222\nbranch refs/heads/agentdock/x-abc123\nlocked reason\n\nworktree C:/ext\nHEAD 3333333\ndetached\nprunable gitdir file points to non-existent location\n";
        let v = parse_worktree_list(out);
        assert_eq!(v.len(), 3);
        assert_eq!((v[0].path.as_str(), v[0].branch.as_deref(), v[0].head.as_deref()), ("C:/repo", Some("main"), Some("1111111")));
        assert_eq!(v[1].branch.as_deref(), Some("agentdock/x-abc123"));
        assert!(v[1].locked && !v[1].detached);
        assert!(v[2].detached && v[2].prunable && v[2].branch.is_none());
    }

    #[test]
    fn porcelain_handles_crlf_bare_and_stray_lines() {
        let v = parse_worktree_list("HEAD orphan\r\nworktree D:/bare\r\nbare\r\n\r\n\r\nworktree D:/b\r\nbranch refs/heads/feature/a\r\n");
        assert_eq!(v.len(), 2);
        assert!(v[0].bare && v[0].branch.is_none());
        assert_eq!(v[1].branch.as_deref(), Some("feature/a"));
        assert!(parse_worktree_list("").is_empty());
    }

    #[test]
    fn branch_slug_is_safe() {
        assert_eq!(safe_branch_slug("Fix the Bug!!"), "fix-the-bug");
        assert_eq!(safe_branch_slug("a/b..c@{d}"), "a-b-c-d");
        assert_eq!(safe_branch_slug("認証の修正 v2"), "認証の修正-v2");
        assert_eq!(safe_branch_slug("   "), "chat");
        assert_eq!(safe_branch_slug("---"), "chat");
        assert_eq!(safe_branch_slug(&"x".repeat(100)).chars().count(), SLUG_MAX_CHARS);
        assert!(!safe_branch_slug("-a-").starts_with('-') && !safe_branch_slug("-a-").ends_with('-'));
    }

    #[test]
    fn names_follow_the_decided_shape() {
        let s = short_id(42);
        assert_eq!(s.len(), 6);
        assert_eq!(s, short_id(42));
        assert_ne!(short_id(1), short_id(2));
        assert_eq!(worktree_branch_name("Fix Bug", "abc123"), "agentdock/fix-bug-abc123");
        assert_eq!(worktree_folder_name("My:Repo", "abc123"), "My_Repo-abc123");
        assert_eq!(repo_name_of("C:/work/repo/"), "repo");
        assert_eq!(repo_name_of("C:\\"), "repo");
    }

    #[test]
    fn under_root_is_lexical_and_rejects_dotdot() {
        let base = r"C:\Users\u\AppData\Local\com.agentdock.app\worktrees";
        assert!(is_under(base, r"C:\Users\u\AppData\Local\com.agentdock.app\worktrees\repo-abc123"));
        assert!(is_under(base, "c:/users/u/appdata/local/com.agentdock.app/worktrees/repo-abc123"));
        assert!(!is_under(base, base));
        assert!(!is_under(base, r"C:\Users\u\AppData\Local\com.agentdock.app\worktrees\..\chats\x"));
        assert!(!is_under(base, r"C:\Users\u\AppData\Local\com.agentdock.app\worktrees-other\x"));
        assert!(!is_under("", r"C:\x"));
    }

    fn input() -> RemoveInput {
        RemoveInput { under_root: true, listed: Listed::Yes { main: false }, path_exists: true, in_use: false, dirty: Some(0) }
    }

    #[test]
    fn remove_allowed_only_when_clean_listed_unused_and_inside_root() {
        assert_eq!(remove_verdict(&input()), RemoveVerdict::Allowed);
        assert_eq!(remove_verdict(&RemoveInput { dirty: Some(3), ..input() }), RemoveVerdict::NeedsDiscard { dirty: 3 });
        assert_eq!(remove_verdict(&RemoveInput { dirty: None, ..input() }), RemoveVerdict::Blocked(RemoveBlock::Unverified));
    }

    #[test]
    fn remove_blocks_in_priority_order() {
        assert_eq!(remove_verdict(&RemoveInput { under_root: false, in_use: true, ..input() }), RemoveVerdict::Blocked(RemoveBlock::OutsideRoot));
        assert_eq!(remove_verdict(&RemoveInput { in_use: true, ..input() }), RemoveVerdict::Blocked(RemoveBlock::InUse));
        assert_eq!(remove_verdict(&RemoveInput { listed: Listed::Unknown, ..input() }), RemoveVerdict::Blocked(RemoveBlock::Unverified));
        assert_eq!(remove_verdict(&RemoveInput { listed: Listed::Yes { main: true }, ..input() }), RemoveVerdict::Blocked(RemoveBlock::MainWorktree));
        assert_eq!(remove_verdict(&RemoveInput { listed: Listed::No, ..input() }), RemoveVerdict::Blocked(RemoveBlock::NotListed));
    }

    #[test]
    fn already_gone_needs_a_readable_list_and_no_directory() {
        assert_eq!(remove_verdict(&RemoveInput { listed: Listed::No, path_exists: false, ..input() }), RemoveVerdict::AlreadyGone);
        // 一覧を読めないときは「消えている」と断定しない。
        assert_eq!(remove_verdict(&RemoveInput { listed: Listed::Unknown, path_exists: false, ..input() }), RemoveVerdict::Blocked(RemoveBlock::Unverified));
    }

    #[test]
    fn any_unfinished_work_blocks_removal() {
        assert!(!ChatUse::default().blocks_removal());
        for u in [
            ChatUse { running: true, ..Default::default() },
            ChatUse { stop_open: true, ..Default::default() },
            ChatUse { unknown_send: true, ..Default::default() },
            ChatUse { pending_ops: true, ..Default::default() },
            ChatUse { queue_pending: true, ..Default::default() },
        ] {
            assert!(u.blocks_removal(), "{u:?}");
        }
    }

    #[test]
    fn chat_use_by_binding_or_working_folder() {
        let p = r"C:\wt\repo-abc123";
        assert!(chat_uses_worktree(Some("w1"), None, "w1", p));
        assert!(chat_uses_worktree(None, Some("c:/wt/repo-abc123"), "w1", p));
        assert!(chat_uses_worktree(None, Some(r"C:\wt\repo-abc123\sub"), "w1", p));
        assert!(!chat_uses_worktree(None, Some(r"C:\wt\repo-abc1234"), "w1", p));
        assert!(!chat_uses_worktree(Some("w2"), Some(r"C:\other"), "w1", p));
        assert!(!chat_uses_worktree(None, None, "w1", p));
    }
}
