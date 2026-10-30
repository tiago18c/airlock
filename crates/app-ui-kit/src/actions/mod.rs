//! What a screen asks the app to do.
//!
//! An action reads what it needs from the [`Session`], runs a use case from
//! `wallet-flows`, and puts the result where the screens look for it. The
//! network monitor, submitter and online-only panes live in `app-online`.

pub mod addresses;
pub mod build;
pub mod idl_check;
pub mod network;
pub mod programs;
pub mod sign;
pub mod sign_screen;
pub mod site_send;
pub mod wallets;

/// Run `fut` for the whole life of the app. It runs in `App`'s scope, which
/// owns the session signals, rather than `spawn_forever`'s `ScopeId::ROOT`
/// above it, where every signal access trips Dioxus's hoisted-value warning.
pub fn spawn_app_task(fut: impl std::future::Future<Output = ()> + 'static) -> dioxus::core::Task {
    dioxus::core::Runtime::current().in_scope(dioxus::prelude::ScopeId::APP, || {
        dioxus::prelude::spawn(fut)
    })
}

/// An error fragment from the core as a sentence.
pub fn sentence(err: impl ToString) -> String {
    let text = err.to_string();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return text,
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Seconds since the Unix epoch, by this device's clock.
pub fn unix_now() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
}
