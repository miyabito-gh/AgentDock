//! 作業フォルダ（複数）の検査（`app/DESIGN_WORKFOLDERS.md` (c)）。パスの正規化・UNC判定・重複排除・システム判定・強い確認の判定は純粋。
//! OSへの問い合わせ（存在・ドライブ種別）は `probe` を引数で受け取り、末尾の薄い層（[`os_probe`]・[`FolderEnv::from_os`]）だけが実行する。

use crate::backend::model::{FolderCheck, FolderReject, StrongReason, WorkFolders};

/// プライマリを含めた上限件数。
pub const MAX_FOLDERS: usize = 8;

/// 検査に使うOS側の特別なフォルダ（すべて絶対パス。取得できなかったものは None／空）。
#[derive(Debug, Clone, Default)]
pub struct FolderEnv {
    pub windows: Option<String>,
    /// `%ProgramFiles%`・`%ProgramFiles(x86)%` など。
    pub program_files: Vec<String>,
    pub home: Option<String>,
    pub app_data: Option<String>,
    pub local_app_data: Option<String>,
    pub program_data: Option<String>,
    pub codex_home: Option<String>,
    /// `%LOCALAPPDATA%\com.agentdock.app`。
    pub agentdock_data: Option<String>,
    /// `agentdock_data\worktrees`（この配下は追加できる）。
    pub worktrees: Option<String>,
}

/// 1件のパスについてOSに問い合わせた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathFacts {
    pub exists: bool,
    pub is_dir: bool,
    /// ネットワークドライブ（`DRIVE_REMOTE`）。
    pub remote: bool,
}

// ───────────────────────────── 正規化 ─────────────────────────────

/// 比較用の鍵（区切り統一・小文字）。正規化済みのパスに使う。
pub fn path_key(normalized: &str) -> String {
    normalized.to_lowercase()
}

fn is_drive_letter_abs(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// 区切りを `\` にそろえ、`\\?\` を外す。`\\?\UNC\…` は `\\…` に戻す（UNC判定のため）。
fn unify(raw: &str) -> String {
    let s = raw.trim().replace('/', "\\");
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        if let Some(unc) = rest.strip_prefix("UNC\\").or_else(|| rest.strip_prefix("unc\\")) {
            return format!("\\\\{unc}");
        }
        return rest.to_string();
    }
    s
}

/// UNC（`\\server\…`、`\\?\UNC\…`、`\\.\…`）か。
pub fn is_unc(raw: &str) -> bool {
    unify(raw).starts_with("\\\\")
}

/// 絶対パス（ドライブ文字付き）を正規化する。重複する区切り・`.`・`..` を字句で解消し、末尾の `\` を落とす（ドライブ直下を除く）。
/// ドライブ文字は大文字にする。UNC・相対パス・ドライブ相対（`C:foo`）は `None`。
pub fn normalize_path(raw: &str) -> Option<String> {
    let s = unify(raw);
    if s.starts_with("\\\\") || !is_drive_letter_abs(&s) {
        return None;
    }
    let drive = s[..1].to_ascii_uppercase();
    let mut parts: Vec<&str> = Vec::new();
    for seg in s[3..].split('\\') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => {
                // Win32は各セグメント末尾のドット・空白を無視する（`Windows.` は `Windows`）。除外判定をすり抜けないよう落とす。
                let t = other.trim_end_matches(['.', ' ']);
                if !t.is_empty() {
                    parts.push(t);
                }
            }
        }
    }
    if parts.is_empty() {
        Some(format!("{drive}:\\"))
    } else {
        Some(format!("{drive}:\\{}", parts.join("\\")))
    }
}

/// `p` が `base` と同じか配下か（どちらも正規化済み）。
pub fn is_same_or_under(p: &str, base: &str) -> bool {
    let (p, base) = (path_key(p), path_key(base));
    let base = base.trim_end_matches('\\');
    p == base || p.strip_prefix(base).is_some_and(|r| r.starts_with('\\'))
}

fn is_under(p: &str, base: &str) -> bool {
    is_same_or_under(p, base) && path_key(p).trim_end_matches('\\') != path_key(base).trim_end_matches('\\')
}

fn is_same(a: &str, b: &str) -> bool {
    path_key(a).trim_end_matches('\\') == path_key(b).trim_end_matches('\\')
}

fn is_drive_root(p: &str) -> bool {
    p.len() == 3 && is_drive_letter_abs(p)
}

fn norm_opt(o: &Option<String>) -> Option<String> {
    o.as_deref().and_then(normalize_path)
}

// ───────────────────────────── 検査 ─────────────────────────────

/// 字句だけで決まる拒否（OSに問い合わせない）。通れば正規化したパスを返す。
fn lexical_check(raw: &str, existing: &[String], env: &FolderEnv) -> Result<String, FolderReject> {
    if is_unc(raw) {
        return Err(FolderReject::Unc);
    }
    let Some(n) = normalize_path(raw) else { return Err(FolderReject::NotAbsolute) };
    if existing.len() >= MAX_FOLDERS {
        return Err(FolderReject::TooMany);
    }
    if existing.iter().filter_map(|e| normalize_path(e)).any(|e| is_same(&e, &n)) {
        return Err(FolderReject::Duplicate);
    }
    if norm_opt(&env.windows).is_some_and(|w| is_same_or_under(&n, &w))
        || env.program_files.iter().filter_map(|p| normalize_path(p)).any(|p| is_same_or_under(&n, &p))
    {
        return Err(FolderReject::SystemFolder);
    }
    if let Some(data) = norm_opt(&env.agentdock_data) {
        if is_same_or_under(&n, &data) && !norm_opt(&env.worktrees).is_some_and(|w| is_same_or_under(&n, &w)) {
            return Err(FolderReject::AppDataOfAgentDock);
        }
    }
    if norm_opt(&env.codex_home).is_some_and(|c| is_same_or_under(&n, &c)) {
        return Err(FolderReject::CodexHome);
    }
    Ok(n)
}

fn strong_reasons(n: &str, existing: &[String], env: &FolderEnv) -> Vec<StrongReason> {
    let mut v = Vec::new();
    if is_drive_root(n) {
        v.push(StrongReason::DriveRoot);
    }
    if norm_opt(&env.home).is_some_and(|h| is_same(n, &h)) {
        v.push(StrongReason::HomeRoot);
    }
    let app_data_dirs = [
        norm_opt(&env.app_data),
        norm_opt(&env.local_app_data),
        // `%USERPROFILE%\AppData` 自体も同じ扱い。
        norm_opt(&env.home).map(|h| format!("{}\\AppData", h.trim_end_matches('\\'))),
    ];
    if app_data_dirs.iter().flatten().any(|a| is_same(n, a)) {
        v.push(StrongReason::AppDataRoot);
    }
    if norm_opt(&env.program_data).is_some_and(|p| is_same_or_under(n, &p)) {
        v.push(StrongReason::ProgramData);
    }
    if existing.iter().filter_map(|e| normalize_path(e)).any(|e| is_under(&e, n)) {
        v.push(StrongReason::ContainsOtherFolder);
    }
    v
}

/// 追加（または変更）しようとする1件を検査する。`existing` は既に並んでいる他のフォルダ（プライマリを含む）。
/// 字句の拒否を先に判定し、通ったものだけ `probe` でOSに問い合わせる（UNC・相対パスには問い合わせない）。
pub fn check_folder(raw: &str, existing: &[String], env: &FolderEnv, probe: &dyn Fn(&str) -> PathFacts) -> FolderCheck {
    let n = match lexical_check(raw, existing, env) {
        Ok(n) => n,
        Err(reason) => return FolderCheck::Rejected { reason },
    };
    let facts = probe(&n);
    if facts.remote {
        return FolderCheck::Rejected { reason: FolderReject::NetworkDrive };
    }
    if !facts.exists {
        return FolderCheck::Rejected { reason: FolderReject::NotFound };
    }
    if !facts.is_dir {
        return FolderCheck::Rejected { reason: FolderReject::NotDirectory };
    }
    let reasons = strong_reasons(&n, existing, env);
    if reasons.is_empty() {
        FolderCheck::Ok { normalized: n }
    } else {
        FolderCheck::NeedsStrongConfirm { normalized: n, reasons }
    }
}

/// フォルダ集合の再検査の失敗。ホストはUIの確認結果を信用せず、これで再判定する（設計 (c)-4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetReject {
    /// 空の集合（プライマリがない）。
    Empty,
    Rejected { index: usize, reason: FolderReject },
    /// 強い確認が必要だが `confirmed_strong` に無い。
    StrongUnconfirmed { index: usize, reasons: Vec<StrongReason> },
}

/// 集合全体を再検査し、正規化した並びを返す。各件は「自分以外の全件」を `existing` として検査する
/// （重複は双方、入れ子は親側、9件以上は全件が拒否される）。強い確認は `confirmed_strong`（パス）に含まれる件だけ通す。
pub fn verify_folder_set(
    folders: &[String],
    confirmed_strong: &[String],
    env: &FolderEnv,
    probe: &dyn Fn(&str) -> PathFacts,
) -> Result<WorkFolders, SetReject> {
    if folders.is_empty() {
        return Err(SetReject::Empty);
    }
    let confirmed: Vec<String> = confirmed_strong.iter().filter_map(|c| normalize_path(c)).map(|c| path_key(&c)).collect();
    let mut out = Vec::with_capacity(folders.len());
    for (i, f) in folders.iter().enumerate() {
        let others: Vec<String> = folders.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.clone()).collect();
        match check_folder(f, &others, env, probe) {
            FolderCheck::Ok { normalized } => out.push(normalized),
            FolderCheck::NeedsStrongConfirm { normalized, reasons } => {
                if !confirmed.contains(&path_key(&normalized)) {
                    return Err(SetReject::StrongUnconfirmed { index: i, reasons });
                }
                out.push(normalized);
            }
            FolderCheck::Rejected { reason } => return Err(SetReject::Rejected { index: i, reason }),
        }
    }
    Ok(WorkFolders { folders: out })
}

// ───────────────────────────── OSへの薄い層 ─────────────────────────────

/// 実際のOSへ問い合わせる `probe`。
pub fn os_probe(path: &str) -> PathFacts {
    let meta = std::fs::metadata(path);
    PathFacts { exists: meta.is_ok(), is_dir: meta.map(|m| m.is_dir()).unwrap_or(false), remote: is_remote_drive(path) }
}

#[cfg(windows)]
fn is_remote_drive(path: &str) -> bool {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4;
    let b = path.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return false;
    }
    let root: Vec<u16> = format!("{}:\\", path[..1].to_ascii_uppercase()).encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `root` はNUL終端のUTF-16で、呼出し中は有効。
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(not(windows))]
fn is_remote_drive(_path: &str) -> bool {
    false
}

impl FolderEnv {
    /// 環境変数から組み立てる。`~/.codex` は `CODEX_HOME` があればそれ、なければ `%USERPROFILE%\.codex`。
    pub fn from_os() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let home = var("USERPROFILE");
        let local = var("LOCALAPPDATA");
        let data = local.as_ref().map(|l| format!("{}\\com.agentdock.app", l.trim_end_matches('\\')));
        FolderEnv {
            windows: var("SystemRoot").or_else(|| var("WINDIR")),
            program_files: ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|k| var(k)).collect(),
            codex_home: var("CODEX_HOME").or_else(|| home.as_ref().map(|h| format!("{}\\.codex", h.trim_end_matches('\\')))),
            home,
            app_data: var("APPDATA"),
            local_app_data: local,
            program_data: var("ProgramData"),
            worktrees: data.as_ref().map(|d| format!("{d}\\worktrees")),
            agentdock_data: data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> FolderEnv {
        FolderEnv {
            windows: Some("C:\\Windows".into()),
            program_files: vec!["C:\\Program Files".into(), "C:\\Program Files (x86)".into()],
            home: Some("C:\\Users\\me".into()),
            app_data: Some("C:\\Users\\me\\AppData\\Roaming".into()),
            local_app_data: Some("C:\\Users\\me\\AppData\\Local".into()),
            program_data: Some("C:\\ProgramData".into()),
            codex_home: Some("C:\\Users\\me\\.codex".into()),
            agentdock_data: Some("C:\\Users\\me\\AppData\\Local\\com.agentdock.app".into()),
            worktrees: Some("C:\\Users\\me\\AppData\\Local\\com.agentdock.app\\worktrees".into()),
        }
    }

    fn dir(_: &str) -> PathFacts {
        PathFacts { exists: true, is_dir: true, remote: false }
    }

    fn chk(raw: &str, existing: &[&str]) -> FolderCheck {
        let ex: Vec<String> = existing.iter().map(|s| s.to_string()).collect();
        check_folder(raw, &ex, &env(), &dir)
    }

    fn rejected(raw: &str, existing: &[&str]) -> Option<FolderReject> {
        match chk(raw, existing) {
            FolderCheck::Rejected { reason } => Some(reason),
            _ => None,
        }
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path("c:/work//proj/").as_deref(), Some("C:\\work\\proj"));
        assert_eq!(normalize_path("\\\\?\\C:\\a\\b").as_deref(), Some("C:\\a\\b"));
        assert_eq!(normalize_path("C:\\a\\.\\b\\..\\c").as_deref(), Some("C:\\a\\c"));
        assert_eq!(normalize_path("c:\\").as_deref(), Some("C:\\"));
        assert_eq!(normalize_path("C:\\..\\.."), Some("C:\\".to_string()));
        assert_eq!(normalize_path("work\\proj"), None);
        assert_eq!(normalize_path("C:proj"), None);
        assert_eq!(normalize_path("\\\\srv\\share"), None);
    }

    #[test]
    fn trailing_dots_and_spaces_do_not_bypass_exclusions() {
        assert_eq!(normalize_path("C:\\Windows.").as_deref(), Some("C:\\Windows"));
        assert_eq!(normalize_path("C:\\a \\b. .").as_deref(), Some("C:\\a\\b"));
        assert_eq!(rejected("C:\\Windows.", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Windows \\", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Windows.\\System32", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Program Files.", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Users\\me\\.codex.", &[]), Some(FolderReject::CodexHome));
        assert_eq!(rejected("C:\\Users\\me\\.codex \\x", &[]), Some(FolderReject::CodexHome));
        assert_eq!(rejected("C:\\Users\\me\\AppData\\Local\\com.agentdock.app.", &[]), Some(FolderReject::AppDataOfAgentDock));
        assert_eq!(rejected("C:\\w\\proj.", &["C:\\w\\proj"]), Some(FolderReject::Duplicate));
    }

    #[test]
    fn dotdot_unc_and_edge_cases() {
        assert_eq!(rejected("C:\\Users\\me\\..\\..\\Windows", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:", &[]), Some(FolderReject::NotAbsolute));
        assert!(is_unc("\\\\.\\C:\\x"));
        assert!(is_unc("//server/share"));
        assert_eq!(rejected("//server/share", &[]), Some(FolderReject::Unc));
        // worktrees ディレクトリ自体は追加できる。
        assert!(matches!(chk("C:\\Users\\me\\AppData\\Local\\com.agentdock.app\\worktrees", &[]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn all_program_files_variants_are_rejected() {
        let mut e = env();
        e.program_files = vec!["C:\\Program Files".into(), "C:\\Program Files (x86)".into(), "D:\\PF64".into()];
        let r = |p: &str| match check_folder(p, &[], &e, &dir) {
            FolderCheck::Rejected { reason } => Some(reason),
            _ => None,
        };
        assert_eq!(r("C:\\Program Files\\a"), Some(FolderReject::SystemFolder));
        assert_eq!(r("C:\\Program Files (x86)\\a"), Some(FolderReject::SystemFolder));
        assert_eq!(r("d:\\pf64\\a"), Some(FolderReject::SystemFolder));
        assert_eq!(r("D:\\other"), None);
    }

    #[test]
    fn detects_unc_and_relative() {
        assert!(is_unc("\\\\server\\share\\x"));
        assert!(is_unc("\\\\?\\UNC\\server\\share"));
        assert!(!is_unc("\\\\?\\C:\\x"));
        assert_eq!(rejected("\\\\server\\share", &[]), Some(FolderReject::Unc));
        assert_eq!(rejected("\\\\?\\UNC\\server\\share", &[]), Some(FolderReject::Unc));
        assert_eq!(rejected("proj", &[]), Some(FolderReject::NotAbsolute));
    }

    #[test]
    fn unc_is_not_probed() {
        let probe = |_: &str| -> PathFacts { panic!("UNC must not be probed") };
        assert!(matches!(check_folder("\\\\srv\\s", &[], &env(), &probe), FolderCheck::Rejected { reason: FolderReject::Unc }));
    }

    #[test]
    fn duplicates_ignore_case_separator_and_verbatim() {
        assert_eq!(rejected("c:/Work/Proj/", &["C:\\work\\proj"]), Some(FolderReject::Duplicate));
        assert_eq!(rejected("\\\\?\\C:\\WORK\\proj", &["C:\\work\\proj"]), Some(FolderReject::Duplicate));
        assert!(matches!(chk("C:\\work\\proj2", &["C:\\work\\proj"]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn rejects_system_folders() {
        assert_eq!(rejected("C:\\Windows", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("c:\\windows\\system32", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Program Files\\App", &[]), Some(FolderReject::SystemFolder));
        assert_eq!(rejected("C:\\Program Files (x86)", &[]), Some(FolderReject::SystemFolder));
        // 前方一致だけの別フォルダは対象外。
        assert!(matches!(chk("C:\\Windows2", &[]), FolderCheck::Ok { .. }));
        assert!(matches!(chk("C:\\Program Files Extra", &[]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn rejects_agentdock_data_except_worktrees() {
        assert_eq!(rejected("C:\\Users\\me\\AppData\\Local\\com.agentdock.app", &[]), Some(FolderReject::AppDataOfAgentDock));
        assert_eq!(rejected("C:\\Users\\me\\AppData\\Local\\com.agentdock.app\\chats", &[]), Some(FolderReject::AppDataOfAgentDock));
        assert!(matches!(chk("C:\\Users\\me\\AppData\\Local\\com.agentdock.app\\worktrees\\wt1", &[]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn rejects_codex_home() {
        assert_eq!(rejected("C:\\Users\\me\\.codex", &[]), Some(FolderReject::CodexHome));
        assert_eq!(rejected("C:\\Users\\me\\.codex\\sessions", &[]), Some(FolderReject::CodexHome));
        assert!(matches!(chk("C:\\Users\\me\\.codex-other", &[]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn limit_is_eight_including_primary() {
        let seven: Vec<String> = (0..7).map(|i| format!("C:\\w\\p{i}")).collect();
        let seven_ref: Vec<&str> = seven.iter().map(|s| s.as_str()).collect();
        assert!(matches!(chk("C:\\w\\new", &seven_ref), FolderCheck::Ok { .. }));
        let eight: Vec<String> = (0..8).map(|i| format!("C:\\w\\p{i}")).collect();
        let eight_ref: Vec<&str> = eight.iter().map(|s| s.as_str()).collect();
        assert_eq!(rejected("C:\\w\\new", &eight_ref), Some(FolderReject::TooMany));
    }

    #[test]
    fn os_facts_decide_rejection() {
        let ex: Vec<String> = Vec::new();
        let r = |f: PathFacts| match check_folder("D:\\x", &ex, &env(), &move |_: &str| f) {
            FolderCheck::Rejected { reason } => Some(reason),
            _ => None,
        };
        assert_eq!(r(PathFacts { exists: true, is_dir: true, remote: true }), Some(FolderReject::NetworkDrive));
        assert_eq!(r(PathFacts { exists: false, is_dir: false, remote: false }), Some(FolderReject::NotFound));
        assert_eq!(r(PathFacts { exists: true, is_dir: false, remote: false }), Some(FolderReject::NotDirectory));
        assert_eq!(r(PathFacts { exists: true, is_dir: true, remote: false }), None);
    }

    fn strong(raw: &str, existing: &[&str]) -> Vec<StrongReason> {
        match chk(raw, existing) {
            FolderCheck::NeedsStrongConfirm { reasons, .. } => reasons,
            _ => Vec::new(),
        }
    }

    #[test]
    fn strong_confirm_reasons() {
        assert_eq!(strong("D:\\", &[]), vec![StrongReason::DriveRoot]);
        assert_eq!(strong("c:/", &[]), vec![StrongReason::DriveRoot]);
        assert_eq!(strong("C:\\Users\\me", &[]), vec![StrongReason::HomeRoot]);
        assert_eq!(strong("C:\\Users\\me\\AppData\\Roaming", &[]), vec![StrongReason::AppDataRoot]);
        assert_eq!(strong("C:\\Users\\me\\AppData\\Local", &[]), vec![StrongReason::AppDataRoot]);
        assert_eq!(strong("C:\\Users\\me\\AppData", &[]), vec![StrongReason::AppDataRoot]);
        assert_eq!(strong("C:\\ProgramData", &[]), vec![StrongReason::ProgramData]);
        assert_eq!(strong("C:\\ProgramData\\vendor", &[]), vec![StrongReason::ProgramData]);
        assert_eq!(strong("C:\\w", &["C:\\w\\proj"]), vec![StrongReason::ContainsOtherFolder]);
        // 通常のフォルダ・ホスト配下は強い確認なし。
        assert!(matches!(chk("C:\\Users\\me\\Documents\\proj", &[]), FolderCheck::Ok { .. }));
        assert!(matches!(chk("C:\\Users\\me\\AppData\\Local\\Tool", &[]), FolderCheck::Ok { .. }));
        // 子を足すのは強い確認なし。
        assert!(matches!(chk("C:\\w\\proj", &["C:\\w"]), FolderCheck::Ok { .. }));
    }

    #[test]
    fn set_verification_requires_confirmation_and_rechecks() {
        let e = env();
        let f = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(verify_folder_set(&[], &[], &e, &dir), Err(SetReject::Empty));
        let ok = verify_folder_set(&f(&["c:/a", "C:\\b\\"]), &[], &e, &dir).unwrap();
        assert_eq!(ok.folders, vec!["C:\\a".to_string(), "C:\\b".to_string()]);
        // 強い確認の対象は、確認済みの一覧に無ければ拒否。
        assert_eq!(
            verify_folder_set(&f(&["C:\\a", "D:\\"]), &[], &e, &dir),
            Err(SetReject::StrongUnconfirmed { index: 1, reasons: vec![StrongReason::DriveRoot] })
        );
        let ok = verify_folder_set(&f(&["C:\\a", "D:\\"]), &f(&["d:/"]), &e, &dir).unwrap();
        assert_eq!(ok.folders.len(), 2);
        // 確認済みでも拒否理由は覆らない。
        assert_eq!(
            verify_folder_set(&f(&["C:\\a", "C:\\Windows"]), &f(&["C:\\Windows"]), &e, &dir),
            Err(SetReject::Rejected { index: 1, reason: FolderReject::SystemFolder })
        );
        // 重複・9件。
        assert!(matches!(verify_folder_set(&f(&["C:\\a", "c:\\A"]), &[], &e, &dir), Err(SetReject::Rejected { reason: FolderReject::Duplicate, .. })));
        let nine: Vec<String> = (0..9).map(|i| format!("C:\\w\\p{i}")).collect();
        assert!(matches!(verify_folder_set(&nine, &[], &e, &dir), Err(SetReject::Rejected { reason: FolderReject::TooMany, .. })));
        let eight: Vec<String> = (0..8).map(|i| format!("C:\\w\\p{i}")).collect();
        assert!(verify_folder_set(&eight, &[], &e, &dir).is_ok());
    }
}
