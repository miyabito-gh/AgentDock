use super::{
    FileAccess, FileScope, ProjectRoot, SelectedFile, VisualError, VisualResult, MAX_FILE_BYTES,
    MAX_TOTAL_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum FileStamp {
    Missing,
    Present { bytes: u64, sha256: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub path: String,
    pub access: FileAccess,
    pub stamp: FileStamp,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotManifest {
    pub version: u32,
    pub files: Vec<ManifestEntry>,
}

/// Byte-exact immutable observation. No public deserialization can substitute
/// forged hashes for the actual bytes used by decision functions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    selected: Vec<SelectedFile>,
    files: BTreeMap<String, Option<Vec<u8>>>,
}
impl Snapshot {
    pub fn capture(root: &ProjectRoot, scope: &FileScope) -> VisualResult<Self> {
        // Keep all present file handles until the complete selected set is read.
        // Absent names and Unix renames still require a second observation; this
        // is conservative preflight, not a claim of an atomic directory snapshot.
        let mut held = Vec::new();
        for (p, _) in scope.files() {
            held.push((p.clone(), root.open_relative(p)?));
        }
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for (p, file) in &held {
            let bytes = match file {
                None => None,
                Some(h) => {
                    let before = h.file.metadata()?;
                    if before.len() > MAX_FILE_BYTES {
                        return Err(VisualError::Limit("single file bytes"));
                    }
                    let mut bytes = Vec::new();
                    (&h.file).take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
                    let after = h.file.metadata()?;
                    if bytes.len() as u64 > MAX_FILE_BYTES {
                        return Err(VisualError::Limit("single file bytes"));
                    }
                    if before.len() != after.len()
                        || before.modified()? != after.modified()?
                        || bytes.len() as u64 != before.len()
                    {
                        return Err(VisualError::Conflict("changed during read"));
                    }
                    total += bytes.len() as u64;
                    if total > MAX_TOTAL_BYTES {
                        return Err(VisualError::Limit("total bytes"));
                    }
                    Some(bytes)
                }
            };
            files.insert(p.as_str().into(), bytes);
        }
        for (p, previous) in &held {
            let current = root.open_relative(p)?;
            if previous.is_some() != current.is_some() {
                return Err(VisualError::Conflict(
                    "file appeared/disappeared during capture",
                ));
            }
        }
        Ok(Self {
            selected: scope.selected(),
            files,
        })
    }
    pub fn manifest(&self) -> SnapshotManifest {
        SnapshotManifest {
            version: 1,
            files: self
                .selected
                .iter()
                .map(|f| ManifestEntry {
                    path: f.path.clone(),
                    access: f.access,
                    stamp: match &self.files[&f.path] {
                        None => FileStamp::Missing,
                        Some(b) => FileStamp::Present {
                            bytes: b.len() as u64,
                            sha256: format!("{:x}", Sha256::digest(b)),
                        },
                    },
                })
                .collect(),
        }
    }
    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).and_then(|b| b.as_deref())
    }
    pub(crate) fn same_scope(&self, other: &Self) -> bool {
        self.selected == other.selected
    }
    pub(crate) fn selected(&self) -> &[SelectedFile] {
        &self.selected
    }
    pub(crate) fn state(&self, path: &str) -> &Option<Vec<u8>> {
        &self.files[path]
    }
}
