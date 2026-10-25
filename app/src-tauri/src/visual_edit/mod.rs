//! Explicit-file foundation and bounded read-only native preview. No source
//! writer. Decisions are observations, never write authorization. Source
//! adoption remains blocked pending writer guards, journals, and replacement.
//! Hashes are not locks; Unix fixtures do not establish Windows race safety.
mod decision;
mod draft;
mod scope;
mod snapshot;
pub use decision::*;
pub use draft::*;
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

pub mod preview;
