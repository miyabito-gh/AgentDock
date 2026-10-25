//! Bounded Visual Editor file scope, immutable snapshots, isolated draft, and
//! Host-gated session transactions. Source adoption and draft undo use the
//! journaled native writer only after fresh stop and folder-operation checks.
//! The writer is per-file, not a multi-file atomic transaction. Unknown or
//! partial results remain blocked for read-only recovery inspection.
pub(crate) mod apply;
mod decision;
pub mod session;
mod draft;
mod scope;
mod snapshot;
pub use decision::*;
pub use draft::*;
pub(crate) use draft::audit_draft_scope;
pub use scope::*;
pub use snapshot::*;

#[derive(Debug, thiserror::Error)]
pub enum VisualError {
    #[error("invalid relative path: {0}")]
    InvalidPath(String),
    #[error("unsafe path: {0:?}")]
    UnsafePath(std::path::PathBuf),
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    #[error("snapshot conflict: {0}")]
    Conflict(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type VisualResult<T> = Result<T, VisualError>;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod apply_tests;

pub mod preview;
