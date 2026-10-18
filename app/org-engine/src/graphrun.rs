//! Graph interpretation (DESIGN_ORG_GRAPH 7). Runs the nodes of a validated `CompiledGraph`: agent, router and
//! gate. One node = one task = one chat; a loop (a feedback edge) goes round on the same task.
//!
//! Same rules as the rest of the core: sans-IO, events (`Persist`) before any outside effect, a turn is sent only
//! through `send_turn`, and nothing here turns termination, disconnection or an unknown send outcome into a
//! decision. An edge is chosen only for a task that is `Confirmed`, from values that are really there
//! (`Route::Unresolved` stops instead of guessing), and a gate is decided only by the user's input.
//!
//! Order notes (what a crash between two events leaves behind, and how a restore repairs it):
//! - forward edge: the task's status first, then the next task (`ActivationAdded`), then `EdgeTaken`. A task that is
//!   `completed` / `reviewing` with an output newer than its last `EdgeTaken` has an undecided edge; the graph pass
//!   chooses it again from the stored slots (pure, same result) and reuses a task that already exists.
//! - feedback edge: `EdgeTaken` and `FeedbackCounted` first, then the statuses, so a crash can only over-count.
use std::collections::HashSet;

use serde_json::{json, Value};

use crate::core::{Flow, Launch, Output, OrgAction, OrgCore, OrgNotice, WorkspaceAction, WorktreeBase};
use crate::graph::{select_edge, CaptureKind, CompiledGraph, EdgeDef, EdgeId, LoopBody, NodeId, NodeKind as GNode, Route, SlotId, WorkspaceKind};
use crate::limits::TurnKind;
use crate::outschema::OutSchema;
use crate::persist::{OrgEvent, OrgEventKind, WorkflowState, SCHEMA_VERSION};
use crate::prompt::{self, NodePrompt, NOTE_MAX_CHARS};
use crate::state::Cause;
use crate::types::*;

/// A slot value larger than this is a format violation (DESIGN_ORG_GRAPH 7.5).
pub const MAX_SLOT_BYTES: usize = 256 * 1024;

fn nonblank(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

impl OrgCore {
    // ------------------------------------------------------------ small helpers

    pub(crate) fn is_graph_task(&self, id: &TaskId) -> bool {
        self.task(id).is_some_and(|t| t.node.is_some())
    }

    /// A router or gate task (never sent to a model, moved by the graph pass).
    pub(crate) fn is_non_agent_task(&self, id: &TaskId) -> bool {
        let Some(g) = self.graph.as_ref() else { return false };
        self.task(id).and_then(|t| t.node.as_ref()).and_then(|n| g.node(n)).is_some_and(|n| matches!(n.kind, GNode::Router | GNode::Gate { .. }))
    }

    pub(crate) fn task_of_node(&self, node: &NodeId) -> Option<TaskId> {
        self.state.as_ref()?.tasks().iter().find(|t| t.node.as_ref() == Some(node)).map(|t| t.id.clone())
    }

    /// The latest stored value of a slot (None = no version yet, or the file could not be read: "not obtained").
    pub(crate) fn slot_latest(&self, slot: &SlotId) -> Option<&Value> {
        let v = self.state.as_ref()?.slot_version(slot)?;
        self.slot_values.get(&(slot.clone(), v))
    }

    fn slot_schema(&self, g: &CompiledGraph, slot: &SlotId) -> Option<&OutSchema> {
        g.def.slots.get(slot).and_then(|d| self.schemas.get(&d.schema))
    }

    /// The slot a node writes: an agent's `output`, or a gate's own id.
    fn output_slot_of(g: &CompiledGraph, node: &NodeId) -> Option<SlotId> {
        match &g.node(node)?.kind {
            GNode::Agent { output, .. } => Some(output.clone()),
            GNode::Gate { .. } => Some(SlotId::new(node.0.clone())),
            _ => None,
        }
    }

    fn status_of(&self, id: &TaskId) -> Option<TaskStatus> {
        self.task(id).map(|t| t.status())
    }

    fn complete_task(&mut self, id: &TaskId) -> bool {
        if self.set_status(id, TaskStatus::Completed, Cause::Engine) {
            self.notify(OrgNotice::TaskCompleted { task: id.clone() });
            true
        } else {
            false
        }
    }

    /// The task cannot go on (a value is missing, the target is in an odd state). Not guessed: a task that can still be
    /// blocked is blocked with the reason; a `completed` one (restore) cannot be, so the reason is reported once.
    fn edge_unresolved(&mut self, id: &TaskId, why: &str) {
        let terminal = self.status_of(id).is_none_or(|s| matches!(s, TaskStatus::Completed | TaskStatus::Failed(_) | TaskStatus::Cancelled));
        if !terminal {
            self.block(id, BlockedReason::NeedsAttention, &format!("次の工程を選べません: {why}"));
        } else if self.edge_stuck.insert(id.clone()) {
            self.notify(OrgNotice::TaskBlocked { task: id.clone(), why: format!("次の工程を選べません: {why}") });
        }
    }

    /// Write a slot value: `PutSlot` first, then `Persist(SlotWritten)`.
    fn write_slot(&mut self, id: &TaskId, slot: SlotId, value: Value) -> bool {
        let version = self.s().slot_version(&slot).unwrap_or(0) + 1;
        let bytes = serde_json::to_string(&value).map(|s| s.len() as u32).unwrap_or(0);
        self.slot_values.insert((slot.clone(), version), value.clone());
        self.out.push(OrgAction::PutSlot { slot: slot.clone(), version, value });
        if self.rec(OrgEventKind::SlotWritten { task: id.clone(), slot: slot.clone(), version, bytes }) {
            true
        } else {
            self.slot_values.remove(&(slot, version));
            self.out.pop();
            false
        }
    }

    pub(crate) fn add_note(&mut self, text: &str, scope: NoteScope) -> Option<NoteId> {
        let id = NoteId::new(format!("note-{}", self.s().last_seq() + 1));
        let text: String = text.chars().take(NOTE_MAX_CHARS).collect();
        self.rec(OrgEventKind::UserNoteAdded { id: id.clone(), text, scope }).then_some(id)
    }

    // ------------------------------------------------------------ start and activation

    pub(crate) fn start_graph(&mut self, workflow: WorkflowId, request: String, repo: String, limits: Limits, roles_fingerprint: String) {
        if self.state.is_some() {
            self.notify(OrgNotice::Refused { reason: "ワークフローは既に開始されています".into() });
            return;
        }
        let Some(g) = self.graph.clone() else {
            self.notify(OrgNotice::Refused { reason: "グラフ定義が設定されていません".into() });
            return;
        };
        if g.def.workspace != WorkspaceKind::Git {
            self.notify(OrgNotice::Refused { reason: "folder のグラフにはまだ対応していません".into() });
            return;
        }
        if g.def.nodes.iter().any(|n| matches!(n.kind, GNode::PlannedTasks { .. })) {
            self.notify(OrgNotice::Refused { reason: "plannedTasks を含むグラフは標準の経路で動かします".into() });
            return;
        }
        self.state = Some(WorkflowState::new(workflow.clone(), limits.clone()));
        self.out.push(OrgAction::Persist(OrgEvent {
            schema_version: SCHEMA_VERSION,
            seq: 0,
            at_ms: self.now_ms,
            kind: OrgEventKind::Created { workflow_id: workflow, limits },
        }));
        self.rec(OrgEventKind::Requested { request, repo });
        self.rec(OrgEventKind::GraphBound { graph_id: g.def.id.clone(), fingerprint: g.fingerprint.clone(), roles_fingerprint });
        let entry = g.def.entry.clone();
        self.activate(&g, &entry, None, None);
    }

    /// The task of a node, created when the first edge reaches it. Reuses a task that already exists (a restore may
    /// choose the same edge again).
    fn activate(&mut self, g: &CompiledGraph, node: &NodeId, parent: Option<&TaskId>, via: Option<EdgeId>) -> Option<TaskId> {
        if let Some(t) = self.task_of_node(node) {
            return Some(t);
        }
        let def = g.node(node)?;
        let (role, instructions) = match &def.kind {
            GNode::Agent { role, instructions, .. } => (role.clone(), instructions.clone()),
            GNode::Gate { .. } => (RoleId::new(ROLE_GATE), String::new()),
            GNode::Router => (RoleId::new(ROLE_ROUTER), String::new()),
            GNode::PlannedTasks { .. } => return None,
        };
        let mut n = 1;
        let id = loop {
            let id = TaskId::new(format!("{node}-{n}"));
            if self.task(&id).is_none() {
                break id;
            }
            n += 1;
        };
        let task = PlanTask {
            id: id.clone(),
            role,
            deps: parent.cloned().into_iter().collect(),
            effects: vec![],
            title: def.label.clone().unwrap_or_else(|| node.0.clone()),
            instructions,
        };
        self.rec(OrgEventKind::ActivationAdded { task, node: node.clone(), via }).then_some(id)
    }

    // ------------------------------------------------------------ launch and turn text

    pub(crate) fn graph_launch(&mut self, id: &TaskId) {
        let Some(g) = self.graph.clone() else {
            self.block(id, BlockedReason::NeedsAttention, "グラフ定義がありません");
            return;
        };
        let Some(t) = self.task(id) else { return };
        let Some(node_id) = t.node.clone() else { return };
        let Some(def) = g.node(&node_id) else { return };
        let GNode::Agent { capture, target, .. } = &def.kind else { return };
        let (has_chat, has_cwd, accepted) = (t.chat().is_some(), t.chat_cwd().is_some(), t.accepted_turn().is_some());
        let target_commit = target.as_ref().and_then(|n| self.task_of_node(n)).and_then(|tid| self.task(&tid)).and_then(|t| t.commit().map(str::to_string));
        if has_chat {
            if target.is_some() && accepted {
                // A reader goes back to its target's newest fixed result before it is sent anything (R-ORG M1).
                let (Some(commit), Some(cwd)) = (target_commit, self.task(id).and_then(|t| t.chat_cwd().map(str::to_string))) else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットまたは作業場所がありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Refresh);
                self.out.push(OrgAction::Workspace(WorkspaceAction::RefreshDetached { task: id.clone(), cwd, commit }));
                return;
            }
            self.continue_launch(id);
            return;
        }
        if has_cwd {
            self.block_unknown_create(id);
            return;
        }
        if let Some(cwd) = self.rechat_cwd.remove(id) {
            self.begin_chat(id, cwd);
            return;
        }
        let repo = self.s().repo().to_string();
        match (capture, target) {
            (Some(CaptureKind::Commit), _) => {
                let base = match self.writer_base(&g, &node_id) {
                    Ok(b) => b,
                    Err(why) => {
                        self.block(id, BlockedReason::NeedsAttention, &why);
                        return;
                    }
                };
                self.launching.insert(id.clone(), Launch::Worktree);
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: id.clone(), repo, base }));
            }
            (Some(CaptureKind::Snapshot), _) => self.block(id, BlockedReason::NeedsAttention, "snapshot の確定にはまだ対応していません"),
            (None, Some(_)) => {
                let Some(commit) = target_commit else {
                    self.block(id, BlockedReason::NeedsAttention, "検証対象のコミットがありません");
                    return;
                };
                self.launching.insert(id.clone(), Launch::Worktree);
                self.out.push(OrgAction::Workspace(WorkspaceAction::CreateWorktree { task: id.clone(), repo, base: WorktreeBase::DetachedAt(commit) }));
            }
            (None, None) => self.begin_chat(id, repo),
        }
    }

    /// Where a writer's new branch starts: the newest fixed commit of the nearest writer before it in the graph
    /// (DESIGN_ORG_GRAPH 9.1), or HEAD when there is none. A writer before it without a commit is an error: nothing is
    /// guessed (the branch would silently miss the earlier work).
    fn writer_base(&self, g: &CompiledGraph, node: &NodeId) -> Result<WorktreeBase, String> {
        let mut cur = node.clone();
        for _ in 0..=g.def.nodes.len() {
            let Some(Some(p)) = g.parent.get(&cur) else { return Ok(WorktreeBase::Head) };
            if matches!(g.node(p).map(|d| &d.kind), Some(GNode::Agent { capture: Some(CaptureKind::Commit), .. })) {
                let commit = self.task_of_node(p).and_then(|t| self.task(&t)).and_then(|t| t.commit().map(str::to_string));
                return commit.map(WorktreeBase::FromCommit).ok_or_else(|| format!("前の書込み工程 {p} のコミットがありません"));
            }
            cur = p.clone();
        }
        Ok(WorktreeBase::Head)
    }

    pub(crate) fn graph_ready_turn(&self, id: &TaskId) -> Option<(TurnKind, String)> {
        let g = self.graph.as_ref()?;
        let t = self.task(id)?;
        let node = t.node.as_ref()?;
        let GNode::Agent { role, reads, instructions, capture, target, .. } = &g.node(node)?.kind else { return None };
        let s = self.s();
        let rework = t.accepted_turn().is_some();
        let workplace = if capture.is_some() {
            "専用の作業場所で作業します。変更はこの場所で行い、確定（コミット）はAgentDockが行うので、gitコミットはしません。".to_string()
        } else if let Some(tn) = target {
            let commit = self.task_of_node(tn).and_then(|tid| self.task(&tid)).and_then(|x| x.commit().map(str::to_string))?;
            format!("ファイルは変更せず、読むことだけをします。検証対象: 工程 {tn} のコミット {commit}（作業ツリーが古い場合は `git show {commit}` などで確認してください）。")
        } else {
            "依頼の作業フォルダを読み取り専用で調べます。ファイルは変更しません。".to_string()
        };
        let round = g.loops.iter().find(|l| l.members.contains(node)).map(|l| {
            let (used, extra) = s.feedback_of(&l.edge);
            (used + 1, l.max_rounds + extra + 1)
        });
        let slot_text = |slot: &SlotId, label: String| (label, self.slot_latest(slot).map(pretty));
        let reads: Vec<(String, Option<String>)> = reads.iter().map(|sl| slot_text(sl, format!("スロット {sl} の最新の内容"))).collect();
        let mut feedback = Vec::new();
        for l in g.loops.iter().filter(|l| &l.target == node && s.feedback_of(&l.edge).0 > 0) {
            if let Some(slot) = Self::output_slot_of(g, &l.source) {
                feedback.push(slot_text(&slot, format!("工程 {} からの差し戻し（スロット {slot} の最新の内容）", l.source)));
            }
        }
        let seen = t.last_output_seq().unwrap_or(0);
        let notes: Vec<String> = s
            .notes()
            .iter()
            .filter(|n| n.seq > seen && match &n.scope {
                NoteScope::Node { node: nn } => nn == node,
                NoteScope::AllFollowing => true,
            })
            .map(|n| n.text.clone())
            .collect();
        let cfg = self.role_cfg.get(role);
        let text = prompt::node_prompt(&NodePrompt {
            role: &role.0,
            workplace,
            round,
            role_instructions: cfg.map(|c| c.instructions.as_str()).unwrap_or(""),
            node_instructions: instructions,
            request: s.request(),
            reads,
            feedback,
            notes,
            rework,
        });
        let kind = if rework {
            TurnKind::Rework
        } else if target.is_some() {
            TurnKind::Review
        } else {
            TurnKind::Task
        };
        Some((kind, text))
    }

    pub(crate) fn graph_schema_for(&self, id: &TaskId) -> Value {
        let (Some(g), Some(t)) = (self.graph.as_ref(), self.task(id)) else { return Value::Null };
        let Some(GNode::Agent { output, .. }) = t.node.as_ref().and_then(|n| g.node(n)).map(|d| &d.kind) else { return Value::Null };
        self.slot_schema(g, output).map(OutSchema::to_json).unwrap_or(Value::Null)
    }

    // ------------------------------------------------------------ an agent's output

    pub(crate) fn graph_parse_output(&self, id: &TaskId, text: &str) -> Result<Output, String> {
        let g = self.graph.as_ref().ok_or("グラフ定義がありません")?;
        let node = self.task(id).and_then(|t| t.node.as_ref()).ok_or("工程がありません")?;
        let Some(GNode::Agent { output, .. }) = g.node(node).map(|d| &d.kind) else { return Err("agent の工程ではありません".into()) };
        let schema = self.slot_schema(g, output).ok_or_else(|| format!("スロット「{output}」のスキーマがありません"))?;
        let value: Value = serde_json::from_str(text.trim()).map_err(|e| format!("JSONを解析できません: {e}"))?;
        schema.validate(&value).map_err(|errs| errs.join("\n"))?;
        if serde_json::to_string(&value).map_or(0, |s| s.len()) > MAX_SLOT_BYTES {
            return Err("出力が大きすぎます（256KBまで）".into());
        }
        Ok(Output::Slot { slot: output.clone(), value })
    }

    /// The agent's output passed the format check and the children are settled: store it, fix it (commit), choose the edge.
    pub(crate) fn graph_complete_slot(&mut self, id: &TaskId, slot: SlotId, value: Value) {
        // Only a live, confirmed result counts: after a disconnect or an unknown send nothing is decided (Freshness).
        if !self.task(id).is_some_and(|t| t.status() == TaskStatus::Running && t.confirm() == ConfirmState::Confirmed) {
            return;
        }
        let Some(g) = self.graph.clone() else { return };
        let capture = self.task(id).and_then(|t| t.node.as_ref()).and_then(|n| g.node(n)).and_then(|d| match &d.kind {
            GNode::Agent { capture, .. } => *capture,
            _ => None,
        });
        if !self.write_slot(id, slot, value) {
            return;
        }
        match capture {
            Some(CaptureKind::Commit) => self.flow_set(id.clone(), Flow::WantCommit),
            Some(CaptureKind::Snapshot) => self.block(id, BlockedReason::NeedsAttention, "snapshot の確定にはまだ対応していません"),
            None => self.settle_node(id),
        }
    }

    /// The commit was made (`Committed` is already recorded): the edge is chosen now.
    pub(crate) fn graph_committed(&mut self, id: &TaskId) {
        self.settle_node(id);
    }

    // ------------------------------------------------------------ edge choice

    /// A loop is closed once its source took a forward edge (or ended the path) and its target is not waiting for the user.
    fn loop_closed(&self, l: &LoopBody) -> bool {
        let Some(src) = self.task_of_node(&l.source).and_then(|tid| self.task(&tid)) else { return false };
        match src.status() {
            TaskStatus::Completed => true,
            TaskStatus::Reviewing => {
                let settled = src.edge_seq().is_some() && src.edge_seq() >= src.last_output_seq();
                let waiting = self.task_of_node(&l.target).and_then(|tid| self.status_of(&tid)) == Some(TaskStatus::Blocked(BlockedReason::RoundLimit));
                settled && !waiting
            }
            _ => false,
        }
    }

    /// Some loop that contains `node` is still open (and is not one of the loops being closed right now).
    fn open_loop_holds(&self, g: &CompiledGraph, node: &NodeId, closing: &HashSet<EdgeId>) -> bool {
        g.loops.iter().any(|l| l.members.contains(node) && !closing.contains(&l.edge) && !self.loop_closed(l))
    }

    /// Choose and take the edge out of the task's node. Does nothing unless the task is confirmed.
    pub(crate) fn settle_node(&mut self, id: &TaskId) {
        let Some(g) = self.graph.clone() else {
            self.edge_unresolved(id, "グラフ定義がありません");
            return;
        };
        let Some(t) = self.task(id) else { return };
        if t.confirm() != ConfirmState::Confirmed || !matches!(t.status(), TaskStatus::Running | TaskStatus::Reviewing | TaskStatus::Completed) {
            return;
        }
        let Some(node) = t.node.clone() else { return };
        let route = select_edge(&g, &node, |s| self.slot_latest(s));
        match route {
            Route::Unresolved(why) => self.edge_unresolved(id, &why),
            Route::End => self.take_forward(id, &g, &node, None),
            Route::Edge(e) if e.feedback.is_none() => self.take_forward(id, &g, &node, Some(e)),
            Route::Edge(e) => self.take_feedback(id, &g, e),
        }
    }

    fn take_forward(&mut self, id: &TaskId, g: &CompiledGraph, node: &NodeId, edge: Option<&EdgeDef>) {
        let closing: HashSet<EdgeId> = g.loops.iter().filter(|l| &l.source == node).map(|l| l.edge.clone()).collect();
        let want = if self.open_loop_holds(g, node, &closing) { TaskStatus::Reviewing } else { TaskStatus::Completed };
        match (self.status_of(id), want) {
            (Some(TaskStatus::Running), TaskStatus::Reviewing) => {
                if !self.set_status(id, TaskStatus::Reviewing, Cause::Engine) {
                    return;
                }
            }
            (Some(TaskStatus::Running | TaskStatus::Reviewing), TaskStatus::Completed) => {
                if !self.complete_task(id) {
                    return;
                }
            }
            _ => {}
        }
        let to = match edge {
            Some(e) => match self.activate(g, &e.to, Some(id), Some(e.id())) {
                Some(t) => Some(t),
                None => return,
            },
            None => None,
        };
        self.rec(OrgEventKind::EdgeTaken { from: id.clone(), edge: edge.map(EdgeDef::id), to });
        self.close_bodies(g, node, &closing);
    }

    /// The loops `node` closes: their members that only waited for the loop are complete now.
    fn close_bodies(&mut self, g: &CompiledGraph, node: &NodeId, closing: &HashSet<EdgeId>) {
        for l in g.loops.iter().filter(|l| closing.contains(&l.edge)) {
            for m in l.members.iter().filter(|m| *m != node) {
                let Some(tid) = self.task_of_node(m) else { continue };
                let holds = self.open_loop_holds(g, m, closing);
                if !holds && self.task(&tid).is_some_and(|t| t.status() == TaskStatus::Reviewing && t.confirm() == ConfirmState::Confirmed) {
                    self.complete_task(&tid);
                }
            }
        }
    }

    fn take_feedback(&mut self, id: &TaskId, g: &CompiledGraph, e: &EdgeDef) {
        let eid = e.id();
        let (Some(l), Some(fb)) = (g.loops.iter().find(|l| l.edge == eid), e.feedback.as_ref()) else {
            self.edge_unresolved(id, "差し戻しの辺の本体を特定できません");
            return;
        };
        let Some(target) = self.task_of_node(&l.target) else {
            self.edge_unresolved(id, "戻り先の工程がまだありません");
            return;
        };
        let (used, extra) = self.s().feedback_of(&eid);
        let remaining = (fb.max_rounds + extra).saturating_sub(used);
        let target_status = self.status_of(&target);
        if remaining > 0 {
            if target_status != Some(TaskStatus::Reviewing) {
                self.edge_unresolved(id, "戻り先が差し戻せる状態ではありません");
                return;
            }
            self.apply_feedback(id, &target, l, &eid);
            return;
        }
        // Out of rounds. The target is left as the definition says; the edge decision is recorded last (a repeat of
        // this choice after a crash finds the target already changed and only records it).
        match fb.on_exhausted {
            crate::graph::OnExhausted::Fail => {
                if target_status == Some(TaskStatus::Reviewing) {
                    self.fail(&target, FailReason::LimitReached);
                }
                if matches!(self.status_of(id), Some(TaskStatus::Running | TaskStatus::Reviewing)) {
                    self.complete_task(id);
                }
            }
            crate::graph::OnExhausted::Ask => {
                if target_status == Some(TaskStatus::Reviewing) {
                    self.block(&target, BlockedReason::RoundLimit, &format!("工程 {} へのやり直しが上限（最大{}回）に達しました。理由を書いてもう1回やり直すか、取り消すか選んでください", l.target, fb.max_rounds + extra));
                }
                // The source stays under review: it must be able to wait again if the user adds a round.
                if self.status_of(id) == Some(TaskStatus::Running) {
                    self.set_status(id, TaskStatus::Reviewing, Cause::Engine);
                }
            }
        }
        self.rec(OrgEventKind::EdgeTaken { from: id.clone(), edge: Some(eid), to: None });
    }

    /// Go back along a feedback edge: record and count the round, then send the loop's members back
    /// (target: ready again, the others: wait for their parent's new output).
    fn apply_feedback(&mut self, source: &TaskId, target: &TaskId, l: &LoopBody, eid: &EdgeId) {
        self.rec(OrgEventKind::EdgeTaken { from: source.clone(), edge: Some(eid.clone()), to: Some(target.clone()) });
        self.rec(OrgEventKind::FeedbackCounted { edge: eid.clone() });
        for m in &l.members {
            let Some(tid) = self.task_of_node(m) else { continue };
            if &tid == target {
                self.set_status(&tid, TaskStatus::Rework, Cause::Engine);
                self.set_status(&tid, TaskStatus::Ready, Cause::Engine);
            } else if matches!(self.status_of(&tid), Some(TaskStatus::Running | TaskStatus::Reviewing)) {
                self.set_status(&tid, TaskStatus::Rework, Cause::Engine);
            }
        }
    }

    // ------------------------------------------------------------ user inputs

    pub(crate) fn user_gate(&mut self, id: &TaskId, choice: &str, note: Option<String>) {
        let Some(g) = self.graph.clone() else { return };
        let Some(t) = self.task(id) else { return };
        let waiting = t.status() == TaskStatus::Blocked(BlockedReason::AwaitingApproval) && t.confirm() == ConfirmState::Confirmed;
        let Some(node) = t.node.clone() else { return };
        let Some(GNode::Gate { choices, .. }) = g.node(&node).map(|d| &d.kind) else {
            self.notify(OrgNotice::Refused { reason: "判断を求めている工程ではありません".into() });
            return;
        };
        let note = nonblank(note);
        let Some(c) = choices.iter().find(|c| c.id == choice) else {
            self.notify(OrgNotice::Refused { reason: "選択肢にない判断です".into() });
            return;
        };
        if !waiting || (c.needs_note && note.is_none()) {
            self.notify(OrgNotice::Refused { reason: "この判断には理由が必要です（または、判断を待っていません）".into() });
            return;
        }
        // The reason goes to the next turn of the node this choice leads to.
        let leads_to = g
            .outgoing(&node)
            .find(|e| e.when.as_ref().and_then(|w| w.eq.as_ref()).and_then(Value::as_str) == Some(choice))
            .map(|e| e.to.clone())
            .unwrap_or_else(|| node.clone());
        let note_id = note.as_deref().and_then(|text| self.add_note(text, NoteScope::Node { node: leads_to }));
        if !self.rec(OrgEventKind::GateDecided { task: id.clone(), choice: choice.to_string(), note: note_id }) {
            return;
        }
        // The slot value is cut like the note itself (a long reason must not break the slot write; R-G M7).
        let kept: String = note.unwrap_or_default().chars().take(NOTE_MAX_CHARS).collect();
        let value = json!({ "choice": choice, "note": kept });
        if !self.set_status(id, TaskStatus::Running, Cause::User) {
            return;
        }
        if self.write_slot(id, SlotId::new(node.0.clone()), value) {
            self.settle_node(id);
        }
    }

    pub(crate) fn user_extra_round(&mut self, id: &TaskId, note: Option<String>) {
        let Some(g) = self.graph.clone() else { return };
        let Some(t) = self.task(id) else { return };
        let waiting = t.status() == TaskStatus::Blocked(BlockedReason::RoundLimit) && t.confirm() == ConfirmState::Confirmed;
        let Some(node) = t.node.clone() else { return };
        let found = self.exhausted_loop(&g, &node);
        let Some((source, l)) = found.filter(|_| waiting) else {
            self.notify(OrgNotice::Refused { reason: "上限に達したやり直しを待っていないため、もう1回やり直せません".into() });
            return;
        };
        if let Some(text) = nonblank(note) {
            self.add_note(&text, NoteScope::Node { node });
        }
        if !self.rec(OrgEventKind::FeedbackAllowanceRaised { edge: l.edge.clone(), by: 1 }) {
            return;
        }
        let eid = l.edge.clone();
        self.apply_feedback(&source, id, &l, &eid);
    }

    /// The loop whose rounds ran out with `target_node` as its target: its source is under review, waiting on it.
    fn exhausted_loop(&self, g: &CompiledGraph, target_node: &NodeId) -> Option<(TaskId, LoopBody)> {
        g.loops.iter().find_map(|l| {
            if &l.target != target_node {
                return None;
            }
            let src = self.task_of_node(&l.source).filter(|s| self.status_of(s) == Some(TaskStatus::Reviewing))?;
            let (used, extra) = self.s().feedback_of(&l.edge);
            (used >= l.max_rounds + extra).then(|| (src, l.clone()))
        })
    }

    /// "Accept this result" for a loop whose rounds ran out: the blocked target goes back under review, the source's
    /// path ends here (`EdgeTaken` with no edge) and the body is closed like a pass. Nothing is sent. The
    /// source and the members stay under review when an outer loop still holds them.
    pub(crate) fn user_accept_round(&mut self, id: &TaskId) {
        let Some(g) = self.graph.clone() else { return };
        let Some(t) = self.task(id) else { return };
        let waiting = t.status() == TaskStatus::Blocked(BlockedReason::RoundLimit) && t.confirm() == ConfirmState::Confirmed;
        let Some(node) = t.node.clone() else { return };
        let found = self.exhausted_loop(&g, &node);
        let Some((source, l)) = found.filter(|_| waiting) else {
            self.notify(OrgNotice::Refused { reason: "上限に達した差し戻しを待っていないため、この結果で受け入れられません".into() });
            return;
        };
        if self.task(&source).is_none_or(|s| s.confirm() != ConfirmState::Confirmed) {
            self.notify(OrgNotice::Refused { reason: "検証側の確認状態が確定ではないため、この結果で受け入れられません".into() });
            return;
        }
        if !self.set_status(id, TaskStatus::Reviewing, Cause::User) {
            return;
        }
        let closing: HashSet<EdgeId> = g.loops.iter().filter(|x| x.source == l.source).map(|x| x.edge.clone()).collect();
        if !self.open_loop_holds(&g, &l.source, &closing) {
            self.complete_task(&source);
        }
        self.rec(OrgEventKind::EdgeTaken { from: source, edge: None, to: None });
        self.close_bodies(&g, &l.source, &closing);
    }

    // ------------------------------------------------------------ the pass (restore repair, waiting members, routers, gates)

    /// A task whose output is newer than its last edge decision and that is not held back (graph mode).
    fn edge_pending(t: &Task) -> bool {
        t.node.is_some()
            && t.confirm() == ConfirmState::Confirmed
            && matches!(t.status(), TaskStatus::Completed | TaskStatus::Reviewing)
            && match (t.last_output_seq(), t.edge_seq()) {
                (Some(o), e) => e.is_none_or(|e| e < o),
                (None, e) => e.is_none(),
            }
    }

    pub(crate) fn graph_done(&self) -> bool {
        let tasks = self.s().tasks();
        !tasks.is_empty() && tasks.iter().all(|t| t.status() == TaskStatus::Completed && t.confirm() == ConfirmState::Confirmed && !Self::edge_pending(t))
    }

    /// Once after a restore: states a crash between two events can leave behind that nothing would ever move on.
    /// They are only reported (the task is held with the reason); no edge is chosen again here and nothing goes on.
    /// The one exception: a gate / router that was `running` (it sent nothing to Codex) is repaired first (below).
    /// - the target of a feedback edge still in `rework` (it is made ready in the same step as the count);
    /// - members of a loop whose source already took its edge and whose target is settled, but that were not
    ///   completed (the loop was being closed) and that no other open loop holds.
    /// An undecided edge (output newer than the last edge) is not reported: pass A of `graph_pass` chooses it again.
    fn audit_restore(&mut self) {
        let Some(g) = self.graph.clone() else { return };
        // A gate or router that was `running` when the app stopped sent nothing to Codex (R-G M2):
        // - a router chooses again (pure, same result);
        // - a gate whose slot was written this round chooses the edge from it; otherwise the decision was lost
        //   before it was stored, so it asks the user again (never decided here).
        let running: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| t.status() == TaskStatus::Running && t.confirm() == ConfirmState::Confirmed && t.is_non_model_node())
            .map(|t| t.id.clone())
            .collect();
        for id in running {
            if !self.is_gate_task(&id) {
                self.settle_node(&id);
                continue;
            }
            let Some(t) = self.task(&id) else { continue };
            let parent_edge = t.deps.first().and_then(|d| self.task(d)).and_then(|d| d.edge_seq()).unwrap_or(0);
            if t.last_output_seq().is_some_and(|o| o > parent_edge) {
                self.settle_node(&id);
            } else {
                self.open_gate(&id);
            }
        }
        let none = HashSet::new();
        let mut found: Vec<(TaskId, String)> = Vec::new();
        for l in &g.loops {
            let tgt = self.task_of_node(&l.target).and_then(|t| self.task(&t));
            let src = self.task_of_node(&l.source).and_then(|t| self.task(&t));
            if let Some(t) = tgt.filter(|t| t.status() == TaskStatus::Rework && t.confirm() == ConfirmState::Confirmed) {
                found.push((t.id.clone(), format!("工程 {} の差し戻しが途中で止まっています（戻り先が再開の準備に入っていません）", l.target)));
            }
            let (Some(src), Some(tgt)) = (src, tgt) else { continue };
            let settled = src.edge_seq().is_some_and(|e| src.last_output_seq().is_none_or(|o| e >= o));
            if !(settled && matches!(src.status(), TaskStatus::Completed | TaskStatus::Reviewing) && matches!(tgt.status(), TaskStatus::Reviewing | TaskStatus::Completed)) {
                continue;
            }
            for m in &l.members {
                let Some(task) = self.task_of_node(m).and_then(|t| self.task(&t)) else { continue };
                if task.status() == TaskStatus::Reviewing && task.confirm() == ConfirmState::Confirmed && !self.open_loop_holds(&g, m, &none) {
                    found.push((task.id.clone(), format!("工程 {m} が、周回を閉じる途中で止まっています（完了にしていません）")));
                }
            }
        }
        let mut seen = HashSet::new();
        for (id, why) in found {
            if seen.insert(id.clone()) {
                self.block(&id, BlockedReason::NeedsAttention, &format!("復元時の確認: {why}"));
            }
        }
    }

    pub(crate) fn graph_pass(&mut self) {
        if !self.is_graph_mode() || self.graph.is_none() {
            return;
        }
        if self.audit_pending {
            self.audit_pending = false;
            self.audit_restore();
        }
        // A. Edge decisions a crash left open: chosen again from the stored values (never from a guess).
        let pending: Vec<TaskId> = self.s().tasks().iter().filter(|t| Self::edge_pending(t) && !self.edge_stuck.contains(&t.id)).map(|t| t.id.clone()).collect();
        for id in pending {
            self.settle_node(&id);
        }
        // B. Members sent back wait until their parent has a newer output.
        let waiting: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| t.node.is_some() && t.status() == TaskStatus::Rework && t.confirm() == ConfirmState::Confirmed)
            .filter(|t| {
                // The parent must have taken the forward edge that leads here, after this task's last output. An edge
                // that ended in "out of rounds" (`to: None`) or went back along a feedback edge does not count (R-G H1).
                t.deps.first().and_then(|d| self.task(d)).is_some_and(|d| {
                    d.confirm() == ConfirmState::Confirmed
                        && matches!(d.status(), TaskStatus::Reviewing | TaskStatus::Completed)
                        && d.edge_to() == Some(&t.id)
                        && d.edge_seq().unwrap_or(0) > t.seen_seq()
                })
            })
            .map(|t| t.id.clone())
            .collect();
        for id in waiting {
            if self.is_gate_task(&id) {
                self.open_gate(&id);
            } else {
                self.set_status(&id, TaskStatus::Ready, Cause::Engine);
            }
        }
        // C. Routers run at once (no turn); a gate waits for the user.
        let ids: Vec<TaskId> = self
            .s()
            .tasks()
            .iter()
            .filter(|t| t.confirm() == ConfirmState::Confirmed && self.is_non_agent_task(&t.id))
            .filter(|t| matches!(t.status(), TaskStatus::Pending | TaskStatus::Ready))
            .filter(|t| t.deps.iter().all(|d| self.task(d).is_some_and(|dep| Self::dep_ok(dep, t))))
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            if self.is_gate_task(&id) {
                self.open_gate(&id);
            } else {
                self.run_router(&id);
            }
        }
    }

    fn is_gate_task(&self, id: &TaskId) -> bool {
        let Some(g) = self.graph.as_ref() else { return false };
        self.task(id).and_then(|t| t.node.as_ref()).and_then(|n| g.node(n)).is_some_and(|n| matches!(n.kind, GNode::Gate { .. }))
    }

    /// The gate asks the user. Nothing decides it but the user's input.
    fn open_gate(&mut self, id: &TaskId) {
        let prompt = self
            .graph
            .as_ref()
            .and_then(|g| self.task(id).and_then(|t| t.node.as_ref()).and_then(|n| g.node(n)))
            .and_then(|d| match &d.kind {
                GNode::Gate { prompt, .. } => Some(prompt.clone()),
                _ => None,
            })
            .unwrap_or_default();
        self.block(id, BlockedReason::AwaitingApproval, &prompt);
    }

    fn run_router(&mut self, id: &TaskId) {
        if self.status_of(id) == Some(TaskStatus::Pending) && !self.set_status(id, TaskStatus::Ready, Cause::Engine) {
            return;
        }
        if self.set_status(id, TaskStatus::Running, Cause::Engine) {
            self.settle_node(id);
        }
    }
}

#[cfg(test)]
mod tests;
