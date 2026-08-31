use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::GzBuilder;
use sha2::{Digest, Sha256};

const ARCHIVE_MAGIC: &[u8] = b"NOTISTDOCS\0";

fn main() {
    if let Err(error) = build_resources() {
        panic!("failed to build embedded Notist resources: {error}");
    }
}

fn build_resources() -> io::Result<()> {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repository = manifest_dir.join("../..");
    let docs_root = repository.join("docs");
    let skill_root = repository.join("skills/notist");
    println!("cargo:rerun-if-changed={}", docs_root.display());
    println!("cargo:rerun-if-changed={}", skill_root.display());

    let docs = collect_files(&docs_root)?;
    let docs_fingerprint = fingerprint(&docs);
    let archive = encode_archive(&docs)?;
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let archive_path = output.join("official-docs.bundle.gz");
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(&archive_path)?, Compression::best());
    finish_gzip(encoder, &archive)?;

    let skill_files = collect_files(&skill_root)?;
    if skill_files.len() != 1 || skill_files[0].0 != "SKILL.md" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "skills/notist must contain exactly SKILL.md",
        ));
    }
    fs::write(output.join("notist-skill.md"), &skill_files[0].1)?;
    println!(
        "cargo:rustc-env=NOTIST_SKILL_FINGERPRINT={}",
        fingerprint(&skill_files)
    );

    println!("cargo:rustc-env=NOTIST_DOCS_FINGERPRINT={docs_fingerprint}");
    Ok(())
}

fn finish_gzip(mut encoder: flate2::write::GzEncoder<fs::File>, bytes: &[u8]) -> io::Result<()> {
    encoder.write_all(bytes)?;
    encoder.finish()?;
    Ok(())
}

fn collect_files(root: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    collect_files_from(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn collect_files_from(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "embedded resources cannot contain symlinks: {}",
                    entry.path().display()
                ),
            ));
        }
        if metadata.is_dir() {
            collect_files_from(root, &entry.path(), files)?;
        } else if metadata.is_file() {
            let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
            let path = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((path, fs::read(entry.path())?));
        }
    }
    Ok(())
}

fn fingerprint(files: &[(String, Vec<u8>)]) -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

fn encode_archive(files: &[(String, Vec<u8>)]) -> io::Result<Vec<u8>> {
    let mut archive = Vec::new();
    archive.extend_from_slice(ARCHIVE_MAGIC);
    archive.extend_from_slice(
        &u32::try_from(files.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many docs files"))?
            .to_le_bytes(),
    );
    for (path, bytes) in files {
        let content_hash = Sha256::digest(bytes);
        archive.extend_from_slice(
            &u32::try_from(path.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "docs path too long"))?
                .to_le_bytes(),
        );
        archive.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        archive.extend_from_slice(&content_hash);
        archive.extend_from_slice(path.as_bytes());
        archive.extend_from_slice(bytes);
    }
    Ok(archive)
}
