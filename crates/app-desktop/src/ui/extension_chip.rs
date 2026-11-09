//! The browser extension's chip in the sidebar's status bar, beside the
//! network chip: whether an extension is connected to the bridge socket.
//! Until one is, its card offers to register the browser bridge, for whoever
//! skipped it in the installer, and says where to get the extension.

use std::path::PathBuf;

use app_ui_kit::Session;
use bridge_host::install::{self, Registration, HOST_EXE};
use dioxus::prelude::*;

use super::connections::ConnectionsModal;
use crate::bridge::Bridge;

/// Where the extension is released. It is not in a store yet, so it is
/// loaded unpacked from the zip on the release page.
const EXTENSION_RELEASES: &str = "https://github.com/tiago18c/airlock/releases/latest";

/// "Extension": green while a browser's extension holds a connection to the
/// app, grey otherwise. Hovering or focusing it says how many browsers, or when one connects.
#[component]
pub fn ExtensionChip() -> Element {
    let bridge = use_context::<Bridge>();
    let manage = use_context::<ConnectionsModal>();
    let session = use_context::<Session>();
    let unavailable = bridge.unavailable().map(str::to_owned);
    let mut connected = use_signal(|| *bridge.subscribe_extension().borrow());
    use_future(move || {
        let mut extension = bridge.subscribe_extension();
        async move {
            while extension.changed().await.is_ok() {
                let n = *extension.borrow_and_update();
                connected.set(n);
            }
        }
    });
    let mut registration = use_signal(install::registration);
    // What the last press of Install did.
    let mut outcome = use_signal(|| None::<Result<(), String>>);
    let mut link_error = use_signal(|| None::<String>);

    let n = connected();
    let (class, status) = if unavailable.is_some() {
        ("status net-chip offline", "Unavailable")
    } else if n > 0 {
        ("status net-chip online", "Connected")
    } else {
        ("status net-chip idle", "Not connected")
    };
    let host = host_beside_app();
    let transient = host.as_deref().and_then(install::transient_location);
    let registered = registration();
    let bridge_text = match &registered {
        Registration::Ready(_) => "Installed".to_owned(),
        Registration::None => "Not installed".to_owned(),
        Registration::Missing(path) => {
            format!("Not installed: {} has moved", path.display())
        }
    };
    // Nothing to set up once a browser has reached the app.
    let setup = n == 0;
    let can_install = setup && !matches!(registered, Registration::Ready(_));

    rsx! {
        span {
            class: "{class}",
            tabindex: "0",
            aria_label: "Extension {status}",
            span { class: "status-dot" }
            span { "Extension" }
            // Not a place to move the window from.
            div { class: "net-card", "data-drag": "false",
                dl {
                    dt { "Status" }
                    dd { "{status}" }
                    if let Some(why) = unavailable {
                        dt { "Why" }
                        dd { class: "net-err", "{why}" }
                    } else if n > 0 {
                        dt { "Browsers" }
                        dd { "{n}" }
                    } else {
                        dt { "Opens" }
                        dd { "When a site or the extension's popup first asks for Airlock" }
                    }
                    if setup {
                        dt { "Bridge" }
                        dd { "{bridge_text}" }
                    }
                    match outcome() {
                        Some(Ok(())) => rsx! {
                            dt { "" }
                            dd { "Installed. Restart the browser if the extension still says the host is missing." }
                        },
                        Some(Err(why)) => rsx! {
                            dt { "" }
                            dd { class: "net-err", "{why}" }
                        },
                        None => rsx! {},
                    }
                    if can_install && host.is_none() {
                        dt { "" }
                        dd { "The bridge ({HOST_EXE}) is not beside Airlock. It comes with every download of the app." }
                    }
                    if let (true, Some(why)) = (can_install, transient) {
                        dt { "" }
                        dd { "{why}" }
                    }
                    if setup {
                        dt { "Get it" }
                        dd { "Download airlock-extension from the latest release and unzip it. In Chrome, Edge or Brave, open the extensions page, turn on Developer mode, and load the folder unpacked." }
                    }
                    if let Some(why) = link_error() {
                        dt { "" }
                        dd { class: "net-err", "{why}" }
                    }
                }
                if can_install {
                    button { class: "sm primary",
                        disabled: host.is_none() || transient.is_some(),
                        onclick: move |_| {
                            let Some(host) = host_beside_app() else { return };
                            let result = install::register(&host)
                                .map(|_| ())
                                .map_err(|err| format!("Could not install the bridge: {err}"));
                            if let Err(why) = &result {
                                tracing::warn!(target: "bridge", "{why}");
                            }
                            outcome.set(Some(result));
                            registration.set(install::registration());
                        },
                        "Install the browser bridge"
                    }
                }
                if setup {
                    button { class: "sm",
                        onclick: move |_| {
                            link_error.set(session.open_link(EXTENSION_RELEASES).err().map(|err| format!("Could not open the release page: {err}")));
                        },
                        "Get the extension"
                    }
                }
                button { class: "sm",
                    onclick: move |_| manage.show(),
                    "Manage connections"
                }
            }
        }
    }
}

/// The host that came with this copy of the app: `bridge-host` beside its
/// exe, where the installer and the portable zip put it, and where cargo
/// builds it.
fn host_beside_app() -> Option<PathBuf> {
    let host = std::env::current_exe().ok()?.parent()?.join(HOST_EXE);
    host.is_file().then_some(host)
}
