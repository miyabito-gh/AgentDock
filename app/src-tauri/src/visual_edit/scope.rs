use super::{VisualError, VisualResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

pub const MAX_FILES: usize = 512;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Filesystem names, not URLs. Dependency URL resolution is a later layer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelativePath(String);
impl RelativePath {
    pub fn parse(s: &str) -> VisualResult<Self> {
        if s.is_empty()
            || s.len() > 1024
            || s.split('/').count() > 32
            || s.contains(['\\', ':', '%'])
            || s.chars().any(|c| c.is_control())
        {
            return Err(VisualError::InvalidPath(s.into()));
        }
        for c in s.split('/') {
            let n = c.to_lowercase();
            let stem = n.split('.').next().unwrap_or("");
            let device = matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
                || ["com", "lpt"].iter().any(|p| {
                    stem.strip_prefix(*p).map_or(false, |v| {
                        matches!(
                            v,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
                });
            let secret = n.starts_with(".env")
                || n.starts_with("id_rsa")
                || n.starts_with("id_ed25519")
                || matches!(
                    n.as_str(),
                    ".git"
                        | ".hg"
                        | ".svn"
                        | ".codex"
                        | ".ssh"
                        | ".aws"
                        | ".azure"
                        | ".config"
                        | ".cache"
                        | "node_modules"
                        | "target"
                        | "dist"
                        | "credentials"
                        | "credentials.json"
                        | "auth.json"
                        | "config.toml"
                        | "agents.md"
                        | "claude.md"
                )
                || [
                    ".pem", ".key", ".pfx", ".p12", ".exe", ".dll", ".bat", ".cmd", ".ps1",
                ]
                .iter()
                .any(|e| n.ends_with(*e));
            if c.is_empty()
                || matches!(c, "." | "..")
                || c.ends_with(['.', ' '])
                || c.contains(['<', '>', '"', '|', '?', '*'])
                || device
                || secret
            {
                return Err(VisualError::InvalidPath(s.into()));
            }
        }
        Ok(Self(s.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn key(&self) -> String {
        self.0.to_uppercase()
    }
    pub(crate) fn join(&self, root: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(root.into(), |p: PathBuf, c| p.join(c))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileAccess {
    Edit,
    ReadOnly,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedFile {
    pub path: String,
    pub access: FileAccess,
}

/// Only explicitly reviewed names, including explicitly approved future files.
/// No sibling/directory permission is inferred. Secret exclusion is defense in
/// depth, not a claim that arbitrary HTML/JSON cannot contain credentials.
#[derive(Clone, Debug)]
pub struct FileScope {
    files: BTreeMap<RelativePath, FileAccess>,
}
impl FileScope {
    pub fn new(selected: Vec<SelectedFile>) -> VisualResult<Self> {
        if selected.is_empty() || selected.len() > MAX_FILES {
            return Err(VisualError::Limit("file count"));
        }
        let mut files = BTreeMap::new();
        let mut keys = BTreeSet::new();
        for f in selected {
            let p = RelativePath::parse(&f.path)?;
            let ext = p
                .as_str()
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let allowed = match f.access {
                FileAccess::Edit => matches!(ext.as_str(), "html" | "htm" | "css" | "js" | "mjs"),
                FileAccess::ReadOnly => matches!(
                    ext.as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "gif"
                        | "webp"
                        | "avif"
                        | "svg"
                        | "ico"
                        | "woff"
                        | "woff2"
                        | "ttf"
                        | "otf"
                        | "json"
                ),
            };
            if !allowed || !keys.insert(p.key()) {
                return Err(VisualError::InvalidPath(f.path));
            }
            files.insert(p, f.access);
        }
        for p in files.keys() {
            let mut prefix = String::new();
            for c in p
                .as_str()
                .split('/')
                .take(p.as_str().split('/').count() - 1)
            {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(c);
                if keys.contains(&prefix.to_uppercase()) {
                    return Err(VisualError::InvalidPath(p.0.clone()));
                }
            }
        }
        Ok(Self { files })
    }
    pub fn files(&self) -> impl Iterator<Item = (&RelativePath, FileAccess)> {
        self.files.iter().map(|(p, a)| (p, *a))
    }
    pub fn selected(&self) -> Vec<SelectedFile> {
        self.files()
            .map(|(p, a)| SelectedFile {
                path: p.0.clone(),
                access: a,
            })
            .collect()
    }
}

/// Windows ancestor handles exclude reparse points and deny rename/deletion.
/// Source file handles deny concurrent writes/deletion while snapshots are read.
/// These constraints are not a general adoption lock or a Unix openat guarantee.
pub struct ProjectRoot {
    path: PathBuf,
    _pins: Vec<File>,
}
impl ProjectRoot {
    pub fn open(path: &Path) -> VisualResult<Self> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(VisualError::UnsafePath(path.into()));
        }
        #[cfg(windows)]
        {
            use std::path::{Component, Prefix};
            if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
            {
                return Err(VisualError::UnsafePath(path.into()));
            }
        }
        let pins = pin_directories(path)?;
        Ok(Self {
            path: fs::canonicalize(path)?,
            _pins: pins,
        })
    }
    pub(crate) fn from_verified_path(path: &Path) -> VisualResult<Self> {
        let pins = pin_directories(path)?;
        Ok(Self {
            path: fs::canonicalize(path)?,
            _pins: pins,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn open_relative(&self, p: &RelativePath) -> VisualResult<Option<HeldFile>> {
        let path = p.join(&self.path);
        let parent = path
            .parent()
            .ok_or_else(|| VisualError::UnsafePath(path.clone()))?;
        let pins = match pin_directories(parent) {
            Ok(v) => v,
            Err(VisualError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let file = match open_checked(&path, false) {
            Ok(v) => v,
            Err(VisualError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !root_contains(&self.path, &fs::canonicalize(&path)?) {
            return Err(VisualError::UnsafePath(path));
        }
        Ok(Some(HeldFile { file, _pins: pins }))
    }
}
pub(crate) struct HeldFile {
    pub file: File,
    _pins: Vec<File>,
}
fn is_link(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
pub(crate) fn open_checked(path: &Path, directory: bool) -> VisualResult<File> {
    let m = fs::symlink_metadata(path)?;
    if is_link(&m) || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err(VisualError::UnsafePath(path.into()));
    }
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // 固定中のフォルダ直下でも他の保存（rename置換を含む）を止めないよう、フォルダは読取り・書込み共有にして削除・改名だけ拒否する。
        o.share_mode(if directory { 3 } else { 1 })
            .custom_flags(0x00200000 | if directory { 0x02000000 } else { 0 });
    }
    let f = o.open(path)?;
    let m = f.metadata()?;
    if is_link(&m) || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err(VisualError::UnsafePath(path.into()));
    }
    Ok(f)
}
pub(crate) fn pin_directories(path: &Path) -> VisualResult<Vec<File>> {
    let mut chain: Vec<_> = path.ancestors().collect();
    chain.reverse();
    chain.into_iter().map(|p| open_checked(p, true)).collect()
}

/// Canonical Windows paths can retain caller casing. Compare components rather
/// than raw prefixes so drive/parent case cannot defeat overlap rejection.
pub(crate) fn root_contains(root: &Path, candidate: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut right = candidate.components();
        root.components().all(|left| {
            right.next().map_or(false, |part| {
                left.as_os_str().to_string_lossy().to_uppercase()
                    == part.as_os_str().to_string_lossy().to_uppercase()
            })
        })
    }
    #[cfg(not(windows))]
    {
        candidate.starts_with(root)
    }
}
