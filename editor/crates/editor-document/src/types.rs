use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Assigned by the host. A new independent history must use a new history_id,
/// even when the file name and initial text happen to be identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentIdentity {
    pub document_id: String,
    pub history_id: String,
}

/// A causal state token, scoped to one document history. Decimal peer strings
/// keep 64-bit writer identities exact across JavaScript and native hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub identity: DocumentIdentity,
    pub clocks: BTreeMap<String, i32>,
}

/// An owned, immutable point-in-time read. `revision` is local to this instance;
/// use `version`, not revision, for cross-instance comparisons and preconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSnapshot {
    pub text: String,
    pub version: Version,
    pub revision: u64,
}

/// Half-open UTF-16 range in the transaction's BEFORE text. Endpoints must be
/// Unicode scalar boundaries; edits must be sorted, disjoint and unambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub expected_version: Version,
    pub origin: String,
    pub edits: Vec<TextEdit>,
    /// Opaque local undo payload, e.g. all selections in a particular view.
    /// Never interpreted as language, view, or replicated document state.
    #[serde(default)]
    pub undo_metadata: Option<Value>,
    /// BEFORE-text UTF-16 positions to transform through collaborative undo.
    /// Unlike ordinary anchors, these retain positions inside replaced text.
    #[serde(default)]
    pub undo_positions: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UndoContext {
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub positions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeCause {
    Local { origin: String },
    Import { origin: String },
    Undo,
    Redo,
    HistoryCleared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoState {
    pub can_undo: bool,
    pub can_redo: bool,
    pub group_open: bool,
}

/// Published after the entire operation and its undo bookkeeping have finished.
/// Applying `edits` to `before.text` must produce `after.text` exactly. Imports
/// may advance causal state without changing text, in which case edits is empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub cause: ChangeCause,
    pub before: TextSnapshot,
    pub after: TextSnapshot,
    pub edits: Vec<TextEdit>,
    pub undo: UndoState,
    pub restored_metadata: Option<Value>,
    /// Positions from the popped undo context, in AFTER-text UTF-16 coordinates.
    pub restored_positions: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketKind {
    Snapshot,
    Updates,
}

/// Host-owned transfer/storage object. This is not an authenticated transport.
/// The envelope prevents accidental cross-document/history imports. Only full
/// history snapshots and updates are supported; shallow history is rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPacket {
    pub identity: DocumentIdentity,
    pub kind: PacketKind,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub event: Option<ChangeEvent>,
    /// The supplied packet contained operations waiting for dependencies.
    pub pending: bool,
}

/// Which side of text inserted at the anchored gap the position follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Affinity {
    Before,
    After,
}

/// Serializable CRDT position, scoped to one document history. Coordinates are
/// deliberately opaque; use Document::resolve_anchor rather than decoding them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub(crate) identity: DocumentIdentity,
    pub(crate) affinity: Affinity,
    pub(crate) target: AnchorTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AnchorTarget {
    Start,
    End,
    Character { cursor: Vec<u8>, after: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAnchor {
    pub offset: usize,
    /// Equivalent position rebased onto current live text, to avoid repeatedly
    /// resolving deleted characters through old history.
    pub refreshed: Anchor,
}

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CoreError {
    #[error("document_id and history_id must be nonempty")]
    InvalidIdentity,
    #[error("the object belongs to a different document or history")]
    IdentityMismatch,
    #[error("the transaction was based on an obsolete state")]
    StaleVersion,
    #[error("invalid causal version")]
    InvalidVersion,
    #[error("invalid UTF-16 scalar boundary: {offset}")]
    InvalidPosition { offset: usize },
    #[error("edits must be ordered, disjoint half-open ranges with distinct starts")]
    InvalidEdits,
    #[error("writer {peer} already has operations in this history; use a fresh writer")]
    WriterAlreadyUsed { peer: String },
    #[error("incoming operations reuse this live writer's identity")]
    WriterCollision,
    #[error("shallow or unsupported CRDT history is not accepted")]
    UnsupportedHistory,
    #[error("packet kind does not match its payload")]
    InvalidPacket,
    #[error("an undo group is already open")]
    UndoGroupOpen,
    #[error("anchor is unavailable in this replica: {message}")]
    AnchorUnavailable { message: String },
    #[error("CRDT error: {message}")]
    Crdt { message: String },
}

pub(crate) fn crdt_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Crdt {
        message: error.to_string(),
    }
}
