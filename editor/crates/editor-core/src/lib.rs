//! Language-independent, single-document editing kernel.
//!
//! The only replicated state is plain text. All writes pass through validated
//! UTF-16 transactions or history-scoped CRDT imports. Language services, view
//! projections, networking, persistence and presence are host responsibilities.
//! Keep full history: this API never exposes checkout, raw Loro containers or
//! shallow snapshots. Undo is local to the lifetime of one fixed writer.

mod text;
mod types;
pub use text::utf16_to_byte;
pub use types::*;

use loro::cursor::{Cursor, Side};
use loro::{
    ContainerTrait, EncodedBlobMode, ExportMode, LoroDoc, UndoItemMeta, UndoManager, VersionVector,
};
use serde_json::Value;
use std::sync::{Arc, Mutex, mpsc};
use text::{difference, validate_edits};
use types::{AnchorTarget, crdt_error};

pub struct Document {
    identity: DocumentIdentity,
    doc: LoroDoc,
    undo: UndoManager,
    revision: u64,
    group_open: bool,
    undo_payload: Arc<Mutex<Option<Value>>>,
    popped_payload: Arc<Mutex<Option<Value>>>,
    undo_cursors: Arc<Mutex<Vec<Cursor>>>,
    popped_positions: Arc<Mutex<Vec<usize>>>,
    subscribers: Vec<mpsc::Sender<ChangeEvent>>,
}

impl Document {
    /// Create a new history once, then distribute its snapshot to replicas.
    /// `peer=None` generates a writer; explicit peers are useful for hosts/tests
    /// which already guarantee uniqueness. Initial text is not an undo step.
    pub fn new(
        identity: DocumentIdentity,
        peer: Option<u64>,
        initial: &str,
    ) -> Result<Self, CoreError> {
        validate_identity(&identity)?;
        let doc = LoroDoc::new();
        if let Some(peer) = peer {
            doc.set_peer_id(peer).map_err(crdt_error)?;
        }
        doc.get_text("source")
            .insert(0, initial)
            .map_err(crdt_error)?;
        doc.commit();
        Ok(Self::attach(identity, doc))
    }

    /// Restore complete history with a fresh writer. Undo history and subscribers
    /// are intentionally local and are not restored from a CRDT snapshot.
    pub fn from_snapshot(packet: &SyncPacket, peer: Option<u64>) -> Result<Self, CoreError> {
        validate_identity(&packet.identity)?;
        validate_packet(packet)?;
        if packet.kind != PacketKind::Snapshot {
            return Err(CoreError::InvalidPacket);
        }
        let doc = LoroDoc::from_snapshot(&packet.data).map_err(crdt_error)?;
        if doc.is_shallow() {
            return Err(CoreError::UnsupportedHistory);
        }
        validate_document(&doc)?;
        let peer = peer.unwrap_or_else(|| LoroDoc::new().peer_id());
        if doc.oplog_vv().get(&peer).copied().unwrap_or(0) != 0 {
            return Err(CoreError::WriterAlreadyUsed {
                peer: peer.to_string(),
            });
        }
        doc.set_peer_id(peer).map_err(crdt_error)?;
        Ok(Self::attach(packet.identity.clone(), doc))
    }

    fn attach(identity: DocumentIdentity, doc: LoroDoc) -> Self {
        let undo_payload = Arc::new(Mutex::new(None::<Value>));
        let popped_payload = Arc::new(Mutex::new(None::<Value>));
        let payload = undo_payload.clone();
        let popped = popped_payload.clone();
        let undo_cursors = Arc::new(Mutex::new(Vec::<Cursor>::new()));
        let popped_positions = Arc::new(Mutex::new(Vec::<usize>::new()));
        let cursors = undo_cursors.clone();
        let positions = popped_positions.clone();
        let mut undo = UndoManager::new(&doc);
        undo.set_merge_interval(0);
        undo.set_max_undo_steps(500);
        undo.set_on_push(Some(Box::new(move |_, _, _| {
            let mut meta = UndoItemMeta::new();
            // The payload is not CRDT document state. JSON also keeps it opaque
            // to the native kernel while allowing arbitrary host selections.
            meta.set_value(
                serde_json::to_string(&*payload.lock().unwrap())
                    .unwrap()
                    .into(),
            );
            for cursor in cursors.lock().unwrap().iter() {
                meta.add_cursor(cursor);
            }
            meta
        })));
        undo.set_on_pop(Some(Box::new(move |_, _, meta| {
            *positions.lock().unwrap() = meta
                .cursors
                .iter()
                .map(|cursor| {
                    // Loro intentionally does not transform absolute end cursors.
                    // Resolve them against AFTER text instead of using a stale size.
                    if cursor.cursor.id.is_none() && cursor.cursor.side == Side::Right {
                        usize::MAX
                    } else {
                        cursor.pos.pos
                    }
                })
                .collect();
            *popped.lock().unwrap() = match meta.value {
                loro::LoroValue::String(json) => {
                    serde_json::from_str::<Option<Value>>(json.as_str())
                        .ok()
                        .flatten()
                }
                _ => None,
            };
        })));
        Self {
            identity,
            doc,
            undo,
            revision: 0,
            group_open: false,
            undo_payload,
            popped_payload,
            undo_cursors,
            popped_positions,
            subscribers: vec![],
        }
    }

    pub fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }
    pub fn writer_id(&self) -> String {
        self.doc.peer_id().to_string()
    }

    pub fn snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            text: self.doc.get_text("source").to_string(),
            version: self.version(),
            revision: self.revision,
        }
    }

    pub fn version(&self) -> Version {
        Version {
            identity: self.identity.clone(),
            clocks: self
                .doc
                .state_vv()
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(peer, count)| (peer.to_string(), *count))
                .collect(),
        }
    }

    pub fn undo_state(&self) -> UndoState {
        UndoState {
            can_undo: self.undo.can_undo(),
            can_redo: self.undo.can_redo(),
            group_open: self.group_open,
        }
    }

    /// Independent ordered subscriber. Delivery is queued AFTER operations,
    /// avoiding callbacks inside Loro commits or a frontend's state update.
    /// Drop the receiver to unsubscribe. Subscribe then read while holding the
    /// same mutable Document to establish an initial snapshot without a gap.
    pub fn subscribe(&mut self) -> mpsc::Receiver<ChangeEvent> {
        let (sender, receiver) = mpsc::channel();
        self.subscribers.push(sender);
        receiver
    }

    /// Atomically validates every edit before touching Loro. One transaction is
    /// one undo step unless the host explicitly opened an undo group.
    pub fn transact(&mut self, transaction: Transaction) -> Result<Option<ChangeEvent>, CoreError> {
        self.check_identity(&transaction.expected_version.identity)?;
        if transaction.expected_version != self.version() {
            return Err(CoreError::StaleVersion);
        }
        let before = self.snapshot();
        let edits = validate_edits(&before.text, &transaction.edits)?;
        let cursors = self.undo_cursors_at(&transaction.undo_positions)?;
        if edits.is_empty() {
            return Ok(None);
        }
        *self.undo_payload.lock().unwrap() = transaction.undo_metadata;
        *self.undo_cursors.lock().unwrap() = cursors;
        let text = self.doc.get_text("source");
        for edit in edits.iter().rev() {
            if edit.to > edit.from {
                text.delete_utf16(edit.from, edit.to - edit.from)
                    .map_err(crdt_error)?;
            }
            if !edit.insert.is_empty() {
                text.insert_utf16(edit.from, &edit.insert)
                    .map_err(crdt_error)?;
            }
        }
        self.doc.set_next_commit_origin(&transaction.origin);
        self.doc.commit();
        Ok(self.publish(
            before,
            ChangeCause::Local {
                origin: transaction.origin,
            },
            Some(edits),
            None,
        ))
    }

    pub fn begin_undo_group(&mut self) -> Result<(), CoreError> {
        if self.group_open {
            return Err(CoreError::UndoGroupOpen);
        }
        self.undo.group_start().map_err(crdt_error)?;
        self.group_open = true;
        Ok(())
    }

    pub fn end_undo_group(&mut self) {
        self.undo.group_end();
        self.group_open = false;
    }

    pub fn clear_undo(&mut self) {
        let before = self.snapshot();
        let previous = self.undo_state();
        self.end_undo_group();
        self.undo.clear();
        if previous != self.undo_state() {
            self.publish(before, ChangeCause::HistoryCleared, None, None);
        }
    }

    /// `metadata` describes the host's current state for the inverse redo step.
    pub fn undo(&mut self, metadata: Option<Value>) -> Result<Option<ChangeEvent>, CoreError> {
        self.undo_with_context(UndoContext {
            metadata,
            positions: vec![],
        })
    }

    pub fn redo(&mut self, metadata: Option<Value>) -> Result<Option<ChangeEvent>, CoreError> {
        self.redo_with_context(UndoContext {
            metadata,
            positions: vec![],
        })
    }

    pub fn undo_with_context(
        &mut self,
        context: UndoContext,
    ) -> Result<Option<ChangeEvent>, CoreError> {
        self.history(false, context)
    }

    pub fn redo_with_context(
        &mut self,
        context: UndoContext,
    ) -> Result<Option<ChangeEvent>, CoreError> {
        self.history(true, context)
    }

    fn undo_cursors_at(&self, positions: &[usize]) -> Result<Vec<Cursor>, CoreError> {
        let text = self.doc.get_text("source");
        let value = text.to_string();
        positions
            .iter()
            .map(|offset| {
                let byte = utf16_to_byte(&value, *offset)?;
                text.get_cursor(value[..byte].chars().count(), Side::Middle)
                    .ok_or(CoreError::InvalidPosition { offset: *offset })
            })
            .collect()
    }

    fn history(
        &mut self,
        redo: bool,
        context: UndoContext,
    ) -> Result<Option<ChangeEvent>, CoreError> {
        let cursors = self.undo_cursors_at(&context.positions)?;
        let previous = self.undo_state();
        self.end_undo_group();
        let before = self.snapshot();
        *self.popped_payload.lock().unwrap() = None;
        self.popped_positions.lock().unwrap().clear();
        *self.undo_payload.lock().unwrap() = context.metadata;
        *self.undo_cursors.lock().unwrap() = cursors;
        let changed = if redo {
            self.undo.redo()
        } else {
            self.undo.undo()
        }
        .map_err(crdt_error)?;
        if !changed && previous == self.undo_state() {
            return Ok(None);
        }
        let restored = self.popped_payload.lock().unwrap().take();
        Ok(self.publish(
            before,
            if redo {
                ChangeCause::Redo
            } else {
                ChangeCause::Undo
            },
            None,
            restored,
        ))
    }

    pub fn export_snapshot(&self) -> Result<SyncPacket, CoreError> {
        Ok(SyncPacket {
            identity: self.identity.clone(),
            kind: PacketKind::Snapshot,
            data: self.doc.export(ExportMode::Snapshot).map_err(crdt_error)?,
        })
    }

    /// A peer may know edits absent locally; Loro sends only locally available
    /// operations beyond the supplied clocks. Receiving updates is idempotent.
    pub fn export_updates_since(&self, version: &Version) -> Result<SyncPacket, CoreError> {
        self.check_identity(&version.identity)?;
        let from = version_vector(version)?;
        Ok(SyncPacket {
            identity: self.identity.clone(),
            kind: PacketKind::Updates,
            data: self
                .doc
                .export(ExportMode::updates(&from))
                .map_err(crdt_error)?,
        })
    }

    /// Import complete-history packets. Causally premature updates are retained
    /// by Loro until predecessors arrive. An import closes an explicit undo
    /// group, making its boundary deterministic rather than timing-dependent.
    pub fn import(
        &mut self,
        packet: &SyncPacket,
        origin: String,
    ) -> Result<ImportResult, CoreError> {
        self.check_identity(&packet.identity)?;
        let meta = validate_packet(packet)?;
        let peer = self.doc.peer_id();
        if meta.partial_end_vv.get(&peer).copied().unwrap_or(0)
            > self.doc.oplog_vv().get(&peer).copied().unwrap_or(0)
        {
            return Err(CoreError::WriterCollision);
        }
        // Validate/decode against a fork first. A corrupt packet must not mutate
        // the live document or its undo history partway through an import.
        let trial = self.doc.fork();
        trial.import(&packet.data).map_err(crdt_error)?;
        if trial.is_shallow() {
            return Err(CoreError::UnsupportedHistory);
        }
        validate_document(&trial)?;
        let before = self.snapshot();
        self.end_undo_group();
        let status = self
            .doc
            .import_with(&packet.data, &origin)
            .map_err(crdt_error)?;
        let event = self.publish(before, ChangeCause::Import { origin }, None, None);
        Ok(ImportResult {
            event,
            pending: status.pending.is_some_and(|p| !p.is_empty()),
        })
    }

    /// Before-affinity binds to the preceding character, after-affinity to the
    /// following one. At outer boundaries they bind to start/end respectively.
    /// Deleting the referenced character collapses the anchor to Loro's retained
    /// deletion gap; resolving returns a refreshed anchor with the same affinity.
    pub fn anchor_at(&self, offset: usize, affinity: Affinity) -> Result<Anchor, CoreError> {
        let text = self.doc.get_text("source");
        let value = text.to_string();
        let byte = utf16_to_byte(&value, offset)?;
        let index = value[..byte].chars().count();
        let target = match affinity {
            Affinity::Before if offset == 0 => AnchorTarget::Start,
            Affinity::After if byte == value.len() => AnchorTarget::End,
            _ => {
                let after = affinity == Affinity::Before;
                let cursor = text
                    .get_cursor(if after { index - 1 } else { index }, Side::Middle)
                    .ok_or_else(|| CoreError::AnchorUnavailable {
                        message: "missing character".into(),
                    })?;
                AnchorTarget::Character {
                    cursor: cursor.encode(),
                    after,
                }
            }
        };
        Ok(Anchor {
            identity: self.identity.clone(),
            affinity,
            target,
        })
    }

    pub fn resolve_anchor(&self, anchor: &Anchor) -> Result<ResolvedAnchor, CoreError> {
        self.check_identity(&anchor.identity)?;
        let text = self.doc.get_text("source");
        let value = text.to_string();
        let offset = match &anchor.target {
            AnchorTarget::Start => 0,
            AnchorTarget::End => value.encode_utf16().count(),
            AnchorTarget::Character { cursor, after } => {
                let cursor = Cursor::decode(cursor).map_err(|e| CoreError::AnchorUnavailable {
                    message: e.to_string(),
                })?;
                if cursor.container != text.id() || cursor.id.is_none() {
                    return Err(CoreError::AnchorUnavailable {
                        message: "not a source character".into(),
                    });
                }
                let resolved =
                    self.doc
                        .get_cursor_pos(&cursor)
                        .map_err(|e| CoreError::AnchorUnavailable {
                            message: e.to_string(),
                        })?;
                let index = resolved.current.pos + usize::from(*after && resolved.update.is_none());
                value.chars().take(index).map(char::len_utf16).sum()
            }
        };
        Ok(ResolvedAnchor {
            offset,
            refreshed: self.anchor_at(offset, anchor.affinity)?,
        })
    }

    fn check_identity(&self, identity: &DocumentIdentity) -> Result<(), CoreError> {
        if identity == &self.identity {
            Ok(())
        } else {
            Err(CoreError::IdentityMismatch)
        }
    }

    fn publish(
        &mut self,
        before: TextSnapshot,
        cause: ChangeCause,
        edits: Option<Vec<TextEdit>>,
        restored_metadata: Option<Value>,
    ) -> Option<ChangeEvent> {
        if before.version == self.version()
            && matches!(
                cause,
                ChangeCause::Local { .. } | ChangeCause::Import { .. }
            )
        {
            return None;
        }
        self.revision += 1;
        let after = self.snapshot();
        let restored_positions = if matches!(cause, ChangeCause::Undo | ChangeCause::Redo) {
            self.popped_positions
                .lock()
                .unwrap()
                .iter()
                .map(|index| after.text.chars().take(*index).map(char::len_utf16).sum())
                .collect()
        } else {
            vec![]
        };
        let event = ChangeEvent {
            edits: edits.unwrap_or_else(|| difference(&before.text, &after.text)),
            before,
            after,
            cause,
            undo: self.undo_state(),
            restored_metadata,
            restored_positions,
        };
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
        Some(event)
    }
}

fn validate_document(doc: &LoroDoc) -> Result<(), CoreError> {
    let loro::LoroValue::Map(roots) = doc.get_value() else {
        return Err(CoreError::InvalidPacket);
    };
    if roots.iter().any(|(name, value)| name != "source" || !matches!(value, loro::LoroValue::Container(id) if *id == doc.get_text("source").id())) {
        return Err(CoreError::InvalidPacket);
    }
    Ok(())
}

fn validate_identity(identity: &DocumentIdentity) -> Result<(), CoreError> {
    if identity.document_id.is_empty() || identity.history_id.is_empty() {
        Err(CoreError::InvalidIdentity)
    } else {
        Ok(())
    }
}

fn version_vector(version: &Version) -> Result<VersionVector, CoreError> {
    let mut vector = VersionVector::default();
    for (peer, count) in &version.clocks {
        let peer_id: u64 = peer.parse().map_err(|_| CoreError::InvalidVersion)?;
        if *count <= 0 || peer_id.to_string() != *peer {
            return Err(CoreError::InvalidVersion);
        }
        vector.insert(peer_id, *count);
    }
    Ok(vector)
}

fn validate_packet(packet: &SyncPacket) -> Result<loro::ImportBlobMetadata, CoreError> {
    let meta = LoroDoc::decode_import_blob_meta(&packet.data, true).map_err(crdt_error)?;
    match (packet.kind, meta.mode) {
        (PacketKind::Snapshot, EncodedBlobMode::Snapshot)
        | (PacketKind::Updates, EncodedBlobMode::Updates) => Ok(meta),
        (_, EncodedBlobMode::ShallowSnapshot) => Err(CoreError::UnsupportedHistory),
        _ => Err(CoreError::InvalidPacket),
    }
}
