//! Shared source snapshots and evaluation inputs, with optional filesystem/editor support.
pub mod debug;
pub mod package;
mod snapshot;
pub use notist_eval::ModuleProvider;
mod query;
pub use query::*;
pub use snapshot::{EvaluationSession, Snapshot, SourceSnapshot};

#[cfg(feature = "filesystem")]
mod editor;
#[cfg(feature = "filesystem")]
pub use editor::{Document, Workspace, file_uri, offset, position, project, range, uri_path};
