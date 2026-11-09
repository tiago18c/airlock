//! Registration of the native messaging host. Opt-in: the online app's installer
//! asks, and the app's Extension chip offers it. The airgap app ships no host,
//! so an offline machine never gets the bridge.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use bridge_protocol::extension_id::{pinned_chrome_extension_id, GECKO_ID, HOST_NAME};

/// The host's file name on this OS.
pub const HOST_EXE: &str = if cfg!(windows) {
    "bridge-host.exe"
} else {
    "bridge-host"
};

pub fn install() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let locations = register(&exe)?;
    for location in &locations {
        println!("bridge-host: {location}");
    }
    println!(
        "bridge-host: registered {} browser location(s)",
        locations.len()
    );
    println!(
        "bridge-host: allowed extension chrome-extension://{}/",
        pinned_chrome_extension_id()
    );
    println!("bridge-host: restart the browser so it loads the host");
    Ok(())
}

/// Registers `exe` as the host for every browser, and returns where it wrote.
pub fn register(exe: &Path) -> io::Result<Vec<String>> {
    if let Some(why) = transient_location(exe) {
        return Err(io::Error::other(why));
    }
    // A browser starts the host itself, not through Finder, so macOS would
    // refuse a quarantined one even after the user allowed the app around
    // it. It came in that app, so it is allowed with it.
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("/usr/bin/xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(exe)
        .stderr(std::process::Stdio::null())
        .status();

    let extension_id = pinned_chrome_extension_id();
    let chromium = serde_json::to_string_pretty(&chromium_manifest(exe, &extension_id))
        .map_err(io::Error::other)?;
    let firefox = serde_json::to_string_pretty(&firefox_manifest(exe)).map_err(io::Error::other)?;

    #[cfg(windows)]
    let locations = install_windows(&chromium, &firefox)?;
    #[cfg(unix)]
    let locations = install_unix(&chromium, &firefox)?;
    #[cfg(not(any(windows, unix)))]
    let locations: Vec<String> = {
        let _ = (chromium, firefox);
        Vec::new()
    };

    if locations.is_empty() {
        return Err(io::Error::other(
            "did not register the host with any browser",
        ));
    }
    Ok(locations)
}

/// The binary to register for the `host` that came with the app: `host`
/// itself, or, inside a running AppImage, a copy in the data folder. An
/// AppImage runs from a mount that goes away when the app quits, so a
/// registration naming the host there would break. The copy is refreshed
/// whenever it differs, so it keeps up with the AppImage it came from.
pub fn stable_host(host: &Path) -> io::Result<PathBuf> {
    let Some(copy) = appimage_copy(host) else {
        return Ok(host.to_path_buf());
    };
    if fs::read(host)? != fs::read(&copy).unwrap_or_default() {
        let dir = copy
            .parent()
            .ok_or_else(|| io::Error::other("no data folder"))?;
        fs::create_dir_all(dir)?;
        // Through a new file and a rename: a browser may be running the old one.
        let fresh = dir.join(format!("{HOST_EXE}.new"));
        fs::copy(host, &fresh)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(&fresh, &copy)?;
    }
    Ok(copy)
}

/// Where [`stable_host`] keeps its copy of a host inside the running AppImage,
/// or `None` when `host` is not in one.
fn appimage_copy(host: &Path) -> Option<PathBuf> {
    let appdir = std::env::var_os("APPDIR").filter(|_| std::env::var_os("APPIMAGE").is_some())?;
    if !host.starts_with(appdir) {
        return None;
    }
    crate::log::data_dir().map(|dir| dir.join(HOST_EXE))
}

/// At the app's start: when the registration names the copy [`stable_host`]
/// made from `host`, brings the copy up to date with it. Nothing otherwise.
pub fn refresh_stable_host(host: &Path) {
    let Some(copy) = appimage_copy(host) else {
        return;
    };
    if registered_host().as_deref() == Some(copy.as_path()) {
        if let Err(err) = stable_host(host) {
            tracing::warn!(target: "bridge", "could not refresh {}: {err}", copy.display());
        }
    }
}

/// Why a host at `exe` would not stay there: a macOS disk image that is
/// still mounted, or the random copy macOS runs a quarantined app from
/// (App Translocation) until it is moved out of the folder it came in.
/// A registration naming either breaks once the image is ejected or the app
/// restarts.
pub fn transient_location(exe: &Path) -> Option<&'static str> {
    let path = exe.to_string_lossy();
    if path.starts_with("/Volumes/") {
        Some("Airlock is running from its disk image. Drag it to Applications, open it from there, and try again.")
    } else if path.contains("/AppTranslocation/") {
        Some("macOS is running Airlock from a temporary copy. Move it to Applications, open it from there, and try again.")
    } else {
        None
    }
}

pub fn uninstall() -> io::Result<()> {
    let mut removed = 0usize;
    #[cfg(windows)]
    {
        removed += uninstall_windows()?;
    }
    #[cfg(unix)]
    {
        removed += uninstall_unix()?;
    }
    println!("bridge-host: removed {removed} registration(s)");
    Ok(())
}

/// `uninstall --if-this-exe`: removes the registration only when it names the
/// binary running this. The installer's uninstaller passes it, so it leaves
/// alone a host registered from somewhere else, such as a portable copy.
pub fn uninstall_if_this_exe() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    if registered_host().is_some_and(|host| same_file(&host, &exe)) {
        uninstall()
    } else {
        println!("bridge-host: registered from elsewhere or not at all, left alone");
        Ok(())
    }
}

/// Where a host is registered, and whether it is still there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Registration {
    /// Not registered.
    None,
    /// Registered, and the binary is there.
    Ready(PathBuf),
    /// Registered, but the binary has moved or been deleted.
    Missing(PathBuf),
}

pub fn registration() -> Registration {
    match registered_host() {
        None => Registration::None,
        Some(path) if path.is_file() => Registration::Ready(path),
        Some(path) => Registration::Missing(path),
    }
}

/// The binary the registration names, read from the manifest the Chromium
/// browsers use. `None` when the host is not registered.
pub fn registered_host() -> Option<PathBuf> {
    #[cfg(windows)]
    let manifest = manifest_dir().ok()?.join(CHROMIUM_MANIFEST);
    #[cfg(unix)]
    let manifest = host_dirs()
        .into_iter()
        .map(|(dir, _)| dir.join(format!("{HOST_NAME}.json")))
        .find(|path| path.is_file())?;
    #[cfg(not(any(windows, unix)))]
    let manifest: PathBuf = return None;

    host_in_manifest(&fs::read_to_string(manifest).ok()?)
}

fn host_in_manifest(text: &str) -> Option<PathBuf> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("name").and_then(Value::as_str) != Some(HOST_NAME) {
        return None;
    }
    value.get("path").and_then(Value::as_str).map(PathBuf::from)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn chromium_manifest(exe: &Path, extension_id: &str) -> Value {
    json!({
        "name": HOST_NAME,
        "description": "Connects Airlock Extension to the Airlock desktop app",
        "path": path_string(exe),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{extension_id}/")]
    })
}

pub fn firefox_manifest(exe: &Path) -> Value {
    json!({
        "name": HOST_NAME,
        "description": "Connects Airlock Extension to the Airlock desktop app",
        "path": path_string(exe),
        "type": "stdio",
        "allowed_extensions": [GECKO_ID]
    })
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(windows)]
const CHROMIUM_MANIFEST: &str = "dev.airlock.bridge.chromium.json";
#[cfg(windows)]
const FIREFOX_MANIFEST: &str = "dev.airlock.bridge.firefox.json";

#[cfg(windows)]
fn manifest_dir() -> io::Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
    Ok(PathBuf::from(local)
        .join("airlock")
        .join("native-messaging"))
}

#[cfg(windows)]
fn install_windows(chromium: &str, firefox: &str) -> io::Result<Vec<String>> {
    let dir = manifest_dir()?;
    fs::create_dir_all(&dir)?;
    let chromium_path = dir.join(CHROMIUM_MANIFEST);
    let firefox_path = dir.join(FIREFOX_MANIFEST);
    fs::write(&chromium_path, chromium)?;
    fs::write(&firefox_path, firefox)?;

    let mut locations = Vec::new();
    for key in CHROMIUM_KEYS {
        write_registry(key, &chromium_path)?;
        locations.push((*key).to_owned());
    }
    write_registry(FIREFOX_KEY, &firefox_path)?;
    locations.push(FIREFOX_KEY.to_owned());
    Ok(locations)
}

#[cfg(windows)]
fn uninstall_windows() -> io::Result<usize> {
    use winreg::RegKey;
    let hkcu = RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let mut count = 0;
    for key in CHROMIUM_KEYS.iter().chain(std::iter::once(&FIREFOX_KEY)) {
        if hkcu.delete_subkey(key).is_ok() {
            println!("bridge-host: removed {key}");
            count += 1;
        }
    }
    if let Ok(dir) = manifest_dir() {
        for name in [CHROMIUM_MANIFEST, FIREFOX_MANIFEST] {
            if remove_our_manifest(&dir.join(name)) {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(windows)]
fn write_registry(key: &str, manifest: &Path) -> io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (reg, _) = hkcu.create_subkey(key)?;
    reg.set_value("", &path_string(manifest))?;
    Ok(())
}

#[cfg(windows)]
const CHROMIUM_KEYS: &[&str] = &[
    r"Software\Google\Chrome\NativeMessagingHosts\dev.airlock.bridge",
    r"Software\Chromium\NativeMessagingHosts\dev.airlock.bridge",
    r"Software\Microsoft\Edge\NativeMessagingHosts\dev.airlock.bridge",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\dev.airlock.bridge",
];

#[cfg(windows)]
const FIREFOX_KEY: &str = r"Software\Mozilla\NativeMessagingHosts\dev.airlock.bridge";

#[cfg(unix)]
fn install_unix(chromium: &str, firefox: &str) -> io::Result<Vec<String>> {
    let mut locations = Vec::new();
    for (dir, kind) in host_dirs() {
        let body = match kind {
            HostKind::Chromium => chromium,
            HostKind::Firefox => firefox,
        };
        if let Err(err) = fs::create_dir_all(&dir) {
            eprintln!("bridge-host: skipping {}: {err}", dir.display());
            continue;
        }
        let path = dir.join(format!("{HOST_NAME}.json"));
        fs::write(&path, body)?;
        locations.push(path.display().to_string());
    }
    Ok(locations)
}

#[cfg(unix)]
fn uninstall_unix() -> io::Result<usize> {
    let mut count = 0;
    for (dir, _) in host_dirs() {
        let path = dir.join(format!("{HOST_NAME}.json"));
        if remove_our_manifest(&path) {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(unix)]
enum HostKind {
    Chromium,
    Firefox,
}

#[cfg(unix)]
fn host_dirs() -> Vec<(PathBuf, HostKind)> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    {
        let app = home.join("Library/Application Support");
        vec![
            (
                app.join("Google/Chrome/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                app.join("Chromium/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                app.join("Microsoft Edge/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                app.join("BraveSoftware/Brave-Browser/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (app.join("Mozilla/NativeMessagingHosts"), HostKind::Firefox),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        vec![
            (
                config.join("google-chrome/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                config.join("chromium/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                config.join("microsoft-edge/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                config.join("BraveSoftware/Brave-Browser/NativeMessagingHosts"),
                HostKind::Chromium,
            ),
            (
                home.join(".mozilla/native-messaging-hosts"),
                HostKind::Firefox,
            ),
        ]
    }
}

fn remove_our_manifest(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    if value.get("name").and_then(Value::as_str) != Some(HOST_NAME) {
        return false;
    }
    fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_pin_this_extension() {
        let exe = Path::new("C:/airlock/bridge-host.exe");
        let chromium = chromium_manifest(exe, "mmbfhgibjpbgmhjfhfhlejdpopgapphp");
        assert_eq!(chromium["name"], HOST_NAME);
        assert_eq!(chromium["type"], "stdio");
        assert_eq!(
            chromium["allowed_origins"][0],
            "chrome-extension://mmbfhgibjpbgmhjfhfhlejdpopgapphp/"
        );
        assert_eq!(chromium["path"], "C:/airlock/bridge-host.exe");

        let firefox = firefox_manifest(exe);
        assert_eq!(firefox["allowed_extensions"][0], GECKO_ID);
        assert!(firefox.get("allowed_origins").is_none());
    }

    #[test]
    fn reads_the_host_back_from_a_manifest() {
        let exe = Path::new("C:/airlock/bridge-host.exe");
        let text = chromium_manifest(exe, "mmbfhgibjpbgmhjfhfhlejdpopgapphp").to_string();
        assert_eq!(host_in_manifest(&text).as_deref(), Some(exe));

        let other = json!({ "name": "com.example.other", "path": "C:/x.exe" }).to_string();
        assert_eq!(host_in_manifest(&other), None);
        assert_eq!(host_in_manifest("not json"), None);
    }

    #[test]
    fn a_mounted_image_or_translocated_app_is_not_a_place_to_register() {
        let stays = Path::new("/Applications/Airlock.app/Contents/MacOS/bridge-host");
        assert_eq!(transient_location(stays), None);
        let image = Path::new("/Volumes/Airlock/Airlock.app/Contents/MacOS/bridge-host");
        assert!(transient_location(image).is_some());
        let moved = Path::new(
            "/private/var/folders/x/T/AppTranslocation/1234/d/Airlock.app/Contents/MacOS/bridge-host",
        );
        assert!(transient_location(moved).is_some());
        assert!(register(image).is_err());
    }
}
