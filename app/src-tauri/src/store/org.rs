//! 組織実行の保存（P-ORG-3、DESIGN_ORG §3.4）。`org-engine` の `OrgStore` のファイル実装。
//!
//! `%LOCALAPPDATA%\com.agentdock.app\org\<workflowId>\` に
//! `workflow.json`（原子的書換え）・`events.jsonl`（追記。末尾の壊れた行は復元時に無視して報告）・`usage.jsonl`（使用量の行）。
//! `graph.json`・`roles.json`（開始時の定義の写し）と `slots\`（スロットの値。`<スロットID>.<版>.json`）も同じフォルダ。
//! 期限・容量による自動削除はしない。読めない `workflow.json` は `.corrupt-<ms>` へ退避して残す（イベントから再構築できる）。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use org_engine::{
    parse_events, parse_usage, EventRead, OrgEvent, OrgStore, SnapshotLoad, StoreError, UsageRow, WorkflowState,
};
use org_engine::graph::SlotId;
use org_engine::TaskId;

use serde::{Deserialize, Serialize};

use super::atomic::{append_line, write_atomic};
use super::layout::{corrupt_aside_name, sanitize_file_name};
use crate::backend::model::UnixMillis;
use crate::rules::org_manage::DeleteRecord;

pub const SNAPSHOT_FILE: &str = "workflow.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const USAGE_FILE: &str = "usage.jsonl";
/// スロットの値のフォルダ（`slots\<スロットID>.<版>.json`）。
pub const SLOTS_DIR: &str = "slots";
/// 計画経路の Worker 報告・Reviewer 判定の控え（`outputs\<タスクID>.<試行>.json`。表示用でエンジンは読まない）。
pub const OUTPUTS_DIR: &str = "outputs";
/// 取り込み・戻すの台帳（追記のみ。DESIGN_ORG_R3 §2.6）。ワークフローの記録と一緒にだけ消える。
pub const IMPORTS_FILE: &str = "imports.jsonl";
/// スロットの値の大きさの上限（書くとき。エンジンの上限は256KB。読むときは余裕を見てこの2倍まで）。
pub const SLOT_MAX_BYTES: usize = 256 * 1024;

/// 削除の進みの記録（`rules::org_manage::DeleteRecord`）。あるフォルダは復元の対象にしない。
pub const DELETE_FILE: &str = "delete.json";
/// アーカイブした印（`{schemaVersion, archivedAt}`）。チャット側には書かない。
pub const LISTING_FILE: &str = "listing.json";
/// 開始時の依頼整理の要約の写し（`IntakeCopy`。依頼整理を経て始めたワークフローだけ）。
pub const INTAKE_FILE: &str = "intake.json";
const GRAPH_COPY_FILE: &str = "graph.json";
const ROLES_COPY_FILE: &str = "roles.json";

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
    fn slot_path(&self, slot: &SlotId, version: u32) -> PathBuf {
        self.dir.join(SLOTS_DIR).join(format!("{}.{version}.json", sanitize_file_name(&slot.0)))
    }
    fn output_path(&self, task: &TaskId, attempt: u32) -> PathBuf {
        self.dir.join(OUTPUTS_DIR).join(format!("{}.{attempt}.json", sanitize_file_name(&task.0)))
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

    fn put_slot(&self, slot: &SlotId, version: u32, value: &serde_json::Value) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(value).map_err(|e| StoreError::Io(e.to_string()))?;
        if bytes.len() > SLOT_MAX_BYTES {
            return Err(StoreError::Io("スロットの値が大きすぎます（256KBまで）".into()));
        }
        write_atomic(&self.slot_path(slot, version), &bytes).map_err(io)
    }

    fn put_output(&self, task: &TaskId, attempt: u32, value: &serde_json::Value) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(value).map_err(|e| StoreError::Io(e.to_string()))?;
        if bytes.len() > SLOT_MAX_BYTES {
            return Err(StoreError::Io("出力が大きすぎます（256KBまで）".into()));
        }
        write_atomic(&self.output_path(task, attempt), &bytes).map_err(io)
    }

    /// 読めない・壊れている・大きすぎるものは「未取得」（`None`）。
    fn read_output(&self, task: &TaskId, attempt: u32) -> Result<Option<serde_json::Value>, StoreError> {
        read_json_capped(&self.output_path(task, attempt))
    }

    /// 読めない・壊れている・大きすぎるものは「未取得」（`None`）。値を補ったり、別の版で代用したりしない。
    fn read_slot(&self, slot: &SlotId, version: u32) -> Result<Option<serde_json::Value>, StoreError> {
        read_json_capped(&self.slot_path(slot, version))
    }
}

fn read_json_capped(path: &Path) -> Result<Option<serde_json::Value>, StoreError> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() as usize > SLOT_MAX_BYTES * 2 => return Ok(None),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    }
    match std::fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice(&b).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

/// `<root>\org\` のワークフローのフォルダを新しい順に並べる（IDは `wf-<ms>` なので名前順＝作成順）。読取りだけ。
pub fn list_workflow_dirs_newest(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root.join("org")) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.reverse();
    dirs
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    schema_version: u32,
    archived_at: UnixMillis,
}

/// アーカイブした時刻。印がない・読めないときは None（一覧から隠さない側に倒す）。
pub fn read_archived_at(dir: &Path) -> Option<UnixMillis> {
    let b = std::fs::read(dir.join(LISTING_FILE)).ok()?;
    serde_json::from_slice::<Listing>(&b).ok().map(|l| l.archived_at)
}

/// アーカイブの印を書く（原子的書換え）。
pub fn write_archived(dir: &Path, at: UnixMillis) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&Listing { schema_version: 1, archived_at: at }).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(LISTING_FILE), &bytes).map_err(|e| e.to_string())
}

/// アーカイブの印を消す（なければ何もしない）。
pub fn remove_archived(dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(dir.join(LISTING_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// 削除の進みの記録。なければ `Ok(None)`、読めなければ `Err`（途中かどうか分からないので、呼び出し側は削除を進めない）。
pub fn read_delete_record(dir: &Path) -> Result<Option<DeleteRecord>, String> {
    match std::fs::read(dir.join(DELETE_FILE)) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("{DELETE_FILE} を読めません: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{DELETE_FILE} を読めません: {e}")),
    }
}

/// 削除の進みの記録を書く（原子的書換え）。書けなければ Err（削除は始めない）。
pub fn write_delete_record(dir: &Path, rec: &DeleteRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(rec).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(DELETE_FILE), &bytes).map_err(|e| format!("{DELETE_FILE} を書けません: {e}"))
}

/// 記録フォルダを順に消す: slots → outputs → graph・roles → imports → usage → events → workflow → listing → フォルダ（delete.json ごと）。
/// ない物は飛ばす。失敗したらそこで止めて理由を返す（前の段は消えたまま。`delete.json` は最後までフォルダに残る）。
pub fn remove_workflow_files(dir: &Path) -> Result<(), String> {
    let skip_missing = |r: std::io::Result<()>, name: &str| match r {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{name}: {e}")),
    };
    skip_missing(std::fs::remove_dir_all(dir.join(SLOTS_DIR)), SLOTS_DIR)?;
    skip_missing(std::fs::remove_dir_all(dir.join(OUTPUTS_DIR)), OUTPUTS_DIR)?;
    for name in [GRAPH_COPY_FILE, ROLES_COPY_FILE, IMPORTS_FILE, USAGE_FILE, EVENTS_FILE, SNAPSHOT_FILE, LISTING_FILE] {
        skip_missing(std::fs::remove_file(dir.join(name)), name)?;
    }
    skip_missing(std::fs::remove_dir_all(dir), "フォルダ")
}

/// 記録フォルダの合計の大きさ。読めない部分があれば None（0で代用しない）。
pub fn dir_bytes(dir: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()? {
            let e = e.ok()?;
            let m = e.metadata().ok()?;
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    Some(total)
}

/// 過去のワークフローを表示するための読取り専用の保存口。何も書かない・退避しない（読めない `workflow.json` はそのまま残し、
/// イベントから再構築する）。書込み系は常に失敗する。
pub struct ReadOnlyOrgStore {
    inner: FileOrgStore,
}

impl ReadOnlyOrgStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { inner: FileOrgStore::new(dir) }
    }
}

fn read_only() -> StoreError {
    StoreError::Io("read-only store".to_string())
}

impl OrgStore for ReadOnlyOrgStore {
    fn save_snapshot(&self, _state: &WorkflowState) -> Result<(), StoreError> {
        Err(read_only())
    }
    fn load_snapshot(&self) -> Result<SnapshotLoad, StoreError> {
        let Some(text) = self.inner.read_text(SNAPSHOT_FILE)? else { return Ok(SnapshotLoad::Missing) };
        // 読めなければ「ない」扱い（ファイルは動かさない）。
        Ok(serde_json::from_str::<WorkflowState>(&text).map_or(SnapshotLoad::Missing, SnapshotLoad::Loaded))
    }
    fn append_event(&self, _ev: &OrgEvent) -> Result<(), StoreError> {
        Err(read_only())
    }
    fn read_events(&self) -> Result<EventRead, StoreError> {
        self.inner.read_events()
    }
    fn append_usage(&self, _row: &UsageRow) -> Result<(), StoreError> {
        Err(read_only())
    }
    fn read_usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        self.inner.read_usage()
    }
    fn put_slot(&self, _slot: &SlotId, _version: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        Err(read_only())
    }
    fn read_slot(&self, slot: &SlotId, version: u32) -> Result<Option<serde_json::Value>, StoreError> {
        self.inner.read_slot(slot, version)
    }
    fn put_output(&self, _task: &TaskId, _attempt: u32, _value: &serde_json::Value) -> Result<(), StoreError> {
        Err(read_only())
    }
    fn read_output(&self, task: &TaskId, attempt: u32) -> Result<Option<serde_json::Value>, StoreError> {
        self.inner.read_output(task, attempt)
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
        PlanTask { id: TaskId::new(id), role: RoleId::new("worker"), deps: vec![], effects: vec![], title: String::new(), instructions: String::new(), kind: org_engine::TaskKind::Change }
    }

    fn seed(store: &FileOrgStore) -> WorkflowState {
        let mut s = WorkflowState::new(WorkflowId::new("w1"), Limits::default());
        let created = OrgEvent {
            schema_version: 1,
            seq: 0,
            at_ms: 0,
            kind: OrgEventKind::Created { workflow_id: s.workflow_id().clone(), limits: s.limits().clone() },
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
        assert_eq!(t.status(), TaskStatus::Running);
        assert_eq!(t.confirm(), ConfirmState::NeedsReconcile(ReconcileCause::Restart));
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
        assert_eq!(s.tasks(), live.tasks());
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

    fn rec(done: &[&str]) -> DeleteRecord {
        DeleteRecord { requested_at: UnixMillis(5), delete_chats: true, chats: vec![], done: done.iter().map(|s| s.to_string()).collect(), failed: vec![] }
    }

    #[test]
    fn delete_record_and_archive_mark_round_trip_and_unreadable_ones_are_reported() {
        let dir = temp();
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_delete_record(&dir).unwrap(), None);
        assert_eq!(read_archived_at(&dir), None);
        write_delete_record(&dir, &rec(&["a"])).unwrap();
        assert_eq!(read_delete_record(&dir).unwrap(), Some(rec(&["a"])));
        write_archived(&dir, UnixMillis(9)).unwrap();
        assert_eq!(read_archived_at(&dir), Some(UnixMillis(9)));
        remove_archived(&dir).unwrap();
        remove_archived(&dir).unwrap();
        assert_eq!(read_archived_at(&dir), None);
        std::fs::write(dir.join(DELETE_FILE), b"{ broken").unwrap();
        assert!(read_delete_record(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_step_stops_the_removal_and_keeps_delete_json_and_the_rest() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        seed(&store);
        store.put_slot(&SlotId::new("s"), 1, &serde_json::json!(1)).unwrap();
        write_delete_record(&dir, &rec(&[])).unwrap();
        // usage.jsonl cannot be removed as a file (it is a folder here), so the steps after it must not run
        std::fs::create_dir_all(dir.join(USAGE_FILE)).unwrap();
        let e = remove_workflow_files(&dir).unwrap_err();
        assert!(e.contains(USAGE_FILE), "{e}");
        assert!(!dir.join(SLOTS_DIR).exists(), "earlier steps are done");
        assert!(dir.join(EVENTS_FILE).exists() && dir.join(DELETE_FILE).exists(), "later steps and delete.json remain");
        // after the cause is gone the removal completes (the folder disappears with delete.json)
        std::fs::remove_dir_all(dir.join(USAGE_FILE)).unwrap();
        remove_workflow_files(&dir).unwrap();
        assert!(!dir.exists());
        remove_workflow_files(&dir).unwrap();
    }

    #[test]
    fn outputs_and_the_import_ledger_round_trip_and_go_with_the_workflow_record() {
        let dir = temp();
        let store = FileOrgStore::new(dir.clone());
        seed(&store);
        let v = serde_json::json!({"summary": "s"});
        assert_eq!(store.read_output(&TaskId::new("t/1"), 0).unwrap(), None, "not obtained is not guessed");
        store.put_output(&TaskId::new("t/1"), 0, &v).unwrap();
        assert_eq!(store.read_output(&TaskId::new("t/1"), 0).unwrap(), Some(v.clone()));
        assert_eq!(store.read_output(&TaskId::new("t/1"), 1).unwrap(), None);
        std::fs::write(dir.join(OUTPUTS_DIR).join("broken.0.json"), b"{ no").unwrap();
        assert_eq!(store.read_output(&TaskId::new("broken"), 0).unwrap(), None);
        let ro = ReadOnlyOrgStore::new(dir.clone());
        assert_eq!(ro.read_output(&TaskId::new("t/1"), 0).unwrap(), Some(v.clone()));
        assert!(ro.put_output(&TaskId::new("t/1"), 2, &v).is_err());
        std::fs::write(dir.join(IMPORTS_FILE), b"{}
").unwrap();
        remove_workflow_files(&dir).unwrap();
        assert!(!dir.exists());
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
