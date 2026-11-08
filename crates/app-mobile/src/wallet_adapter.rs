//! Mobile Wallet Adapter: other apps on the phone, and web pages in Chrome,
//! ask Airlock to connect and sign, as the extension does on desktop.
//!
//! `MwaBridge` in the app's `MainActivity.kt` holds each session with Solana
//! Mobile's wallet library and hands its requests over as JSON. Each goes to
//! [`Bridge`]'s app sessions (`app_desktop::bridge::AppSession`): the same
//! checks, queue and request modal as a browser's.
//!
//! Only the online app serves them, and only with "Apps on this phone" on in
//! Settings: otherwise Airlock is not listed as a wallet at all
//! ([`set_listed`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use app_desktop::bridge::{AppAnswer, AppCall, AppIdentity, AppSession, Bridge};
use serde::Deserialize;

const FILE: &str = "apps";

fn path() -> Option<PathBuf> {
    crate::files_dir().map(|dir| dir.join(FILE))
}

/// Whether apps on this phone may ask the online app to connect and sign:
/// yes unless it was turned off.
pub fn allowed() -> bool {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_none_or(|text| text.trim() != "off")
}

/// Keep the choice, for this launch and the next.
pub fn set_allowed(on: bool) -> Result<(), String> {
    let path = path().ok_or("no private files folder")?;
    std::fs::write(path, if on { "on" } else { "off" }).map_err(|err| err.to_string())
}

/// Whether other apps find Airlock as a wallet. Off, a `solana-wallet:` link
/// does not offer it, and its open sessions end.
pub fn set_listed(on: bool) {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::mwa_set_enabled(on) {
        tracing::error!(target: "airlock::bridge", %err, "could not change the wallet listing");
    }
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// One event from `MwaBridge`.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Event {
    Open {
        session: i64,
    },
    Request {
        id: i64,
        session: i64,
        identity: AppIdentity,
        call: serde_json::Value,
    },
    Closed {
        session: i64,
    },
}

/// The app sessions of each MWA session open now, by the site each request
/// names. A session's requests normally all name one.
#[derive(Default)]
struct Sessions(HashMap<i64, Vec<Arc<AppSession>>>);

impl Sessions {
    fn for_request(
        &mut self,
        bridge: &Bridge,
        session: i64,
        identity: AppIdentity,
    ) -> Arc<AppSession> {
        let open = self.0.entry(session).or_default();
        let origin = identity.origin();
        if let Some(found) = open
            .iter()
            .find(|app| app.origin() == origin.as_deref() && app.origin().is_some())
        {
            return found.clone();
        }
        let app = Arc::new(bridge.app_session(identity));
        open.push(app.clone());
        app
    }

    fn close(&mut self, session: i64) {
        for app in self.0.remove(&session).unwrap_or_default() {
            app.close();
        }
    }
}

/// Take MWA requests for as long as the app runs. Call once, after
/// `bridge` is attached to the core.
pub fn serve(bridge: Bridge) {
    std::thread::Builder::new()
        .name("mwa".into())
        .spawn(move || run(bridge))
        .map(|_| ())
        .unwrap_or_else(|err| {
            tracing::error!(target: "airlock::bridge", %err, "could not start taking app requests");
        });
}

fn run(bridge: Bridge) {
    let mut sessions = Sessions::default();
    loop {
        let Some(text) = next_event() else {
            continue;
        };
        let event = match serde_json::from_str::<Event>(&text) {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!(target: "airlock::bridge", %err, "unreadable app event");
                continue;
            }
        };
        match event {
            Event::Open { session } => {
                tracing::debug!(target: "airlock::bridge", session, "app session opened");
            }
            Event::Closed { session } => {
                tracing::debug!(target: "airlock::bridge", session, "app session closed");
                sessions.close(session);
            }
            Event::Request {
                id,
                session,
                identity,
                call,
            } => {
                let call = serde_json::from_value::<AppCall>(call);
                let app = sessions.for_request(&bridge, session, identity);
                // A request may wait for the user; the rest go on meanwhile.
                std::thread::spawn(move || {
                    let answer = match call {
                        Ok(call) => app.answer(call),
                        Err(err) => {
                            tracing::warn!(target: "airlock::bridge", %err, "unreadable app request");
                            AppAnswer::Declined
                        }
                    };
                    answer_request(id, &answer.to_json());
                });
            }
        }
    }
}

/// The next event, or `None` after a while with none.
fn next_event() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        match crate::android::mwa_next(30_000) {
            Ok(event) => event,
            Err(err) => {
                tracing::error!(target: "airlock::bridge", %err, "could not read app requests");
                std::thread::sleep(std::time::Duration::from_secs(5));
                None
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        None
    }
}

fn answer_request(id: i64, json: &str) {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::mwa_answer(id, json) {
        tracing::error!(target: "airlock::bridge", %err, "could not answer an app");
    }
    #[cfg(not(target_os = "android"))]
    let _ = (id, json);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_read_as_mwa_bridge_sends_them() {
        let event: Event = serde_json::from_str(
            r#"{"kind":"request","id":3,"session":1,"identity":{"name":"Dapp","uri":"https://dapp.example","package":null},"call":{"method":"authorize"}}"#,
        )
        .unwrap();
        let Event::Request {
            id, identity, call, ..
        } = event
        else {
            panic!("a request");
        };
        assert_eq!(id, 3);
        assert_eq!(identity.origin().as_deref(), Some("https://dapp.example"));
        assert!(matches!(
            serde_json::from_value::<AppCall>(call),
            Ok(AppCall::Authorize { sign_in: None })
        ));
        assert!(matches!(
            serde_json::from_str::<Event>(r#"{"kind":"closed","session":1}"#),
            Ok(Event::Closed { session: 1 })
        ));
    }
}
