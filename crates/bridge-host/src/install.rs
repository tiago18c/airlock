//! Opt-in registration of the native messaging host. Not part of the app installer,
//! so an offline machine never gets the bridge.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use bridge_protocol::extension_id::{pinned_chrome_extension_id, GECKO_ID, HOST_NAME};

pub fn install() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let extension_id = pinned_chrome_extension_id();
    let chromium = serde_json::to_string_pretty(&chromium_manifest(&exe, &extension_id))
        .map_err(io::Error::other)?;
    let firefox =
        serde_json::to_string_pretty(&firefox_manifest(&exe)).map_err(io::Error::other)?;
    let mut wrote = 0usize;

    #[cfg(windows)]
    {
        wrote += install_windows(&chromium, &firefox)?;
    }
    #[cfg(unix)]
    {
        wrote += install_unix(&chromium, &firefox)?;
    }

    if wrote == 0 {
        return Err(io::Error::other(
            "did not register the host with any browser",
        ));
    }
    println!("bridge-host: registered {wrote} browser location(s)");
    println!("bridge-host: allowed extension chrome-extension://{extension_id}/");
    println!("bridge-host: restart the browser so it loads the host");
    Ok(())
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
fn manifest_dir() -> io::Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
    Ok(PathBuf::from(local)
        .join("airlock")
        .join("native-messaging"))
}

#[cfg(windows)]
fn install_windows(chromium: &str, firefox: &str) -> io::Result<usize> {
    let dir = manifest_dir()?;
    fs::create_dir_all(&dir)?;
    let chromium_path = dir.join("dev.airlock.bridge.chromium.json");
    let firefox_path = dir.join("dev.airlock.bridge.firefox.json");
    fs::write(&chromium_path, chromium)?;
    fs::write(&firefox_path, firefox)?;

    let mut count = 0;
    for key in CHROMIUM_KEYS {
        write_registry(key, &chromium_path)?;
        println!("bridge-host: {key}");
        count += 1;
    }
    write_registry(FIREFOX_KEY, &firefox_path)?;
    println!("bridge-host: {FIREFOX_KEY}");
    count += 1;
    Ok(count)
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
        for name in [
            "dev.airlock.bridge.chromium.json",
            "dev.airlock.bridge.firefox.json",
        ] {
            let path = dir.join(name);
            if remove_our_manifest(&path) {
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
fn install_unix(chromium: &str, firefox: &str) -> io::Result<usize> {
    let mut count = 0;
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
        println!("bridge-host: {}", path.display());
        count += 1;
    }
    Ok(count)
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
}
