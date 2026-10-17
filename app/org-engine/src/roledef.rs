//! Role definition files (DESIGN_ORG 4.2, P-ORG-8). Pure: data type, validation and the
//! permission-breadth comparison. Reading and writing files is the host's job.
//!
//! A definition may only narrow permissions relative to its baseline. A definition that widens
//! them is still loaded, but disabled, with the reasons kept; it is enabled only by a user
//! confirmation that is tied to the permission fingerprint (so a later edit needs a new one).

use serde::{Deserialize, Serialize};

use crate::core::SandboxKind;
use crate::schemas::{ROLE_PLANNER, ROLE_REVIEWER, ROLE_WORKER};
use crate::types::{Role, RoleId};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_INSTRUCTIONS_CHARS: usize = 8000;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_ID_CHARS: usize = 40;
pub const MAX_CONCURRENT_CAP: u32 = 4;
pub const MAX_EXTRA_DIRS: usize = 16;
/// Output schema ids the engine knows.
pub const SCHEMA_PLAN: &str = "plan";
pub const SCHEMA_WORKER_REPORT: &str = "workerReport";
pub const SCHEMA_REVIEWER_VERDICT: &str = "reviewerVerdict";

/// Approval policy. Ordered from narrow to wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalPolicy {
    /// Ask for everything not known to be safe.
    Untrusted,
    /// The default (on-request).
    OnRequest,
    /// Never ask: widens.
    Never,
}

/// Where the role works. Ordered by how far it can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Workplace {
    SharedReadOnly,
    DetachedAtCommit,
    OwnWorktree,
}

fn sandbox_rank(s: SandboxKind) -> u8 {
    match s {
        SandboxKind::ReadOnly => 0,
        SandboxKind::WorkspaceWrite => 1,
    }
}

fn default_version() -> u32 {
    SCHEMA_VERSION
}
fn default_approval() -> ApprovalPolicy {
    ApprovalPolicy::OnRequest
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDefinition {
    #[serde(default = "default_version")]
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    /// Opaque model id. None = the chat default.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    pub instructions: String,
    pub sandbox: SandboxKind,
    #[serde(default = "default_approval")]
    pub approval: ApprovalPolicy,
    pub workplace: Workplace,
    pub output_schema: String,
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    /// Extra writable folders. Any entry widens.
    #[serde(default)]
    pub extra_writable_dirs: Vec<String>,
    /// Network access from the sandbox. true widens.
    #[serde(default)]
    pub allow_network: bool,
}

/// A definition after validation and the breadth check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedRole {
    pub def: RoleDefinition,
    /// Reasons the definition is wider than its baseline (empty = within).
    pub widened: Vec<String>,
    /// A user confirmation matching the current permission fingerprint exists.
    pub confirmed: bool,
}

impl LoadedRole {
    /// Usable: within the baseline, or widened but confirmed.
    pub fn enabled(&self) -> bool {
        self.widened.is_empty() || self.confirmed
    }
    pub fn to_role(&self) -> Role {
        Role {
            id: RoleId::new(self.def.id.clone()),
            enabled: self.enabled(),
            writes: self.def.sandbox == SandboxKind::WorkspaceWrite,
            max_concurrent: self.def.max_concurrent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Breadth {
    Within,
    Widened { reasons: Vec<String> },
}

struct Baseline {
    sandbox: SandboxKind,
    approval: ApprovalPolicy,
    workplace: Workplace,
}

/// Standard roles are held to their own defaults; any other role to the ceiling
/// (workspace-write + on-request, DESIGN_ORG 1.3).
fn baseline(id: &str) -> Baseline {
    let (sandbox, workplace) = match id {
        ROLE_PLANNER => (SandboxKind::ReadOnly, Workplace::SharedReadOnly),
        ROLE_REVIEWER => (SandboxKind::ReadOnly, Workplace::DetachedAtCommit),
        _ => (SandboxKind::WorkspaceWrite, Workplace::OwnWorktree),
    };
    Baseline { sandbox, approval: ApprovalPolicy::OnRequest, workplace }
}

/// Compares the definition's permissions with its baseline. Narrower is `Within`.
pub fn compare_breadth(def: &RoleDefinition) -> Breadth {
    let base = baseline(&def.id);
    let mut reasons = Vec::new();
    if sandbox_rank(def.sandbox) > sandbox_rank(base.sandbox) {
        reasons.push("sandbox が既定より広い（書込みを許可）".to_string());
    }
    if def.approval > base.approval {
        reasons.push("承認方針が既定より広い（承認を求めない）".to_string());
    }
    if def.workplace > base.workplace {
        reasons.push("作業場所が既定より広い".to_string());
    }
    if !def.extra_writable_dirs.is_empty() {
        reasons.push(format!("追加の作業フォルダがある（{}件）", def.extra_writable_dirs.len()));
    }
    if def.allow_network {
        reasons.push("ネットワークを許可している".to_string());
    }
    if reasons.is_empty() {
        Breadth::Within
    } else {
        Breadth::Widened { reasons }
    }
}

/// Stable fingerprint of the permission-relevant fields (FNV-1a 64). A confirmation applies
/// only while this is unchanged.
pub fn permission_fingerprint(def: &RoleDefinition) -> String {
    let mut dirs = def.extra_writable_dirs.clone();
    dirs.sort();
    let text = format!("{}|{:?}|{:?}|{:?}|{}|{}", def.id, def.sandbox, def.approval, def.workplace, def.allow_network, dirs.join("\u{1}"));
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Validation of a parsed definition. `file_stem` is the file name without `.json`.
pub fn validate(def: &RoleDefinition, file_stem: &str) -> Result<(), Vec<String>> {
    let mut e = Vec::new();
    if def.schema_version != SCHEMA_VERSION {
        e.push(format!("schemaVersion {} は未対応です（対応: {SCHEMA_VERSION}）", def.schema_version));
    }
    let id_ok = !def.id.is_empty()
        && def.id.chars().count() <= MAX_ID_CHARS
        && def.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !id_ok {
        e.push("id は小文字英数字・-・_ の1〜40文字にしてください".to_string());
    } else if def.id != file_stem {
        e.push(format!("id「{}」がファイル名「{file_stem}」と一致しません", def.id));
    }
    if def.display_name.trim().is_empty() || def.display_name.chars().count() > MAX_NAME_CHARS {
        e.push(format!("displayName は1〜{MAX_NAME_CHARS}文字にしてください"));
    }
    if def.instructions.trim().is_empty() {
        e.push("instructions が空です".to_string());
    } else if def.instructions.chars().count() > MAX_INSTRUCTIONS_CHARS {
        e.push(format!("instructions は{MAX_INSTRUCTIONS_CHARS}文字までです"));
    }
    if let Some(m) = &def.model {
        if m.trim().is_empty() || m.chars().count() > 120 {
            e.push("model は空にできません（既定にするなら項目を省いてください）".to_string());
        }
    }
    if let Some(x) = &def.effort {
        if x.trim().is_empty() || x.chars().count() > 40 {
            e.push("effort は空にできません（既定にするなら項目を省いてください）".to_string());
        }
    }
    if ![SCHEMA_PLAN, SCHEMA_WORKER_REPORT, SCHEMA_REVIEWER_VERDICT].contains(&def.output_schema.as_str()) {
        e.push(format!("outputSchema「{}」は未知です", def.output_schema));
    }
    if let Some(n) = def.max_concurrent {
        if !(1..=MAX_CONCURRENT_CAP).contains(&n) {
            e.push(format!("maxConcurrent は1〜{MAX_CONCURRENT_CAP}にしてください"));
        }
    }
    if def.extra_writable_dirs.len() > MAX_EXTRA_DIRS || def.extra_writable_dirs.iter().any(|d| d.trim().is_empty()) {
        e.push("extraWritableDirs の指定が不正です".to_string());
    }
    if def.workplace == Workplace::SharedReadOnly && def.sandbox != SandboxKind::ReadOnly {
        e.push("sharedReadOnly の作業場所には sandbox readOnly が必要です".to_string());
    }
    // Fields the engine does not apply are refused instead of being silently ignored (R-ORG M4):
    // what the file says must be what the role does.
    if ![ROLE_PLANNER, ROLE_WORKER, ROLE_REVIEWER].contains(&def.id.as_str()) {
        e.push("この版では、標準の3役（planner・worker・reviewer）以外の役割は未対応です".to_string());
    } else if def.workplace != baseline(&def.id).workplace {
        e.push("この版では、作業場所（workplace）を標準から変えられません（未対応）".to_string());
    }
    if def.approval != ApprovalPolicy::OnRequest {
        e.push("この版では、承認方針（approval）は onRequest 以外にできません（未対応）".to_string());
    }
    if !def.extra_writable_dirs.is_empty() {
        e.push("この版では、追加の作業フォルダ（extraWritableDirs）は未対応です".to_string());
    }
    if def.allow_network {
        e.push("この版では、ネットワークの許可（allowNetwork）は未対応です".to_string());
    }
    if e.is_empty() {
        Ok(())
    } else {
        Err(e)
    }
}

/// Validate, compare breadth, and apply a stored confirmation (`confirmed_fingerprint`).
pub fn load_role(def: RoleDefinition, file_stem: &str, confirmed_fingerprint: Option<&str>) -> Result<LoadedRole, Vec<String>> {
    validate(&def, file_stem)?;
    let widened = match compare_breadth(&def) {
        Breadth::Within => Vec::new(),
        Breadth::Widened { reasons } => reasons,
    };
    let confirmed = !widened.is_empty() && confirmed_fingerprint == Some(permission_fingerprint(&def).as_str());
    Ok(LoadedRole { def, widened, confirmed })
}

/// The three standard definitions written on first start.
pub fn standard_definitions() -> Vec<RoleDefinition> {
    let mk = |id: &str, name: &str, instructions: &str, sandbox, workplace, schema: &str, max: Option<u32>| RoleDefinition {
        schema_version: SCHEMA_VERSION,
        id: id.into(),
        display_name: name.into(),
        model: None,
        effort: None,
        instructions: instructions.into(),
        sandbox,
        approval: ApprovalPolicy::OnRequest,
        workplace,
        output_schema: schema.into(),
        max_concurrent: max,
        extra_writable_dirs: Vec::new(),
        allow_network: false,
    };
    vec![
        mk(ROLE_PLANNER, "Planner", "依頼を、依存関係のあるタスクの計画に分解する。", SandboxKind::ReadOnly, Workplace::SharedReadOnly, SCHEMA_PLAN, Some(1)),
        mk(ROLE_WORKER, "Worker", "割り当てられたタスクを自分のworktreeで実装し、完了条件（テストを含む）を確かめる。", SandboxKind::WorkspaceWrite, Workplace::OwnWorktree, SCHEMA_WORKER_REPORT, None),
        mk(ROLE_REVIEWER, "Reviewer", "対象コミットを読み取り専用で検証し、検証項目ごとに合否を報告する。", SandboxKind::ReadOnly, Workplace::DetachedAtCommit, SCHEMA_REVIEWER_VERDICT, None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str) -> RoleDefinition {
        standard_definitions().into_iter().find(|d| d.id == id).unwrap()
    }

    #[test]
    fn standard_definitions_are_valid_and_within() {
        for d in standard_definitions() {
            let l = load_role(d.clone(), &d.id, None).unwrap();
            assert!(l.enabled() && l.widened.is_empty());
        }
    }

    #[test]
    fn read_only_role_made_writable_is_widened_and_disabled_with_reasons() {
        let mut d = def("reviewer");
        d.sandbox = SandboxKind::WorkspaceWrite;
        let l = load_role(d.clone(), "reviewer", None).unwrap();
        assert!(!l.enabled());
        assert_eq!(l.widened.len(), 1);
        assert!(!l.to_role().enabled);
        // The breadth comparison itself still names every kind of widening.
        d.allow_network = true;
        d.extra_writable_dirs = vec!["C:\\x".into()];
        d.approval = ApprovalPolicy::Never;
        let Breadth::Widened { reasons } = compare_breadth(&d) else { panic!("widened") };
        assert_eq!(reasons.len(), 4);
    }

    #[test]
    fn settings_the_engine_does_not_apply_are_refused_instead_of_ignored() {
        for (name, change) in [
            ("approval", Box::new(|d: &mut RoleDefinition| d.approval = ApprovalPolicy::Untrusted) as Box<dyn Fn(&mut RoleDefinition)>),
            ("approval never", Box::new(|d| d.approval = ApprovalPolicy::Never)),
            ("network", Box::new(|d| d.allow_network = true)),
            ("dirs", Box::new(|d| d.extra_writable_dirs = vec!["C:\\x".into()])),
            ("workplace", Box::new(|d| d.workplace = Workplace::DetachedAtCommit)),
        ] {
            let mut d = def("worker");
            change(&mut d);
            let errs = load_role(d, "worker", None).unwrap_err();
            assert!(errs.iter().any(|e| e.contains("未対応")), "{name}: {errs:?}");
        }
        let mut other = def("worker");
        other.id = "tester".into();
        assert!(load_role(other, "tester", None).unwrap_err().iter().any(|e| e.contains("標準の3役")));
    }

    #[test]
    fn narrowing_stays_enabled() {
        let mut d = def("worker");
        d.sandbox = SandboxKind::ReadOnly;
        let l = load_role(d, "worker", None).unwrap();
        assert!(l.enabled() && l.widened.is_empty());
        assert!(!l.to_role().writes);
    }

    #[test]
    fn the_breadth_baseline_of_a_non_standard_id_is_the_default_ceiling() {
        let mut d = def("worker");
        d.id = "tester".into();
        assert_eq!(compare_breadth(&d), Breadth::Within);
        d.approval = ApprovalPolicy::Never;
        assert!(matches!(compare_breadth(&d), Breadth::Widened { .. }));
    }

    #[test]
    fn a_confirmation_applies_only_to_the_same_permissions() {
        let mut d = def("reviewer");
        d.sandbox = SandboxKind::WorkspaceWrite;
        let fp = permission_fingerprint(&d);
        assert!(load_role(d.clone(), "reviewer", Some(&fp)).unwrap().enabled());
        let before = d.clone();
        d.allow_network = true;
        assert_ne!(permission_fingerprint(&before), permission_fingerprint(&d), "any permission field changes the fingerprint");
        d.allow_network = false;
        d.sandbox = SandboxKind::ReadOnly;
        // back within the baseline: nothing to confirm
        assert!(load_role(d, "reviewer", Some(&fp)).unwrap().widened.is_empty());
    }

    #[test]
    fn the_engine_takes_only_a_complete_standard_set() {
        let all: Vec<LoadedRole> = standard_definitions().into_iter().map(|d| load_role(d.clone(), &d.id.clone(), None).unwrap()).collect();
        let mut core = crate::OrgCore::new(0);
        assert!(core.set_roles(&all).is_ok());
        assert!(core.set_roles(&all[..2]).is_err());
    }

    #[test]
    fn validation_rejects_bad_definitions() {
        let mut d = def("worker");
        d.instructions = "x".repeat(MAX_INSTRUCTIONS_CHARS + 1);
        d.max_concurrent = Some(9);
        d.output_schema = "nope".into();
        assert_eq!(load_role(d, "worker", None).unwrap_err().len(), 3);
        assert!(load_role(def("worker"), "other", None).is_err());
        let mut s = def("planner");
        s.workplace = Workplace::SharedReadOnly;
        s.sandbox = SandboxKind::WorkspaceWrite;
        assert!(validate(&s, "planner").is_err());
    }
}
