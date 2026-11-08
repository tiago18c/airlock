//! Which app's folder and environment variables this process uses. Airlock
//! and Airlock Offline keep separate data, so both can run on one machine,
//! each with its own vault.

use std::sync::OnceLock;

/// The data folder's name and the prefix of the environment variables that
/// override it (`<prefix>_DATA_DIR`) and the log filter (`<prefix>_LOG`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppDirs {
    /// The folder under the platform's app data folder, and the log files'
    /// name.
    pub folder: &'static str,
    pub env_prefix: &'static str,
}

impl AppDirs {
    pub const ONLINE: Self = Self {
        folder: "airlock",
        env_prefix: "AIRLOCK",
    };
    pub const OFFLINE: Self = Self {
        folder: "airlock-offline",
        env_prefix: "AIRLOCK_OFFLINE",
    };

    /// `<prefix>_<name>`, such as `AIRLOCK_OFFLINE_LOG`.
    pub fn env_var(&self, name: &str) -> String {
        format!("{}_{name}", self.env_prefix)
    }
}

static DIRS: OnceLock<AppDirs> = OnceLock::new();

/// The folder the app's own folder goes under, when the platform names one
/// that is not in the environment: Android's app-private files folder.
static BASE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Put the app's folder under `dir` instead of the platform's app data
/// folder. A mobile shell calls it first thing in `main`, before logging or
/// any store opens. `<prefix>_DATA_DIR` still wins, so tests stay apart.
pub fn set_base_dir(dir: std::path::PathBuf) {
    let chosen = BASE.get_or_init(|| dir.clone());
    assert_eq!(
        *chosen, dir,
        "set_base_dir is called before anything reads the data folder"
    );
}

/// The folder [`set_base_dir`] named, if any. The web keeps no files.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn base_dir() -> Option<&'static std::path::Path> {
    BASE.get().map(|dir| dir.as_path())
}

/// Pick this process's folder and variables. A shell other than the online
/// one calls it first thing in `main`, before logging or any store opens;
/// without it the process uses [`AppDirs::ONLINE`].
pub fn set_app_dirs(dirs: AppDirs) {
    let chosen = *DIRS.get_or_init(|| dirs);
    assert_eq!(
        chosen, dirs,
        "set_app_dirs is called before anything reads the data folder"
    );
}

/// The folder and variables in use. The first call fixes them for the
/// process.
pub fn app_dirs() -> AppDirs {
    *DIRS.get_or_init(|| AppDirs::ONLINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_app_has_its_own_variables() {
        assert_eq!(AppDirs::ONLINE.env_var("DATA_DIR"), "AIRLOCK_DATA_DIR");
        assert_eq!(AppDirs::OFFLINE.env_var("LOG"), "AIRLOCK_OFFLINE_LOG");
        assert_ne!(AppDirs::ONLINE.folder, AppDirs::OFFLINE.folder);
    }
}
