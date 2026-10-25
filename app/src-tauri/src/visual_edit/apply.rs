#[cfg(test)]
thread_local! { pub(super) static TEST_FAIL_AFTER_FIRST: std::cell::Cell<bool> = std::cell::Cell::new(false); }
use super::{
    scope::{open_checked, pin_directories, root_contains},
    FileAccess, FileScope, ProjectRoot, RelativePath, Snapshot, VisualError, VisualResult,
    MAX_FILE_BYTES, MAX_TOTAL_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// This module has no IPC entry point. The Host must keep its stop and operation
/// guard alive while calling this function. A permit is deliberately not
/// deserializable or constructible by a webview.
pub(crate) struct HostWritePermit {
    _private: (),
}
impl HostWritePermit {
    /// Call only after the Host has checked fresh stop evidence and acquired
    /// source/draft operation guards. Integration remains a Host responsibility.
    pub(crate) fn after_host_checks() -> Self {
        Self { _private: () }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WritePurpose {
    AdoptSource,
    UndoLatestDraft,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    Applied {
        journal: PathBuf,
        changed: Vec<String>,
    },
    Partial {
        journal: PathBuf,
        changed: Vec<String>,
        problem: String,
    },
    Conflict {
        paths: Vec<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReconcileState {
    Before,
    After,
    Diverged,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReconciledFile {
    pub path: String,
    pub state: ReconcileState,
}
#[derive(Serialize, Deserialize)]
struct Step {
    path: String,
    before: Option<String>,
    after: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    version: u32,
    source_root: String,
    purpose: WritePurpose,
    steps: Vec<Step>,
}
#[derive(Serialize)]
struct Event<'a> {
    phase: &'a str,
    index: Option<usize>,
    detail: Option<&'a str>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn note(
    events: &mut File,
    phase: &str,
    index: Option<usize>,
    detail: Option<&str>,
) -> std::io::Result<()> {
    serde_json::to_writer(
        &mut *events,
        &Event {
            phase,
            index,
            detail,
        },
    )?;
    events.write_all(b"\n")?;
    events.sync_all()
}
fn new_file(path: &Path, bytes: &[u8]) -> VisualResult<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn root_pair(source: &ProjectRoot, journal_parent: &Path) -> VisualResult<ProjectRoot> {
    let storage = ProjectRoot::open(journal_parent)?;
    if root_contains(source.path(), storage.path()) || root_contains(storage.path(), source.path())
    {
        return Err(VisualError::Conflict("journal and source overlap"));
    }
    Ok(storage)
}
fn operation_name(name: &str) -> VisualResult<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(VisualError::InvalidPath(name.into()));
    }
    Ok(())
}

/// No successful result is returned unless each desired byte state was written
/// and checked. A journal name is one-use, including conflicts after creation.
/// Existing files are mutated through held exclusive handles: this is not
/// atomic replacement. Crash recovery is observation only.
pub(crate) fn apply_checked(
    _permit: &mut HostWritePermit,
    source: &ProjectRoot,
    scope: &FileScope,
    expected: &Snapshot,
    desired: &Snapshot,
    journal_parent: &Path,
    operation_id: &str,
    purpose: WritePurpose,
) -> VisualResult<WriteOutcome> {
    operation_name(operation_id)?;
    let storage = root_pair(source, journal_parent)?;
    if expected.selected() != scope.selected() || !expected.same_scope(desired) {
        return Err(VisualError::Conflict("different scopes"));
    }
    let mut steps = Vec::new();
    let mut total = 0u64;
    for selected in expected.selected() {
        let before = expected.state(&selected.path);
        let after = desired.state(&selected.path);
        if selected.access == FileAccess::ReadOnly && before != after {
            return Err(VisualError::Conflict("read-only change"));
        }
        for bytes in [before, after].into_iter().flatten() {
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(VisualError::Limit("single file bytes"));
            }
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or(VisualError::Limit("total bytes"))?;
            if total > MAX_TOTAL_BYTES * 2 {
                return Err(VisualError::Limit("total bytes"));
            }
        }
        if before != after {
            steps.push(Step {
                path: selected.path.clone(),
                before: before.as_ref().map(|v| digest(v)),
                after: after.as_ref().map(|v| digest(v)),
            });
        }
    }
    if steps.is_empty() {
        return Err(VisualError::Conflict("no changes"));
    }

    // Open every existing selected file exclusively before comparing any
    // bytes. Ancestor handles pin the path against rename/delete on Windows.
    let mut held = Vec::new();
    let mut conflicts = Vec::new();
    for selected in expected.selected() {
        let path = RelativePath::parse(&selected.path)?;
        let full = path.join(source.path());
        let pins = pin_directories(full.parent().unwrap())?;
        let file = if expected.state(&selected.path) != desired.state(&selected.path) {
            open_exclusive_existing(&full)?
        } else {
            match open_checked(&full, false) {
                Ok(file) => Some(file),
                Err(VisualError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            }
        };
        held.push((path, pins, file));
    }
    for (path, _, file) in &mut held {
        let current = match file {
            Some(f) => Some(read_bounded(f)?),
            None => None,
        };
        if &current != expected.state(path.as_str()) {
            conflicts.push(path.as_str().to_owned());
        }
    }
    if !conflicts.is_empty() {
        return Ok(WriteOutcome::Conflict { paths: conflicts });
    }

    let plan = Plan {
        version: 1,
        source_root: source.path().to_string_lossy().into_owned(),
        purpose,
        steps,
    };
    let journal = storage.path().join(operation_id);
    fs::create_dir(&journal)?;
    let _journal_pins = match pin_directories(&journal) {
        Ok(pins) => pins,
        Err(e) => {
            return Ok(WriteOutcome::Partial {
                journal,
                changed: Vec::new(),
                problem: e.to_string(),
            })
        }
    };
    let mut events = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(journal.join("events.jsonl"))
    {
        Ok(v) => v,
        Err(e) => {
            return Ok(WriteOutcome::Partial {
                journal,
                changed: Vec::new(),
                problem: e.to_string(),
            })
        }
    };
    let mut changed = Vec::new();
    let prepared = (|| -> VisualResult<()> {
        for (i, step) in plan.steps.iter().enumerate() {
            if let Some(bytes) = expected.bytes(&step.path) {
                new_file(&journal.join(format!("{i:04}.before")), bytes)?;
            }
            if let Some(bytes) = desired.bytes(&step.path) {
                new_file(&journal.join(format!("{i:04}.after")), bytes)?;
            }
        }

        new_file(
            &journal.join("plan.json"),
            &serde_json::to_vec_pretty(&plan)?,
        )?;
        note(&mut events, "prepared", None, None)?;
        Ok(())
    })();
    if let Err(e) = prepared {
        return Ok(WriteOutcome::Partial {
            journal,
            changed,
            problem: e.to_string(),
        });
    }
    for (i, step) in plan.steps.iter().enumerate() {
        if let Err(e) = note(&mut events, "step-start", Some(i), None) {
            return Ok(WriteOutcome::Partial {
                journal,
                changed,
                problem: e.to_string(),
            });
        }
        changed.push(step.path.clone()); // attempted; a failed in-place write may have touched bytes
        let at = held
            .iter_mut()
            .find(|(p, _, _)| p.as_str() == step.path)
            .expect("scope step");
        let result = write_step(
            source,
            &at.0,
            &mut at.1,
            &mut at.2,
            desired.state(&step.path),
        );
        if let Err(e) = result {
            let message = e.to_string();
            let _ = note(&mut events, "partial", Some(i), Some(&message));
            return Ok(WriteOutcome::Partial {
                journal,
                changed,
                problem: message,
            });
        }
        #[cfg(test)]
        if i == 0 && TEST_FAIL_AFTER_FIRST.with(|v| v.get()) {
            let _ = note(
                &mut events,
                "partial",
                Some(i),
                Some("injected test failure"),
            );
            return Ok(WriteOutcome::Partial {
                journal,
                changed,
                problem: "injected test failure".into(),
            });
        }
        if let Err(e) = note(&mut events, "step-end", Some(i), None) {
            return Ok(WriteOutcome::Partial {
                journal,
                changed,
                problem: e.to_string(),
            });
        }
    }
    // Recheck every selected name, including unchanged missing files that could
    // have appeared while the journal was being written. Held handles protect
    // existing names; a new external file must never be reported as success.
    for (path, _, file) in &mut held {
        let observed = (|| -> VisualResult<Option<Vec<u8>>> {
            match file {
                Some(existing) => Ok(Some(read_bounded(existing)?)),
                None => match open_checked(&path.join(source.path()), false) {
                    Ok(mut appeared) => Ok(Some(read_bounded(&mut appeared)?)),
                    Err(VisualError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e),
                },
            }
        })();
        let current = match observed {
            Ok(state) => state,
            Err(e) => {
                let message = e.to_string();
                let _ = note(&mut events, "partial", None, Some(&message));
                return Ok(WriteOutcome::Partial {
                    journal,
                    changed,
                    problem: message,
                });
            }
        };
        if &current != desired.state(path.as_str()) {
            let message = format!("post-apply scope conflict: {}", path.as_str());
            let _ = note(&mut events, "partial", None, Some(&message));
            return Ok(WriteOutcome::Partial {
                journal,
                changed,
                problem: message,
            });
        }
    }
    if let Err(e) = note(&mut events, "applied", None, None) {
        return Ok(WriteOutcome::Partial {
            journal,
            changed,
            problem: e.to_string(),
        });
    }
    Ok(WriteOutcome::Applied { journal, changed })
}
fn read_bounded(file: &mut File) -> VisualResult<Vec<u8>> {
    let len = file.metadata()?.len();
    if len > MAX_FILE_BYTES {
        return Err(VisualError::Limit("single file bytes"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(VisualError::Conflict("changed during read"));
    }
    Ok(bytes)
}
fn write_step(
    source: &ProjectRoot,
    path: &RelativePath,
    _pins: &mut Vec<File>,
    file: &mut Option<File>,
    desired: &Option<Vec<u8>>,
) -> VisualResult<()> {
    let full = path.join(source.path());
    match (file.as_mut(), desired) {
        (Some(existing), Some(bytes)) => {
            existing.seek(SeekFrom::Start(0))?;
            existing.set_len(0)?;
            existing.write_all(bytes)?;
            existing.sync_all()?;
            existing.seek(SeekFrom::Start(0))?;
            if read_bounded(existing)? != *bytes {
                return Err(VisualError::Conflict("post-write mismatch"));
            }
        }
        (Some(existing), None) => {
            delete_held(existing)?;
            *file = None; // close DELETE handle to commit disposition
            match fs::symlink_metadata(&full) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
                Ok(_) => return Err(VisualError::Conflict("delete not committed")),
            }
        }
        (None, Some(bytes)) => {
            let _parent = full
                .parent()
                .ok_or_else(|| VisualError::UnsafePath(full.clone()))?;
            let mut options = OpenOptions::new();
            options.write(true).read(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0).custom_flags(0x00200000);
            }
            let mut created = options.open(&full)?;
            created.write_all(bytes)?;
            created.sync_all()?;
            if read_bounded(&mut created)? != *bytes {
                return Err(VisualError::Conflict("post-create mismatch"));
            }
            *file = Some(created);
        }
        (None, None) => (),
    }
    Ok(())
}
#[cfg(windows)]
pub(super) fn open_exclusive_existing(path: &Path) -> VisualResult<Option<File>> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
        },
    };
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    name.push(0);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            0x8000_0000 | 0x4000_0000 | DELETE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error.into());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let meta = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if !meta.is_file() || meta.file_attributes() & 0x400 != 0 {
        return Err(VisualError::UnsafePath(path.into()));
    }
    // In-place writes mutate every hard-link alias, including aliases outside
    // the reviewed root. Query the held file object before any source writes.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if info.nNumberOfLinks != 1 {
        return Err(VisualError::Conflict("hard-linked target"));
    }
    Ok(Some(file))
}
#[cfg(not(windows))]
fn open_exclusive_existing(_path: &Path) -> VisualResult<Option<File>> {
    Err(VisualError::Conflict("native Windows writer required"))
}
#[cfg(windows)]
fn delete_held(file: &File) -> VisualResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    file.sync_all()?;
    Ok(())
}
#[cfg(not(windows))]
fn delete_held(_file: &File) -> VisualResult<()> {
    Err(VisualError::Conflict("native Windows writer required"))
}

/// Read-only crash/restart inspection. It never retries or rolls back writes.
pub(crate) fn reconcile(source: &ProjectRoot, journal: &Path) -> VisualResult<Vec<ReconciledFile>> {
    let plan: Plan = serde_json::from_slice(&fs::read(journal.join("plan.json"))?)?;
    if plan.version != 1 || plan.source_root != source.path().to_string_lossy() {
        return Err(VisualError::Conflict("journal source mismatch"));
    }
    let mut result = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let path = RelativePath::parse(&step.path)?;
        let now = source.open_relative(&path)?;
        let current = match now {
            Some(mut held) => Some(read_bounded(&mut held.file)?),
            None => None,
        };
        let before = match &step.before {
            Some(hash) => {
                let b = fs::read(journal.join(format!("{i:04}.before")))?;
                if &digest(&b) != hash {
                    return Err(VisualError::Conflict("journal backup mismatch"));
                }
                Some(b)
            }
            None => None,
        };
        let after = match &step.after {
            Some(hash) => {
                let b = fs::read(journal.join(format!("{i:04}.after")))?;
                if &digest(&b) != hash {
                    return Err(VisualError::Conflict("journal desired mismatch"));
                }
                Some(b)
            }
            None => None,
        };
        result.push(ReconciledFile {
            path: step.path.clone(),
            state: if current == after {
                ReconcileState::After
            } else if current == before {
                ReconcileState::Before
            } else {
                ReconcileState::Diverged
            },
        });
    }
    Ok(result)
}

/// The only rejection variant that proves this call created no journal and no target
/// file was mutated. `apply_checked` returns every post-journal failure as a
/// journaled `Partial`, so the Host may durably settle this rejection.
pub(crate) enum CheckedAttempt {
    PrewriteRejected(VisualError),
    Journaled(WriteOutcome),
}
pub(crate) fn apply_checked_classified(
    permit: &mut HostWritePermit,
    source: &ProjectRoot,
    scope: &FileScope,
    expected: &Snapshot,
    desired: &Snapshot,
    journal_parent: &Path,
    operation_id: &str,
    purpose: WritePurpose,
) -> CheckedAttempt {
    match apply_checked(permit,source,scope,expected,desired,journal_parent,operation_id,purpose) {
        Ok(outcome)=>CheckedAttempt::Journaled(outcome),
        Err(error)=>CheckedAttempt::PrewriteRejected(error),
    }
}