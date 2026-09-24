//! One-way materialization of committed CRDT text. Files are never imported.
use super::{config::Config, store::Store};
use crate::{
    PeerNode,
    document::{DocumentIdentity, TextSnapshot, Version},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFileState {
    Pending,
    Synced,
    Conflict,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct TextFileStatus {
    pub document: String,
    pub path: PathBuf,
    pub state: TextFileState,
    pub version: Option<Version>,
    pub error: Option<String>,
}
pub(super) type Statuses = Arc<Mutex<BTreeMap<String, TextFileStatus>>>;

/// Both hashes are retained across the rename: after a crash, either the old
/// file or the prepared replacement is legitimate. Unknown bytes are a conflict.
#[derive(Serialize, Deserialize)]
pub(super) struct FileRecord {
    identity: DocumentIdentity,
    written: Option<[u8; 32]>,
    pending: Option<[u8; 32]>,
}

struct Target {
    path: PathBuf,
    attempted: Option<Instant>,
}
pub(super) struct TextFiles {
    targets: BTreeMap<String, Target>,
    pub statuses: Statuses,
}

impl TextFiles {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Shutdown must recheck files even when the normal retry delay has not
    /// elapsed (for example, a conflicting file was just moved aside).
    pub fn flush(&mut self, store: &Store, node: &Mutex<PeerNode>) -> Vec<String> {
        for target in self.targets.values_mut() {
            target.attempted = None;
        }
        self.sync(store, node)
    }

    pub fn new(config: &Config) -> Result<Self> {
        let mut targets = BTreeMap::new();
        let mut paths = BTreeSet::new();
        let mut statuses = BTreeMap::new();
        for document in &config.peer.documents {
            let Some(path) = &document.text_file else {
                continue;
            };
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            fs::create_dir_all(parent)
                .with_context(|| format!("create text_file directory {}", parent.display()))?;
            let path = parent
                .canonicalize()?
                .join(path.file_name().context("text_file needs a filename")?);
            for reserved in ["documents.redb", "node-id"] {
                if path == config.state_dir.join(reserved).canonicalize()? {
                    bail!(
                        "text_file cannot overwrite node storage: {}",
                        path.display()
                    );
                }
            }
            if !paths.insert(path.clone()) {
                bail!("documents share a text_file: {}", path.display());
            }
            let id = document.identity.document_id.clone();
            statuses.insert(
                id.clone(),
                TextFileStatus {
                    document: id.clone(),
                    path: path.clone(),
                    state: TextFileState::Pending,
                    version: None,
                    error: None,
                },
            );
            targets.insert(
                id,
                Target {
                    path,
                    attempted: None,
                },
            );
        }
        Ok(Self {
            targets,
            statuses: Arc::new(Mutex::new(statuses)),
        })
    }

    /// Called by the ordered native persistence worker, outside async I/O threads.
    pub fn sync(&mut self, store: &Store, node: &Mutex<PeerNode>) -> Vec<String> {
        let mut errors = Vec::new();
        for (id, target) in &mut self.targets {
            let previous = self.statuses.lock().unwrap()[id].clone();
            if previous.state != TextFileState::Synced
                && target
                    .attempted
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
            {
                continue;
            }
            let snapshot: Result<Option<TextSnapshot>> = (|| {
                let node = node
                    .lock()
                    .map_err(|_| anyhow::anyhow!("node lock poisoned"))?;
                let Some(durable) = node.durable_version(id) else {
                    return Ok(None);
                };
                if previous.state == TextFileState::Synced
                    && previous.version.as_ref() == Some(durable)
                    && target
                        .attempted
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
                {
                    return Ok(None);
                }
                let document = node.document(id)?;
                let document = document
                    .lock()
                    .map_err(|_| anyhow::anyhow!("document lock poisoned"))?;
                // A UI handle may already contain edits not captured by the last
                // committed journal entry. Wait until their commit catches up.
                if &document.version() != durable {
                    return Ok(None);
                }
                Ok(Some(document.snapshot()))
            })();
            let result = match snapshot {
                Ok(None) => continue,
                Ok(Some(snapshot)) => {
                    publish(store, &target.path, &snapshot).map(|state| (state, snapshot.version))
                }
                Err(error) => Err(error),
            };
            target.attempted = Some(Instant::now());
            let (state, version, error) = match result {
                Ok((TextFileState::Synced, version)) => (TextFileState::Synced, Some(version), None),
                Ok((state, _)) => (state, previous.version.clone(), Some("file was changed outside this node; output paused (move/remove the file to regenerate)".into())),
                Err(error) => (TextFileState::Error, previous.version.clone(), Some(format!("{error:#}"))),
            };
            if error != previous.error
                && let Some(error) = &error
            {
                errors.push(format!(
                    "text_file {} ({}): {error}",
                    id,
                    target.path.display()
                ));
            }
            self.statuses.lock().unwrap().insert(
                id.clone(),
                TextFileStatus {
                    document: id.clone(),
                    path: target.path.clone(),
                    state,
                    version,
                    error,
                },
            );
        }
        errors
    }
}

fn read_hash(path: &Path) -> Result<Option<[u8; 32]>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            // Do not replace a symlink or a directory, including links made by
            // an external editor after startup.
            if !metadata.is_file() {
                bail!("text_file must be a regular file: {}", path.display());
            }
            Ok(Some(Sha256::digest(fs::read(path)?).into()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn publish(store: &Store, path: &Path, snapshot: &TextSnapshot) -> Result<TextFileState> {
    let key = path.to_str().context("text_file path must be UTF-8")?;
    let existing = read_hash(path)?;
    let desired: [u8; 32] = Sha256::digest(snapshot.text.as_bytes()).into();
    let previous = store.text_file_record(key)?;
    if previous
        .as_ref()
        .is_some_and(|p| p.identity != snapshot.version.identity)
    {
        bail!("text_file belongs to another document history");
    }
    if existing == Some(desired)
        && previous
            .as_ref()
            .is_some_and(|p| p.written == existing && p.pending.is_none())
    {
        return Ok(TextFileState::Synced);
    }
    if let Some(existing) = existing
        && existing != desired
        && !previous
            .as_ref()
            .is_some_and(|p| p.written == Some(existing) || p.pending == Some(existing))
    {
        return Ok(TextFileState::Conflict);
    }
    let mut record = FileRecord {
        identity: snapshot.version.identity.clone(),
        written: existing,
        pending: Some(desired),
    };
    if existing != Some(desired) {
        let parent = path.parent().context("text_file parent missing")?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".notist-text-")
            .tempfile_in(parent)?;
        temporary.write_all(snapshot.text.as_bytes())?;
        if existing.is_some() {
            temporary
                .as_file()
                .set_permissions(fs::metadata(path)?.permissions())?;
        }
        temporary.as_file().sync_all()?;
        store.save_text_file_record(key, &record)?;
        // Recheck after preparing the replacement, so observed external edits
        // are not silently overwritten. This is not a cross-process file lock.
        if read_hash(path)? != existing {
            return Ok(TextFileState::Conflict);
        }
        if existing.is_some() {
            temporary.persist(path)?;
        } else {
            temporary.persist_noclobber(path)?;
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
    }
    record.written = Some(desired);
    record.pending = None;
    store.save_text_file_record(key, &record)?;
    Ok(TextFileState::Synced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Effect,
        document::{Document, TextEdit, Transaction},
    };

    fn document() -> Document {
        Document::new(
            DocumentIdentity {
                document_id: "note".into(),
                history_id: "history".into(),
            },
            None,
            "old 🧠\r\n",
        )
        .unwrap()
    }

    fn edit(doc: &mut Document, text: &str) {
        doc.transact(Transaction {
            expected_version: doc.version(),
            origin: "test".into(),
            edits: vec![TextEdit {
                from: 0,
                to: doc.snapshot().text.encode_utf16().count(),
                insert: text.into(),
            }],
            undo_metadata: None,
            undo_positions: vec![],
        })
        .unwrap();
    }

    fn commit(store: &Store, node: &mut PeerNode) {
        for effect in node.take_effects() {
            if let Effect::Persist { document, entry } = effect {
                store.commit(&document, &entry).unwrap();
                node.persisted(&document, entry.sequence).unwrap();
            }
        }
    }

    #[test]
    fn output_never_gets_ahead_of_journal_and_recovers_when_export_lags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.not");
        let database = dir.path().join("documents.redb");
        let store = Store::open(&database).unwrap();
        let doc = Arc::new(Mutex::new(document()));
        let mut node = PeerNode::new("node".into(), "session".into()).unwrap();
        node.attach(doc.clone(), "credential".into(), None).unwrap();
        commit(&store, &mut node);
        let mut writer = TextFiles {
            targets: BTreeMap::from([(
                "note".into(),
                Target {
                    path: path.clone(),
                    attempted: None,
                },
            )]),
            statuses: Arc::new(Mutex::new(BTreeMap::from([(
                "note".into(),
                TextFileStatus {
                    document: "note".into(),
                    path: path.clone(),
                    state: TextFileState::Pending,
                    version: None,
                    error: None,
                },
            )]))),
        };
        let node = Mutex::new(node);
        assert!(writer.sync(&store, &node).is_empty());
        let initial = fs::read(&path).unwrap();
        edit(&mut doc.lock().unwrap(), "committed edit");
        node.lock().unwrap().poll().unwrap();
        // The edit is queued but not stored: the text output must stay behind.
        writer.sync(&store, &node);
        assert_eq!(fs::read(&path).unwrap(), initial);
        commit(&store, &mut node.lock().unwrap());
        // Even after that commit a view handle can already contain a newer edit.
        edit(&mut doc.lock().unwrap(), "not yet committed");
        writer.sync(&store, &node);
        assert_eq!(fs::read(&path).unwrap(), initial);
        drop(store);
        drop(node);
        drop(doc);
        // Simulate restart after journal commit but before text replacement.
        let store = Store::open(&database).unwrap();
        let (recovered, _) = store.recover("note").unwrap().unwrap();
        assert_eq!(
            publish(&store, &path, &recovered.snapshot()).unwrap(),
            TextFileState::Synced
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "committed edit");
    }

    #[test]
    fn interrupted_atomic_replacement_accepts_old_or_prepared_bytes_but_not_external_edits() {
        for existing in ["old", "prepared", "external"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("note.not");
            let store = Store::open(&dir.path().join("documents.redb")).unwrap();
            let mut doc = document();
            edit(&mut doc, "latest committed source");
            fs::write(&path, existing).unwrap();
            store
                .save_text_file_record(
                    path.to_str().unwrap(),
                    &FileRecord {
                        identity: doc.identity().clone(),
                        written: Some(Sha256::digest(b"old").into()),
                        pending: Some(Sha256::digest(b"prepared").into()),
                    },
                )
                .unwrap();
            let state = publish(&store, &path, &doc.snapshot()).unwrap();
            if existing == "external" {
                assert_eq!(state, TextFileState::Conflict);
                assert_eq!(fs::read_to_string(&path).unwrap(), "external");
            } else {
                assert_eq!(state, TextFileState::Synced);
                assert_eq!(
                    fs::read_to_string(&path).unwrap(),
                    "latest committed source"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_does_not_replace_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.not");
        let other = dir.path().join("other.not");
        fs::write(&other, "other document").unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        let store = Store::open(&dir.path().join("documents.redb")).unwrap();
        assert!(publish(&store, &path, &document().snapshot()).is_err());
        assert!(path.is_symlink());
        assert_eq!(fs::read_to_string(other).unwrap(), "other document");
    }
}
