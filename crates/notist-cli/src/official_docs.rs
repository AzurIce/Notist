use std::collections::HashSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::resources::{OFFICIAL_DOCS_ARCHIVE, OFFICIAL_DOCS_FINGERPRINT};

const ARCHIVE_MAGIC: &[u8] = b"NOTISTDOCS\0";
const MANIFEST_NAME: &str = ".notist-docs.json";
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 10_000;

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
struct OfficialDocsManifest {
    bundle_schema: u32,
    notist_version: String,
    protocol_version: String,
    docs_fingerprint: String,
    source_revision: Option<String>,
}

pub(crate) fn ensure_synced() -> io::Result<PathBuf> {
    let data_root = notist_data_root()?;
    ensure_synced_to(&data_root)
}

pub(crate) fn docs_root() -> io::Result<PathBuf> {
    Ok(notist_data_root()?.join("docs"))
}

pub(crate) fn generation_for_root(root: &Path) -> io::Result<Option<String>> {
    let docs_root = docs_root()?;
    let root = dunce::canonicalize(root)?;
    let docs_root = match dunce::canonicalize(docs_root) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if root != docs_root {
        return Ok(None);
    }
    Ok(read_manifest(&docs_root)?.map(|manifest| manifest.docs_fingerprint))
}

fn ensure_synced_to(data_root: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(data_root)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(data_root.join(".docs-sync.lock"))?;
    FileExt::lock_exclusive(&lock)?;

    let target = data_root.join("docs");
    let previous = data_root.join("docs.previous");
    if !target.exists() && previous.exists() {
        fs::rename(&previous, &target)?;
    }
    let expected = expected_manifest();
    if read_manifest(&target)?.as_ref() == Some(&expected) {
        return Ok(target);
    }

    let staging = tempfile::Builder::new()
        .prefix(".docs-staging-")
        .tempdir_in(data_root)?;
    unpack_docs(staging.path())?;
    let manifest = serde_json::to_vec_pretty(&expected).map_err(io::Error::other)?;
    write_new(&staging.path().join(MANIFEST_NAME), &manifest)?;
    let staging_path = staging.keep();

    if previous.exists() {
        fs::remove_dir_all(&previous)?;
    }
    if target.exists() {
        fs::rename(&target, &previous)?;
    }
    if let Err(error) = fs::rename(&staging_path, &target) {
        if !target.exists() && previous.exists() {
            let _ = fs::rename(&previous, &target);
        }
        let _ = fs::remove_dir_all(&staging_path);
        return Err(error);
    }
    if previous.exists() {
        fs::remove_dir_all(previous)?;
    }
    Ok(target)
}

fn expected_manifest() -> OfficialDocsManifest {
    OfficialDocsManifest {
        bundle_schema: 1,
        notist_version: env!("CARGO_PKG_VERSION").into(),
        protocol_version: format!(
            "{}.{}",
            notist_service::protocol::PROTOCOL_MAJOR,
            notist_service::protocol::PROTOCOL_MINOR
        ),
        docs_fingerprint: OFFICIAL_DOCS_FINGERPRINT.into(),
        source_revision: option_env!("NOTIST_SOURCE_REVISION").map(str::to_owned),
    }
}

fn read_manifest(root: &Path) -> io::Result<Option<OfficialDocsManifest>> {
    match fs::read(root.join(MANIFEST_NAME)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn unpack_docs(root: &Path) -> io::Result<()> {
    let mut decoder = GzDecoder::new(OFFICIAL_DOCS_ARCHIVE);
    let mut archive = Vec::new();
    decoder
        .by_ref()
        .take((MAX_ARCHIVE_BYTES + 1) as u64)
        .read_to_end(&mut archive)?;
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded docs archive exceeds the extraction limit",
        ));
    }

    let mut cursor = ArchiveCursor::new(&archive);
    if cursor.take(ARCHIVE_MAGIC.len())? != ARCHIVE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid embedded docs archive",
        ));
    }
    let count = cursor.u32()? as usize;
    if count > MAX_FILE_COUNT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded docs archive contains too many files",
        ));
    }
    let mut paths = HashSet::with_capacity(count);
    for _ in 0..count {
        let path_len = cursor.u32()? as usize;
        let content_len = usize::try_from(cursor.u64()?).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "embedded docs file is too large",
            )
        })?;
        let expected_hash = cursor.take(32)?;
        let path = std::str::from_utf8(cursor.take(path_len)?).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "embedded docs path is not UTF-8",
            )
        })?;
        if !paths.insert(path.to_owned()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "embedded docs archive contains a duplicate path",
            ));
        }
        let relative = safe_relative_path(path)?;
        let content = cursor.take(content_len)?;
        if Sha256::digest(content).as_slice() != expected_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "embedded docs file fingerprint does not match its content",
            ));
        }
        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        write_new(&destination, content)?;
    }
    if !cursor.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded docs archive has trailing data",
        ));
    }
    Ok(())
}

fn safe_relative_path(path: &str) -> io::Result<PathBuf> {
    if path.is_empty() || path.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded docs path is not a normalized relative path",
        ));
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded docs path escapes the docs root",
        ));
    }
    Ok(path.to_path_buf())
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn notist_data_root() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("NOTIST_DATA_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    {
        return env::var_os("LOCALAPPDATA")
            .map(|path| PathBuf::from(path).join("Notist"))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"));
    }
    #[cfg(target_os = "macos")]
    {
        return home_dir().map(|path| path.join("Library/Application Support/Notist"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path).join("notist"));
        }
        return home_dir().map(|path| path.join(".local/share/notist"));
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cannot determine the Notist user-data directory on this platform",
    ))
}

#[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"))))]
fn home_dir() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

struct ArchiveCursor<'a> {
    remaining: &'a [u8],
}

impl<'a> ArchiveCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if length > self.remaining.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated embedded docs archive",
            ));
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronizes_embedded_docs_and_repairs_manifest_mismatch() {
        let data = tempfile::tempdir().unwrap();
        let root = ensure_synced_to(data.path()).unwrap();
        assert!(root.join("Notist.toml").is_file());
        assert!(
            root.join("designs/D0013-self-contained-agent-skill.not")
                .is_file()
        );
        assert_eq!(read_manifest(&root).unwrap(), Some(expected_manifest()));

        fs::write(root.join(MANIFEST_NAME), b"{}").unwrap();
        fs::write(root.join("unmanaged.not"), b"stale").unwrap();
        let repaired = ensure_synced_to(data.path()).unwrap();
        assert!(!repaired.join("unmanaged.not").exists());
        assert_eq!(read_manifest(&repaired).unwrap(), Some(expected_manifest()));
    }

    #[test]
    fn rejects_unsafe_archive_paths() {
        assert!(safe_relative_path("../escape.not").is_err());
        assert!(safe_relative_path("/absolute.not").is_err());
        assert!(safe_relative_path("nested\\windows.not").is_err());
        assert_eq!(
            safe_relative_path("designs/D0013.not").unwrap(),
            PathBuf::from("designs/D0013.not")
        );
    }
}
