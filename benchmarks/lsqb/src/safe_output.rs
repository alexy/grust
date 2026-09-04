//! Creation-only report output helpers.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

/// Write a new report without following or replacing an existing output path.
pub fn write_new(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match fs::symlink_metadata(parent) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "output parent is not a regular non-symlink directory: {}",
                    parent.display()
                ));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            let metadata = fs::symlink_metadata(parent)
                .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "created output parent is not a regular non-symlink directory: {}",
                    parent.display()
                ));
            }
        }
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", parent.display()));
        }
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create new output {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect new output {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "new output is not a regular file: {}",
            path.display()
        ));
    }
    file.write_all(content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_once_and_refuses_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        write_new(&path, b"first\n").unwrap();
        assert!(write_new(&path, b"second\n").is_err());
        assert_eq!(fs::read(path).unwrap(), b"first\n");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_output_and_parent() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::write(&target, b"target\n").unwrap();
        let output = directory.path().join("output");
        symlink(&target, &output).unwrap();
        assert!(write_new(&output, b"replacement\n").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"target\n");

        let broken = directory.path().join("broken");
        symlink(directory.path().join("missing"), &broken).unwrap();
        assert!(write_new(&broken, b"replacement\n").is_err());
        assert!(broken.is_symlink());

        let real_parent = directory.path().join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let linked_parent = directory.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(write_new(&linked_parent.join("report"), b"report\n").is_err());
        assert!(!real_parent.join("report").exists());
    }
}
