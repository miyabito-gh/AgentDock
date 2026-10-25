use super::{
    scope::{pin_directories, root_contains},
    FileScope, ProjectRoot, RelativePath, Snapshot, VisualError, VisualResult, MAX_FILES,
};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Created only in a caller-supplied unused directory outside the source. A
/// partial creation is retained on failure; there is no destructive cleanup.
/// Snapshots are in memory plus immutable byte files/manifest for initial state.
/// This is not yet a restart loader, journal, sandbox, or adoption capability.
pub struct IsolatedDraft {
    root: ProjectRoot,
    scope: FileScope,
    initial: Snapshot,
    storage: PathBuf,
}
impl IsolatedDraft {
    pub fn root(&self) -> &ProjectRoot {
        &self.root
    }
    pub fn initial(&self) -> &Snapshot {
        &self.initial
    }
    pub fn storage_path(&self) -> &Path {
        &self.storage
    }
    /// Reject any unselected file/directory, and collect the full explicit set.
    /// A caller must establish real stop evidence; audit is not an OS sandbox.
    pub fn observe(&self) -> VisualResult<Snapshot> {
        audit(&self.root, &self.scope)?;
        let result = Snapshot::capture(&self.root, &self.scope)?;
        audit(&self.root, &self.scope)?;
        Ok(result)
    }
}

pub fn create_isolated_draft(
    source: &ProjectRoot,
    scope: FileScope,
    storage_parent: &Path,
    name: &str,
) -> VisualResult<IsolatedDraft> {
    let name = RelativePath::parse(name)?;
    if name.as_str().contains('/') {
        return Err(VisualError::InvalidPath(name.as_str().into()));
    }
    let storage_parent = ProjectRoot::open(storage_parent)?;
    if root_contains(source.path(), storage_parent.path())
        || root_contains(storage_parent.path(), source.path())
    {
        return Err(VisualError::Conflict("source and storage overlap"));
    }
    let initial = Snapshot::capture(source, &scope)?;
    if !scope.files().any(|(p, a)| {
        a == super::FileAccess::Edit
            && matches!(
                p.as_str()
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "html" | "htm"
            )
            && initial.bytes(p.as_str()).is_some()
    }) {
        return Err(VisualError::Conflict(
            "an existing editable HTML is required",
        ));
    }
    let storage = name.join(storage_parent.path());
    fs::create_dir(&storage)?;
    let _storage_pins = pin_directories(&storage)?;
    let draft_path = storage.join("draft");
    let snapshot_path = storage.join("initial");
    fs::create_dir(&draft_path)?;
    fs::create_dir(&snapshot_path)?;
    let draft_root = ProjectRoot::from_verified_path(&draft_path)?;
    let snapshot_root = ProjectRoot::from_verified_path(&snapshot_path)?;
    for (p, _) in scope.files() {
        if let Some(bytes) = initial.bytes(p.as_str()) {
            write_new(&draft_root, p, bytes)?;
            write_new(&snapshot_root, p, bytes)?;
        }
    }
    let manifest = serde_json::to_vec_pretty(&initial.manifest())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(storage.join("manifest.json"))?;
    file.write_all(&manifest)?;
    file.sync_all()?;
    // Detect ordinary concurrent additions/deletions and changes during copying.
    // No success is returned on conflict. Retain partial artifacts for recovery.
    if Snapshot::capture(source, &scope)? != initial {
        return Err(VisualError::Conflict("source changed while copying"));
    }
    let draft = IsolatedDraft {
        root: draft_root,
        scope,
        initial,
        storage,
    };
    if draft.observe()? != *draft.initial() {
        return Err(VisualError::Conflict("draft changed while copying"));
    }
    Ok(draft)
}
fn write_new(root: &ProjectRoot, path: &RelativePath, bytes: &[u8]) -> VisualResult<()> {
    let full = path.join(root.path());
    let parent = full
        .parent()
        .ok_or_else(|| VisualError::UnsafePath(full.clone()))?;
    let relative = parent
        .strip_prefix(root.path())
        .map_err(|_| VisualError::UnsafePath(full.clone()))?;
    let mut dir = root.path().to_path_buf();
    let mut pins = Vec::new();
    for c in relative.components() {
        dir.push(c);
        match fs::create_dir(&dir) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        pins.extend(pin_directories(&dir)?);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0).custom_flags(0x00200000);
    }
    let mut file = options.open(&full)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn audit(root: &ProjectRoot, scope: &FileScope) -> VisualResult<()> {
    let mut allowed = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    for (p, _) in scope.files() {
        allowed.insert(p.as_str().to_string());
        let parts: Vec<_> = p.as_str().split('/').collect();
        for i in 1..parts.len() {
            dirs.insert(parts[..i].join("/"));
        }
    }
    let mut pending = vec![(root.path().to_path_buf(), String::new())];
    let mut count = 0usize;
    while let Some((directory, prefix)) = pending.pop() {
        let _pins = pin_directories(&directory)?;
        for entry in fs::read_dir(&directory)? {
            count += 1;
            if count > MAX_FILES * 33 {
                return Err(VisualError::Limit("draft entries"));
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| VisualError::Conflict("non UTF-8 draft path"))?;
            let name = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let p = RelativePath::parse(&name)?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                if !dirs.contains(p.as_str()) {
                    return Err(VisualError::Conflict("unselected draft directory"));
                }
                // Opening through the full checked chain rejects junctions too.
                let _check = pin_directories(&entry.path())?;
                pending.push((entry.path(), name));
            } else {
                if !allowed.contains(p.as_str()) {
                    return Err(VisualError::Conflict("unselected draft file"));
                }
                let _check = root
                    .open_relative(&p)?
                    .ok_or(VisualError::Conflict("draft disappeared"))?;
            }
        }
    }
    Ok(())
}
