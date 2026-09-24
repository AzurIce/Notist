use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use notist_editor_document::*;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, mpsc::Receiver},
};

pub type DocumentHandle = Arc<Mutex<Document>>;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;
const CHUNK_BYTES: usize = 12 * 1024;
const MAX_LINKS: usize = 32;
const MAX_DOCUMENTS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error(transparent)]
    Document(#[from] CoreError),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("document not open: {0}")]
    MissingDocument(String),
    #[error("document lock poisoned")]
    Poisoned,
}
pub type Result<T> = std::result::Result<T, NodeError>;
fn invalid(message: &str) -> NodeError {
    NodeError::Protocol(message.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    pub session: String,
    pub body: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Hello {
        protocol: String,
        node: String,
    },
    Open {
        identity: DocumentIdentity,
        proof: String,
    },
    State {
        applied: Version,
        durable: Option<Version>,
    },
    Want {
        version: Version,
        request: u64,
    },
    Update {
        document: String,
        request: u64,
        kind: PacketKind,
        index: usize,
        total: usize,
        data: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub sequence: u64,
    pub packet: SyncPacket,
    /// Only this version is covered by this commit, never a later live version.
    pub applied: Version,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Send {
        link: String,
        message: WireMessage,
    },
    Persist {
        document: String,
        entry: JournalEntry,
    },
}

struct Managed {
    document: DocumentHandle,
    events: Receiver<ChangeEvent>,
    credential: String,
    queued: Version,
    durable: Option<Version>,
    sequence: u64,
    pending_commits: VecDeque<(u64, Version)>,
}
struct Incoming {
    request: u64,
    kind: Option<PacketKind>,
    total: usize,
    next: usize,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct Link {
    session: Option<String>,
    node: Option<String>,
    allowed: BTreeSet<String>,
    remote: BTreeMap<String, Version>,
    durable: BTreeMap<String, Version>,
    incoming: BTreeMap<String, Incoming>,
}

/// One node manages many documents. Editing handles and synchronization share
/// exactly the same Document; hosts call poll after edits and received messages.
pub struct PeerNode {
    node_id: String,
    session_id: String,
    documents: BTreeMap<String, Managed>,
    links: BTreeMap<String, Link>,
    effects: VecDeque<Effect>,
    next_request: u64,
}

impl PeerNode {
    pub fn new(node_id: String, session_id: String) -> Result<Self> {
        if node_id.is_empty()
            || session_id.is_empty()
            || node_id.len() > 256
            || session_id.len() > 256
        {
            return Err(invalid("invalid node/session identity"));
        }
        Ok(Self {
            node_id,
            session_id,
            documents: BTreeMap::new(),
            links: BTreeMap::new(),
            effects: VecDeque::new(),
            next_request: 1,
        })
    }
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn document(&self, id: &str) -> Result<DocumentHandle> {
        self.documents
            .get(id)
            .map(|d| d.document.clone())
            .ok_or_else(|| NodeError::MissingDocument(id.into()))
    }
    /// Attach an existing authoritative Document. A recovered sequence means
    /// the host has replayed a committed store record and its pending packets.
    pub fn attach(
        &mut self,
        document: DocumentHandle,
        credential: String,
        recovered: Option<(u64, bool)>,
    ) -> Result<()> {
        if credential.is_empty() || credential.len() > 4096 || self.documents.len() >= MAX_DOCUMENTS
        {
            return Err(invalid("invalid document credential or document limit"));
        }
        let (version, events, snapshot) = {
            let mut d = document.lock().map_err(|_| NodeError::Poisoned)?;
            (d.version(), d.subscribe(), d.export_snapshot()?)
        };
        let id = version.identity.document_id.clone();
        if id.is_empty()
            || id.len() > 256
            || version.identity.history_id.is_empty()
            || version.identity.history_id.len() > 256
        {
            return Err(invalid("invalid document identity"));
        }
        if self.documents.contains_key(&id) {
            return Err(invalid("document already open"));
        }
        self.documents.insert(
            id.clone(),
            Managed {
                document,
                events,
                credential,
                queued: version.clone(),
                durable: recovered
                    .filter(|(_, durable)| *durable)
                    .map(|_| version.clone()),
                sequence: recovered.map(|(seq, _)| seq).unwrap_or(0),
                pending_commits: VecDeque::new(),
            },
        );
        if recovered.is_none() {
            self.persist(&id, snapshot, version.clone())?;
        }
        for link in self.ready_links() {
            self.open(&link, &id);
        }
        Ok(())
    }
    pub fn connect(&mut self, link: String) -> Result<()> {
        if self.links.len() >= MAX_LINKS || self.links.contains_key(&link) || link.len() > 256 {
            return Err(invalid("duplicate link or link limit"));
        }
        self.links.insert(link.clone(), Link::default());
        self.send(
            &link,
            Message::Hello {
                protocol: "notist-peer".into(),
                node: self.node_id.clone(),
            },
        );
        Ok(())
    }
    pub fn disconnect(&mut self, link: &str) {
        self.links.remove(link);
        self.effects
            .retain(|effect| !matches!(effect, Effect::Send { link: id, .. } if id == link));
    }
    fn ready_links(&self) -> Vec<String> {
        self.links
            .iter()
            .filter(|(_, l)| l.session.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }
    fn send(&mut self, link: &str, body: Message) {
        self.effects.push_back(Effect::Send {
            link: link.into(),
            message: WireMessage {
                session: self.session_id.clone(),
                body,
            },
        });
    }
    fn open(&mut self, link: &str, id: &str) {
        let d = &self.documents[id];
        let proof = document_proof(
            &d.credential,
            &self.session_id,
            self.links[link].session.as_ref().unwrap(),
            &d.queued.identity,
        );
        self.send(
            link,
            Message::Open {
                identity: d.queued.identity.clone(),
                proof: STANDARD.encode(proof.finalize().into_bytes()),
            },
        );
    }
    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.effects.drain(..).collect()
    }

    fn persist(&mut self, id: &str, packet: SyncPacket, applied: Version) -> Result<()> {
        let d = self
            .documents
            .get_mut(id)
            .ok_or_else(|| NodeError::MissingDocument(id.into()))?;
        if d.pending_commits.len() >= 1024 {
            return Err(invalid("storage queue is full"));
        }
        d.sequence += 1;
        d.queued = applied.clone();
        d.pending_commits.push_back((d.sequence, applied.clone()));
        self.effects.push_back(Effect::Persist {
            document: id.into(),
            entry: JournalEntry {
                sequence: d.sequence,
                packet,
                applied,
            },
        });
        Ok(())
    }
    pub fn persisted(&mut self, id: &str, sequence: u64) -> Result<()> {
        self.stored(id, sequence, true)
    }
    pub fn stored(&mut self, id: &str, sequence: u64, durable: bool) -> Result<()> {
        let d = self
            .documents
            .get_mut(id)
            .ok_or_else(|| NodeError::MissingDocument(id.into()))?;
        let Some((expected, version)) = d.pending_commits.front() else {
            return Err(invalid("unknown storage receipt"));
        };
        if *expected != sequence {
            return Err(invalid("out-of-order storage receipt"));
        }
        if durable {
            d.durable = Some(version.clone());
        }
        d.pending_commits.pop_front();
        self.advertise(id)
    }
    pub fn durable_version(&self, id: &str) -> Option<&Version> {
        self.documents.get(id).and_then(|d| d.durable.as_ref())
    }
    pub fn remote_durable_version(&self, link: &str, id: &str) -> Option<&Version> {
        self.links.get(link)?.durable.get(id)
    }

    /// Capture changes made by UI handles, including undo/redo and imports.
    pub fn poll(&mut self) -> Result<()> {
        for id in self.documents.keys().cloned().collect::<Vec<_>>() {
            let d = &self.documents[&id];
            let mut changed = d.events.try_iter().count() != 0;
            let (version, packet) = {
                let doc = d.document.lock().map_err(|_| NodeError::Poisoned)?;
                (
                    doc.version(),
                    if doc.version() != d.queued {
                        Some(doc.export_updates_since(&d.queued)?)
                    } else {
                        None
                    },
                )
            };
            if let Some(packet) = packet {
                self.persist(&id, packet, version)?;
                changed = true;
            }
            if changed {
                self.advertise(&id)?;
            }
        }
        Ok(())
    }
    /// Preserve raw causally premature packets as well as applied operations.
    pub fn import(&mut self, packet: SyncPacket, origin: String) -> Result<ImportResult> {
        self.poll()?;
        let id = packet.identity.document_id.clone();
        let handle = self.document(&id)?;
        if self.documents[&id].pending_commits.len() >= 1024 {
            return Err(invalid("storage queue is full"));
        }
        let (result, version) = {
            let mut doc = handle.lock().map_err(|_| NodeError::Poisoned)?;
            let result = doc.import(&packet, origin)?;
            (result, doc.version())
        };
        if result.event.is_some() || result.pending {
            self.persist(&id, packet, version)?;
        }
        self.poll()?;
        Ok(result)
    }
    fn advertise(&mut self, id: &str) -> Result<()> {
        let d = &self.documents[id];
        let applied = d
            .document
            .lock()
            .map_err(|_| NodeError::Poisoned)?
            .version();
        let durable = d.durable.clone();
        for link in self
            .links
            .iter()
            .filter(|(_, l)| l.allowed.contains(id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
        {
            self.send(
                &link,
                Message::State {
                    applied: applied.clone(),
                    durable: durable.clone(),
                },
            );
        }
        Ok(())
    }
    fn authorized(&self, link: &str, id: &str) -> Result<()> {
        if !self.links.get(link).is_some_and(|l| l.allowed.contains(id)) {
            return Err(invalid("document is not authorized on this link"));
        }
        Ok(())
    }
    fn check_version(&self, version: &Version) -> Result<()> {
        if version.clocks.len() > 4096 {
            return Err(invalid("version too large"));
        }
        let doc = self.document(&version.identity.document_id)?;
        if doc.lock().map_err(|_| NodeError::Poisoned)?.identity() != &version.identity {
            return Err(CoreError::IdentityMismatch.into());
        }
        version.encode()?;
        Ok(())
    }
    fn request_missing(&mut self, link: &str, id: &str) -> Result<()> {
        let local = self
            .document(id)?
            .lock()
            .map_err(|_| NodeError::Poisoned)?
            .version();
        let l = self
            .links
            .get_mut(link)
            .ok_or_else(|| invalid("unknown link"))?;
        if l.incoming.contains_key(id) || !l.remote.get(id).is_some_and(|v| !covers(&local, v)) {
            return Ok(());
        }
        // One aggregate transfer per link bounds reassembly memory.
        if !l.incoming.is_empty() {
            return Ok(());
        }
        let request = self.next_request;
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or_else(|| invalid("request overflow"))?;
        l.incoming.insert(
            id.into(),
            Incoming {
                request,
                kind: None,
                total: 0,
                next: 0,
                bytes: Vec::new(),
            },
        );
        self.send(
            link,
            Message::Want {
                version: local,
                request,
            },
        );
        Ok(())
    }
    pub fn receive(&mut self, link: &str, wire: WireMessage) -> Result<()> {
        if self.effects.len() > 2048 {
            return Err(invalid("host must drain pending effects"));
        }
        self.poll()?;
        let l = self
            .links
            .get_mut(link)
            .ok_or_else(|| invalid("unknown link"))?;
        if let Message::Hello { protocol, node } = &wire.body {
            if l.session.is_some()
                || protocol != "notist-peer"
                || node == &self.node_id
                || node.is_empty()
                || node.len() > 256
                || wire.session.is_empty()
                || wire.session.len() > 256
            {
                return Err(invalid("invalid peer hello"));
            }
            l.session = Some(wire.session);
            l.node = Some(node.clone());
            for id in self.documents.keys().cloned().collect::<Vec<_>>() {
                self.open(link, &id);
            }
            return Ok(());
        }
        if l.session.as_deref() != Some(&wire.session) {
            return Err(invalid("stale or missing session"));
        }
        match wire.body {
            Message::Hello { .. } => unreachable!(),
            Message::Open { identity, proof } => {
                let id = &identity.document_id;
                // An unrelated document is not an error for a multi-document peer.
                let Some(d) = self.documents.get(id) else {
                    return Ok(());
                };
                let mac = document_proof(
                    &d.credential,
                    self.links[link].session.as_ref().unwrap(),
                    &self.session_id,
                    &identity,
                );
                if identity != d.queued.identity
                    || STANDARD
                        .decode(proof)
                        .ok()
                        .is_none_or(|proof| mac.verify_slice(&proof).is_err())
                {
                    return Err(invalid("document authorization failed"));
                }
                if self.links.get_mut(link).unwrap().allowed.insert(id.clone()) {
                    self.open(link, id);
                }
                self.advertise(id)?;
            }
            Message::State { applied, durable } => {
                let id = applied.identity.document_id.clone();
                self.authorized(link, &id)?;
                self.check_version(&applied)?;
                if let Some(v) = &durable {
                    self.check_version(v)?;
                    if !covers(&applied, v) {
                        return Err(invalid("durable state exceeds applied state"));
                    }
                }
                let l = self.links.get_mut(link).unwrap();
                l.remote.insert(id.clone(), applied);
                if let Some(v) = durable {
                    l.durable.insert(id.clone(), v);
                } else {
                    l.durable.remove(&id);
                }
                self.request_missing(link, &id)?;
            }
            Message::Want { version, request } => {
                let id = version.identity.document_id.clone();
                self.authorized(link, &id)?;
                self.check_version(&version)?;
                let packet = self
                    .document(&id)?
                    .lock()
                    .map_err(|_| NodeError::Poisoned)?
                    .export_updates_since(&version)?;
                if packet.data.len() > MAX_PACKET_BYTES {
                    return Err(invalid("update exceeds packet limit"));
                }
                let total = packet.data.len().div_ceil(CHUNK_BYTES);
                for (index, data) in packet.data.chunks(CHUNK_BYTES).enumerate() {
                    self.send(
                        link,
                        Message::Update {
                            document: id.clone(),
                            request,
                            kind: packet.kind,
                            index,
                            total,
                            data: STANDARD.encode(data),
                        },
                    );
                }
            }
            Message::Update {
                document,
                request,
                kind,
                index,
                total,
                data,
            } => {
                self.authorized(link, &document)?;
                if total == 0
                    || total > MAX_PACKET_BYTES.div_ceil(CHUNK_BYTES)
                    || data.len() > CHUNK_BYTES * 4 / 3
                {
                    return Err(invalid("invalid transfer size"));
                }
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| invalid("invalid base64"))?;
                let l = self.links.get_mut(link).unwrap();
                let transfer = l
                    .incoming
                    .get_mut(&document)
                    .ok_or_else(|| invalid("unsolicited transfer"))?;
                if request != transfer.request
                    || index != transfer.next
                    || transfer.kind.is_some_and(|k| k != kind)
                    || (index > 0 && total != transfer.total)
                    || transfer.bytes.len() + bytes.len() > MAX_PACKET_BYTES
                {
                    return Err(invalid("invalid transfer sequence"));
                }
                transfer.kind = Some(kind);
                transfer.total = total;
                transfer.next += 1;
                transfer.bytes.extend(bytes);
                if transfer.next == total {
                    let transfer = l.incoming.remove(&document).unwrap();
                    let identity = self.documents[&document].queued.identity.clone();
                    let before = self
                        .document(&document)?
                        .lock()
                        .map_err(|_| NodeError::Poisoned)?
                        .version();
                    self.import(
                        SyncPacket {
                            identity,
                            kind,
                            data: transfer.bytes,
                        },
                        format!("peer:{link}"),
                    )?;
                    let after = self
                        .document(&document)?
                        .lock()
                        .map_err(|_| NodeError::Poisoned)?
                        .version();
                    if before == after
                        && self.links[link]
                            .remote
                            .get(&document)
                            .is_some_and(|v| !covers(&after, v))
                    {
                        return Err(invalid("peer transfer made no progress"));
                    }
                    for id in self.links[link].remote.keys().cloned().collect::<Vec<_>>() {
                        self.request_missing(link, &id)?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn covers(a: &Version, b: &Version) -> bool {
    a.identity == b.identity
        && b.clocks
            .iter()
            .all(|(peer, clock)| a.clocks.get(peer).copied().unwrap_or(0) >= *clock)
}

fn document_proof(secret: &str, from: &str, to: &str, identity: &DocumentIdentity) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(&serde_json::to_vec(&("notist-document", from, to, identity)).unwrap());
    mac
}
