//! 子孫ツリーの組立て（要件§3.5、M02・M03）。純粋関数。
//!
//! - 親子は明示された直接親ID（[`ParentLink::Explicit`]）だけで結ぶ。深さ・順序から親を推定しない。
//! - 親が不明な子、親がまだ見つかっていない子（遅延生成・ページ途中）、循環に入った子は
//!   ルート配下へ入れず `orphans` に理由つきで返す（捨てない）。
//! - 同じエージェントの重複（再走査・再開・通知とreadの重複）は1件にまとめる。

use crate::backend::model::{Agent, AgentKey, ParentLink};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode {
    pub agent: Agent,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanReason {
    /// 子であることは分かるが親IDが不明（「親不明」枠）。
    ParentUnknown,
    /// 親IDは明示されているが、その親が今回の集合にない（遅延生成・取りこぼし。再走査で解消し得る）。
    ParentMissing,
    /// 親の連鎖が循環している（自己参照を含む）。
    Cycle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Orphan {
    pub agent: Agent,
    pub reason: OrphanReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssembledTree {
    pub root: AgentKey,
    /// ルートの直接の子（再帰）。ルート自身はノードに含めない。
    pub children: Vec<TreeNode>,
    pub orphans: Vec<Orphan>,
    /// 重複として1件にまとめた数。
    pub duplicates_merged: usize,
}

impl AssembledTree {
    /// ルートから到達できたエージェント数。
    pub fn attached_count(&self) -> usize {
        fn count(n: &[TreeNode]) -> usize {
            n.iter().map(|x| 1 + count(&x.children)).sum()
        }
        count(&self.children)
    }

    /// 取りこぼしの可能性（再走査すべきか）。親不明は「不明」であり取りこぼしではない。
    pub fn needs_rescan(&self) -> bool {
        self.orphans.iter().any(|o| o.reason != OrphanReason::ParentUnknown)
    }
}

/// 重複時の採用: 親が明示されている方を優先し、同程度なら後から来た方（新しい観測）。
fn better(old: &Agent, new: &Agent) -> bool {
    let rank = |a: &Agent| match a.parent {
        ParentLink::Explicit { .. } => 2,
        ParentLink::Root => 1,
        ParentLink::Unknown => 0,
    };
    rank(new) >= rank(old)
}

pub fn assemble(root: &AgentKey, agents: &[Agent]) -> AssembledTree {
    // 重複を除く（入力順を保つ）。
    let mut order: Vec<AgentKey> = Vec::new();
    let mut by_key: HashMap<AgentKey, Agent> = HashMap::new();
    let mut duplicates_merged = 0;
    for a in agents {
        if &a.key == root {
            continue;
        }
        match by_key.get(&a.key) {
            None => {
                order.push(a.key.clone());
                by_key.insert(a.key.clone(), a.clone());
            }
            Some(old) => {
                duplicates_merged += 1;
                if better(old, a) {
                    by_key.insert(a.key.clone(), a.clone());
                }
            }
        }
    }

    // 親→子の索引（明示された親のみ。自己参照は除く）。
    let mut kids: HashMap<AgentKey, Vec<AgentKey>> = HashMap::new();
    for k in &order {
        if let ParentLink::Explicit { parent } = &by_key[k].parent {
            if parent != k {
                kids.entry(parent.clone()).or_default().push(k.clone());
            }
        }
    }

    // ルートから到達できるものを組み立てる（visitedで循環・多重到達を防ぐ）。
    let mut visited: HashSet<AgentKey> = HashSet::new();
    visited.insert(root.clone());
    fn build(key: &AgentKey, kids: &HashMap<AgentKey, Vec<AgentKey>>, by_key: &HashMap<AgentKey, Agent>, visited: &mut HashSet<AgentKey>) -> Vec<TreeNode> {
        let mut out = Vec::new();
        for c in kids.get(key).into_iter().flatten() {
            if !visited.insert(c.clone()) {
                continue;
            }
            let agent = by_key[c].clone();
            let children = build(c, kids, by_key, visited);
            out.push(TreeNode { agent, children });
        }
        out
    }
    let children = build(root, &kids, &by_key, &mut visited);

    // 到達できなかったものの理由を、親の連鎖をたどって決める。
    let mut orphans = Vec::new();
    for k in &order {
        if visited.contains(k) {
            continue;
        }
        let reason = orphan_reason(k, &by_key, root);
        orphans.push(Orphan { agent: by_key[k].clone(), reason });
    }

    AssembledTree { root: root.clone(), children, orphans, duplicates_merged }
}

fn orphan_reason(start: &AgentKey, by_key: &HashMap<AgentKey, Agent>, root: &AgentKey) -> OrphanReason {
    let mut seen: HashSet<AgentKey> = HashSet::new();
    let mut cur = start.clone();
    loop {
        if !seen.insert(cur.clone()) {
            return OrphanReason::Cycle;
        }
        match &by_key[&cur].parent {
            ParentLink::Unknown => return OrphanReason::ParentUnknown,
            // ルート（チャット本体）を名乗る別エージェントは、このルート配下ではない（親不明扱い）。
            ParentLink::Root => return OrphanReason::ParentUnknown,
            ParentLink::Explicit { parent } => {
                if parent == &cur {
                    return OrphanReason::Cycle;
                }
                if parent == root {
                    // ルート直下なら到達済みのはず。到達していないなら矛盾なので取りこぼし扱い。
                    return OrphanReason::ParentMissing;
                }
                match by_key.get(parent) {
                    None => return OrphanReason::ParentMissing,
                    Some(_) => cur = parent.clone(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{BackendKind, ChatKey, ExternalId, Known};

    fn key(id: &str) -> AgentKey {
        AgentKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }

    fn agent(id: &str, parent: ParentLink) -> Agent {
        Agent {
            key: key(id),
            chat: ChatKey { backend: BackendKind::Codex, id: ExternalId("root".into()) },
            parent,
            forked_from: Known::NotFetched,
            display_name: Known::NotFetched,
            role: Known::NotFetched,
            assignment: Known::NotFetched,
            agent_path: Known::NotFetched,
            latest_turn: None,
        }
    }

    fn child(id: &str, p: &str) -> Agent {
        agent(id, ParentLink::Explicit { parent: key(p) })
    }

    #[test]
    fn builds_nested_tree_regardless_of_input_order() {
        let t = assemble(&key("root"), &[child("gc", "c1"), child("c1", "root"), child("c2", "root")]);
        assert_eq!(t.attached_count(), 3);
        assert_eq!(t.children.len(), 2);
        let c1 = t.children.iter().find(|n| n.agent.key == key("c1")).unwrap();
        assert_eq!(c1.children[0].agent.key, key("gc"));
        assert!(t.orphans.is_empty());
        assert!(!t.needs_rescan());
    }

    #[test]
    fn unknown_parent_goes_to_orphan_frame_without_guessing() {
        let t = assemble(&key("root"), &[agent("x", ParentLink::Unknown), child("y", "root")]);
        assert_eq!(t.attached_count(), 1);
        assert_eq!(t.orphans.len(), 1);
        assert_eq!(t.orphans[0].reason, OrphanReason::ParentUnknown);
        assert!(!t.needs_rescan());
    }

    #[test]
    fn late_created_parent_is_missing_and_triggers_rescan() {
        // gc の親 c1 がまだ見つかっていない（ページ途中の生成）。
        let t = assemble(&key("root"), &[child("gc", "c1"), child("ggc", "gc")]);
        assert_eq!(t.attached_count(), 0);
        assert_eq!(t.orphans.len(), 2);
        assert!(t.orphans.iter().all(|o| o.reason == OrphanReason::ParentMissing));
        assert!(t.needs_rescan());
        // 後から親が見つかれば全員つながる。
        let t2 = assemble(&key("root"), &[child("gc", "c1"), child("ggc", "gc"), child("c1", "root")]);
        assert_eq!(t2.attached_count(), 3);
        assert!(t2.orphans.is_empty());
    }

    #[test]
    fn cycles_and_self_parent_do_not_loop_and_are_reported() {
        let t = assemble(&key("root"), &[child("a", "b"), child("b", "a"), child("s", "s"), child("ok", "root")]);
        assert_eq!(t.attached_count(), 1);
        assert_eq!(t.orphans.len(), 3);
        assert!(t.orphans.iter().all(|o| o.reason == OrphanReason::Cycle));
    }

    #[test]
    fn duplicates_are_merged_preferring_explicit_parent() {
        let t = assemble(&key("root"), &[agent("c", ParentLink::Unknown), child("c", "root"), child("c", "root"), agent("root", ParentLink::Root)]);
        assert_eq!(t.attached_count(), 1);
        assert_eq!(t.duplicates_merged, 2);
        assert!(t.orphans.is_empty());
    }

    #[test]
    fn explicit_parent_wins_over_later_unknown_duplicate() {
        let t = assemble(&key("root"), &[child("c", "root"), agent("c", ParentLink::Unknown)]);
        assert_eq!(t.attached_count(), 1);
    }
}
