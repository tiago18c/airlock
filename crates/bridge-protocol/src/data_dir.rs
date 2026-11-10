//! Where the desktop app keeps its data. `bridge-host` logs into the same
//! folder, so both find it here and never disagree.
//!
//! Windows: `%LOCALAPPDATA%\<folder>`. macOS: `~/Library/Application
//! Support/<folder>`. Linux and other Unix: `$XDG_DATA_HOME/<folder>` when
//! that is set and absolute, else `~/.local/share/<folder>`. Builds before
//! these kept it at `~/<folder>` on Unix, and [`resolve`] moves it from there.

#[cfg(any(windows, unix, test))]
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Files only the app's data folder holds. A folder of the same name that
/// holds none of them is someone else's, and is left where it is.
#[cfg(any(unix, test))]
const DATA_FILES: &[&str] = &["vault.json", "workspace.json", "airlock.lock"];

/// What [`resolve`] did to find the folder, for the log once there is one.
#[derive(Debug)]
pub enum Moved {
    /// The folder was where an earlier build kept it, and is now here.
    Moved { from: PathBuf, to: PathBuf },
    /// It could not be moved, so the old one is used for now.
    Failed {
        from: PathBuf,
        to: PathBuf,
        error: io::Error,
    },
}

/// The platform's folder for `folder`, without moving anything into it.
/// `None` when the environment names no base folder.
pub fn platform_dir(folder: &str) -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name);
    #[cfg(windows)]
    {
        windows_dir(&env, folder)
    }
    #[cfg(target_os = "macos")]
    {
        macos_dir(&env, folder)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_dir(&env, folder)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (env, folder);
        None
    }
}

/// [`platform_dir`], after moving the folder there from where builds
/// before it kept it, if it is there. Call it before anything opens a file
/// in either folder, and log what it did once logging is up.
pub fn resolve(folder: &str) -> (Option<PathBuf>, Option<Moved>) {
    let Some(new) = platform_dir(folder) else {
        return (None, None);
    };
    #[cfg(unix)]
    if let Some(old) = old_dir(&|name: &str| std::env::var_os(name), folder) {
        let (dir, moved) = adopt(&old, &new);
        return (Some(dir), moved);
    }
    (Some(new), None)
}

/// Make `dir` for files only this user may read: on Unix it is made `0700`,
/// and tightened to that if it is wider and this user owns it. Its parent is
/// made as usual, since it is not the app's.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => {}
            Err(err) => return Err(err),
        }
        let mode = std::fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            match std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
                Ok(()) => {}
                // Someone else's folder: not ours to change.
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(any(windows, test))]
fn windows_dir(env: &dyn Fn(&str) -> Option<OsString>, folder: &str) -> Option<PathBuf> {
    absolute(env("LOCALAPPDATA")?).map(|dir| dir.join(folder))
}

#[cfg(any(target_os = "macos", test))]
fn macos_dir(env: &dyn Fn(&str) -> Option<OsString>, folder: &str) -> Option<PathBuf> {
    Some(
        absolute(env("HOME")?)?
            .join("Library")
            .join("Application Support")
            .join(folder),
    )
}

#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn xdg_dir(env: &dyn Fn(&str) -> Option<OsString>, folder: &str) -> Option<PathBuf> {
    // The XDG spec says a relative path in it is to be ignored.
    if let Some(data) = env("XDG_DATA_HOME").and_then(absolute) {
        return Some(data.join(folder));
    }
    Some(
        absolute(env("HOME")?)?
            .join(".local")
            .join("share")
            .join(folder),
    )
}

/// Where builds before this one kept the folder on Unix.
#[cfg(any(unix, test))]
fn old_dir(env: &dyn Fn(&str) -> Option<OsString>, folder: &str) -> Option<PathBuf> {
    absolute(env("HOME")?).map(|home| home.join(folder))
}

#[cfg(any(windows, unix, test))]
fn absolute(value: OsString) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Move the app's data from `old` to `new` when `old` holds it and `new`
/// does not, and say which folder to use.
#[cfg(any(unix, test))]
fn adopt(old: &Path, new: &Path) -> (PathBuf, Option<Moved>) {
    if old == new || !holds_data(old) || holds_data(new) {
        return (new.to_path_buf(), None);
    }
    match move_into(old, new) {
        Ok(()) => (
            new.to_path_buf(),
            Some(Moved::Moved {
                from: old.to_path_buf(),
                to: new.to_path_buf(),
            }),
        ),
        // The app and `bridge-host` both look, and the other may have moved
        // it first.
        Err(_) if holds_data(new) && !holds_data(old) => (new.to_path_buf(), None),
        Err(error) => (
            old.to_path_buf(),
            Some(Moved::Failed {
                from: old.to_path_buf(),
                to: new.to_path_buf(),
                error,
            }),
        ),
    }
}

#[cfg(any(unix, test))]
fn holds_data(dir: &Path) -> bool {
    DATA_FILES.iter().any(|name| dir.join(name).is_file())
}

/// Rename `old` to `new`. When `new` is already a folder, as the bridge
/// socket's is on macOS, move what `old` holds into it one by one, leaving
/// in `old` whatever `new` already has a file of that name for, and put
/// everything back if one move fails.
#[cfg(any(unix, test))]
fn move_into(old: &Path, new: &Path) -> io::Result<()> {
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(old, new) {
        Ok(()) => return Ok(()),
        Err(_) if new.is_dir() => {}
        Err(err) => return Err(err),
    }
    let entries = std::fs::read_dir(old)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    let mut moved = Vec::new();
    for name in entries {
        let (from, to) = (old.join(&name), new.join(&name));
        if std::fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        if let Err(err) = std::fs::rename(&from, &to) {
            for (from, to) in moved.iter().rev() {
                let _ = std::fs::rename(to, from);
            }
            return Err(err);
        }
        moved.push((from, to));
    }
    // Only goes when nothing was left behind.
    let _ = std::fs::remove_dir(old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    // Absolute on the machine the test runs on.
    fn root(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_string()
        }
    }

    #[test]
    fn each_platform_has_its_folder() {
        let home = root("/home/me");
        let xdg = root("/data");
        let local = root("/Users/me/AppData/Local");
        assert_eq!(
            xdg_dir(&env(&[("HOME", &home)]), "airlock"),
            Some(
                PathBuf::from(&home)
                    .join(".local")
                    .join("share")
                    .join("airlock")
            )
        );
        assert_eq!(
            xdg_dir(&env(&[("HOME", &home), ("XDG_DATA_HOME", &xdg)]), "airlock"),
            Some(PathBuf::from(&xdg).join("airlock"))
        );
        // A relative XDG_DATA_HOME is ignored, as the spec says.
        assert_eq!(
            xdg_dir(
                &env(&[("HOME", &home), ("XDG_DATA_HOME", "rel")]),
                "airlock"
            ),
            Some(
                PathBuf::from(&home)
                    .join(".local")
                    .join("share")
                    .join("airlock")
            )
        );
        assert_eq!(
            macos_dir(&env(&[("HOME", &home)]), "airlock-offline"),
            Some(
                PathBuf::from(&home)
                    .join("Library")
                    .join("Application Support")
                    .join("airlock-offline")
            )
        );
        assert_eq!(
            windows_dir(&env(&[("LOCALAPPDATA", &local)]), "airlock"),
            Some(PathBuf::from(&local).join("airlock"))
        );
        assert_eq!(xdg_dir(&env(&[]), "airlock"), None);
        assert_eq!(macos_dir(&env(&[("HOME", "")]), "airlock"), None);
        assert_eq!(
            old_dir(&env(&[("HOME", &home)]), "airlock"),
            Some(PathBuf::from(&home).join("airlock"))
        );
    }

    #[test]
    fn the_old_folder_moves_when_it_holds_the_data() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join("airlock");
        let new = base.path().join(".local").join("share").join("airlock");
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::write(old.join("vault.json"), b"v").unwrap();

        let (dir, moved) = adopt(&old, &new);
        assert_eq!(dir, new);
        assert!(matches!(moved, Some(Moved::Moved { .. })));
        assert_eq!(std::fs::read(new.join("vault.json")).unwrap(), b"v");
        assert!(new.join("logs").is_dir());
        assert!(!old.exists());

        // Once moved, nothing more happens.
        let (dir, moved) = adopt(&old, &new);
        assert_eq!(dir, new);
        assert!(moved.is_none());
    }

    #[test]
    fn a_folder_that_is_not_the_apps_stays() {
        let base = tempfile::tempdir().unwrap();
        // Say, a checkout of the source called `airlock`.
        let old = base.path().join("airlock");
        let new = base.path().join("new").join("airlock");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("Cargo.toml"), b"").unwrap();
        let (dir, moved) = adopt(&old, &new);
        assert_eq!(dir, new);
        assert!(moved.is_none());
        assert!(old.join("Cargo.toml").exists());
    }

    #[test]
    fn data_already_in_the_new_folder_wins() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join("old");
        let new = base.path().join("new");
        for dir in [&old, &new] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(old.join("vault.json"), b"old").unwrap();
        std::fs::write(new.join("workspace.json"), b"new").unwrap();
        let (dir, moved) = adopt(&old, &new);
        assert_eq!(dir, new);
        assert!(moved.is_none());
        assert!(old.join("vault.json").exists());
    }

    #[test]
    fn data_moves_into_a_folder_that_holds_only_other_things() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join("old");
        let new = base.path().join("new");
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::create_dir_all(new.join("logs")).unwrap();
        std::fs::write(old.join("vault.json"), b"v").unwrap();
        std::fs::write(old.join("logs").join("a.log"), b"a").unwrap();
        // What the macOS socket folder can already hold.
        std::fs::write(new.join("bridge.sock"), b"").unwrap();

        let (dir, moved) = adopt(&old, &new);
        assert_eq!(dir, new);
        assert!(matches!(moved, Some(Moved::Moved { .. })));
        assert_eq!(std::fs::read(new.join("vault.json")).unwrap(), b"v");
        assert!(new.join("bridge.sock").exists());
        // The clash stays where it was.
        assert!(old.join("logs").join("a.log").exists());
    }

    #[test]
    fn a_private_folder_is_made() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("a").join("airlock");
        create_private_dir(&dir).unwrap();
        create_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            create_private_dir(&dir).unwrap();
            assert_eq!(mode(&dir), 0o700);
        }
    }
}
