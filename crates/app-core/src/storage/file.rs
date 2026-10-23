//! Files in the app's data folder.

use std::path::{Path, PathBuf};

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
