//! A vault's file that cannot be read, or that turns out damaged, is never
//! written over.

use app_core::storage::VAULT_FILE;
use app_core::testing::{kit, kit_on, settle, MemoryStorage};
use app_core::{notice_row_text, NoticeKind, VaultState};
use wallet_core::{VaultError, WalletRecord};

/// A vault under the password `pw` with one wallet, on `storage`.
async fn stored() -> MemoryStorage {
    let kit = kit();
    kit.core.create_vault("pw").unwrap();
    kit.core
        .vault_checked("add", |data| {
            data.wallets
                .push(WalletRecord::from_keypair_seed("Main", [7; 32]));
        })
        .unwrap();
    settle().await;
    kit.storage.clone()
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_not_offered_as_a_new_vault() {
    let storage = stored().await;
    let sealed = storage.read(VAULT_FILE).unwrap();
    storage.refuse_reads(true);

    let kit = kit_on(storage.clone());
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unreadable);
    let vault_problem = || {
        kit.core
            .load_problems()
            .into_iter()
            .any(|problem| problem.contains("vault's file"))
    };
    assert!(vault_problem());

    assert!(matches!(
        kit.core.create_vault("new"),
        Err(VaultError::Io(_))
    ));
    assert!(matches!(kit.core.unlock("pw"), Err(VaultError::Io(_))));
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unreadable);

    // The file comes back, and it was never touched.
    storage.refuse_reads(false);
    assert_eq!(storage.read(VAULT_FILE).unwrap(), sealed);
    assert!(matches!(
        kit.core.unlock("nope"),
        Err(VaultError::UnlockFailed)
    ));
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
    assert!(!vault_problem());
    kit.core.unlock("pw").unwrap();
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);
    assert_eq!(kit.core.slices().wallets.get().summaries.len(), 1);
}

#[tokio::test]
async fn a_new_vault_is_not_made_over_one_that_appeared_after_the_app_opened() {
    let storage = stored().await;
    let sealed = storage.read(VAULT_FILE).unwrap();
    let empty = MemoryStorage::default();
    let kit = kit_on(empty.clone());
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Uninitialized);

    empty.put(VAULT_FILE, &sealed);
    assert!(matches!(
        kit.core.create_vault("new"),
        Err(VaultError::AlreadyExists)
    ));
    assert_eq!(empty.read(VAULT_FILE).unwrap(), sealed);
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);
    kit.core.unlock("pw").unwrap();
}

#[tokio::test]
async fn a_damaged_file_opens_from_its_backup_and_says_so() {
    let storage = stored().await;
    storage.put(VAULT_FILE, b"not a sealed file");
    let kit = kit_on(storage.clone());
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Locked);

    assert!(matches!(
        kit.core.unlock("nope"),
        Err(VaultError::UnlockFailed)
    ));
    assert_eq!(storage.read(VAULT_FILE).unwrap(), b"not a sealed file");

    kit.core.unlock("pw").unwrap();
    assert_eq!(*kit.core.slices().vault.get(), VaultState::Unlocked);
    let notices = kit.core.slices().notices.get();
    let notice = &notices.items()[0];
    assert_eq!(notice.kind, NoticeKind::VaultRecovered);
    assert!(notice_row_text(notice).contains("damaged"));
    assert_eq!(
        storage.read(&format!("{VAULT_FILE}.unreadable")).as_deref(),
        Some(&b"not a sealed file"[..])
    );
    // The file was written whole again.
    assert!(storage.read(VAULT_FILE).unwrap().starts_with(b"{"));
}
