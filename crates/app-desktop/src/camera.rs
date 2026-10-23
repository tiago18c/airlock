//! Desktop camera permission. Scanner list/preview JS stays in `app-ui-kit`.

/// WebView2 denies the camera unless the host grants it. The app's own
/// page gets it; a request from any other page is refused. (Other pages
/// open in the system browser anyway, and an embedded frame is judged by
/// the page that embeds it, so this is a second line, not the first.)
///
/// The grant is also stored in the WebView2 profile up front, so the
/// scanner can list cameras by name before it opens one. Without a stored
/// grant a page sees one unnamed camera, and opening it is the only way to
/// learn the rest.
#[cfg(all(feature = "desktop", windows))]
pub fn allow_windows_camera(window: &dioxus::desktop::DesktopContext) {
    use dioxus::desktop::wry::WebViewExtWindows;
    use std::sync::atomic::{AtomicBool, Ordering};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DENY,
    };
    use webview2_com::{PermissionRequestedEventHandler, SetPermissionStateCompletedHandler};
    use windows_core::{Interface, HSTRING, PWSTR};

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let webview = window.webview.webview();
    let source = unsafe {
        let mut uri = PWSTR::null();
        webview
            .Source(&mut uri)
            .map(|_| webview2_com::take_pwstr(uri))
    };
    let Some(app) = source.ok().as_deref().and_then(origin) else {
        tracing::error!(target: "airlock::app", "no page origin; the camera stays off");
        return;
    };

    let ours = app.clone();
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        unsafe {
            let mut kind = Default::default();
            args.PermissionKind(&mut kind)?;
            // Every other kind of request keeps its default.
            if kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA {
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let from = origin(&webview2_com::take_pwstr(uri));
                args.SetState(if from.as_deref() == Some(ours.as_str()) {
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW
                } else {
                    COREWEBVIEW2_PERMISSION_STATE_DENY
                })?;
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        if let Err(err) = webview.add_PermissionRequested(&handler, &mut token) {
            tracing::error!(
                target: "airlock::app",
                error = %err,
                "could not register the camera permission handler"
            );
        }
    }

    let stored = webview
        .cast::<ICoreWebView2_13>()
        .and_then(|w| unsafe { w.Profile() })
        .and_then(|p| p.cast::<ICoreWebView2Profile4>())
        .and_then(|profile| {
            let done = SetPermissionStateCompletedHandler::create(Box::new(|result| {
                if let Err(err) = result {
                    tracing::warn!(target: "airlock::app", error = %err, "camera grant not stored");
                }
                Ok(())
            }));
            unsafe {
                profile.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_CAMERA,
                    &HSTRING::from(app.as_str()),
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                    &done,
                )
            }
        });
    if let Err(err) = stored {
        // Older WebView2 runtimes: the camera list then fills once scanning
        // starts, as on the web.
        tracing::warn!(target: "airlock::app", error = %err, "camera grant not stored");
    }
}

/// `scheme://host[:port]` of a URL, lowercased.
#[cfg_attr(not(all(feature = "desktop", windows)), allow(dead_code))]
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!scheme.is_empty() && !host.is_empty()).then(|| format!("{scheme}://{host}").to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::origin;

    #[test]
    fn origins() {
        assert_eq!(
            origin("http://dioxus.index.html/").as_deref(),
            Some("http://dioxus.index.html")
        );
        assert_eq!(
            origin("http://dioxus.index.html").as_deref(),
            Some("http://dioxus.index.html")
        );
        assert_eq!(
            origin("HTTPS://Example.com:8443/a?b#c").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(origin("data:text/html,hi"), None);
        assert_eq!(origin("about:blank"), None);
    }
}
