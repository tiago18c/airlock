//! The desktop app's local socket. Windows uses a named pipe; macOS and Linux
//! use a filesystem socket in a per-user directory.

use std::{io, path::PathBuf};

use interprocess::local_socket::{ConnectOptions, ListenerOptions, Name};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
pub use interprocess::local_socket::{Listener, Stream};

pub struct Endpoint {
    pub path: Option<PathBuf>,
    pub name: Name<'static>,
}

pub fn desktop_endpoint() -> io::Result<Endpoint> {
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        let safe: String = user
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect();
        let name = format!("airlock-bridge-{safe}").to_ns_name::<GenericNamespaced>()?;
        Ok(Endpoint { path: None, name })
    }
    #[cfg(unix)]
    {
        let path = socket_path()?;
        let name = path.clone().to_fs_name::<GenericFilePath>()?;
        Ok(Endpoint {
            path: Some(path),
            name,
        })
    }
}

#[cfg(unix)]
#[allow(unsafe_code)] // getuid, which std does not wrap.
fn socket_path() -> io::Result<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        home_dir()?.join("Library/Application Support/airlock")
    } else if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime).join("airlock")
    } else {
        let uid = unsafe { getuid() };
        std::env::temp_dir().join(format!("airlock-{uid}"))
    };
    Ok(dir.join("bridge.sock"))
}

#[cfg(unix)]
fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

#[cfg(unix)]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn getuid() -> u32;
}

pub fn prepare_directory(endpoint: &Endpoint) -> io::Result<()> {
    let Some(path) = &endpoint.path else {
        return Ok(());
    };
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn connect(name: &Name<'_>) -> io::Result<Stream> {
    ConnectOptions::new().name(name.clone()).connect_sync()
}

/// Binds the app's socket. On macOS and Linux an instance that was killed
/// leaves its socket file behind, and the bind then fails as if one were still
/// running. A file no listener answers is removed and the bind tried again; a
/// live instance keeps its socket.
pub fn bind_endpoint(endpoint: &Endpoint) -> io::Result<Listener> {
    let err = match bind(&endpoint.name) {
        Ok(listener) => return Ok(listener),
        Err(err) => err,
    };
    let Some(path) = &endpoint.path else {
        return Err(err);
    };
    if !path.exists() {
        return Err(err);
    }
    match connect(&endpoint.name) {
        Err(refused) if refused.kind() == io::ErrorKind::ConnectionRefused => {
            tracing::info!(target: "airlock::bridge", "removing a stale bridge socket");
            std::fs::remove_file(path)?;
            bind(&endpoint.name)
        }
        _ => Err(err),
    }
}

pub fn bind(name: &Name<'_>) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        match ListenerOptions::new()
            .name(name.clone())
            .mode(0o600)
            .create_sync()
        {
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {
                ListenerOptions::new().name(name.clone()).create_sync()
            }
            other => other,
        }
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::{
            local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
        };
        use widestring::U16CString;
        // Owner gets full access. The creating user owns the pipe.
        let sddl = U16CString::from_str("D:P(A;;GA;;;OW)")
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let descriptor = SecurityDescriptor::deserialize(sddl.as_ucstr())?;
        ListenerOptions::new()
            .name(name.clone())
            .security_descriptor(descriptor)
            .create_sync()
    }
}

#[cfg(test)]
pub fn test_endpoint(label: &str) -> io::Result<Endpoint> {
    #[cfg(windows)]
    {
        let name = format!("airlock-test-{}-{label}", std::process::id())
            .to_ns_name::<GenericNamespaced>()?;
        Ok(Endpoint { path: None, name })
    }
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("airlock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{label}.sock"));
        let name = path.clone().to_fs_name::<GenericFilePath>()?;
        Ok(Endpoint {
            path: Some(path),
            name,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_stale_socket_file_is_reclaimed() {
        let endpoint = test_endpoint("stale").unwrap();
        let path = endpoint.path.clone().unwrap();
        let _ = std::fs::remove_file(&path);
        // A std listener leaves its file when dropped, as a killed app does.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let listener = bind_endpoint(&endpoint).unwrap();
        connect(&endpoint.name).unwrap();
        drop(listener);
    }

    #[test]
    fn a_live_socket_is_kept() {
        let endpoint = test_endpoint("live").unwrap();
        let _ = std::fs::remove_file(endpoint.path.as_ref().unwrap());
        let first = bind_endpoint(&endpoint).unwrap();

        assert!(bind_endpoint(&endpoint).is_err());
        connect(&endpoint.name).unwrap();
        drop(first);
    }
}
