//! 過去のワークフローの読取り専用の一覧（再起動後も完了・失敗・取消し済みを画面に出し続ける）。
//!
//! 守ること:
//! - 保存データを読むだけ。追記・退避・状態の分類（`halted Restart`）・resume・送信・照合をしない（`ReadOnlyOrgStore`）。
//! - 読めない保存データは飛ばして、他のワークフローは出す。自動で消さない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use org_engine::{restore, OrgStore, WorkflowId};

use super::driver::snapshot_of;
use super::view::{workflow_view, ViewInput};
use crate::backend::org::OrgWorkflowView;
use crate::store::org::{list_workflow_dirs_newest, workflow_dir, ReadOnlyOrgStore, EVENTS_FILE, SNAPSHOT_FILE, USAGE_FILE};

/// 画面に出す過去のワークフローの件数（新しい順の先頭から。動いている1件を含む）。
pub const HISTORY_KEEP: usize = 10;

/// 保存ファイルの更新の目印（変わっていなければ読み直さない）。
type Stamp = [(Option<SystemTime>, u64); 3];

/// フォルダごとの読込み結果の控え。`None`＝読めなかった（次も同じ目印なら再度読まない）。
#[derive(Default)]
pub struct HistoryCache(HashMap<PathBuf, (Stamp, Option<OrgWorkflowView>)>);

fn stamp_of(dir: &Path) -> Stamp {
    let one = |name: &str| std::fs::metadata(dir.join(name)).map(|m| (m.modified().ok(), m.len())).unwrap_or((None, 0));
    [one(SNAPSHOT_FILE), one(EVENTS_FILE), one(USAGE_FILE)]
}

/// 1件を読み取り専用で読む。読めなければ None。
fn read_one(dir: &Path) -> Option<OrgWorkflowView> {
    let store = ReadOnlyOrgStore::new(dir.to_path_buf());
    let (state, _report) = restore(&store).ok()?;
    let snapshot = snapshot_of(&state, None);
    let usage_rows = store.read_usage().unwrap_or_default();
    let mut v = workflow_view(&ViewInput { snapshot: &snapshot, notices: &[], usage_rows: &usage_rows, slots: &Default::default(), dispatching: false });
    v.readonly = true;
    Some(v)
}

/// 保存済みの過去のワークフロー（新しい順に先頭 `keep` 件のうち、`active` 以外の読めるもの）。
pub fn read_history(root: &Path, active: Option<&WorkflowId>, keep: usize, cache: &mut HistoryCache) -> Vec<OrgWorkflowView> {
    let active_dir = active.map(|w| workflow_dir(root, &w.0));
    let dirs: Vec<PathBuf> = list_workflow_dirs_newest(root).into_iter().take(keep).collect();
    cache.0.retain(|k, _| dirs.contains(k));
    let mut out = Vec::new();
    for dir in dirs {
        if active_dir.as_ref() == Some(&dir) {
            continue;
        }
        let stamp = stamp_of(&dir);
        let entry = match cache.0.get(&dir) {
            Some((s, v)) if *s == stamp => v.clone(),
            _ => {
                let v = read_one(&dir);
                cache.0.insert(dir.clone(), (stamp, v.clone()));
                v
            }
        };
        out.extend(entry);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::{Cause, Limits, OrgEvent, OrgEventKind, PlanTask, RoleId, TaskId, TaskStatus, WorkflowState};
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);
    fn temp_root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-orghist-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn pt(id: &str) -> PlanTask {
        PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new() }
    }

    /// 保存済みのワークフローを作る。`last` はタスクの最終状態。
    fn seed(root: &Path, id: &str, last: TaskStatus) -> PathBuf {
        let dir = workflow_dir(root, id);
        let store = crate::store::org::FileOrgStore::new(dir.clone());
        let mut s = WorkflowState::new(WorkflowId::new(id), Limits::default());
        let created = OrgEvent { schema_version: 1, seq: 0, at_ms: 0, kind: OrgEventKind::Created { workflow_id: s.workflow_id().clone(), limits: s.limits().clone() } };
        store.append_event(&created).unwrap();
        let mut kinds = vec![OrgEventKind::TaskAdded { task: pt("t1") }, OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Ready, cause: Cause::Engine }];
        if last != TaskStatus::Ready {
            kinds.push(OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Running, cause: Cause::Engine });
        }
        if matches!(last, TaskStatus::Completed) {
            kinds.push(OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Completed, cause: Cause::Engine });
        }
        for k in kinds {
            let ev = s.record(k, 1).unwrap();
            store.append_event(&ev).unwrap();
        }
        store.save_snapshot(&s).unwrap();
        dir
    }

    fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        v.sort();
        v
    }

    #[test]
    fn a_finished_workflow_is_listed_after_restart_without_writing_anything() {
        let root = temp_root();
        let dir = seed(&root, "wf-100", TaskStatus::Completed);
        let before = files(&dir);
        let mut cache = HistoryCache::default();
        let h = read_history(&root, None, HISTORY_KEEP, &mut cache);
        assert_eq!(h.len(), 1);
        assert!(h[0].readonly && h[0].finished);
        assert_eq!(h[0].workflow_id, "wf-100");
        // events.jsonl・workflow.json は1バイトも変わらず、ファイルも増えない（halted Restart を追記しない）
        assert_eq!(files(&dir), before);
        // 目印が同じなら読み直さない（控えを使う）。動いているものは除く。
        assert_eq!(read_history(&root, None, HISTORY_KEEP, &mut cache).len(), 1);
        assert!(read_history(&root, Some(&WorkflowId::new("wf-100")), HISTORY_KEEP, &mut cache).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_newest_n_are_listed_newest_first() {
        let root = temp_root();
        for i in 1..=5 {
            seed(&root, &format!("wf-{}", 100 + i), TaskStatus::Completed);
        }
        let mut cache = HistoryCache::default();
        let ids: Vec<String> = read_history(&root, None, 3, &mut cache).into_iter().map(|v| v.workflow_id).collect();
        assert_eq!(ids, vec!["wf-105", "wf-104", "wf-103"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_workflow_is_skipped_and_the_others_are_listed_and_nothing_is_moved_aside() {
        let root = temp_root();
        seed(&root, "wf-101", TaskStatus::Completed);
        let bad = workflow_dir(&root, "wf-102");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(SNAPSHOT_FILE), b"{ broken").unwrap();
        std::fs::write(bad.join(EVENTS_FILE), b"not json\n").unwrap();
        seed(&root, "wf-103", TaskStatus::Completed);
        let before = files(&bad);
        let mut cache = HistoryCache::default();
        let ids: Vec<String> = read_history(&root, None, HISTORY_KEEP, &mut cache).into_iter().map(|v| v.workflow_id).collect();
        assert_eq!(ids, vec!["wf-103", "wf-101"]);
        assert_eq!(files(&bad), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unfinished_older_workflow_is_listed_as_not_finished() {
        let root = temp_root();
        seed(&root, "wf-101", TaskStatus::Running);
        let h = read_history(&root, None, HISTORY_KEEP, &mut HistoryCache::default());
        assert_eq!(h.len(), 1);
        assert!(h[0].readonly && !h[0].finished);
        let _ = std::fs::remove_dir_all(&root);
    }
}
