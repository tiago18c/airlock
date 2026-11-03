//! The wallets in the vault: listing them, adding and removing them, and how
//! each one signs.

use super::*;
use crate::state::forms::{GeneratedKey, MadeHere};
use wallet_core::vector::RotationError;

impl Session {
    /// Accounts this device can act as: the ones it holds a key for, and its
    /// programmatic signers and Squads vaults. Before there is a vault the
    /// list comes from the address-book entries tied to a wallet.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn shareable_accounts(&self) -> Vec<wallet_core::ShareableAccount> {
        self.track_wallets();
        let core = self.core();
        // Wallets first, then workspace: the order the core takes them in.
        core.with_wallets(|data| {
            core.with_workspace(|workspace| wallet_core::shareable_accounts(data, workspace))
        })
    }

    /// Whether the screens offer Vector wallets. Settings turns them on.
    pub fn vector_offered(&self) -> bool {
        let _ = self.settings.read();
        self.core().with_workspace(wallet_core::vector::offered)
    }

    /// Whether `tx` asks a programmatic signer or a Squads vault of this
    /// vault to sign, and how to rebuild it if so. Before there is a vault
    /// there are no wallets, so it finds nothing.
    pub fn plan_adaptation(
        &self,
        tx: &wallet_core::DecodedTx,
    ) -> Result<Option<wallet_core::adapt::AdaptPlan>, wallet_core::adapt::AdaptError> {
        self.plan_adaptation_allowing(tx, &[])
    }

    /// [`Self::plan_adaptation`], also taking `one_time` for keys made here
    /// in place of a site's one-time keys, besides those already made.
    pub fn plan_adaptation_allowing(
        &self,
        tx: &wallet_core::DecodedTx,
        one_time: &[Pubkey],
    ) -> Result<Option<wallet_core::adapt::AdaptPlan>, wallet_core::adapt::AdaptError> {
        let offered = self.vector_offered();
        let mut made_here = self.made_here_pubkeys();
        made_here.extend_from_slice(one_time);
        self.core().with_wallets(|data| {
            let Some(data) = data else {
                return Ok(None);
            };
            // The acting wallet pays when the wallet asked to sign names nobody.
            wallet_core::adapt::plan_adaptation_with(
                tx,
                &data.wallets,
                data.chosen_wallet(),
                &made_here,
            )?
            .map(|plan| plan.unless_vector_off(offered))
            .transpose()
        })
    }

    /// Keep each key of `keys`, made in place of the site's one-time key it
    /// is paired with, as a key of this device until the app closes.
    pub fn keep_made_here<'k>(
        &self,
        keys: impl IntoIterator<Item = (Pubkey, &'k wallet_core::KeypairSigner)>,
    ) {
        let mut made_here = self.forms.sign.made_here;
        made_here
            .write()
            .extend(keys.into_iter().map(|(replaces, key)| MadeHere {
                replaces,
                key: GeneratedKey::from_signer(key),
            }));
    }

    fn made_here_pubkeys(&self) -> Vec<Pubkey> {
        self.forms
            .sign
            .made_here
            .read()
            .iter()
            .map(|made| made.key.pubkey)
            .collect()
    }

    /// The wallets that vote for `multisig` from this device, acting member
    /// first, and the durable nonce its row names for approving together.
    pub fn chosen_members(
        &self,
        multisig: WalletId,
        state: &wallet_core::squads::SquadState,
    ) -> (Vec<wallet_core::proposals::ChosenMember>, Option<Pubkey>) {
        self.track_wallets();
        self.core().with_wallets(|data| {
            let Some(data) = data else {
                return (Vec::new(), None);
            };
            let nonce = data
                .wallets
                .iter()
                .find(|wallet| wallet.id == multisig)
                .and_then(|wallet| match &wallet.kind {
                    WalletKind::Squads { approval_nonce, .. } => *approval_nonce,
                    _ => None,
                });
            let chosen = wallet_core::proposals::chosen_members(&data.wallets, multisig, state);
            (chosen, nonce)
        })
    }

    /// Whether the fee payer `tx` names could be replaced by a hot wallet
    /// of this vault: the one set for the wallet asked to sign, or the one
    /// the app is acting as.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn plan_payer_swap(
        &self,
        tx: &wallet_core::DecodedTx,
    ) -> Option<wallet_core::adapt::PayerSwap> {
        self.track_wallets();
        self.core().with_wallets(|data| {
            let data = data?;
            wallet_core::adapt::plan_payer_swap(tx, &data.wallets, data.chosen_wallet())
        })
    }

    /// Store what `add` decides in the vault and let `name` write it in the
    /// address book. Nothing is kept if either save fails.
    fn enroll<T>(
        &self,
        command: &'static str,
        add: impl FnOnce(&mut VaultData) -> Result<T, RosterError>,
        name: impl FnOnce(&mut AddressBook, &T),
    ) -> Result<T, EnrollError> {
        let stored = self.command(|core| core.enroll(command, add, name));
        let mut shown = self.save_error;
        match &stored {
            Ok(_) => shown.set(None),
            Err(EnrollError::Save(err)) => shown.set(Some(format!("Could not save: {err}"))),
            Err(_) => {}
        }
        stored
    }

    /// Store several signers with one vault save and one address-book save.
    pub fn add_signers(&self, signers: Vec<NewSigner>) -> Result<(), EnrollError> {
        self.enroll(
            "add_signers",
            |data| data.add_signers(&signers),
            |book, _| wallet_core::name_signers(book, &signers),
        )
    }

    /// Addresses this device can sign for: the vault's keys, which sign once
    /// it is unlocked, and the keypairs Build made for this run of the app.
    pub fn local_signer_pubkeys(&self) -> Vec<Pubkey> {
        let mut keys = (self.wallets)().local_signer_pubkeys.clone();
        keys.extend(
            self.forms
                .build
                .generated
                .read()
                .iter()
                .map(|key| key.pubkey),
        );
        keys.extend(self.made_here_pubkeys());
        keys
    }

    /// Every wallet address this device knows, including offline ones.
    pub fn known_wallet_pubkeys(&self) -> Vec<Pubkey> {
        self.track_wallets();
        self.core().with_wallets(|data| {
            data.map(|data| {
                data.wallets
                    .iter()
                    .filter_map(|wallet| wallet.address())
                    .collect()
            })
            .unwrap_or_default()
        })
    }

    /// A signer for every key the unlocked vault holds, and for each keypair
    /// Build made.
    pub fn local_signers(&self) -> Vec<KeypairSigner> {
        self.track_wallets();
        let generated = self.forms.build.generated.read().clone();
        let mut signers: Vec<KeypairSigner> = self.core().with_vault(|vault| {
            Vault::data(vault)
                .map(|data| {
                    data.wallets
                        .iter()
                        .filter_map(|wallet| wallet.signer().ok())
                        .collect()
                })
                .unwrap_or_default()
        });
        signers.extend(generated.iter().map(GeneratedKey::signer));
        let made_here = self.forms.sign.made_here.read().clone();
        signers.extend(made_here.iter().map(|made| made.key.signer()));
        signers
    }

    /// Vector accounts whose key this device holds, which sign once the vault
    /// is unlocked. Empty for a scheme this build cannot sign with. No key is built,
    /// so a screen may ask on every render.
    pub fn local_vector_accounts(&self) -> Vec<Pubkey> {
        (self.wallets)().local_vector_accounts.clone()
    }

    /// The Vector keys of this device for one press of Sign over `wire`, with
    /// what each hash-based key signs with recorded first. `signers` sign in
    /// the same press. `known` is what is known of the accounts: their
    /// nonces, keys and leaf floors, and `floors` where the floors came from.
    pub fn vector_keys_for_sign(
        &self,
        wire: &[u8],
        signers: &[&dyn wallet_core::WalletSigner],
        known: &[wallet_core::relay::VectorContext],
        floors: wallet_core::vector::FloorsFrom,
    ) -> Result<Vec<wallet_core::vector::VectorKey>, wallet_core::SignError> {
        let mut marks = self.leaf_marks;
        let keys = self.command(|core| {
            core.with_vault_mut(|vault| {
                wallet_core::vector::keys_to_sign_with(
                    vault,
                    &mut *marks.write(),
                    &self.xmss_retries.peek(),
                    wire,
                    signers,
                    known,
                    floors,
                )
            })
        });
        keys.map_err(|err| wallet_core::SignError::Message(err.to_string()))
    }

    /// Sign the move of `id`'s XMSS account to its next key, here and with
    /// no node, at each nonce its own signatures may have left it at. The
    /// next key is made first when there is none.
    pub fn sign_rotation(
        &self,
        id: WalletId,
    ) -> Result<wallet_core::protocol::KeyRotation, String> {
        let mut marks = self.leaf_marks;
        self.command(|core| {
            core.with_vault_mut(|vault| {
                wallet_core::vector::sign_rotation(vault, &mut *marks.write(), id)
            })
        })
        .map_err(|err| err.to_string())
    }

    /// Take what the online device read of an account whose key is kept
    /// here: where its nonce is, its lowest leaf, and the key it holds,
    /// which for a wallet made from a recovery phrase is found among the
    /// phrase's keys.
    pub fn adopt_account_state(
        &self,
        state: wallet_core::protocol::AccountState,
    ) -> Result<wallet_core::vector::Adopted, String> {
        let mut marks = self.leaf_marks;
        self.command(|core| {
            core.with_vault_mut(|vault| {
                wallet_core::vector::adopt_account_state(vault, &mut *marks.write(), &state)
            })
        })
        .map_err(|err| err.to_string())
    }

    /// The key a rotation signed here moves the account at `address` to,
    /// while it may not have landed.
    pub fn pending_rotation(&self, address: &Pubkey) -> Option<Vec<u8>> {
        self.leaf_marks.read().pending_rotation(address)
    }

    /// Sign `message` as a wallet of the vault, for a site. Only a key the
    /// vault holds: nothing Build made, and no programmatic signer or Squads
    /// vault. The caller checks that the bytes may be signed
    /// (`wallet_core::site_message`).
    pub fn sign_site_message(
        &self,
        account: solana_pubkey::Pubkey,
        message: &[u8],
    ) -> Result<[u8; 64], String> {
        let signer = self.core().with_vault(|vault| {
            let data = Vault::data(vault).map_err(|err| err.to_string())?;
            data.wallets
                .iter()
                .filter_map(|wallet| wallet.signer().ok())
                .find(|signer| wallet_core::WalletSigner::pubkey(signer) == account)
                .ok_or_else(|| "No key on this device signs as this account.".to_string())
        })?;
        let signature = wallet_core::WalletSigner::try_sign_message(&signer, message)
            .map_err(|err| err.to_string())?;
        Ok(signature.into())
    }

    /// Sign everything in `wire` that a vault key can sign: authorizations
    /// first, then the transaction itself. `known` is what a rebuild or a
    /// context note said of the Vector accounts.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sign_locally(
        &self,
        wire: &[u8],
        known: &[wallet_core::relay::VectorContext],
    ) -> Result<wallet_core::LocalSigning, wallet_core::SignError> {
        let signers = self.local_signers();
        let refs: Vec<&dyn wallet_core::WalletSigner> = signers
            .iter()
            .map(|signer| signer as &dyn wallet_core::WalletSigner)
            .collect();
        // A rebuild read `known` from the chain, and no hash-based key signs
        // here unless one did: the modal leaves authorizations to Review.
        let floors = wallet_core::vector::FloorsFrom::Chain;
        let vector = self.vector_keys_for_sign(wire, &refs, known, floors)?;
        let mut cache = self.xmss_retries.peek().clone();
        let done = wallet_core::sign_locally_in_with(
            wire,
            &refs,
            &vector,
            &wallet_core::relay::AuthContext { vector: known },
            Some(&mut cache),
        )?;
        let mut retries = self.xmss_retries;
        retries.set(cache);
        Ok(done)
    }

    /// Change the vault by `f`, which may refuse, and save it.
    fn change_vault<T>(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut VaultData) -> Result<T, RotationError>,
    ) -> Result<T, RotationError> {
        let mut answer = None;
        self.mutate_vault(name, |data| answer = Some(f(data)))
            .map_err(|err| RotationError::Save(err.to_string()))?;
        answer.unwrap_or(Err(RotationError::Locked))
    }

    /// Make the next key of `id`'s hash-based key, kept here until the
    /// account rotates into it.
    pub fn prepare_rotating_next_key(&self, id: WalletId) -> Result<(), RotationError> {
        self.change_vault("prepare_rotating_next_key", |data| {
            data.vector_wallet_mut(id)?.prepare_vector_next_key()
        })
    }

    /// Store a next public key scanned from the device that holds a wallet's
    /// key. Answers the wallet's label.
    pub fn accept_next_public_key(
        &self,
        scheme: u8,
        key: Vec<u8>,
    ) -> Result<String, RotationError> {
        self.change_vault("accept_next_public_key", |data| {
            data.accept_next_public_key(scheme, key)
        })
    }

    /// Replace `id`'s key by its next one, now that the account holds
    /// `current` on chain.
    pub fn install_rotation(&self, id: WalletId, current: &[u8]) -> Result<(), RotationError> {
        self.change_vault("install_rotation", |data| {
            data.vector_wallet_mut(id)?
                .install_vector_next_secret(current)
        })
    }

    /// Every wallet of the vault, locked or not.
    /// Prefer `(session.wallets)().summaries` in UI; this remains for actions.
    pub fn wallet_summaries(&self) -> Vec<WalletSummary> {
        (self.wallets)().summaries.clone()
    }

    /// The wallet the app is acting as: the one chosen, or the first when
    /// none was chosen or the choice was removed.
    pub fn selected_wallet(&self) -> Option<WalletId> {
        (self.wallets)().selected
    }

    /// Next Vector public key on `id`, when one was recorded.
    pub fn vector_next_pubkey(&self, id: WalletId) -> Option<Vec<u8>> {
        self.track_wallets();
        self.core()
            .with_wallets(|data| crate::state::slices::vector_next_pubkey(data?, id))
    }

    /// Prepared next Vector key on `id`, when the vault is unlocked.
    pub fn vector_prepared_next_key(&self, id: WalletId) -> Option<Vec<u8>> {
        self.core().with_vault(|vault| {
            Vault::data(vault)
                .ok()
                .and_then(|data| crate::state::slices::vector_prepared_next_key(data, id))
        })
    }

    /// Public Vector key recorded for `account`, when this device holds it.
    pub fn vector_recorded_key(&self, account: Pubkey) -> Option<Vec<u8>> {
        self.track_wallets();
        self.core().with_wallets(|data| {
            data?
                .wallets
                .iter()
                .find(|wallet| wallet.address() == Some(account))
                .and_then(|wallet| wallet.kind.vector_settings())
                .map(|settings| settings.key.to_vec())
        })
    }

    /// Act as `id` from now on, locked or not. Build starts from its fee
    /// payer.
    pub fn select_wallet(&self, id: WalletId) -> Result<(), VaultError> {
        let saved = self.command(|core| core.select_wallet(id));
        self.report_save(saved)?;
        let mut payer_for = self.forms.build.payer_for;
        payer_for.set(None);
        self.default_build_fee_payer();
        Ok(())
    }

    /// Put the selected wallet's fee payer on Build, once per wallet: a
    /// payer changed by hand stays until another wallet is selected. Reads
    /// the wallets, so an effect calling it runs again once they load.
    pub fn default_build_fee_payer(&self) {
        let Some(id) = self.selected_wallet() else {
            return;
        };
        let form = self.forms.build;
        if *form.payer_for.peek() == Some(id) {
            return;
        }
        let Ok(route) = self.route_of(id) else {
            return;
        };
        let (mut fee_payer, mut payer_for) = (form.fee_payer, form.payer_for);
        fee_payer.set(route.fee_payer.map(|(_, pubkey)| pubkey));
        payer_for.set(Some(id));
        self.default_build_nonce(route.durable_nonce, route.address);
    }

    /// Put an offline wallet's durable nonce on Build, with the key as its
    /// authority, or take off the one another offline wallet put there. A
    /// nonce of no wallet, picked by hand, stays. Deprecated once the
    /// programmatic signer or Vector is on mainnet.
    fn default_build_nonce(&self, nonce: Option<Pubkey>, key: Pubkey) {
        let form = self.forms.build;
        let (mut use_nonce, mut account) = (form.use_nonce, form.nonce);
        let (mut authority, mut hash) = (form.nonce_authority, form.nonce_hash);
        match nonce {
            Some(nonce) => {
                use_nonce.set(true);
                account.set(Some(nonce));
                authority.set(Some(key));
                let known = (self.book)().get(&nonce).and_then(|entry| entry.nonce_hash);
                hash.set(known.map(|hash| hash.to_string()).unwrap_or_default());
            }
            None if account
                .peek()
                .is_some_and(|picked| self.is_offline_nonce(picked)) =>
            {
                use_nonce.set(false);
                account.set(None);
                authority.set(None);
                hash.set(String::new());
            }
            None => {}
        }
    }

    /// The wallet that acts when `id` is chosen: an offline wallet whose
    /// default path is its programmatic signer hands over to that signer.
    pub fn acting_wallet(&self, id: WalletId) -> WalletId {
        self.track_wallets();
        self.core().with_wallets(|data| match data {
            Some(data) => wallet_core::acting_wallet(&data.wallets, id),
            None => id,
        })
    }

    /// The offline key and its durable nonce, when `tx` waits on that one
    /// key alone: every other signer is a key of this device, the key's
    /// wallet has a durable nonce, and nothing has signed yet. Such a
    /// request can be moved onto the nonce. Deprecated once the programmatic
    /// signer or Vector is on mainnet.
    pub fn offline_nonce_for(&self, tx: &wallet_core::DecodedTx) -> Option<(Pubkey, Pubkey)> {
        if tx.uses_durable_nonce || tx.signers.iter().any(|slot| slot.filled) {
            return None;
        }
        let local = self.local_signer_pubkeys();
        let mut outside = tx
            .signers
            .iter()
            .map(|slot| slot.pubkey)
            .filter(|pubkey| !local.contains(pubkey));
        let key = outside.next()?;
        if outside.next().is_some() {
            return None;
        }
        self.core().with_wallets(|data| {
            let nonce = data?
                .wallets
                .iter()
                .find(|wallet| {
                    matches!(wallet.kind, WalletKind::Offline { .. })
                        && wallet.address() == Some(key)
                })?
                .kind
                .durable_nonce()?;
            Some((key, nonce))
        })
    }

    /// Whether `account` is the durable nonce of an offline wallet here.
    fn is_offline_nonce(&self, account: Pubkey) -> bool {
        self.core().with_wallets(|data| {
            data.is_some_and(|data| {
                data.wallets
                    .iter()
                    .any(|wallet| wallet.kind.durable_nonce() == Some(account))
            })
        })
    }

    /// How `id` signs and who pays for it.
    pub fn route_of(&self, id: WalletId) -> Result<SigningRoute, RouteError> {
        self.track_wallets();
        self.core().with_wallets(|data| {
            let data = data.ok_or(RouteError::UnknownWallet)?;
            wallet_core::signing_route(&data.wallets, id)
        })
    }

    /// Store a programmatic signer or a multisig, and name its address in the
    /// address book.
    pub fn add_contract_wallet(&self, wallet: WalletRecord) -> Result<(), EnrollError> {
        let label = wallet.label.clone();
        self.enroll(
            "add_contract_wallet",
            |data| data.add_contract_wallet(wallet),
            |book, address| {
                if let Some(address) = address {
                    book.insert_contact(*address, label);
                }
            },
        )
        .map(|_| ())
    }

    /// Store an offline wallet: an address whose key is kept on another
    /// device. A programmatic signer already here for that key is tied to it.
    pub fn add_offline_wallet(
        &self,
        label: String,
        pubkey: Pubkey,
        fee_payer: Option<WalletId>,
    ) -> Result<WalletId, EnrollError> {
        self.enroll(
            "add_offline_wallet",
            |data| data.add_offline_wallet(label.clone(), pubkey, fee_payer),
            |book, _| wallet_core::name_offline_wallet(book, pubkey, label.clone()),
        )
    }

    /// Store wallets whose keys another device holds, as offline wallets
    /// and Vector wallets with their key elsewhere, each paid for by
    /// `fee_payer`. All of them, or none.
    pub fn add_wallet_keys(
        &self,
        keys: Vec<wallet_core::shared_keys::KeyToAdd>,
        fee_payer: Option<WalletId>,
    ) -> Result<Vec<wallet_core::shared_keys::AddedKey>, EnrollError> {
        self.enroll(
            "add_wallet_keys",
            |data| data.add_wallet_keys(&keys, fee_payer),
            |book, added| {
                for wallet in added {
                    if wallet.offline {
                        wallet_core::name_offline_wallet(
                            book,
                            wallet.address,
                            wallet.label.clone(),
                        );
                    } else {
                        book.insert_contact(wallet.address, wallet.label.clone());
                    }
                }
            },
        )
    }

    /// Change the settings of one wallet.
    pub fn update_wallet(
        &self,
        id: WalletId,
        change: impl FnOnce(&mut WalletKind),
    ) -> Result<(), VaultError> {
        self.mutate_vault("update_wallet", |data| {
            if let Some(wallet) = data.wallets.iter_mut().find(|wallet| wallet.id == id) {
                change(&mut wallet.kind);
            }
        })
    }

    /// The wallet that acts as `pubkey`, when one of this device does.
    pub fn wallet_at(&self, pubkey: &Pubkey) -> Option<WalletSummary> {
        (self.wallets)()
            .summaries
            .iter()
            .find(|wallet| wallet.address == Some(*pubkey))
            .cloned()
    }

    /// Rename a wallet. Its address in the book takes the same name, so the
    /// two never disagree.
    pub fn rename_wallet(&self, id: WalletId, label: String) -> Result<(), VaultError> {
        let mut address = None;
        self.mutate_vault("rename_wallet", |data| {
            if let Some(wallet) = data.wallets.iter_mut().find(|wallet| wallet.id == id) {
                wallet.label = label.clone();
                address = wallet.address();
            }
        })?;
        if let Some(address) = address {
            self.mutate_workspace(|ws| {
                ws.book.set_label(&address, label.clone());
            })?;
        }
        Ok(())
    }

    /// Remove a wallet, and clear every setting of another wallet that pointed at it.
    pub fn remove_wallet(&self, id: WalletId) -> Result<(), VaultError> {
        self.mutate_vault("remove_wallet", |data| data.remove_wallet(id))
    }
}
