//! Shared application service for embedded and daemon-hosted Notist clients.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use notist_analysis::{
    AnalyzerConfiguration, AnalyzerView, DocumentVersions, SourceOverlays, VaultEngine,
    WorkspaceSnapshot, resolve_vault_root,
};
use serde::{Deserialize, Serialize};

pub mod protocol;
pub mod query;
mod request;
pub mod transport;

pub use query::*;
pub use request::*;

/// Protocol-independent service shared by embedded clients and the daemon.
pub struct NotistService {
    instance_id: DaemonInstanceId,
    vault_root: Option<PathBuf>,
    vaults: Mutex<BTreeMap<PathBuf, Arc<VaultHost>>>,
    views: Mutex<HashMap<ServiceViewId, ViewEntry>>,
    edit_plans: Mutex<HashMap<String, request::StoredEditPlan>>,
    applied_edits: Mutex<HashMap<String, request::ApplyEditRecord>>,
    renamed_sources: Mutex<HashMap<String, request::RenameSourceRecord>>,
    search_indexes: Arc<Mutex<HashMap<String, Arc<query::SearchIndex>>>>,
    search_index_builds: Arc<Mutex<HashMap<String, Arc<SearchIndexBuild>>>>,
    runtime_mode: &'static str,
    next_view: AtomicU64,
}

struct SearchIndexBuild {
    operation_handle: String,
    result: Mutex<Option<Result<Arc<query::SearchIndex>, String>>>,
    ready: Condvar,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct DaemonInstanceId(pub String);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ServiceViewId(pub u64);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct VaultIdentity {
    pub canonical_root: PathBuf,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotIdentity {
    pub daemon_instance: DaemonInstanceId,
    pub vault: VaultIdentity,
    pub view_id: ServiceViewId,
    pub view_kind: String,
    pub analyzer_view_id: u64,
    pub revision: u64,
    pub source_fingerprint: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewKind {
    Disk,
    Session,
}

struct VaultHost {
    identity: VaultIdentity,
    engine: VaultEngine,
    disk: Arc<Mutex<AnalyzerView>>,
    sessions: Arc<Mutex<Vec<Weak<Mutex<AnalyzerView>>>>>,
    _watcher: Mutex<Debouncer<RecommendedWatcher>>,
    write_lock: Mutex<()>,
}

struct ViewEntry {
    host: Arc<VaultHost>,
    view: Arc<Mutex<AnalyzerView>>,
    kind: ViewKind,
}

impl NotistService {
    pub fn new() -> Self {
        Self::with_root(None, "embedded")
    }

    pub fn for_root(root: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::with_root(
            Some(dunce::canonicalize(root)?),
            "embedded",
        ))
    }

    pub fn for_daemon_root(root: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::with_root(Some(dunce::canonicalize(root)?), "daemon"))
    }

    fn with_root(vault_root: Option<PathBuf>, runtime_mode: &'static str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            instance_id: DaemonInstanceId(format!("{}-{nonce:x}", std::process::id())),
            vault_root,
            vaults: Mutex::new(BTreeMap::new()),
            views: Mutex::new(HashMap::new()),
            edit_plans: Mutex::new(HashMap::new()),
            applied_edits: Mutex::new(HashMap::new()),
            renamed_sources: Mutex::new(HashMap::new()),
            search_indexes: Arc::new(Mutex::new(HashMap::new())),
            search_index_builds: Arc::new(Mutex::new(HashMap::new())),
            runtime_mode,
            next_view: AtomicU64::new(1),
        }
    }

    pub fn instance_id(&self) -> &DaemonInstanceId {
        &self.instance_id
    }

    pub fn runtime_mode(&self) -> &'static str {
        self.runtime_mode
    }

    pub fn open_view(
        &self,
        root: impl AsRef<Path>,
        kind: ViewKind,
    ) -> io::Result<(ServiceViewId, VaultIdentity)> {
        let root = resolve_vault_root(root.as_ref())?;
        if self.vault_root.as_ref().is_some_and(|bound| bound != &root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "service only serves its configured vault",
            ));
        }
        let host = self.open_host(&root)?;
        let view = match kind {
            ViewKind::Disk => host.disk.clone(),
            ViewKind::Session => {
                let view = Arc::new(Mutex::new(
                    host.engine
                        .view_with_versions(SourceOverlays::new(), DocumentVersions::new())?,
                ));
                host.sessions.lock().unwrap().push(Arc::downgrade(&view));
                view
            }
        };
        let id = ServiceViewId(self.next_view.fetch_add(1, Ordering::Relaxed));
        self.views.lock().unwrap().insert(
            id,
            ViewEntry {
                host: host.clone(),
                view,
                kind,
            },
        );
        Ok((id, host.identity.clone()))
    }

    pub fn close_view(&self, view_id: ServiceViewId) {
        self.views.lock().unwrap().remove(&view_id);
    }

    pub fn replace_view_inputs(
        &self,
        view_id: ServiceViewId,
        overlays: SourceOverlays,
        versions: DocumentVersions,
        configuration: Option<AnalyzerConfiguration>,
    ) -> io::Result<SnapshotIdentity> {
        let (host, view, kind) = self.view(view_id)?;
        if kind != ViewKind::Session {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "disk views do not accept client overlays",
            ));
        }
        let snapshot = {
            let mut view = view.lock().unwrap();
            if let Some(configuration) = configuration {
                view.replace_configuration(configuration)?;
            }
            view.replace_inputs(overlays, versions)?
        };
        Ok(self.snapshot_identity(view_id, &host, &snapshot))
    }

    /// Captures one immutable snapshot before executing a core operation.
    pub fn with_snapshot<T>(
        &self,
        view_id: ServiceViewId,
        operation: impl FnOnce(&WorkspaceSnapshot) -> T,
    ) -> io::Result<(SnapshotIdentity, T)> {
        let (host, view, _) = self.view(view_id)?;
        let snapshot = view.lock().unwrap().snapshot();
        let identity = self.snapshot_identity(view_id, &host, &snapshot);
        Ok((identity, operation(&snapshot)))
    }

    pub fn with_snapshot_identity<T>(
        &self,
        view_id: ServiceViewId,
        operation: impl FnOnce(&WorkspaceSnapshot, &SnapshotIdentity) -> T,
    ) -> io::Result<(SnapshotIdentity, T)> {
        let (host, view, _) = self.view(view_id)?;
        let snapshot = view.lock().unwrap().snapshot();
        let identity = self.snapshot_identity(view_id, &host, &snapshot);
        let result = operation(&snapshot, &identity);
        Ok((identity, result))
    }

    pub fn view_kind(&self, view_id: ServiceViewId) -> io::Result<ViewKind> {
        self.views
            .lock()
            .unwrap()
            .get(&view_id)
            .map(|entry| entry.kind)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown view handle"))
    }

    pub fn reload_disk_view(&self, view_id: ServiceViewId) -> io::Result<SnapshotIdentity> {
        let (host, view, kind) = self.view(view_id)?;
        if kind != ViewKind::Disk {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only disk views can be explicitly reloaded",
            ));
        }
        let snapshot = view.lock().unwrap().reload()?;
        Ok(self.snapshot_identity(view_id, &host, &snapshot))
    }

    fn open_host(&self, root: &Path) -> io::Result<Arc<VaultHost>> {
        if let Some(host) = self.vaults.lock().unwrap().get(root).cloned() {
            return Ok(host);
        }

        let engine = VaultEngine::open(root)?;
        let disk = Arc::new(Mutex::new(engine.disk_view()?));
        let sessions: Arc<Mutex<Vec<Weak<Mutex<AnalyzerView>>>>> = Arc::new(Mutex::new(Vec::new()));
        let watcher_disk = disk.clone();
        let watcher_sessions = sessions.clone();
        let mut watcher = new_debouncer(
            Duration::from_millis(250),
            move |result: DebounceEventResult| {
                let Ok(events) = result else {
                    return;
                };
                if !events.iter().any(|event| {
                    event
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        == Some("not")
                        || event.path.file_name().and_then(|name| name.to_str())
                            == Some(notist_analysis::MANIFEST_FILE)
                }) {
                    return;
                }
                if let Ok(mut view) = watcher_disk.lock() {
                    let _ = view.reload();
                }
                let mut sessions = watcher_sessions.lock().unwrap();
                sessions.retain(|session| {
                    let Some(session) = session.upgrade() else {
                        return false;
                    };
                    if let Ok(mut view) = session.lock() {
                        let overlays = view.overlays().clone();
                        let versions = view.document_versions().clone();
                        let _ = view.replace_inputs(overlays, versions);
                    }
                    true
                });
            },
        )
        .map_err(io::Error::other)?;
        watcher
            .watcher()
            .watch(root, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;

        let canonical_root = dunce::canonicalize(root)?;
        let identity = VaultIdentity {
            fingerprint: format!(
                "{:016x}",
                fingerprint(canonical_root.to_string_lossy().as_bytes())
            ),
            canonical_root: canonical_root.clone(),
        };
        let host = Arc::new(VaultHost {
            identity,
            engine,
            disk,
            sessions,
            _watcher: Mutex::new(watcher),
            write_lock: Mutex::new(()),
        });
        let mut vaults = self.vaults.lock().unwrap();
        Ok(vaults
            .entry(canonical_root)
            .or_insert_with(|| host.clone())
            .clone())
    }

    fn view(
        &self,
        view_id: ServiceViewId,
    ) -> io::Result<(Arc<VaultHost>, Arc<Mutex<AnalyzerView>>, ViewKind)> {
        self.views
            .lock()
            .unwrap()
            .get(&view_id)
            .map(|entry| (entry.host.clone(), entry.view.clone(), entry.kind))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown view handle"))
    }

    fn snapshot_identity(
        &self,
        view_id: ServiceViewId,
        host: &VaultHost,
        snapshot: &WorkspaceSnapshot,
    ) -> SnapshotIdentity {
        let view_kind = self
            .view_kind(view_id)
            .map(|kind| match kind {
                ViewKind::Disk => "disk",
                ViewKind::Session => "session",
            })
            .unwrap_or("unknown")
            .to_owned();
        SnapshotIdentity {
            daemon_instance: self.instance_id.clone(),
            vault: host.identity.clone(),
            view_id,
            view_kind,
            analyzer_view_id: snapshot.view_id().raw(),
            revision: snapshot.revision().raw(),
            source_fingerprint: snapshot_fingerprint(snapshot),
        }
    }
}

impl Default for NotistService {
    fn default() -> Self {
        Self::new()
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn snapshot_fingerprint(snapshot: &WorkspaceSnapshot) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for source in snapshot.sources() {
        for bytes in [
            source.canonical_path.to_string_lossy().as_bytes(),
            source.text.as_bytes(),
        ] {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn shared_disk_and_isolated_session_views_use_one_vault_host() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "disk").unwrap();
        let service = NotistService::new();
        let (disk, vault) = service.open_view(root.path(), ViewKind::Disk).unwrap();
        let (session, same_vault) = service.open_view(root.path(), ViewKind::Session).unwrap();
        assert_eq!(vault, same_vault);

        let canonical = dunce::canonicalize(path).unwrap();
        let mut overlays = SourceOverlays::new();
        overlays.insert(canonical.clone(), Arc::from("overlay"));
        service
            .replace_view_inputs(session, overlays, DocumentVersions::new(), None)
            .unwrap();

        let (_, disk_text) = service
            .with_snapshot(disk, |snapshot| {
                snapshot
                    .source(snapshot.file_id(&canonical).unwrap())
                    .unwrap()
                    .text
                    .to_string()
            })
            .unwrap();
        let (_, session_text) = service
            .with_snapshot(session, |snapshot| {
                snapshot
                    .source(snapshot.file_id(&canonical).unwrap())
                    .unwrap()
                    .text
                    .to_string()
            })
            .unwrap();
        assert_eq!(disk_text, "disk");
        assert_eq!(session_text, "overlay");
    }

    #[test]
    fn snapshot_identity_separates_views_and_revisions() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "one").unwrap();
        let service = NotistService::new();
        let (first, _) = service.open_view(root.path(), ViewKind::Disk).unwrap();
        let (second, _) = service.open_view(root.path(), ViewKind::Session).unwrap();
        let (first_identity, ()) = service.with_snapshot(first, |_| ()).unwrap();
        let (second_identity, ()) = service.with_snapshot(second, |_| ()).unwrap();

        assert_eq!(
            first_identity.daemon_instance,
            second_identity.daemon_instance
        );
        assert_eq!(first_identity.vault, second_identity.vault);
        assert_ne!(first_identity.view_id, second_identity.view_id);
        assert_ne!(
            first_identity.analyzer_view_id,
            second_identity.analyzer_view_id
        );
    }

    #[test]
    fn root_bound_service_rejects_another_vault() {
        let first = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let second = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let service = NotistService::for_root(first.path()).unwrap();
        service.open_view(first.path(), ViewKind::Disk).unwrap();
        let error = service
            .open_view(second.path(), ViewKind::Disk)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
}
