use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::resources::{NOTIST_SKILL_FINGERPRINT, NOTIST_SKILL_MD};

const SKILL_DIR_NAME: &str = "notist";
const SKILL_FILE_NAME: &str = "SKILL.md";

pub(crate) fn init(skills_root: PathBuf, force: bool) -> io::Result<PathBuf> {
    let root = match dunce::canonicalize(&skills_root) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&skills_root)?;
            dunce::canonicalize(&skills_root)?
        }
        Err(error) => return Err(error),
    };
    let output = root.join(SKILL_DIR_NAME);
    if output.exists() {
        if !force {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "skill directory already exists: {} (pass --force to replace SKILL.md)",
                    output.display()
                ),
            ));
        }
        if !output.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("skill target must be a directory: {}", output.display()),
            ));
        }
        notist_service::write_artifact_atomic(
            &output.join(SKILL_FILE_NAME),
            NOTIST_SKILL_MD,
            &format!("skill-init-{}", std::process::id()),
        )?;
        return Ok(output);
    }
    let staging = tempfile::Builder::new()
        .prefix(".notist-skill-staging-")
        .tempdir_in(&root)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staging.path().join(SKILL_FILE_NAME))?;
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

/// Stateless startup probe (2026-08-30 ruling): only the two conventional
/// skills roots are examined, installs elsewhere are invisible by design.
pub(crate) fn startup_notices() -> Vec<String> {
    let mut roots = Vec::new();
    if let Some(root) = home_skills_root() {
        roots.push(root);
    }
    roots.push(PathBuf::from(".agents").join("skills"));
    notices_for(&roots)
}

fn notices_for(roots: &[PathBuf]) -> Vec<String> {
    let mut notices = Vec::new();
    let mut installed = false;
    for root in roots {
        let skill_path = root.join(SKILL_DIR_NAME).join(SKILL_FILE_NAME);
        let content = match fs::read(&skill_path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        installed = true;
        if embedded_fingerprint_for(&content) != NOTIST_SKILL_FINGERPRINT {
            notices.push(format!(
                "note: Notist Skill at {} does not match this CLI build; run `notist skill init {} --force` to update it",
                skill_path.display(),
                root.display()
            ));
        }
    }
    if !installed {
        let checked = roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        notices.push(format!(
            "note: Notist Skill is not installed (checked {checked}); run `notist skill init {}` to install it",
            roots[0].display()
        ));
    }
    notices
}

fn embedded_fingerprint_for(content: &[u8]) -> String {
    // Must mirror build.rs `fingerprint` framing for the single authored entry.
    let mut digest = Sha256::new();
    digest.update((SKILL_FILE_NAME.len() as u64).to_le_bytes());
    digest.update(SKILL_FILE_NAME.as_bytes());
    digest.update((content.len() as u64).to_le_bytes());
    digest.update(content);
    format!("{:x}", digest.finalize())
}

fn home_skills_root() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = env::var_os("HOME");
    let home = home.filter(|value| !value.is_empty())?;
    Some(PathBuf::from(home).join(".agents").join("skills"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn install_skill_at(root: &Path, bytes: &[u8]) -> PathBuf {
        let dir = root.join(SKILL_DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SKILL_FILE_NAME);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn embedded_fingerprint_matches_runtime_framing() {
        assert_eq!(
            embedded_fingerprint_for(NOTIST_SKILL_MD),
            NOTIST_SKILL_FINGERPRINT
        );
    }

    #[test]
    fn initializes_skill_directory_under_created_root_without_overwrite() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("skills");
        let output = root.join(SKILL_DIR_NAME);
        assert_eq!(init(root.clone(), false).unwrap(), output);
        assert!(root.is_dir());
        let entries = fs::read_dir(&output)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name(), SKILL_FILE_NAME);
        assert_eq!(
            fs::read(output.join(SKILL_FILE_NAME)).unwrap(),
            NOTIST_SKILL_MD
        );
        assert_eq!(
            init(root, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn force_replaces_only_skill_file_in_existing_directory() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        let output = root.join(SKILL_DIR_NAME);
        fs::create_dir(&output).unwrap();
        fs::write(output.join(SKILL_FILE_NAME), b"stale skill").unwrap();
        fs::write(output.join("keep.txt"), b"preserve me").unwrap();

        assert_eq!(init(root.to_path_buf(), true).unwrap(), output);
        assert_eq!(
            fs::read(output.join(SKILL_FILE_NAME)).unwrap(),
            NOTIST_SKILL_MD
        );
        assert_eq!(fs::read(output.join("keep.txt")).unwrap(), b"preserve me");
    }

    #[test]
    fn force_rejects_existing_file_target() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path();
        fs::write(root.join(SKILL_DIR_NAME), b"not a directory").unwrap();

        assert_eq!(
            init(root.to_path_buf(), true).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn startup_notices_distinguish_missing_stale_and_current_installs() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let roots = vec![home.path().to_path_buf(), project.path().to_path_buf()];

        let missing = notices_for(&roots);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("not installed"), "{}", missing[0]);
        assert!(missing[0].contains(&home.path().display().to_string()));
        assert!(missing[0].contains(&project.path().display().to_string()));

        install_skill_at(home.path(), NOTIST_SKILL_MD);
        install_skill_at(project.path(), NOTIST_SKILL_MD);
        assert!(notices_for(&roots).is_empty());

        install_skill_at(home.path(), b"stale skill");
        let stale = notices_for(&roots);
        assert_eq!(stale.len(), 1);
        assert!(stale[0].contains("--force"), "{}", stale[0]);
        assert!(stale[0].contains(&home.path().display().to_string()));
        assert!(!stale[0].contains(&project.path().display().to_string()));
    }
}
