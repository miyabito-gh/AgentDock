use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "visual-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        fs::create_dir_all(path.join("source")).unwrap();
        fs::create_dir_all(path.join("storage")).unwrap();
        Self { path }
    }
    fn source(&self) -> PathBuf {
        self.path.join("source")
    }
    fn root(&self) -> ProjectRoot {
        ProjectRoot::open(&self.source()).unwrap()
    }
    fn write(&self, path: &str, b: &[u8]) {
        let p = self.source().join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
fn scope(files: &[(&str, FileAccess)]) -> FileScope {
    FileScope::new(
        files
            .iter()
            .map(|(p, a)| SelectedFile {
                path: (*p).into(),
                access: *a,
            })
            .collect(),
    )
    .unwrap()
}
fn evidence() -> ReviewEvidence {
    ReviewEvidence {
        stopped: true,
        draft_scope_audited: true,
    }
}
#[test]
fn rejects_windows_escape_secret_and_alias_names() {
    for name in [
        "../x.html",
        "/x.html",
        "C:/x.html",
        "C:x.html",
        "//server/x.html",
        "a\\..\\x.html",
        "x.html:stream",
        "%2e%2e/x.html",
        "a//x.html",
        "a/./x.html",
        "NUL.html",
        "COM1.css",
        "a./x.js",
        ".git/x.html",
        ".env.local/x.html",
        "auth.json",
        "x.pem",
        "a/AGENTS.md",
    ] {
        assert!(RelativePath::parse(name).is_err(), "{name}");
    }
    assert!(FileScope::new(vec![
        SelectedFile {
            path: "A.css".into(),
            access: FileAccess::Edit
        },
        SelectedFile {
            path: "a.css".into(),
            access: FileAccess::Edit
        }
    ])
    .is_err());
    assert!(FileScope::new(vec![SelectedFile {
        path: "a.png".into(),
        access: FileAccess::Edit
    }])
    .is_err());
    assert!(RelativePath::parse("画面/案.html").is_ok());
}
#[test]
fn isolated_non_git_dirty_bytes_and_immutable_initial_manifest() {
    let f = Fixture::new();
    let original = b"\xef\xbb\xbf<html>dirty\r\n\xff</html>";
    f.write("index.html", original);
    f.write("styles/main.css", b"a{}\r\n");
    f.write("logo.png", &[0, 255, 1]);
    let s = scope(&[
        ("index.html", FileAccess::Edit),
        ("styles/main.css", FileAccess::Edit),
        ("logo.png", FileAccess::ReadOnly),
    ]);
    let d = create_isolated_draft(&f.root(), s, &f.path.join("storage"), "session1").unwrap();
    assert_eq!(d.initial().bytes("index.html"), Some(original.as_slice()));
    fs::write(d.root().path().join("index.html"), b"edited").unwrap();
    assert_eq!(fs::read(f.source().join("index.html")).unwrap(), original);
    assert_eq!(d.initial().bytes("index.html"), Some(original.as_slice()));
    assert_eq!(
        fs::read(d.storage_path().join("initial/index.html")).unwrap(),
        original
    );
    let manifest: SnapshotManifest =
        serde_json::from_slice(&fs::read(d.storage_path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest, d.initial().manifest());
    assert_eq!(
        d.observe().unwrap().bytes("index.html"),
        Some(b"edited".as_slice())
    );
    fs::write(d.root().path().join("unapproved.js"), b"surprise").unwrap();
    assert!(d.observe().is_err());
}
#[test]
fn adoption_classifies_add_change_delete_and_blocks_external_nonoverlap() {
    let f = Fixture::new();
    f.write("index.html", b"before");
    f.write("delete.css", b"remove");
    let s = scope(&[
        ("index.html", FileAccess::Edit),
        ("delete.css", FileAccess::Edit),
        ("new.js", FileAccess::Edit),
    ]);
    let root = f.root();
    let b = Snapshot::capture(&root, &s).unwrap();
    f.write("index.html", b"after");
    fs::remove_file(f.source().join("delete.css")).unwrap();
    f.write("new.js", b"added");
    let e = Snapshot::capture(&root, &s).unwrap();
    let review = review_adoption(&b, &e, &b, evidence()).unwrap();
    assert!(!review.has_conflicts());
    assert_eq!(
        review.files.iter().map(|v| v.change).collect::<Vec<_>>(),
        vec![
            Some(ChangeKind::Delete),
            Some(ChangeKind::Change),
            Some(ChangeKind::Add)
        ]
    );
    let conflict = review_adoption(&b, &e, &e, evidence()).unwrap();
    assert!(conflict.has_conflicts());
    assert!(conflict
        .files
        .iter()
        .all(|v| v.conflict == Some(ConflictKind::ExternalChange)));
    assert_eq!(fs::read(f.source().join("index.html")).unwrap(), b"after");
}
#[test]
fn latest_undo_requires_exact_end_state_stop_and_unadopted_latest() {
    let f = Fixture::new();
    f.write("index.html", b"before");
    let s = scope(&[("index.html", FileAccess::Edit)]);
    let root = f.root();
    let b = Snapshot::capture(&root, &s).unwrap();
    f.write("index.html", b"after");
    let e = Snapshot::capture(&root, &s).unwrap();
    assert!(!review_latest_undo(&b, &e, &e, true, false, evidence())
        .unwrap()
        .has_conflicts());
    assert!(review_latest_undo(&b, &e, &b, true, false, evidence())
        .unwrap()
        .has_conflicts());
    assert!(review_latest_undo(&b, &e, &e, false, true, evidence())
        .unwrap()
        .has_conflicts());
    assert!(review_latest_undo(
        &b,
        &e,
        &e,
        true,
        false,
        ReviewEvidence {
            stopped: false,
            draft_scope_audited: false
        }
    )
    .unwrap()
    .has_conflicts());
    assert_eq!(fs::read(f.source().join("index.html")).unwrap(), b"after");
}
#[test]
fn assets_limits_and_scope_mismatch_fail_closed() {
    let f = Fixture::new();
    f.write("index.html", b"ok");
    f.write("a.png", b"before");
    let root = f.root();
    let s = scope(&[
        ("index.html", FileAccess::Edit),
        ("a.png", FileAccess::ReadOnly),
    ]);
    let b = Snapshot::capture(&root, &s).unwrap();
    f.write("a.png", b"changed");
    let e = Snapshot::capture(&root, &s).unwrap();
    assert!(review_adoption(&b, &e, &b, evidence())
        .unwrap()
        .has_conflicts());
    let different = Snapshot::capture(&root, &scope(&[("index.html", FileAccess::Edit)])).unwrap();
    assert!(review_adoption(&b, &e, &different, evidence()).is_err());
    let big = fs::File::create(f.source().join("big.js")).unwrap();
    big.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(big);
    assert!(Snapshot::capture(&root, &scope(&[("big.js", FileAccess::Edit)])).is_err());
    assert!(FileScope::new(
        (0..=MAX_FILES)
            .map(|i| SelectedFile {
                path: format!("{i}.js"),
                access: FileAccess::Edit
            })
            .collect()
    )
    .is_err());
    assert!(create_isolated_draft(&root, s, &f.source(), "nested").is_err());
}
#[test]
fn rejects_directory_and_file_links_in_existing_chain() {
    let f = Fixture::new();
    f.write("index.html", b"ok");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.source().join("index.html"), f.source().join("link.html"))
            .unwrap();
    }
    #[cfg(windows)]
    {
        // Junction creation needs no symbolic-link privilege on Windows.
        let link = f.source().join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(f.path.join("storage"))
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "junction fixture: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(
            Snapshot::capture(&f.root(), &scope(&[("linked/x.js", FileAccess::Edit)])).is_err()
        );
        assert!(ProjectRoot::open(&link).is_err());
        // Remove only the junction itself, before fixture recursive cleanup.
        fs::remove_dir(link).unwrap();
    }
    #[cfg(unix)]
    assert!(Snapshot::capture(&f.root(), &scope(&[("link.html", FileAccess::Edit)])).is_err());
}
#[test]
fn uppercase_html_entry_uses_same_extension_policy() {
    let f = Fixture::new();
    f.write("INDEX.HTML", b"uppercase");
    let d = create_isolated_draft(
        &f.root(),
        scope(&[("INDEX.HTML", FileAccess::Edit)]),
        &f.path.join("storage"),
        "uppercase",
    )
    .unwrap();
    assert_eq!(
        d.initial().bytes("INDEX.HTML"),
        Some(b"uppercase".as_slice())
    );
}
