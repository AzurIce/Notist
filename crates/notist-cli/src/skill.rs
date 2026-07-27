use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::resources::NOTIST_SKILL_MD;

pub(crate) fn init(output: PathBuf, force: bool) -> io::Result<PathBuf> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = dunce::canonicalize(parent)?;
    let name = output.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill output must name a directory",
        )
    })?;
    let output = parent.join(name);
    if output.exists() {
        if !force {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("output already exists: {}", output.display()),
            ));
        }
        if !output.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("skill output must be a directory: {}", output.display()),
            ));
        }
        notist_service::write_artifact_atomic(
            &output.join("SKILL.md"),
            NOTIST_SKILL_MD,
            &format!("skill-init-{}", std::process::id()),
        )?;
        return Ok(output);
    }
    let staging = tempfile::Builder::new()
        .prefix(".notist-skill-staging-")
        .tempdir_in(&parent)?;
    let path = staging.path().join("SKILL.md");
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(NOTIST_SKILL_MD)?;
    file.sync_all()?;
    drop(file);
    let staging_path = staging.keep();
    if let Err(error) = fs::rename(&staging_path, &output) {
        let _ = fs::remove_dir_all(staging_path);
        return Err(error);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_exactly_one_skill_file_without_overwrite() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("notist");
        assert_eq!(init(output.clone(), false).unwrap(), output);
        let entries = fs::read_dir(&output)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), "SKILL.md");
        assert_eq!(fs::read(output.join("SKILL.md")).unwrap(), NOTIST_SKILL_MD);
        assert_eq!(
            init(output, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn force_replaces_only_skill_file_in_existing_directory() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("notist");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("SKILL.md"), b"stale skill").unwrap();
        fs::write(output.join("keep.txt"), b"preserve me").unwrap();

        assert_eq!(init(output.clone(), true).unwrap(), output);
        assert_eq!(fs::read(output.join("SKILL.md")).unwrap(), NOTIST_SKILL_MD);
        assert_eq!(fs::read(output.join("keep.txt")).unwrap(), b"preserve me");
    }

    #[test]
    fn force_rejects_existing_file() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("notist");
        fs::write(&output, b"not a directory").unwrap();

        assert_eq!(
            init(output, true).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
