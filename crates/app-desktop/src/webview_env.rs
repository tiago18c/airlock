//! Environment variables the webview obeys. WebView2 reads every
//! `WEBVIEW2_*` one: `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` can open a
//! debugging port into the wallet's page, and
//! `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` can swap the browser it runs on.
//! WebKitGTK opens its inspector on a port when `WEBKIT_INSPECTOR_SERVER` or
//! `WEBKIT_INSPECTOR_HTTP_SERVER` is set. Anyone who can set a variable for
//! this user could do either, so a release build removes them, on every
//! platform, since a name one webview does not read is harmless to drop. A
//! debug build keeps them: driving the app over the DevTools protocol
//! (`scripts/drive-desktop.mjs`) depends on them.

use std::ffi::OsStr;

/// Variables named exactly these are removed.
const NAMES: &[&str] = &["WEBKIT_INSPECTOR_SERVER", "WEBKIT_INSPECTOR_HTTP_SERVER"];

/// Variables whose names start with this are removed.
const PREFIX: &str = "WEBVIEW2_";

/// In a release build, remove every variable the webview would take
/// orders from, and return their names, never their values, for the log.
/// Call it first thing in `main`, before any thread starts: changing the
/// environment while another thread reads it is not sound.
pub fn drop_webview_overrides() -> Vec<String> {
    if cfg!(debug_assertions) {
        return Vec::new();
    }
    let names: Vec<_> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_override(name))
        .collect();
    for name in &names {
        std::env::remove_var(name);
    }
    names
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

/// Say which variables [`drop_webview_overrides`] removed, once logging is up.
pub fn log_dropped_webview_overrides(names: &[String]) {
    if !names.is_empty() {
        tracing::warn!(
            target: "airlock::app",
            names = %names.join(","),
            "ignored environment variables that would change how the window's webview runs"
        );
    }
}

/// Whether the webview obeys a variable of this name. Windows reads them
/// without regard to case, so case is not regarded here either.
fn is_override(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let name = name.to_ascii_uppercase();
    name.starts_with(PREFIX) || NAMES.contains(&name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_webviews_variables_are_the_ones_dropped() {
        for name in [
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            "WEBVIEW2_USER_DATA_FOLDER",
            "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
            "webview2_additional_browser_arguments",
            "WEBKIT_INSPECTOR_SERVER",
            "WEBKIT_INSPECTOR_HTTP_SERVER",
        ] {
            assert!(is_override(OsStr::new(name)), "{name}");
        }
        for name in [
            "PATH",
            "LOCALAPPDATA",
            "AIRLOCK_LOG",
            "WEBVIEW",
            "MY_WEBVIEW2_THING",
            "WEBKIT_DISABLE_COMPOSITING_MODE",
        ] {
            assert!(!is_override(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn a_debug_build_keeps_them() {
        if cfg!(debug_assertions) {
            assert!(drop_webview_overrides().is_empty());
        }
    }
}
