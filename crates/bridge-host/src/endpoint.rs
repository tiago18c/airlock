//! The desktop app's local socket. Windows uses a named pipe; macOS and Linux
//! use a filesystem socket in a per-user directory.

use std::{io, path::PathBuf, sync::mpsc, thread, time::Duration};

use interprocess::local_socket::{ConnectOptions, ListenerOptions, Name};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
pub use interprocess::local_socket::{Listener, Stream};

/// How long reaching the socket may take. A listener can keep every way in
/// busy, and connecting to one would otherwise wait for ever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Endpoint {
    pub path: Option<PathBuf>,
    pub name: Name<'static>,
}

/// The app's socket. Its name can be worked out by anyone, so what it is called
/// protects nothing: [`connect`] checks who holds it. `USERNAME` is in the name
/// on Windows so that a test can have a pipe of its own, apart from an app that
/// is running.
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
fn socket_path() -> io::Result<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        home_dir()?.join("Library/Application Support/airlock")
    } else if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime).join("airlock")
    } else {
        std::env::temp_dir().join(format!("airlock-{}", own_uid()))
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
#[allow(unsafe_code)] // getuid, which std does not wrap.
fn own_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

fn not_ours(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{what} belongs to someone else"),
    )
}

/// Make the folder of the socket, and refuse one that is not this user's. In the
/// temp folder anyone can make it first, and whoever owns it decides what is in it.
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
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Not followed: a link would make this folder wherever it points.
        let folder = std::fs::symlink_metadata(parent)?;
        if !folder.file_type().is_dir() || folder.uid() != own_uid() {
            return Err(not_ours("the folder of the bridge socket"));
        }
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Connect to the socket, if this user holds it. Anyone on this machine can make
/// a pipe of this name before the app does, and would hear what every site asks
/// and be believed in what it answers. So nothing is written before this says so.
pub fn connect(name: &Name<'static>) -> io::Result<Stream> {
    connect_within(name, CONNECT_TIMEOUT)
}

fn connect_within(name: &Name<'static>, timeout: Duration) -> io::Result<Stream> {
    let (done, finished) = mpsc::channel();
    let name = name.clone();
    thread::spawn(move || {
        let _ = done.send(ConnectOptions::new().name(name).connect_sync());
    });
    let stream = finished
        .recv_timeout(timeout)
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    check_holder(&stream)?;
    Ok(stream)
}

#[cfg(unix)]
fn check_holder(stream: &Stream) -> io::Result<()> {
    use interprocess::local_socket::traits::StreamCommon;
    match stream.peer_creds()?.euid() {
        Some(uid) if uid == own_uid() => Ok(()),
        _ => Err(not_ours("the bridge socket")),
    }
}

#[cfg(windows)]
fn check_holder(stream: &Stream) -> io::Result<()> {
    use std::os::windows::io::AsHandle;
    let Stream::NamedPipe(pipe) = stream;
    owner::require(pipe.inner().as_handle(), &owner::own()?)
}

/// Who owns a pipe. No account can make something that another owns, so this
/// says who made it, and unlike the pipe's process it cannot be reused.
#[cfg(windows)]
#[allow(unsafe_code)] // The owner of a handle and of a token, which std does not wrap.
mod owner {
    use std::os::windows::io::{AsRawHandle, BorrowedHandle};
    use std::{io, ptr, slice};

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TokenOwner, OWNER_SECURITY_INFORMATION, PSID,
        TOKEN_OWNER, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The bytes of the SID at `sid`, which must be valid.
    unsafe fn sid_bytes(sid: PSID) -> Vec<u8> {
        unsafe {
            let length = GetLengthSid(sid) as usize;
            slice::from_raw_parts(sid.cast::<u8>(), length).to_vec()
        }
    }

    /// The SID that owns what this process makes.
    pub(super) fn own() -> io::Result<Vec<u8>> {
        unsafe {
            let mut token = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut needed = 0;
            GetTokenInformation(token, TokenOwner, ptr::null_mut(), 0, &mut needed);
            // In whole words: it starts with a pointer, into itself.
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            let read = GetTokenInformation(
                token,
                TokenOwner,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            let found = if read == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(sid_bytes((*buffer.as_ptr().cast::<TOKEN_OWNER>()).Owner))
            };
            CloseHandle(token);
            found
        }
    }

    /// The SID that owns the object behind `handle`.
    fn of(handle: BorrowedHandle<'_>) -> io::Result<Vec<u8>> {
        unsafe {
            let mut owner = ptr::null_mut();
            let mut descriptor = ptr::null_mut();
            let status = GetSecurityInfo(
                handle.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            );
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let bytes = sid_bytes(owner);
            LocalFree(descriptor);
            Ok(bytes)
        }
    }

    /// Refuse a pipe that `expected` does not own.
    pub(super) fn require(pipe: BorrowedHandle<'_>, expected: &[u8]) -> io::Result<()> {
        if of(pipe)? == expected {
            Ok(())
        } else {
            Err(super::not_ours("the bridge pipe"))
        }
    }
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

/// A socket for one test, apart from an app that is running.
#[doc(hidden)]
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use interprocess::local_socket::traits::Listener as _;
    use std::os::windows::io::AsHandle;

    #[test]
    fn a_pipe_owned_by_anyone_else_is_refused() {
        let endpoint = test_endpoint("owner").unwrap();
        let listener = bind(&endpoint.name).unwrap();
        thread::spawn(move || {
            let held = listener.accept();
            thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let stream = connect(&endpoint.name).unwrap();
        let Stream::NamedPipe(pipe) = &stream;
        let handle = pipe.inner().as_handle();

        assert!(owner::require(handle, &owner::own().unwrap()).is_ok());
        // Everyone, S-1-1-0: an owner that is not this account.
        let someone_else = [1u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let refused = owner::require(handle, &someone_else).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_pipe_with_no_way_in_is_given_up_on() {
        let endpoint = test_endpoint("busy").unwrap();
        // It takes one connection and never accepts another.
        let _listener = bind(&endpoint.name).unwrap();
        let _first = connect(&endpoint.name).unwrap();
        let refused = connect_within(&endpoint.name, Duration::from_millis(300)).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::TimedOut);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_not_ours_is_refused() {
        if own_uid() == 0 {
            return;
        }
        // The root of the disk, which root owns.
        let endpoint = Endpoint {
            path: Some(PathBuf::from("/bridge.sock")),
            name: test_endpoint("unused").unwrap().name,
        };
        let refused = prepare_directory(&endpoint).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_link_in_place_of_the_folder_is_refused() {
        let base = std::env::temp_dir().join(format!("airlock-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let endpoint = Endpoint {
            path: Some(link.join("bridge.sock")),
            name: test_endpoint("unused-link").unwrap().name,
        };
        let refused = prepare_directory(&endpoint).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_of_our_own_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("airlock-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let endpoint = Endpoint {
            path: Some(base.join("bridge.sock")),
            name: test_endpoint("unused-own").unwrap().name,
        };
        prepare_directory(&endpoint).unwrap();
        let mode = std::fs::metadata(&base).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        std::fs::remove_dir_all(&base).unwrap();
    }

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
