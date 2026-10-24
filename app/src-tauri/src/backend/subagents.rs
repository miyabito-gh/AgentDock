//! Additive records for parent chats using backend-native descendants. No org workflow.
use serde::{Deserialize, Serialize};
use super::model::*;
use super::org_artifacts::{OrgArtifactFile, OrgImportOutcome};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentRun {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub created_at: UnixMillis,
    pub repo: String,
    pub worktree: Option<WorktreeRecord>,
    pub parent: Option<ChatKey>,
    pub first_attempt: Option<LocalId>,
    /// Creation milestones only; runtime completion is derived from host evidence.
    pub phase: String,
    pub detail: Option<String>,
    pub observed_children: Vec<AgentKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunView {
    pub run: SubagentRun,
    pub agents: Vec<AgentView>,
    pub blockers: Vec<String>,
    pub imports: Vec<SubagentImportRecord>,
    pub recovery_candidates: Vec<ChatKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentStartArgs {
    pub repo: String,
    pub request: String,
    pub model: Option<ModelChoice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunArgs { pub id: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentImportArgs { pub id: String, pub token: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentImportPreview {
    pub token: Option<String>,
    pub source_head: Option<String>,
    pub target_head: Option<String>,
    pub result_head: Option<String>,
    pub files: Vec<OrgArtifactFile>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentImportRecord {
    pub id: String,
    pub at: UnixMillis,
    pub before: String,
    pub result: String,
    pub source: String,
    /// None means a durable begin exists without a confirmed end: never retry automatically.
    pub outcome: Option<OrgImportOutcome>,
}

/// Explicit recovery chooses an already created parent; it never sends or resumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SubagentRecoverArgs { pub id: String, pub parent: ChatKey }
