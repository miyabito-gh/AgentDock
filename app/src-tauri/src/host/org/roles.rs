//! 役割定義ファイルの読み書き（P-ORG-8、DESIGN_ORG §4.2）。判定そのもの（検証・権限の広さの比較）は `org-engine::roledef`。
//!
//! - 置き場所: `<root>\org\roles\<id>.json`（1役割1ファイル）。確認の印は `<root>\org\role-confirmations.json`
//!   （役割ID → 権限の指紋）。役割ファイルの外に置くので、ファイルを書き換えても印は引き継がれない。
//! - 標準3役は、ファイルが無いときだけ作る（`create_new`。既存ファイルは上書きしない）。
//! - 壊れたファイル（JSONとして読めない）は `.corrupt-<ms>` へ退避し、理由付きで報告する。他の役割は読む。
//! - 権限を広げる定義は無効のまま読み込む。有効化は利用者の確認（`UserConfirmed`）を値で消費する `enable_role` だけ。
//! - `~/.codex` は読まない・書かない。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use org_engine::{load_role, permission_fingerprint, standard_definitions, LoadedRole, RoleDefinition};

use crate::backend::backend::UserConfirmed;
use crate::store::atomic::write_atomic;
use crate::store::layout::corrupt_aside_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleProblem {
    pub file: String,
    pub reasons: Vec<String>,
    /// 退避した先（読めないファイルのときだけ）。
    pub aside: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RoleReport {
    /// 検証に通った定義。権限を広げるものは `enabled() == false` で `widened` に理由が入る。
    pub roles: Vec<LoadedRole>,
    /// 使えない定義（読めない・検証に通らない）。
    pub problems: Vec<RoleProblem>,
    /// この回に作った標準役割のID。
    pub created: Vec<String>,
}

pub fn roles_dir(root: &Path) -> PathBuf {
    root.join("org").join("roles")
}

fn confirmations_path(root: &Path) -> PathBuf {
    root.join("org").join("role-confirmations.json")
}

fn read_confirmations(root: &Path) -> BTreeMap<String, String> {
    std::fs::read(confirmations_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 標準3役のうちファイルが無いものだけを作る。既存ファイルには触れない。作ったIDを返す。
pub fn ensure_standard_roles(dir: &Path) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut created = Vec::new();
    for def in standard_definitions() {
        let path = dir.join(format!("{}.json", def.id));
        let mut bytes = serde_json::to_vec_pretty(&def).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let r = f.write_all(&bytes).and_then(|_| f.sync_all());
                drop(f);
                if let Err(e) = r {
                    // 書きかけを残さない（次回また作れるように）。
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                created.push(def.id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(created)
}

/// 役割ファイルを読む（作成はしない）。壊れたファイルは退避する。
pub fn load_roles(root: &Path, now_ms: i64) -> RoleReport {
    let dir = roles_dir(root);
    let confirmations = read_confirmations(root);
    let mut report = RoleReport::default();
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json")).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            report.problems.push(RoleProblem { file: dir.to_string_lossy().into_owned(), reasons: vec![format!("役割フォルダを読めません: {e}")], aside: None });
            return report;
        }
    };
    files.sort();
    for path in files {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                report.problems.push(RoleProblem { file: name, reasons: vec![format!("読めません: {e}")], aside: None });
                continue;
            }
        };
        let def: RoleDefinition = match serde_json::from_slice(&bytes) {
            Ok(d) => d,
            Err(e) => {
                let aside = corrupt_aside_name(&path, now_ms);
                let moved = std::fs::rename(&path, &aside).is_ok();
                report.problems.push(RoleProblem {
                    file: name,
                    reasons: vec![format!("役割定義として読めません: {e}")],
                    aside: moved.then(|| aside.to_string_lossy().into_owned()),
                });
                continue;
            }
        };
        if report.roles.iter().any(|r| r.def.id.eq_ignore_ascii_case(&def.id)) {
            report.problems.push(RoleProblem { file: name, reasons: vec![format!("id「{}」が重複しています", def.id)], aside: None });
            continue;
        }
        let confirmed = confirmations.get(&def.id).map(String::as_str);
        match load_role(def, &stem, confirmed) {
            Ok(r) => report.roles.push(r),
            Err(reasons) => report.problems.push(RoleProblem { file: name, reasons, aside: None }),
        }
    }
    report
}

/// 読込み → 足りない標準役割の作成 → 作った場合は読み直し。最初の読込みで出た問題は残す
/// （壊れた標準役割は退避され、新しく作り直される）。作成に失敗したら問題として報告する。
pub fn prepare_roles(root: &Path, now_ms: i64) -> RoleReport {
    let first = load_roles(root, now_ms);
    match ensure_standard_roles(&roles_dir(root)) {
        Ok(created) if created.is_empty() => first,
        Ok(created) => {
            let mut second = load_roles(root, now_ms);
            let mut problems = first.problems;
            // 壊れた標準役割は退避した後、既定の内容で作り直している。黙って既定に戻さず、その旨を理由に足す（R-ORG L9）。
            for p in problems.iter_mut().filter(|p| p.aside.is_some()) {
                if created.iter().any(|id| p.file == format!("{id}.json")) {
                    p.reasons.push("標準の内容で作り直しました（元のファイルは退避しています。直したい場合は、退避したファイルの内容を元の名前へ戻してください）".to_string());
                }
            }
            let more: Vec<RoleProblem> = second.problems.drain(..).filter(|p| !problems.contains(p)).collect();
            problems.extend(more);
            RoleReport { roles: second.roles, problems, created }
        }
        Err(e) => {
            let mut r = first;
            r.problems.push(RoleProblem { file: roles_dir(root).to_string_lossy().into_owned(), reasons: vec![format!("標準役割を作れません: {e}")], aside: None });
            r
        }
    }
}

/// 権限を広げる定義を、利用者の確認のうえで有効にする。印は今の権限の指紋に対してだけ有効。
/// 印を保存できなければ有効にしない。
pub fn enable_role(root: &Path, report: &mut RoleReport, id: &str, _confirmed: UserConfirmed) -> Result<(), String> {
    let Some(role) = report.roles.iter_mut().find(|r| r.def.id == id) else {
        return Err(format!("役割「{id}」は読み込まれていません"));
    };
    if role.widened.is_empty() {
        return Err(format!("役割「{id}」は確認なしで使えます"));
    }
    let mut all = read_confirmations(root);
    all.insert(id.to_string(), permission_fingerprint(&role.def));
    let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
    write_atomic(&confirmations_path(root), &bytes).map_err(|e| format!("確認を保存できませんでした: {e}"))?;
    role.confirmed = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::ApprovalPolicy;

    fn temp_root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-roles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_def(root: &Path, def: &RoleDefinition) {
        let dir = roles_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.json", def.id)), serde_json::to_vec_pretty(def).unwrap()).unwrap();
    }

    fn std_def(id: &str) -> RoleDefinition {
        standard_definitions().into_iter().find(|d| d.id == id).unwrap()
    }

    #[test]
    fn first_start_creates_the_three_roles_and_never_overwrites() {
        let root = temp_root("create");
        let r = prepare_roles(&root, 1);
        assert_eq!(r.created.len(), 3);
        assert_eq!(r.roles.len(), 3);
        assert!(r.roles.iter().all(|x| x.enabled()));
        // 利用者が書き換えたファイルは、再実行しても変わらない。
        let p = roles_dir(&root).join("worker.json");
        let mut d = std_def("worker");
        d.instructions = "my own wording".into();
        write_def(&root, &d);
        let before = std::fs::read(&p).unwrap();
        assert!(ensure_standard_roles(&roles_dir(&root)).unwrap().is_empty());
        let r2 = prepare_roles(&root, 2);
        assert!(r2.created.is_empty());
        assert_eq!(std::fs::read(&p).unwrap(), before);
        assert_eq!(r2.roles.iter().find(|x| x.def.id == "worker").unwrap().def.instructions, "my own wording");
        // 消した1つだけが作り直される。
        std::fs::remove_file(roles_dir(&root).join("planner.json")).unwrap();
        let r3 = prepare_roles(&root, 3);
        assert_eq!(r3.created, vec!["planner".to_string()]);
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_widening_definition_loads_disabled_with_reasons_until_confirmed() {
        let root = temp_root("widen");
        prepare_roles(&root, 1);
        let mut d = std_def("reviewer");
        d.sandbox = org_engine_sandbox_write();
        write_def(&root, &d);
        let mut r = load_roles(&root, 2);
        let rev = r.roles.iter().find(|x| x.def.id == "reviewer").unwrap();
        assert!(!rev.enabled());
        assert!(!rev.widened.is_empty());
        assert!(!rev.to_role().enabled);
        assert!(r.problems.is_empty());
        enable_role(&root, &mut r, "reviewer", UserConfirmed::from_user_command()).unwrap();
        assert!(r.roles.iter().find(|x| x.def.id == "reviewer").unwrap().enabled());
        // 読み直しても印が効く。権限をさらに広げると効かなくなる。
        assert!(load_roles(&root, 3).roles.iter().find(|x| x.def.id == "reviewer").unwrap().enabled());
        // 適用されない設定（承認方針の変更）は、無効ではなく読込みの問題として理由つきで報告する。
        d.approval = ApprovalPolicy::Never;
        write_def(&root, &d);
        let r4 = load_roles(&root, 4);
        assert!(r4.roles.iter().all(|x| x.def.id != "reviewer"));
        assert!(r4.problems.iter().any(|p| p.file == "reviewer.json" && p.reasons.iter().any(|m| m.contains("未対応"))));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn org_engine_sandbox_write() -> org_engine::SandboxKind {
        org_engine::SandboxKind::WorkspaceWrite
    }

    #[test]
    fn narrowing_definitions_are_enabled() {
        let root = temp_root("narrow");
        prepare_roles(&root, 1);
        let mut d = std_def("worker");
        d.sandbox = org_engine::SandboxKind::ReadOnly;
        write_def(&root, &d);
        let r = load_roles(&root, 2);
        let w = r.roles.iter().find(|x| x.def.id == "worker").unwrap();
        assert!(w.enabled() && w.widened.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_file_is_moved_aside_and_the_others_still_load() {
        let root = temp_root("broken");
        prepare_roles(&root, 1);
        std::fs::write(roles_dir(&root).join("reviewer.json"), b"{ not json").unwrap();
        let mut bad = std_def("worker");
        bad.id = "tester".into();
        bad.instructions = String::new();
        write_def(&root, &bad);
        let r = prepare_roles(&root, 77);
        // 壊れた標準役割は退避され、新しく作り直される。
        let aside = roles_dir(&root).join("reviewer.json.corrupt-77");
        assert_eq!(std::fs::read(&aside).unwrap(), b"{ not json");
        assert_eq!(r.created, vec!["reviewer".to_string()]);
        assert!(r.problems.iter().any(|p| p.file == "reviewer.json" && p.aside.is_some() && p.reasons.iter().any(|m| m.contains("作り直しました"))));
        // 検証に通らない定義は、そのまま残して理由を返す。
        assert!(r.problems.iter().any(|p| p.file == "tester.json" && p.aside.is_none() && !p.reasons.is_empty()));
        assert!(roles_dir(&root).join("tester.json").exists());
        assert_eq!(r.roles.len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
