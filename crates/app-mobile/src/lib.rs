//! Android services for `apps/airlock-mobile`: the app's private folder,
//! the WebView's settings, opening links, whether the app runs online or
//! offline, requests from other apps ([`wallet_adapter`]), and reporting to
//! the core when the app is out of sight.
//!
//! Everything that calls into Android goes through [`android`], over JNI.
//! On any other OS this crate builds, so the workspace checks it, but its
//! Android calls answer that there is no Android.

#[cfg(target_os = "android")]
#[allow(unsafe_code)] // JNI: the VM and activity pointers ndk-context hands over.
mod android;

pub mod key_guard;
pub mod mode;
pub mod seed_vault;
mod usb;
pub mod wallet_adapter;

use std::sync::Arc;

use app_core::{Core, Links};
use dioxus::prelude::*;

pub use mode::Mode;

/// Get the process ready before logging or any store opens: put the data
/// folder under the app's private files folder, and pick the folder of the
/// mode the app was left in, so online and offline each keep their vault.
pub fn prepare() -> Mode {
    if let Some(dir) = files_dir() {
        app_core::storage::set_base_dir(dir);
    }
    let mode = mode::load();
    if mode == Mode::Offline {
        app_core::storage::set_app_dirs(app_core::storage::AppDirs::OFFLINE);
    }
    mode
}

/// The app's private no-backup folder (`Context.getNoBackupFilesDir`),
/// which no other app can read, no backup or transfer to a new phone copies,
/// and which is removed with the app.
pub fn files_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "android")]
    {
        match android::files_dir() {
            Ok(dir) => Some(dir),
            Err(err) => {
                tracing::error!(target: "airlock::app", %err, "no private files folder");
                None
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// The WebView's settings: the same page policy as on desktop, and no
/// navigation away from the app.
pub fn mobile_config() -> dioxus::mobile::Config {
    dioxus::mobile::Config::new()
        .with_custom_head(app_ui_kit_head())
        .with_navigation_handler(|_| false)
}

/// The page policy every shell shares, plus the viewport a phone needs:
/// the page fills the screen under the system bars, and the CSS keeps its
/// controls out of them with the safe-area insets.
fn app_ui_kit_head() -> String {
    format!(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n{}",
        app_ui_kit::platform::page_policy::page_head()
    )
}

/// Opens web links in the phone's browser. Only `https` and `http` go out:
/// an `intent:` or other scheme could start any app with what it carries.
pub struct AndroidLinks;

impl Links for AndroidLinks {
    fn open(&self, url: &str) -> Result<(), String> {
        let lower = url.trim_start().to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) {
            return Err("only web links open outside the app".to_string());
        }
        #[cfg(target_os = "android")]
        {
            android::open_url(url)
        }
        #[cfg(not(target_os = "android"))]
        {
            Err("not on Android".to_string())
        }
    }
}

/// The [`Links`] the core opens URLs with on Android.
pub fn links() -> Arc<dyn Links> {
    Arc::new(AndroidLinks)
}

/// Tell the core when the app is in front and when it is not, so auto-lock
/// counts the time it spent in the background, as it counts a hidden window
/// on desktop.
pub fn use_lifecycle(core: CopyValue<Core>) {
    use dioxus::mobile::tao::event::{Event, WindowEvent};

    use_hook(move || core.peek().set_ui_visible(true));
    dioxus::mobile::use_wry_event_handler(move |event, _| match event {
        Event::Suspended => {
            tracing::info!(target: "airlock::app", "app in the background");
            core.peek().set_ui_visible(false);
        }
        Event::Resumed => {
            tracing::info!(target: "airlock::app", "app in front");
            core.peek().set_ui_visible(true);
        }
        Event::WindowEvent {
            event: WindowEvent::Focused(focused),
            ..
        } => core.peek().set_ui_visible(*focused),
        _ => {}
    });
}

/// Start the app again from the launcher, so it opens in the mode just
/// chosen with that mode's folder. The process ends here.
pub fn restart() -> ! {
    #[cfg(target_os = "android")]
    if let Err(err) = android::relaunch() {
        tracing::error!(target: "airlock::app", %err, "could not schedule the restart");
    }
    std::process::exit(0)
}

/// Settings: run online or offline. Each mode has its own vault, so the
/// switch restarts the app on the other mode's folder.
#[component]
pub fn ModeCard(mode: Mode) -> Element {
    let mut error = use_signal(|| None::<String>);
    let mut pick = move |to: Mode| {
        if to == mode {
            return;
        }
        match mode::save(to) {
            Ok(()) => {
                // Offline, no app may find Airlock, even before it restarts.
                if to == Mode::Offline {
                    wallet_adapter::set_listed(false);
                }
                restart()
            }
            Err(err) => error.set(Some(err)),
        }
    };
    rsx! {
        div { class: "card",
            h3 { "Mode" }
            div { class: "segmented",
                button {
                    class: if mode == Mode::Online { "tab on" } else { "tab" },
                    onclick: move |_| pick(Mode::Online),
                    "Online"
                }
                button {
                    class: if mode == Mode::Offline { "tab on" } else { "tab" },
                    onclick: move |_| pick(Mode::Offline),
                    "Offline"
                }
            }
            p { class: "hint", "Each mode has its own vault. Switching restarts the app." }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
        }
    }
}

/// Settings, online: whether apps on this phone, and sites open in Chrome,
/// may ask Airlock to connect and sign, and the sites they connected.
#[component]
pub fn AppsCard() -> Element {
    let mut on = use_signal(wallet_adapter::allowed);
    let mut error = use_signal(|| None::<String>);
    let manage = use_context::<app_desktop::ConnectionsModal>();
    let mut set = move |to: bool| match wallet_adapter::set_allowed(to) {
        Ok(()) => {
            wallet_adapter::set_listed(to);
            on.set(to);
            error.set(None);
        }
        Err(err) => error.set(Some(err)),
    };
    rsx! {
        div { class: "card",
            h3 { "Apps on this phone" }
            div { class: "segmented",
                button {
                    class: if on() { "tab" } else { "tab on" },
                    onclick: move |_| set(false),
                    "Off"
                }
                button {
                    class: if on() { "tab on" } else { "tab" },
                    onclick: move |_| set(true),
                    "On"
                }
            }
            p { class: "hint",
                "Other apps, and sites open in Chrome, can ask Airlock to connect and sign through Mobile Wallet Adapter, as the browser extension does on a computer. Each request opens here for you to approve. Off, Airlock is not offered to them as a wallet."
            }
            div { class: "btn-row",
                button {
                    onclick: move |_| manage.show(),
                    "Manage connections"
                }
            }
            if let Some(err) = error() {
                p { class: "error", "{err}" }
            }
        }
    }
}

/// The first launch, before a vault is created: how Airlock is to be used
/// on this phone. Online goes on to create its vault here; offline starts
/// the app again on the offline vault's folder.
#[component]
pub fn Setup(on_done: EventHandler<()>) -> Element {
    let mut mode = use_signal(|| Mode::Online);
    let mut apps = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let go = move |_| {
        let chosen = mode();
        let saved = mode::save(chosen).and_then(|()| wallet_adapter::set_allowed(apps()));
        match (saved, chosen) {
            (Err(err), _) => error.set(Some(err)),
            (Ok(()), Mode::Offline) => {
                wallet_adapter::set_listed(false);
                restart()
            }
            (Ok(()), Mode::Online) => {
                wallet_adapter::set_listed(apps());
                on_done.call(());
            }
        }
    };
    let online = mode() == Mode::Online;
    rsx! {
        section { class: "card auth setup",
            div { class: "stack",
                h2 { "Set up Airlock" }
                p { class: "lede", "How will you use it on this phone?" }
                div { class: "segmented",
                    button {
                        class: if online { "tab on" } else { "tab" },
                        onclick: move |_| mode.set(Mode::Online),
                        "Online"
                    }
                    button {
                        class: if online { "tab" } else { "tab on" },
                        onclick: move |_| mode.set(Mode::Offline),
                        "Offline"
                    }
                }
                if online {
                    p { class: "hint",
                        "Balances, history, staking and sending, through the network."
                    }
                    label { class: "check",
                        input {
                            r#type: "checkbox",
                            checked: apps(),
                            onchange: move |e| apps.set(e.checked()),
                        }
                        "Let apps on this phone ask Airlock to connect and sign"
                    }
                    p { class: "hint",
                        "Other apps, and sites open in Chrome, find Airlock as a wallet (Mobile Wallet Adapter). You approve each request here."
                    }
                } else {
                    p { class: "hint",
                        "Airlock never asks the network anything. It signs what you scan, and shows the signatures as QR codes for another device to send. Other apps cannot reach it."
                    }
                }
                p { class: "hint",
                    "Each mode has its own vault. You can change both in Settings."
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                div { class: "btn-row",
                    button { class: "primary", onclick: go, "Continue" }
                }
            }
        }
    }
}
