//! Files in the app's data folder.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bridge_protocol::data_dir::{create_private_dir, Moved};

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

    fn open_record(&self, name: &str) -> Box<dyn VaultStore> {
        Box::new(FileStore::without_backup(self.dir.join(name)))
    }
}

/// This app's data folder: `<prefix>_DATA_DIR` when set, else its own folder
/// under the platform's app data folder (see [`AppDirs`](super::AppDirs) and
/// [`bridge_protocol::data_dir`]). `None` when the environment names no such
/// folder. There is no fallback to the working directory: the vault and the
/// record of used leaves would then be wherever the app happened to be
/// started.
pub fn data_dir() -> Option<PathBuf> {
    let dirs = app_dirs();
    if let Some(dir) = std::env::var_os(dirs.env_var("DATA_DIR")) {
        return Some(PathBuf::from(dir));
    }
    if let Some(base) = super::dirs::base_dir() {
        return Some(base.join(dirs.folder));
    }
    platform_data_dir()
}

/// The platform's folder, found once. The first call moves the data there
/// from where builds before this one kept it on Unix. Logging reads this
/// folder first thing in `main`, so the move comes before any file in it is
/// opened.
fn platform_data_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let (dir, moved) = bridge_protocol::data_dir::resolve(app_dirs().folder);
        *MOVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = moved;
        dir
    })
    .clone()
}

/// What finding the folder moved, until logging is up to say so.
static MOVED: Mutex<Option<Moved>> = Mutex::new(None);

/// Log what finding the data folder moved, once. Logging calls it as soon
/// as it is installed.
pub(crate) fn log_data_dir_move() {
    let moved = MOVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    match moved {
        Some(Moved::Moved { from, to }) => tracing::info!(
            target: "airlock::core",
            from = %from.display(),
            to = %to.display(),
            "moved the data folder to where this platform keeps app data"
        ),
        Some(Moved::Failed { from, to, error }) => tracing::warn!(
            target: "airlock::core",
            from = %from.display(),
            to = %to.display(),
            error = %error,
            "could not move the data folder, using it where it is"
        ),
        None => {}
    }
}

pub fn logs_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("logs"))
}

/// The file in the data folder that a running app holds locked.
const LOCK_FILE: &str = "airlock.lock";

/// The lock on this app's data folder, held until the process ends.
static HELD: Mutex<Option<File>> = Mutex::new(None);

/// Why this process may not use its data folder.
#[derive(Debug)]
pub enum DataDirClaim {
    /// The environment names no folder for the app's data.
    NoFolder(String),
    /// Another process of this app has it open.
    InUse(PathBuf),
    /// The lock could not be taken at all.
    Failed(PathBuf, std::io::Error),
}

impl std::fmt::Display for DataDirClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFolder(variable) => write!(
                f,
                "There is no folder for the app's data: neither {variable} nor {} is set. Set one and start the app again.",
                if cfg!(windows) { "LOCALAPPDATA" } else { "HOME" }
            ),
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
    let dir = data_dir().ok_or_else(|| DataDirClaim::NoFolder(app_dirs().env_var("DATA_DIR")))?;
    claim_dir(&dir)
}

fn claim_dir(dir: &Path) -> Result<(), DataDirClaim> {
    let mut held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.is_some() {
        return Ok(());
    }
    let failed = |err| DataDirClaim::Failed(dir.to_path_buf(), err);
    let file = open_lock(dir).map_err(failed)?;
    match file.try_lock() {
        Ok(()) => {
            *held = Some(file);
            Ok(())
        }
        Err(TryLockError::WouldBlock) => Err(DataDirClaim::InUse(dir.to_path_buf())),
        Err(TryLockError::Error(err)) => Err(failed(err)),
    }
}

/// Make the data folder and open its lock file. Only this user may read the
/// folder: it holds the vault, the RPC URL with its key, the sites' grants
/// and the logs.
fn open_lock(dir: &Path) -> std::io::Result<File> {
    create_private_dir(dir)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(dir.join(LOCK_FILE))
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

    #[cfg(unix)]
    #[test]
    fn a_claimed_folder_is_only_this_users() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("airlock");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        // `claim_dir` holds one folder per process, so this asks what it
        // does before taking the lock.
        open_lock(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(LOCK_FILE)), 0o600);
        let fresh = base.path().join("fresh");
        open_lock(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o700);
    }
}
