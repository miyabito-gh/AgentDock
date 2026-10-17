//! 組織実行の保存（P-ORG-3、DESIGN_ORG §3.4）。`org-engine` の `OrgStore` のファイル実装。
//!
//! `%LOCALAPPDATA%\com.agentdock.app\org\<workflowId>\` に
//! `workflow.json`（原子的書換え）・`events.jsonl`（追記。末尾の壊れた行は復元時に無視して報告）・`usage.jsonl`（使用量の行）。
//! 期限・容量による自動削除はしない。読めない `workflow.json` は `.corrupt-<ms>` へ退避して残す（イベントから再構築できる）。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use org_engine::{
    parse_events, parse_usage, EventRead, OrgEvent, OrgStore, SnapshotLoad, StoreError, UsageRow, WorkflowState,
};

use super::atomic::{append_line, write_atomic};
use super::layout::{corrupt_aside_name, sanitize_file_name};

pub const SNAPSHOT_FILE: &str = "workflow.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const USAGE_FILE: &str = "usage.jsonl";

/// `<root>\org\<workflowId>`。IDはファイル名として安全な形にする。
pub fn workflow_dir(root: &Path, workflow_id: &str) -> PathBuf {
    root.join("org").join(sanitize_file_name(workflow_id))
}

pub struct FileOrgStore {
    dir: PathBuf,
}

impl FileOrgStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn read_text(&self, name: &str) -> Result<Option<String>, StoreError> {
        match std::fs::read(self.dir.join(name)) {
            // 壊れた末尾にUTF-8でないバイトがあっても読めるようにする。
            Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl OrgStore for FileOrgStore {
    fn save_snapshot(&self, state: &WorkflowState) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|e| StoreError::Io(e.to_string()))?;
        write_atomic(&self.dir.join(SNAPSHOT_FILE), &bytes).map_err(io)
    }

    fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError> {
        let path = self.dir.join(SNAPSHOT_FILE);
        let Some(text) = self.read_text(SNAPSHOT_FILE)? else { return Ok(SnapshotLoad::Missing) };
        match serde_json::from_str::<WorkflowState>(&text) {
            Ok(s) => Ok(SnapshotLoad::Loaded(s)),
            Err(_) => {
                let aside = corrupt_aside_name(&path, now_ms());
                std::fs::rename(&path, &aside).map_err(io)?;
                Ok(SnapshotLoad::Corrupt { aside: aside.to_string_lossy().into_owned() })
            }
        }
    }

    fn append_event(&self, ev: &OrgEvent) -> Result<(), StoreError> {
        let line = serde_json::to_string(ev).map_err(|e| StoreError::Io(e.to_string()))?;
        append_line(&self.dir.join(EVENTS_FILE), &line).map_err(io)
    }

    fn read_events(&self) -> Result<EventRead, StoreError> {
        Ok(self.read_text(EVENTS_FILE)?.map(|t| parse_events(&t)).unwrap_or_default())
    }

    fn append_usage(&self, row: &UsageRow) -> Result<(), StoreError> {
        let line = serde_json::to_string(row).map_err(|e| StoreError::Io(e.to_string()))?;
        append_line(&self.dir.join(USAGE_FILE), &line).map_err(io)
    }

    fn read_usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        Ok(self.read_text(USAGE_FILE)?.map(|t| parse_usage(&t)).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_engine::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);
    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-org-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn pt(id: &str) -> PlanTask {
        PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![] }
    }

    fn seed(store: &FileOrgStore) -> WorkflowState {
        let mut s = WorkflowState::new(WorkflowId::new("w1"), Limits::default());
        let created = OrgEvent {
            schema_version: 1,
            seq: 0,
            at_ms: 0,
            kind: OrgEventKind::Created { workflow_id: s.workflow_id.clone(), limits: s.limits.clone() },
        };
        store.append_event(&created).unwrap();
        for k in [
            OrgEventKind::TaskAdded { task: pt("t1") },
            OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Ready, cause: Cause::Engine },
            OrgEventKind::StatusChanged { task: TaskId::new("t1"), to: TaskStatus::Running, cause: Cause::Engine },
        ] {
            let ev = s.record(k, 1).unwrap();
            store.append_event(&ev).unwrap();
        }
        s
    }

    #[test]
    fn torn_events_tail_is_ignored_and_running_becomes_needs_reconcile() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        seed(&store);
        // 異常終了で最終行が途中で切れた状態
        let p = dir.join(EVENTS_FILE);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        std::io::Write::write_all(&mut f, b"{\"schemaVersion\":1,\"seq\":9,\"atMs\":1,\"type\":\"turnS").unwrap();
        drop(f);
        let r = recover_on_restart(&store, 10).unwrap();
        assert!(r.report.dropped_tail.is_some());
        let t = r.state.task(&TaskId::new("t1")).unwrap();
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.confirm, ConfirmState::NeedsReconcile(ReconcileCause::Restart));
        // 分類結果と新しい snapshot が保存され、次回の復元でも同じ状態（壊れた行は途中行として報告される）
        let again = recover_on_restart(&store, 11).unwrap();
        assert_eq!(again.state, r.state);
        assert!(again.reclassified.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_snapshot_is_moved_aside_and_rebuilt_from_events() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        let live = seed(&store);
        std::fs::write(dir.join(SNAPSHOT_FILE), b"{ broken").unwrap();
        let (s, rep) = restore(&store).unwrap();
        assert_eq!(s.tasks, live.tasks);
        let aside = rep.snapshot_corrupt_aside.expect("moved aside");
        assert!(Path::new(&aside).exists());
        assert!(!dir.join(SNAPSHOT_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_rewrite_is_atomic_and_leaves_no_temp_files() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        let mut s = seed(&store);
        store.save_snapshot(&s).unwrap();
        s.record(OrgEventKind::TurnSpent { kind: TurnKind::Task }, 2).unwrap();
        store.save_snapshot(&s).unwrap();
        assert_eq!(store.load_snapshot().unwrap(), SnapshotLoad::Loaded(s));
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.contains(".tmp-")), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_rows_round_trip() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        let row = UsageRow {
            schema_version: 1,
            at_ms: 1,
            workflow_id: WorkflowId::new("w1"),
            task: Some(TaskId::new("t1")),
            kind: TurnKind::Task,
            turns_used: 1,
            last_total_tokens: Some(17448),
            thread_total_tokens: None,
            model_context_window: Some(475000),
        };
        store.append_usage(&row).unwrap();
        assert_eq!(store.read_usage().unwrap(), vec![row]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
