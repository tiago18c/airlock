//! While the vault is locked the screens still list its wallets, from a copy
//! kept without their secrets. Choosing the wallet to act as needs no
//! password; the vault takes the choice when it is unlocked.

use app_core::storage::WALLETS_FILE;
use app_core::testing::{kit, kit_on, Kit};
use app_core::VaultState;
use solana_pubkey::Pubkey;
use wallet_core::{NewSigner, Origin, WalletId, WalletKind, WalletRecord};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// An unlocked vault with a hot key, a seed phrase wallet and an offline one.
fn with_wallets() -> (Kit, Vec<WalletId>) {
    let kit = kit();
    kit.core.create_vault("pw").unwrap();
    let hot = WalletRecord::from_keypair_seed("Hot", [7; 32]);
    let mut seed = WalletRecord::from_seed_phrase(
        "Seed",
        PHRASE,
        None,
        "m/44'/501'/0'/0'",
        Pubkey::new_unique(),
    );
    // As an older record: the address is derived from the phrase.
    if let WalletKind::Seed { pubkey, .. } = &mut seed.kind {
        *pubkey = None;
    }
    let signers: Vec<NewSigner> = [hot, seed]
        .into_iter()
        .map(|wallet| NewSigner {
            pubkey: wallet.address().unwrap(),
            label: wallet.label.clone(),
            origin: Origin::Imported,
            wallet,
        })
        .collect();
    let mut ids: Vec<WalletId> = signers.iter().map(|signer| signer.wallet.id).collect();
    kit.core
        .enroll(
            "add_signers",
            |data| data.add_signers(&signers),
            |book, _| wallet_core::name_signers(book, &signers),
        )
        .unwrap();
    let cold = kit
        .core
        .enroll(
            "add_offline_wallet",
            |data| data.add_offline_wallet("Cold", Pubkey::new_unique(), None),
            |_, _| {},
        )
        .unwrap();
    ids.push(cold);
    (kit, ids)
}

#[tokio::test]
async fn a_locked_vault_still_lists_its_wallets_and_their_addresses() {
    let (kit, ids) = with_wallets();
    let open = kit.core.slices().wallets.get();
    kit.core.lock();

    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
    let locked = kit.core.slices().wallets.get();
    assert_eq!(locked.summaries, open.summaries);
    assert_eq!(locked.local_signer_pubkeys, open.local_signer_pubkeys);
    assert_eq!(locked.selected, Some(ids[0]));
    // Nothing read while locked can sign.
    kit.core.with_wallets(|data| {
        let data = data.unwrap();
        assert_eq!(data.wallets.len(), 3);
        assert!(data.wallets.iter().all(|wallet| wallet.signer().is_err()));
    });

    // The copy outlives the app: a new start shows the wallets before any
    // password is typed.
    let again = kit_on(kit.storage.clone());
    assert_eq!(*again.core.slices().vault.get(), VaultState::Locked);
    assert_eq!(again.core.slices().wallets.get().summaries, open.summaries);
}

#[tokio::test]
async fn the_copy_on_disk_holds_no_secret() {
    let (kit, _) = with_wallets();
    let copy = String::from_utf8(kit.storage.read(WALLETS_FILE).unwrap()).unwrap();
    assert!(copy.contains("Hot") && copy.contains("Seed") && copy.contains("Cold"));
    assert!(!copy.contains("abandon"));
    let seed_bytes = serde_json::to_string(&[7u8; 32]).unwrap();
    assert!(!copy.contains(&seed_bytes[1..seed_bytes.len() - 1]));
}

#[tokio::test]
async fn a_wallet_chosen_while_locked_is_the_one_used_after_unlocking() {
    let (kit, ids) = with_wallets();
    kit.core.lock();

    kit.core.select_wallet(ids[2]).unwrap();
    assert_eq!(kit.core.slices().wallets.get().selected, Some(ids[2]));

    kit.core.unlock("pw").unwrap();
    assert_eq!(kit.core.slices().wallets.get().selected, Some(ids[2]));
    kit.core
        .with_vault(|vault| assert_eq!(vault.data().unwrap().selected, Some(ids[2])));
}

#[tokio::test]
async fn before_there_is_a_vault_there_are_no_wallets() {
    let kit = kit();
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Uninitialized);
    assert!(kit.core.slices().wallets.get().summaries.is_empty());
    kit.core.with_wallets(|data| assert!(data.is_none()));
}
