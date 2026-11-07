//! Bounded Visual Editor file scope, immutable snapshots, source hints, direct-edit turn records, and preview.
pub(crate) mod direct;
mod scope;
mod snapshot;
pub(crate) mod source_hint;
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

pub mod preview;
