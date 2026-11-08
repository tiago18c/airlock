//! Fingerprint unlock, where the shell has a [`KeyGuard`]: the vault's key,
//! kept by the platform's secure hardware behind the user's fingerprint.
//! Only a phone provides one; elsewhere every call here finds none.

use std::sync::Arc;

use app_core::{KeyGuard, KeyGuardError};
use dioxus::prelude::*;
use wallet_core::VaultError;

use crate::state::session::Session;

/// What a shell provides as context to offer fingerprint unlock.
#[derive(Clone)]
pub struct KeyGuardHandle(pub Arc<dyn KeyGuard>);

/// The shell's key guard, if it has one.
pub fn use_key_guard() -> Option<Arc<dyn KeyGuard>> {
    try_use_context::<KeyGuardHandle>().map(|handle| handle.0)
}

/// What fingerprint unlock says when it gave nothing: nothing when the user
/// backed out, a sentence otherwise.
pub fn guard_said(error: &KeyGuardError) -> Option<String> {
    match error {
        KeyGuardError::Declined => None,
        other => Some(other.to_string()),
    }
}

/// Open the vault with the key the guard keeps, once the user's fingerprint
/// matches. A key that no longer opens it (a new password, a raised cost)
/// is forgotten, and the answer says to use the password.
pub async fn unlock_with_fingerprint(
    session: Session,
    guard: Arc<dyn KeyGuard>,
) -> Result<(), Option<String>> {
    let key = guard.open().await.map_err(|err| guard_said(&err))?;
    match session.unlock_with_key(key).await {
        Ok(()) => Ok(()),
        Err(VaultError::UnlockFailed) => {
            guard.forget();
            Err(Some(
                "The vault's key changed since fingerprint unlock was turned on, so it is off now. Unlock with the password, then turn it on again in Settings.".into(),
            ))
        }
        Err(err) => Err(Some(err.to_string())),
    }
}

/// Turn fingerprint unlock on: seal the unlocked vault's key behind the
/// user's fingerprint, asked for now.
pub async fn turn_on(session: Session, guard: Arc<dyn KeyGuard>) -> Result<(), Option<String>> {
    let key = session
        .vault_key_bytes()
        .map_err(|err| Some(err.to_string()))?;
    guard.enroll(key).await.map_err(|err| guard_said(&err))
}
