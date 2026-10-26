//! The IDL pack: the IDLs of third-party programs, shipped beside the app as
//! one compressed file and read once. `build.rs` makes it from
//! `resources/idls` and pins its SHA-256 here; a pack that does not match is
//! refused whole, and those programs read raw.
//!
//! A desktop app reads it from its bundle while the catalog opens. A browser
//! fetches it once, after the page loads, so for a moment only the core IDLs
//! compiled into the app are there.

use crate::platform::storage::{open_idls, AppIdls};
use dioxus::prelude::*;
use wallet_core::{idls_from_pack, EmbeddedIdl, IdlBodies, IdlNetwork, IdlStore, VaultStore};

pub const IDL_PACK: Asset = asset!("/assets/idls.pack");

/// What `build.rs` packed.
const PINNED_SHA256: &str = env!("AIRLOCK_IDL_PACK_SHA256");

/// Whether the IDL pack is in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdlPackState {
    Loading,
    Ready,
    /// Why it could not be read. Its programs read raw.
    Failed(String),
}

fn pinned() -> [u8; 32] {
    hex::decode(PINNED_SHA256)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .expect("build.rs pins 32 bytes")
}

fn read_pack(bytes: Result<Vec<u8>, String>) -> Result<Vec<EmbeddedIdl>, String> {
    idls_from_pack(&bytes?, &pinned()).map_err(|err| err.to_string())
}

/// Put what the pack holds in the catalog, and say how it went.
pub fn add_pack<S: VaultStore, B: IdlBodies>(
    store: &mut IdlStore<S, B>,
    idls: Result<Vec<EmbeddedIdl>, String>,
) -> IdlPackState {
    match idls {
        Ok(idls) => {
            let refused = store.add_embedded(idls);
            if refused.is_empty() {
                IdlPackState::Ready
            } else {
                IdlPackState::Failed(format!(
                    "the pack lists programs the app already has: {}",
                    refused.join(", ")
                ))
            }
        }
        Err(err) => IdlPackState::Failed(err),
    }
}

/// The catalog on `network`, with the pack in it where it can be read now:
/// on desktop. In a browser the pack comes later, from [`fetch_pack`].
pub fn open_catalog(network: IdlNetwork) -> (AppIdls, IdlPackState) {
    let store = open_idls(network);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut store = store;
        let state = add_pack(&mut store, read_bundled_pack());
        (store, state)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (store, IdlPackState::Loading)
    }
}

/// Read the pack from the app's bundle, or from the source tree when the
/// app runs without one.
#[cfg(not(target_arch = "wasm32"))]
fn read_bundled_pack() -> Result<Vec<EmbeddedIdl>, String> {
    let bytes = dioxus::asset_resolver::asset_path(IDL_PACK)
        .map_err(|err| err.to_string())
        .and_then(|path| std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display())));
    read_pack(bytes)
}

/// Fetch the pack in a browser, where it is a file of the page.
pub async fn fetch_pack() -> Result<Vec<EmbeddedIdl>, String> {
    let bytes = dioxus::asset_resolver::read_asset_bytes(&IDL_PACK)
        .await
        .map_err(|err| err.to_string());
    read_pack(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::MemoryBodies;
    use wallet_vault::store::MemoryStore;

    fn memory_catalog(network: IdlNetwork) -> IdlStore<MemoryStore, MemoryBodies> {
        IdlStore::open_on(MemoryStore::new(), MemoryBodies::new(), network).with_builtins()
    }

    /// The pack this build ships reads back whole, against the pin.
    #[test]
    fn the_shipped_pack_matches_its_pin_and_adds_its_programs() {
        let mut store = memory_catalog(IdlNetwork::Mainnet);
        let state = add_pack(&mut store, read_bundled_pack());
        assert_eq!(state, IdlPackState::Ready);
        let whirlpool: solana_pubkey::Pubkey = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
            .parse()
            .unwrap();
        assert!(store.embedded(&whirlpool).is_some());
        assert!(store.program_client(&whirlpool).is_some());
    }

    #[test]
    fn a_pack_with_other_bytes_is_refused_and_says_why() {
        let mut store = memory_catalog(IdlNetwork::Devnet);
        let state = add_pack(&mut store, read_pack(Ok(b"not the pack".to_vec())));
        assert!(matches!(state, IdlPackState::Failed(why) if why.contains("SHA-256")));
        let state = add_pack(&mut store, read_pack(Err("missing".into())));
        assert_eq!(state, IdlPackState::Failed("missing".into()));
    }
}
