//! Android services for `apps/airlock-mobile`: the app's private folder,
//! the WebView's settings, opening links, whether the app runs online or
//! offline, and reporting to the core when the app is out of sight.
//!
//! Everything that calls into Android goes through [`android`], over JNI.
//! On any other OS this crate builds, so the workspace checks it, but its
//! Android calls answer that there is no Android.

#[cfg(target_os = "android")]
#[allow(unsafe_code)] // JNI: the VM and activity pointers ndk-context hands over.
mod android;

pub mod mode;
pub mod seed_vault;

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
            Ok(()) => restart(),
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
