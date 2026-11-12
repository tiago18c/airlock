//! `localStorage` in a browser.

use std::sync::atomic::{AtomicBool, Ordering};

use js_sys::{Function, Object, Promise, Reflect};
use solana_pubkey::Pubkey;
use wallet_core::{IdlBodies, IdlNetwork, VaultError, VaultStore};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};

use super::Storage;

/// The Web Lock held for the life of the tab that uses the app's data.
const DATA_LOCK: &str = "airlock.data";

/// Whether this tab holds [`DATA_LOCK`].
static HELD: AtomicBool = AtomicBool::new(false);

/// Why this tab may not use the app's data.
#[derive(Debug)]
pub enum WebDataClaim {
    /// Another tab or window of the app has it open.
    InUse,
    /// The browser has no Web Locks, so there is no telling.
    Unsupported(String),
}

impl std::fmt::Display for WebDataClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse => f.write_str(
                "Airlock is already open in another tab or window of this browser. Only one may run at a time: two would each keep their own copy of the vault and of the record of used signing leaves, and could sign with one leaf twice. Close the other one and reload this page.",
            ),
            Self::Unsupported(why) => write!(
                f,
                "This browser cannot tell whether Airlock is open in another tab ({why}), so Airlock does not open its data here: two tabs would undo each other's saves. Use a current browser, over https or on localhost."
            ),
        }
    }
}

impl std::error::Error for WebDataClaim {}

/// Take the app's data for this tab, the browser's counterpart of
/// `claim_data_dir`. Every store keeps its value in memory and writes it
/// whole, and tabs share `localStorage`, so two tabs would undo each
/// other's saves, and a mark that only rises could go down. It asks for an
/// exclusive Web Lock without waiting and keeps it for the tab's life.
/// Call it before any store opens; [`platform_storage`](super::platform_storage)
/// refuses to open one without it. Taking it again in the same tab does
/// nothing.
pub async fn claim_web_data() -> Result<(), WebDataClaim> {
    if HELD.load(Ordering::SeqCst) {
        return Ok(());
    }
    let unsupported = |why: &str| WebDataClaim::Unsupported(why.to_string());
    let navigator = Reflect::get(&js_sys::global(), &"navigator".into())
        .map_err(|_| unsupported("no navigator"))?;
    let locks = Reflect::get(&navigator, &"locks".into())
        .ok()
        .filter(|locks| locks.is_object())
        .ok_or_else(|| unsupported("no navigator.locks"))?;
    let request: Function = Reflect::get(&locks, &"request".into())
        .ok()
        .and_then(|request| request.dyn_into().ok())
        .ok_or_else(|| unsupported("no navigator.locks.request"))?;

    // Settled with whether the lock was granted, or with why asking failed.
    let mut settle = None;
    let answered = Promise::new(&mut |resolve, _| settle = Some(resolve));
    let settle = settle.expect("a promise runs its executor at once");
    let on_grant = settle.clone();
    let callback = Closure::once_into_js(move |lock: JsValue| -> JsValue {
        let granted = !lock.is_null() && !lock.is_undefined();
        let _ = on_grant.call1(&JsValue::NULL, &JsValue::from_bool(granted));
        if granted {
            // Never settles, so the lock is held until the tab goes away.
            Promise::new(&mut |_, _| {}).into()
        } else {
            JsValue::UNDEFINED
        }
    });
    let options = Object::new();
    Reflect::set(&options, &"ifAvailable".into(), &JsValue::TRUE)
        .map_err(|_| unsupported("could not ask for the lock"))?;
    let asked = request
        .call3(&locks, &DATA_LOCK.into(), &options, &callback)
        .map_err(|err| unsupported(&format!("{err:?}")))?;
    // A request the browser refuses outright never calls back.
    if let Ok(asked) = asked.dyn_into::<Promise>() {
        let on_refusal = Closure::once(move |err: JsValue| {
            let why = err
                .as_string()
                .or_else(|| {
                    Reflect::get(&err, &"message".into())
                        .ok()
                        .and_then(|m| m.as_string())
                })
                .unwrap_or_else(|| "the request was refused".into());
            let _ = settle.call1(&JsValue::NULL, &JsValue::from_str(&why));
        });
        let _ = asked.catch(&on_refusal);
        // Called at most once, some time later: it has to outlive this call.
        on_refusal.forget();
    }

    let answer = wasm_bindgen_futures::JsFuture::from(answered)
        .await
        .map_err(|_| unsupported("no answer"))?;
    match answer.as_bool() {
        Some(true) => {
            HELD.store(true, Ordering::SeqCst);
            tracing::info!(target: "airlock::app", lock = DATA_LOCK, "holding the app's data for this tab");
            Ok(())
        }
        Some(false) => Err(WebDataClaim::InUse),
        None => Err(WebDataClaim::Unsupported(
            answer.as_string().unwrap_or_default(),
        )),
    }
}

/// Stop unless this tab holds the app's data: see [`claim_web_data`].
pub(super) fn held_web_data() {
    if !HELD.load(Ordering::SeqCst) {
        panic!("the app's data is opened only after claim_web_data, which this tab has not got");
    }
}

pub struct LocalStorage;

impl Storage for LocalStorage {
    fn open(&self, name: &str) -> Box<dyn VaultStore> {
        Box::new(LocalStorageStore::new(format!("airlock.{name}")))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, VaultError> {
        let storage = LocalStorageStore::storage()?;
        let count = storage
            .length()
            .map_err(|err| VaultError::Io(format!("localStorage read: {err:?}")))?;
        let start = format!("airlock.{prefix}");
        let mut names = Vec::new();
        for index in 0..count {
            let key = storage
                .key(index)
                .map_err(|err| VaultError::Io(format!("localStorage read: {err:?}")))?;
            if let Some(name) = key.as_deref().and_then(|key| key.strip_prefix("airlock.")) {
                if key.as_deref().is_some_and(|key| key.starts_with(&start)) {
                    names.push(name.to_string());
                }
            }
        }
        Ok(names)
    }

    fn remove(&self, name: &str) -> Result<(), VaultError> {
        LocalStorageStore::new(format!("airlock.{name}")).remove()
    }
}

/// One `localStorage` key. Everything the app keeps is JSON, the vault too,
/// so it is stored as the text it is and can be read in the browser's tools.
struct LocalStorageStore {
    key: String,
}

impl LocalStorageStore {
    fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    fn storage() -> Result<web_sys::Storage, VaultError> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or_else(|| VaultError::Io("localStorage is not available".into()))
    }

    fn remove(&self) -> Result<(), VaultError> {
        Self::storage()?
            .remove_item(&self.key)
            .map_err(|err| VaultError::Io(format!("localStorage remove: {err:?}")))
    }
}

impl VaultStore for LocalStorageStore {
    fn load(&self) -> Result<Option<Vec<u8>>, VaultError> {
        match Self::storage()?.get_item(&self.key) {
            Ok(Some(text)) => Ok(Some(text.into_bytes())),
            Ok(None) => Ok(None),
            Err(err) => Err(VaultError::Io(format!("localStorage read: {err:?}"))),
        }
    }

    fn save(&self, bytes: &[u8]) -> Result<(), VaultError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| VaultError::Io("localStorage holds text, and this is not".into()))?;
        Self::storage()?
            .set_item(&self.key, text)
            .map_err(|e| VaultError::Io(format!("localStorage write: {e:?}")))
    }

    fn set_aside(&self, bytes: &[u8]) -> Result<(), VaultError> {
        LocalStorageStore::new(format!("{}.unreadable", self.key)).save(bytes)
    }
}

/// `airlock.idl.<network>.<program id>`. Files from before IDLs were kept
/// per network are `airlock.idl.<program id>`.
pub struct WebIdlBodies;

impl WebIdlBodies {
    fn store(network: IdlNetwork, program_id: &Pubkey) -> LocalStorageStore {
        LocalStorageStore::new(format!("airlock.idl.{network}.{program_id}"))
    }

    fn unscoped(program_id: &Pubkey) -> LocalStorageStore {
        LocalStorageStore::new(format!("airlock.idl.{program_id}"))
    }
}

impl IdlBodies for WebIdlBodies {
    fn load(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        Self::store(network, program_id).load()
    }

    fn save(
        &self,
        network: IdlNetwork,
        program_id: &Pubkey,
        bytes: &[u8],
    ) -> Result<(), VaultError> {
        Self::store(network, program_id).save(bytes)
    }

    fn remove(&self, network: IdlNetwork, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::store(network, program_id).remove()
    }

    fn load_unscoped(&self, program_id: &Pubkey) -> Result<Option<Vec<u8>>, VaultError> {
        Self::unscoped(program_id).load()
    }

    fn remove_unscoped(&self, program_id: &Pubkey) -> Result<(), VaultError> {
        Self::unscoped(program_id).remove()
    }
}
