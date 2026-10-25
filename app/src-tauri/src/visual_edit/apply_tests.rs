use super::{
    apply::{
        apply_checked, open_exclusive_existing, reconcile, HostWritePermit, ReconcileState,
        WriteOutcome, WritePurpose, TEST_FAIL_AFTER_FIRST,
    },
    FileAccess, FileScope, ProjectRoot, SelectedFile, Snapshot,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "visual-apply-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        fs::create_dir_all(base.join("source")).unwrap();
        fs::create_dir_all(base.join("journals")).unwrap();
        Self { base }
    }
    fn root(&self) -> ProjectRoot {
        ProjectRoot::open(&self.base.join("source")).unwrap()
    }
    fn source(&self, path: &str) -> PathBuf {
        self.base.join("source").join(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let full = self.source(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, bytes).unwrap();
    }
    fn journals(&self) -> PathBuf {
        self.base.join("journals")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn scope(paths: &[&str]) -> FileScope {
    FileScope::new(
        paths
            .iter()
            .map(|path| SelectedFile {
                path: (*path).to_owned(),
                access: FileAccess::Edit,
            })
            .collect(),
    )
    .unwrap()
}
fn apply(
    f: &Fixture,
    scope: &FileScope,
    before: &Snapshot,
    after: &Snapshot,
    id: &str,
) -> WriteOutcome {
    let mut permit = HostWritePermit::after_host_checks();
    apply_checked(
        &mut permit,
        &f.root(),
        scope,
        before,
        after,
        &f.journals(),
        id,
        WritePurpose::AdoptSource,
    )
    .unwrap()
}
#[test]
fn conflict_prevents_all_writes_and_does_not_consume_name() {
    let f = Fixture::new();
    f.write("a.html", b"a");
    f.write("b.css", b"b");
    let s = scope(&["a.html", "b.css"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"A");
    f.write("b.css", b"B");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"external");
    f.write("b.css", b"b");
    let result = apply(&f, &s, &before, &after, "conflict");
    assert!(matches!(result, WriteOutcome::Conflict { .. }));
    assert_eq!(fs::read(f.source("a.html")).unwrap(), b"external");
    assert_eq!(fs::read(f.source("b.css")).unwrap(), b"b");
    assert!(!f.journals().join("conflict").exists());
}
#[test]
fn held_handle_blocks_other_writer_and_rename() {
    let f = Fixture::new();
    f.write("a.html", b"before");
    let held = open_exclusive_existing(&f.source("a.html"))
        .unwrap()
        .unwrap();
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(f.source("a.html"))
        .is_err());
    assert!(fs::rename(f.source("a.html"), f.source("renamed.html")).is_err());
    drop(held);
}
#[test]
fn change_add_delete_are_exact_and_journal_is_one_use() {
    let f = Fixture::new();
    f.write("a.html", b"\xEF\xBB\xBFone\r\n");
    f.write("b.css", b"remove");
    f.write("other.css", b"unrelated");
    fs::create_dir_all(f.source("nested")).unwrap();
    let s = scope(&["a.html", "b.css", "nested/c.js"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"\xEF\xBB\xBFtwo\r\n");
    fs::remove_file(f.source("b.css")).unwrap();
    f.write("nested/c.js", b"added");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"\xEF\xBB\xBFone\r\n");
    f.write("b.css", b"remove");
    fs::remove_file(f.source("nested/c.js")).unwrap();
    let first = apply(&f, &s, &before, &after, "one");
    assert!(matches!(first, WriteOutcome::Applied { .. }), "{first:?}");
    assert_eq!(
        fs::read(f.source("a.html")).unwrap(),
        b"\xEF\xBB\xBFtwo\r\n"
    );
    assert!(!f.source("b.css").exists());
    assert_eq!(fs::read(f.source("nested/c.js")).unwrap(), b"added");
    assert_eq!(fs::read(f.source("other.css")).unwrap(), b"unrelated");
    let states = reconcile(&f.root(), &f.journals().join("one")).unwrap();
    assert!(states.iter().all(|s| s.state == ReconcileState::After));
    assert!(apply_checked(
        &mut HostWritePermit::after_host_checks(),
        &f.root(),
        &s,
        &after,
        &before,
        &f.journals(),
        "one",
        WritePurpose::UndoLatestDraft
    )
    .is_err());
}
#[test]
fn partial_retains_journal_and_reconcile_does_not_mutate() {
    let f = Fixture::new();
    f.write("a.html", b"a");
    f.write("b.css", b"b");
    let s = scope(&["a.html", "b.css"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"A");
    f.write("b.css", b"B");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"a");
    f.write("b.css", b"b");
    TEST_FAIL_AFTER_FIRST.with(|v| v.set(true));
    let result = apply(&f, &s, &before, &after, "partial");
    TEST_FAIL_AFTER_FIRST.with(|v| v.set(false));
    assert!(matches!(result, WriteOutcome::Partial { .. }), "{result:?}");
    let states = reconcile(&f.root(), &f.journals().join("partial")).unwrap();
    assert_eq!(states[0].state, ReconcileState::After);
    assert_eq!(states[1].state, ReconcileState::Before);
    assert_eq!(fs::read(f.source("a.html")).unwrap(), b"A");
    assert_eq!(fs::read(f.source("b.css")).unwrap(), b"b");
    f.write("b.css", b"external");
    assert_eq!(
        reconcile(&f.root(), &f.journals().join("partial")).unwrap()[1].state,
        ReconcileState::Diverged
    );
}
#[test]
fn new_file_requires_existing_parent_and_keeps_source_untouched() {
    let f = Fixture::new();
    f.write("index.html", b"keep");
    let s = scope(&["index.html", "missing/new.js"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("missing/new.js", b"desired");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    fs::remove_file(f.source("missing/new.js")).unwrap();
    fs::remove_dir(f.source("missing")).unwrap();
    assert!(apply_checked(
        &mut HostWritePermit::after_host_checks(),
        &f.root(),
        &s,
        &before,
        &after,
        &f.journals(),
        "missing-parent",
        WritePurpose::AdoptSource
    )
    .is_err());
    assert_eq!(fs::read(f.source("index.html")).unwrap(), b"keep");
    assert!(!f.journals().join("missing-parent").exists());
}
#[test]
fn latest_undo_targets_only_selected_draft_bytes() {
    let f = Fixture::new();
    f.write("index.html", b"before");
    f.write("unrelated.css", b"external-one");
    let s = scope(&["index.html"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("index.html", b"after");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("unrelated.css", b"external-two");
    let result = apply_checked(
        &mut HostWritePermit::after_host_checks(),
        &f.root(),
        &s,
        &after,
        &before,
        &f.journals(),
        "undo",
        WritePurpose::UndoLatestDraft,
    )
    .unwrap();
    assert!(matches!(result, WriteOutcome::Applied { .. }));
    assert_eq!(fs::read(f.source("index.html")).unwrap(), b"before");
    assert_eq!(
        fs::read(f.source("unrelated.css")).unwrap(),
        b"external-two"
    );
}
#[test]
#[ignore = "hard-link fixture needs Windows access denied by the restricted runner"]
fn hardlinked_target_rejects_entire_plan_before_writing() {
    let f = Fixture::new();
    f.write("a.html", b"ordinary-before");
    let outside = f.base.join("outside.css");
    fs::write(&outside, b"outside-before").unwrap();
    fs::hard_link(&outside, f.source("z.css")).unwrap();
    let s = scope(&["a.html", "z.css"]);
    let before = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"ordinary-after");
    f.write("z.css", b"outside-after");
    let after = Snapshot::capture(&f.root(), &s).unwrap();
    f.write("a.html", b"ordinary-before");
    f.write("z.css", b"outside-before");
    let result = apply_checked(
        &mut HostWritePermit::after_host_checks(),
        &f.root(),
        &s,
        &before,
        &after,
        &f.journals(),
        "reject-link",
        WritePurpose::AdoptSource,
    );
    assert!(result.is_err(), "{result:?}");
    assert_eq!(fs::read(f.source("a.html")).unwrap(), b"ordinary-before");
    assert_eq!(fs::read(&outside).unwrap(), b"outside-before");
    assert!(!f.journals().join("reject-link").exists());
}
