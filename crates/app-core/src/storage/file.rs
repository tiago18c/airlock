//! Files in the app's data folder.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use wallet_core::{FileStore, VaultStore};

use super::{app_dirs, Storage};

pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Storage for FileStorage {
    fn open(&self, name: &str) -> Box<dyn VaultStore> {
        Box::new(FileStore::new(self.dir.join(name)))
    }
}

/// This app's data folder: `<prefix>_DATA_DIR` when set, else its own folder
/// under the platform's app data folder (see [`AppDirs`](super::AppDirs)).
pub fn data_dir() -> PathBuf {
    let dirs = app_dirs();
    if let Some(dir) = std::env::var_os(dirs.env_var("DATA_DIR")) {
        return PathBuf::from(dir);
    }
    let mut dir = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.push(dirs.folder);
    dir
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The file in the data folder that a running app holds locked.
const LOCK_FILE: &str = "airlock.lock";

/// The lock on this app's data folder, held until the process ends.
static HELD: Mutex<Option<File>> = Mutex::new(None);

/// Why this process may not use its data folder.
#[derive(Debug)]
pub enum DataDirClaim {
    /// Another process of this app has it open.
    InUse(PathBuf),
    /// The lock could not be taken at all.
    Failed(PathBuf, std::io::Error),
}

impl std::fmt::Display for DataDirClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse(dir) => write!(
                f,
                "Another window of this app is already using its data folder ({}). Only one may run at a time: two would each keep their own copy of the vault and of the record of used signing leaves, and could sign with one leaf twice. Close the other one first.",
                dir.display()
            ),
            Self::Failed(dir, err) => write!(
                f,
                "The data folder ({}) could not be locked ({err}), so the app does not open it: another copy of the app could be using it.",
                dir.display()
            ),
        }
    }
}

impl std::error::Error for DataDirClaim {}

/// Take this app's data folder for this process. Each store keeps its file
/// in memory and writes it whole, so two processes on one folder would undo
/// each other's saves, and a mark that only rises could go down. Call it
/// before any store opens; [`platform_storage`](super::platform_storage)
/// refuses to open one without it. Taking it again in the same process does
/// nothing.
pub fn claim_data_dir() -> Result<(), DataDirClaim> {
    claim_dir(&data_dir())
}

fn claim_dir(dir: &Path) -> Result<(), DataDirClaim> {
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.is_some() {
        return Ok(());
    }
    let failed = |err| DataDirClaim::Failed(dir.to_path_buf(), err);
    std::fs::create_dir_all(dir).map_err(failed)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(failed)?;
    match file.try_lock() {
        Ok(()) => {
            *held = Some(file);
            Ok(())
        }
        Err(TryLockError::WouldBlock) => Err(DataDirClaim::InUse(dir.to_path_buf())),
        Err(TryLockError::Error(err)) => Err(failed(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_another_process_holds_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        // What another process holds: a lock on its own handle.
        let other = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.path().join(LOCK_FILE))
            .unwrap();
        other.lock().unwrap();
        assert!(matches!(claim_dir(dir.path()), Err(DataDirClaim::InUse(_))));
        other.unlock().unwrap();
        claim_dir(dir.path()).unwrap();
        // Held now, so taking it again is a no-op.
        claim_dir(dir.path()).unwrap();
    }
}
