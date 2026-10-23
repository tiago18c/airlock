//! Adding a wallet: the vault keeps it and the address book names it, or
//! neither does.

use crate::book::address_book::AddressBook;
use crate::book::workspace::WorkspaceStore;
use crate::error::VaultError;
use crate::wallets::roster::RosterError;
use crate::wallets::vault::{Vault, VaultData};
use wallet_vault::VaultStore;

/// Why a wallet was not stored.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("Unlock the vault first.")]
    Locked,
    #[error(transparent)]
    Refused(#[from] RosterError),
    #[error(transparent)]
    Save(#[from] VaultError),
}

/// Add to the vault what `add` decides, then let `name` write it in the
/// address book. A refusal saves nothing. When the address book cannot be
/// saved, the vault is put back as it was.
pub fn enroll<V: VaultStore, W: VaultStore, T>(
    vault: &mut Vault<V>,
    workspace: &mut WorkspaceStore<W>,
    add: impl FnOnce(&mut VaultData) -> Result<T, RosterError>,
    name: impl FnOnce(&mut AddressBook, &T),
) -> Result<T, EnrollError> {
    let previous = vault.data().map_err(|_| EnrollError::Locked)?.clone();
    let mut next = previous.clone();
    let added = add(&mut next)?;
    vault.change(|data| *data = next)?;
    if let Err(err) = workspace.change(|ws| name(&mut ws.book, &added)) {
        let undone = vault.change(|data| *data = previous);
        return Err(EnrollError::Save(match undone {
            Ok(()) => err,
            Err(undo) => VaultError::Io(format!(
                "{err}. The wallet could not be taken out of the vault again: {undo}"
            )),
        }));
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::address_book::Origin;
    use crate::fixtures::pk;
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::wallets::import::NewSigner;
    use crate::wallets::record::{Authority, WalletKind, WalletRecord};
    use crate::wallets::roster::{name_offline_wallet, name_signers};
    use std::sync::Arc;
    use wallet_vault::MemoryStore;

    struct Device {
        vault_file: Arc<MemoryStore>,
        book_file: Arc<MemoryStore>,
        vault: Vault<Arc<MemoryStore>>,
        workspace: WorkspaceStore<Arc<MemoryStore>>,
    }

    impl Device {
        fn new() -> Self {
            let vault_file = Arc::new(MemoryStore::new());
            let book_file = Arc::new(MemoryStore::new());
            let mut vault = Vault::new(vault_file.clone());
            vault.create("pw").unwrap();
            Self {
                workspace: WorkspaceStore::open(book_file.clone()),
                vault,
                vault_file,
                book_file,
            }
        }

        fn add_signers(&mut self, signers: &[NewSigner]) -> Result<(), EnrollError> {
            enroll(
                &mut self.vault,
                &mut self.workspace,
                |data| data.add_signers(signers),
                |book, _| name_signers(book, signers),
            )
        }

        fn add_offline(&mut self, pubkey: solana_pubkey::Pubkey) -> Result<(), EnrollError> {
            enroll(
                &mut self.vault,
                &mut self.workspace,
                |data| data.add_offline_wallet("Cold", pubkey, None),
                |book, _| name_offline_wallet(book, pubkey, "Cold"),
            )
            .map(|_| ())
        }

        /// What a restart would find: both files read again.
        fn stored(&self) -> (VaultData, WorkspaceStore<Arc<MemoryStore>>) {
            let mut vault: Vault<_> = Vault::new(self.vault_file.clone());
            vault.unlock("pw").unwrap();
            (
                vault.data().unwrap().clone(),
                WorkspaceStore::open(self.book_file.clone()),
            )
        }
    }

    fn hot(label: &str) -> NewSigner {
        let key = KeypairSigner::new_random();
        NewSigner {
            wallet: WalletRecord::from_keypair_seed(label, key.seed_bytes()),
            pubkey: key.pubkey(),
            label: label.to_string(),
            origin: Origin::Imported,
        }
    }

    #[test]
    fn a_stored_wallet_is_in_both_files() {
        let mut device = Device::new();
        let a = hot("A");
        device.add_signers(std::slice::from_ref(&a)).unwrap();

        let (vault, workspace) = device.stored();
        assert_eq!(vault.wallets.len(), 1);
        let entry = workspace.data().book.get(&a.pubkey).unwrap();
        assert_eq!(entry.wallet_id, Some(a.wallet.id));
    }

    #[test]
    fn a_locked_vault_and_a_refused_wallet_change_neither_file() {
        let mut device = Device::new();
        let a = hot("A");
        device.add_signers(std::slice::from_ref(&a)).unwrap();

        // A save from here on would come back as a save error, not as the refusal.
        device.vault_file.refuse_saves(true);
        device.book_file.refuse_saves(true);
        let again = device.add_signers(std::slice::from_ref(&a)).unwrap_err();
        assert!(matches!(
            again,
            EnrollError::Refused(RosterError::Duplicate(_))
        ));

        device.vault.lock();
        let locked = device.add_signers(&[hot("B")]).unwrap_err();
        assert!(matches!(locked, EnrollError::Locked));
    }

    #[test]
    fn when_the_address_book_cannot_be_saved_the_vault_is_put_back() {
        let mut device = Device::new();
        let waiting = WalletRecord::programmatic("Waiting", pk(1), None);
        device
            .vault
            .change(|data| data.wallets.push(waiting))
            .unwrap();

        device.book_file.refuse_saves(true);
        let err = device.add_offline(pk(1)).unwrap_err();
        assert!(matches!(err, EnrollError::Save(_)), "{err}");

        // The offline wallet is gone, and so is the tie the signer had to it.
        let (stored, workspace) = device.stored();
        for data in [&stored, device.vault.data().unwrap()] {
            assert_eq!(data.wallets.len(), 1);
            assert!(matches!(
                data.wallets[0].kind,
                WalletKind::Programmatic {
                    authority: Authority::Outside,
                    ..
                }
            ));
        }
        assert!(workspace.data().book.get(&pk(1)).is_none());
        assert!(device.workspace.data().book.get(&pk(1)).is_none());
    }

    #[test]
    fn when_the_vault_cannot_be_saved_the_address_book_is_not_touched() {
        let mut device = Device::new();
        device.vault_file.refuse_saves(true);
        let a = hot("A");
        let err = device.add_signers(std::slice::from_ref(&a)).unwrap_err();
        assert!(matches!(err, EnrollError::Save(_)), "{err}");
        assert!(device.vault.data().unwrap().wallets.is_empty());
        assert!(device.workspace.data().book.get(&a.pubkey).is_none());
    }

    #[test]
    fn a_wallet_that_cannot_be_taken_out_again_is_reported_and_stays_in_memory_as_stored() {
        let mut device = Device::new();
        let a = hot("A");
        device.book_file.refuse_saves(true);
        // The vault takes the first save and refuses the one that undoes it.
        let vault_file = device.vault_file.clone();
        let err = enroll(
            &mut device.vault,
            &mut device.workspace,
            |data| data.add_signers(std::slice::from_ref(&a)),
            |_, _| vault_file.refuse_saves(true),
        )
        .unwrap_err();
        assert!(err.to_string().contains("could not be taken out"), "{err}");

        device.vault_file.refuse_saves(false);
        let (stored, _) = device.stored();
        assert_eq!(stored.wallets.len(), 1);
        assert_eq!(device.vault.data().unwrap().wallets.len(), 1);
    }
}
