//! A file on disk, replaced whole on every save.

use super::VaultStore;
use crate::VaultError;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl VaultStore for FileStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        match read_optional(&self.path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => read_optional(&sibling(&self.path, ".bak")),
        }
    }

    /// Write to a sibling temp file, fsync it, move the current file aside as
    /// `.bak`, then rename the temp file into place. A crash mid-write leaves
    /// the previous file intact. A crash after the previous file was moved
    /// aside is recovered by [`load`](Self::load) reading `.bak`.
    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
        }
        let tmp = sibling(&self.path, ".tmp");
        let bak = sibling(&self.path, ".bak");
        write_synced(&tmp, bytes)?;
        if self.path.exists() {
            if bak.exists() {
                std::fs::remove_file(&bak).map_err(io)?;
            }
            std::fs::rename(&self.path, &bak).map_err(io)?;
        }
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            if !self.path.exists() && bak.exists() {
                let _ = std::fs::rename(&bak, &self.path);
            }
            return Err(io(err));
        }
        sync_parent(&self.path)?;
        Ok(())
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        write_synced(&sibling(&self.path, ".unreadable"), bytes)
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store");
    path.with_file_name(format!("{name}{suffix}"))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), VaultError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    let dir = std::fs::File::open(parent).map_err(io)?;
    dir.sync_all().map_err(io)?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), VaultError> {
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, VaultError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io(err)),
    }
}

fn io(err: std::io::Error) -> VaultError {
    VaultError::Io(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{check_saveable, load_json, LoadProblem};

    #[test]
    fn save_keeps_the_previous_file_as_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.json");
        let store = FileStore::new(&path);

        store.save(b"first").unwrap();
        store.save(b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::read(sibling(&path, ".bak")).unwrap(), b"first");
        assert!(!sibling(&path, ".tmp").exists());
        assert_eq!(store.load().unwrap().unwrap(), b"second");
    }

    #[test]
    fn unreadable_json_is_set_aside_and_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = FileStore::new(&path);

        let (data, problem): (Vec<u8>, _) = load_json(&store, "address book");
        assert!(data.is_empty());
        assert!(matches!(problem, Some(LoadProblem::SetAside(_))));
        assert!(check_saveable(&problem).is_ok());
        assert_eq!(
            std::fs::read(sibling(&path, ".unreadable")).unwrap(),
            b"{ not json"
        );
    }

    #[test]
    fn a_store_that_cannot_keep_a_copy_blocks_saving() {
        struct ReadOnly;
        impl VaultStore for ReadOnly {
            fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
                Ok(Some(b"{ not json".to_vec()))
            }
            fn save(&self, _bytes: &[u8]) -> Result<(), VaultError> {
                Ok(())
            }
        }
        let (_, problem): (Vec<u8>, _) = load_json(&ReadOnly, "address book");
        assert!(matches!(problem, Some(LoadProblem::Blocked(_))));
        assert!(check_saveable(&problem).is_err());
    }

    #[test]
    fn load_uses_backup_when_the_current_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.json");
        let store = FileStore::new(&path);

        store.save(b"first").unwrap();
        store.save(b"second").unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(store.load().unwrap().unwrap(), b"first");
    }
}
