//! `ParityOps` のCodex実装（段階③、`app/DESIGN_P3.md` §2.2・§3）。
//!
//! 操作ごとの節に分けてある。P3-1〜P3-6 は自分の節のメソッドだけを実装し、`DECLARED` の `implemented` を真にする。
//! Codex固有の変換（レビュー対象・協調モード・速度ID・MCP接続状態など）はこのファイルと `cli.rs` に閉じ込める。
//! 実装していない操作は、能力宣言も呼出しも `Unsupported`（成功を装わない）。

use super::adapter::CodexBackend;
use super::parity_table;
use crate::backend::backend::*;
use crate::backend::model::*;
use crate::backend::parity::*;
use async_trait::async_trait;

/// 宣言の1行。`schema_support` はschema上の有無（experimentalなら `Experimental`）、`implemented` はAgentDock側の実装有無。
struct Declared {
    op: ParityOp,
    route: OpRoute,
    schema_support: Support,
    deprecated: bool,
    implemented: bool,
}

const fn d(op: ParityOp, route: OpRoute, schema_support: Support) -> Declared {
    Declared { op, route, schema_support, deprecated: false, implemented: false }
}

/// 0.160.0 のschemaと `DESIGN_P3.md` §1 に基づく宣言。実装したタスクが `implemented` を真にする。
const DECLARED: [Declared; 20] = [
    d(ParityOp::ChangeList, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::RevertChanges, OpRoute::AppManaged, Support::Supported),
    d(ParityOp::CodeReview, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::ReviewToNewChat, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::WorkMode, OpRoute::BackendApi, Support::Experimental),
    d(ParityOp::Fork, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::Compact, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::ReferenceChat, OpRoute::AppManaged, Support::Supported),
    d(ParityOp::Goal, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::SideChat, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::Skills, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::InstructionFiles, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::ToolServers, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::Extensions, OpRoute::CliHelper, Support::Supported),
    d(ParityOp::CloudDelegation, OpRoute::CliHelper, Support::Experimental),
    d(ParityOp::Worktree, OpRoute::AppManaged, Support::Supported),
    d(ParityOp::BackendStatus, OpRoute::BackendApi, Support::Supported),
    d(ParityOp::SpeedTier, OpRoute::BackendApi, Support::Supported),
    // schema注記: 常に効果なし。操作は置かない。
    Declared { deprecated: true, ..d(ParityOp::Personality, OpRoute::BackendApi, Support::Unsupported) },
    d(ParityOp::Memory, OpRoute::BackendApi, Support::Experimental),
];

/// 操作ごとの能力。`version` は接続先の実際の版（不明なら None＝確認状況はすべて未確認）。
/// 未実装の操作は `Unsupported`（理由を note に書く）。
pub fn op_capabilities(version: Option<&str>) -> Vec<OpCapability> {
    DECLARED
        .iter()
        .map(|x| {
            let (support, note) = if x.deprecated {
                (Support::Unsupported, Some("非推奨（このCodex版では選べない）".to_string()))
            } else if !x.implemented {
                (Support::Unsupported, Some("AgentDockでは未実装".to_string()))
            } else {
                (x.schema_support, None)
            };
            OpCapability { op: x.op, support, route: x.route, verification: parity_table::verification(version, x.op), deprecated: x.deprecated, note }
        })
        .collect()
}

#[async_trait]
impl ParityOps for CodexBackend {
    // ── 分岐・side相談（P3-2・P3-4） ──
    async fn fork_chat(&self, _chat: ChatKey, _params: ForkParams, _confirmed: &UserConfirmed) -> BackendResult<ForkOutcome> {
        unsupported(ParityOp::Fork)
    }

    // ── レビュー・圧縮（P3-2） ──
    async fn start_review(&self, _chat: ChatKey, _target: ReviewTarget, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CodeReview)
    }
    async fn compact(&self, _chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Compact)
    }

    // ── Goal（P3-3） ──
    async fn get_goal(&self, _chat: ChatKey) -> BackendResult<Known<Goal>> {
        unsupported(ParityOp::Goal)
    }
    async fn set_goal(&self, _chat: ChatKey, _update: GoalUpdate, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Goal)
    }
    async fn clear_goal(&self, _chat: ChatKey, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Goal)
    }

    // ── 計画／実行の切替（P3-3） ──
    async fn list_work_modes(&self) -> BackendResult<Vec<WorkModeInfo>> {
        unsupported(ParityOp::WorkMode)
    }

    // ── Skills・指示ファイル（P3-4） ──
    async fn list_skills(&self, _cwd: String, _force_reload: bool) -> BackendResult<Vec<SkillInfo>> {
        unsupported(ParityOp::Skills)
    }
    async fn instruction_sources(&self, _chat: ChatKey) -> BackendResult<Known<Vec<String>>> {
        unsupported(ParityOp::InstructionFiles)
    }

    // ── ツールサーバー・拡張（P3-5） ──
    async fn list_tool_servers(&self) -> BackendResult<Vec<ToolServerView>> {
        unsupported(ParityOp::ToolServers)
    }
    async fn login_tool_server(&self, _name: String, _confirmed: &UserConfirmed) -> BackendResult<String> {
        unsupported(ParityOp::ToolServers)
    }
    async fn reload_tool_servers(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::ToolServers)
    }
    async fn list_extensions(&self) -> BackendResult<Vec<ExtensionView>> {
        unsupported(ParityOp::Extensions)
    }
    async fn manage_extension(&self, _op: ExtensionOp, _confirmed: &UserConfirmed) -> BackendResult<ExtensionOpResult> {
        unsupported(ParityOp::Extensions)
    }

    // ── クラウド委任（P3-6） ──
    async fn cloud_submit(&self, _request: CloudSubmit, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_list(&self) -> BackendResult<Vec<CloudTaskInfo>> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_diff(&self, _task_id: String) -> BackendResult<String> {
        unsupported(ParityOp::CloudDelegation)
    }
    async fn cloud_apply(&self, _task_id: String, _cwd: String, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::CloudDelegation)
    }

    // ── 状態・速度・memories（P3-3） ──
    async fn backend_status(&self) -> BackendResult<BackendStatus> {
        unsupported(ParityOp::BackendStatus)
    }
    async fn memory_status(&self) -> BackendResult<MemoryStatus> {
        unsupported(ParityOp::Memory)
    }
    async fn set_memory_mode(&self, _chat: ChatKey, _mode: String, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Memory)
    }
    async fn reset_memory(&self, _confirmed: &UserConfirmed) -> BackendResult<OpAck> {
        unsupported(ParityOp::Memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_operations_are_unsupported_never_supported() {
        let caps = op_capabilities(Some("0.160.0"));
        assert_eq!(caps.len(), ParityOp::ALL.len());
        for c in &caps {
            assert_eq!(c.support, Support::Unsupported, "{:?}", c.op);
            assert!(c.note.is_some());
        }
        let personality = caps.iter().find(|c| c.op == ParityOp::Personality).unwrap();
        assert!(personality.deprecated);
    }
}
