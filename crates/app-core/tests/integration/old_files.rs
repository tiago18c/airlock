//! Files written by the app before `app-core` existed open unchanged.
//!
//! `fixtures/` was written by `cargo run --example seed_data` on the commit
//! before this crate (password `fixture-password`): 20 wallets, a workspace with
//! 200 contacts and a nonce, and a holdings file. The vault was sealed again
//! when it became JSON (format 3), with the same wallets.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use app_core::runtime::{TokioClock, TokioSpawner};
use app_core::storage::FileStorage;
use app_core::{Core, NoLinks, NoNotifier, Ports, VaultState};
use wallet_core::{AddressKind, Cluster};

const FILES: [&str; 3] = ["vault.json", "workspace.json", "holdings.json"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[tokio::test]
async fn old_files_load() {
    // Work on a copy, so a write would show instead of changing the fixtures.
    let dir = tempfile::tempdir().unwrap();
    for name in FILES {
        std::fs::copy(fixtures().join(name), dir.path().join(name)).unwrap();
    }
    let core = Core::open(Ports {
        spawner: Arc::new(TokioSpawner::current()),
        clock: Arc::new(TokioClock),
        notifier: Arc::new(NoNotifier),
        storage: Arc::new(FileStorage::new(dir.path())),
        links: Arc::new(NoLinks),
    });
    let slices = core.slices();

    assert_eq!(*slices.vault.get(), VaultState::Locked);
    assert!(
        slices.wallets.get().summaries.is_empty(),
        "locked shows no wallets"
    );

    let network = slices.network.get();
    assert_eq!(network.cluster, Cluster::Devnet);
    assert!(slices.settings.get().vector_wallets);
    let contacts = slices
        .book
        .get()
        .iter()
        .filter(|entry| entry.kind == AddressKind::Account)
        .count();
    assert!(contacts >= 200, "{contacts} contacts");
    let nonces = slices
        .book
        .get()
        .iter()
        .filter(|entry| entry.kind == AddressKind::NonceAccount)
        .count();
    assert_eq!(nonces, 1);

    assert!(core.unlock("wrong").is_err());
    core.unlock("fixture-password").unwrap();
    assert_eq!(*slices.vault.get(), VaultState::Unlocked);
    let wallets = slices.wallets.get();
    assert_eq!(wallets.summaries.len(), 20);
    let holdings = slices.holdings.get();
    assert!(
        wallets
            .local_signer_pubkeys
            .iter()
            .any(|owner| holdings.summary(owner).total > 0),
        "the first hot wallet has token accounts"
    );

    core.lock();
    // Locked, the wallets are still listed, from the copy the unlock wrote.
    assert_eq!(slices.wallets.get().summaries, wallets.summaries);
    core.flush();
    for name in FILES {
        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            std::fs::read(fixtures().join(name)).unwrap(),
            "{name} was rewritten"
        );
    }
    let mut left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    // Locking pushes a VaultLocked notice (phase 3), and unlocking copies the
    // wallets without their keys; the three fixtures stay byte-identical.
    assert_eq!(
        left,
        [
            "holdings.json",
            "notices.json",
            "vault.json",
            "wallets.json",
            "workspace.json"
        ]
    );
}
