use super::*;
use crate::fixtures::pk;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::programmatic::programmatic_signer_address;
use crate::wallets::squads::{Squad, SquadsProgram};

fn hot(label: &str) -> NewSigner {
    let key = KeypairSigner::new_random();
    NewSigner {
        wallet: WalletRecord::from_keypair_seed(label, key.seed_bytes()),
        pubkey: key.pubkey(),
        label: label.to_string(),
        origin: Origin::Imported,
    }
}

fn squad(label: &str, members: Vec<Pubkey>) -> WalletRecord {
    let squad = Squad {
        program: SquadsProgram::V4,
        address: pk(40),
    };
    WalletRecord::squads(label, squad, members, 1)
}

#[test]
fn a_signer_already_here_or_listed_twice_stores_nothing() {
    let mut data = VaultData::default();
    let (a, b) = (hot("A"), hot("B"));
    data.add_signers(std::slice::from_ref(&a)).unwrap();

    let again = data.add_signers(&[b.clone(), a.clone()]).unwrap_err();
    assert_eq!(again, RosterError::Duplicate(a.pubkey));
    let twice = data.add_signers(&[b.clone(), b.clone()]).unwrap_err();
    assert_eq!(twice, RosterError::Duplicate(b.pubkey));
    assert_eq!(data.wallets.len(), 1, "nothing of a refused set is kept");

    data.add_signers(std::slice::from_ref(&b)).unwrap();
    assert_eq!(data.local_signer_pubkeys(), [a.pubkey, b.pubkey]);
}

#[test]
fn an_address_is_one_wallet_whatever_its_kind() {
    let mut data = VaultData::default();
    let a = hot("A");
    data.add_signers(std::slice::from_ref(&a)).unwrap();

    // The same key cannot also be an offline wallet.
    assert_eq!(
        data.add_offline_wallet("Cold", a.pubkey, None),
        Err(RosterError::DuplicateWallet(a.pubkey))
    );
    data.add_offline_wallet("Cold", pk(1), None).unwrap();
    assert_eq!(
        data.add_signers(&[NewSigner {
            pubkey: pk(1),
            ..hot("C")
        }]),
        Err(RosterError::Duplicate(pk(1))),
    );

    let signer = WalletRecord::programmatic("Signer", pk(2), None);
    let address = signer.address().unwrap();
    assert_eq!(data.add_contract_wallet(signer), Ok(Some(address)));
    assert_eq!(
        data.add_contract_wallet(WalletRecord::programmatic("Again", pk(2), None)),
        Err(RosterError::DuplicateWallet(address))
    );
    assert_eq!(data.wallets.len(), 3);
    // An offline wallet signs nothing here, and neither does a contract.
    assert_eq!(data.local_signer_pubkeys(), [a.pubkey]);
}

#[test]
fn an_offline_wallet_becomes_the_authority_of_the_signer_waiting_for_its_key() {
    let mut data = VaultData::default();
    let payer = hot("Payer");
    data.add_signers(std::slice::from_ref(&payer)).unwrap();
    let waiting = WalletRecord::programmatic("Waiting", pk(1), None);
    let taken = WalletRecord::programmatic("Other key", pk(2), None);
    let (waiting_id, taken_id) = (waiting.id, taken.id);
    data.add_contract_wallet(waiting).unwrap();
    data.add_contract_wallet(taken).unwrap();

    let cold = data
        .add_offline_wallet("Cold", pk(1), Some(payer.wallet.id))
        .unwrap();

    let authority_wallet = |data: &VaultData, id| {
        let wallet = data.wallets.iter().find(|wallet| wallet.id == id).unwrap();
        match &wallet.kind {
            WalletKind::Programmatic { authority, .. } => authority.wallet(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(authority_wallet(&data, waiting_id), Some(cold));
    assert_eq!(authority_wallet(&data, taken_id), None);
    let stored = data.wallets.last().unwrap();
    assert_eq!(stored.kind.fee_payer(), Some(payer.wallet.id));
    assert_eq!(stored.address(), Some(pk(1)));
}

#[test]
fn removing_a_wallet_clears_every_setting_that_pointed_at_it() {
    let mut data = VaultData::default();
    let (payer, other) = (hot("Payer"), hot("Other"));
    data.add_signers(&[payer.clone(), other.clone()]).unwrap();
    let gone = payer.wallet.id;

    let mut signer = WalletRecord::programmatic("Signer", payer.pubkey, Some(gone));
    signer.kind.set_fee_payer(Some(gone));
    let mut multisig = squad("Team", vec![payer.pubkey, other.pubkey]);
    if let WalletKind::Squads { member_wallet, .. } = &mut multisig.kind {
        *member_wallet = Some(gone);
    }
    multisig.kind.set_fee_payer(Some(other.wallet.id));
    data.add_contract_wallet(signer).unwrap();
    data.add_contract_wallet(multisig).unwrap();
    data.add_offline_wallet("Cold", pk(9), Some(gone)).unwrap();
    data.selected = Some(gone);

    data.remove_wallet(gone);

    assert_eq!(data.wallets.len(), 4);
    assert_eq!(data.selected, None);
    for wallet in &data.wallets {
        match &wallet.kind {
            WalletKind::Programmatic {
                authority,
                fee_payer,
                ..
            } => {
                assert_eq!((authority, *fee_payer), (&Authority::Outside, None));
                assert_eq!(
                    wallet.kind.signer_authority(),
                    Some(payer.pubkey),
                    "the key itself is kept"
                );
            }
            WalletKind::Squads {
                member_wallet,
                fee_payer,
                ..
            } => {
                assert_eq!(*member_wallet, None);
                // A pointer at a wallet that stays is left alone.
                assert_eq!(*fee_payer, Some(other.wallet.id));
            }
            WalletKind::Offline { fee_payer, .. } => assert_eq!(*fee_payer, None),
            _ => {}
        }
    }
}

#[test]
fn the_app_acts_as_the_chosen_wallet_or_the_first_one() {
    let mut data = VaultData::default();
    assert_eq!((data.chosen_wallet(), data.acting_wallet()), (None, None));

    let (a, b) = (hot("A"), hot("B"));
    data.add_signers(&[a.clone(), b.clone()]).unwrap();
    assert_eq!(data.chosen_wallet(), None);
    assert_eq!(data.acting_wallet(), Some(a.wallet.id));

    data.selected = Some(b.wallet.id);
    assert_eq!(data.chosen_wallet(), Some(b.wallet.id));
    assert_eq!(data.acting_wallet(), Some(b.wallet.id));

    // A choice that names a wallet no longer here counts for nothing.
    data.selected = Some(WalletId::new());
    assert_eq!(data.chosen_wallet(), None);
    assert_eq!(data.acting_wallet(), Some(a.wallet.id));
}

#[test]
fn a_summary_carries_settings_only_for_contract_wallets_and_never_a_secret() {
    let mut data = VaultData::default();
    let a = hot("A");
    data.add_signers(std::slice::from_ref(&a)).unwrap();
    let mut signer = WalletRecord::programmatic("Signer", pk(2), None);
    signer.kind.set_fee_payer(Some(a.wallet.id));
    let address = programmatic_signer_address(&pk(2));
    data.add_contract_wallet(signer).unwrap();
    data.add_offline_wallet("Cold", pk(3), None).unwrap();

    let listed = data.summaries();
    let classes: Vec<WalletClass> = listed.iter().map(|wallet| wallet.class).collect();
    assert_eq!(
        classes,
        [
            WalletClass::Hot,
            WalletClass::SmartContract,
            WalletClass::Offline
        ]
    );
    assert_eq!(listed[0].address, Some(a.pubkey));
    assert_eq!((&listed[0].contract, listed[0].fee_payer), (&None, None));
    assert_eq!(listed[1].address, Some(address));
    assert!(matches!(
        listed[1].contract,
        Some(WalletKind::Programmatic { .. })
    ));
    assert_eq!(listed[1].fee_payer, Some(a.wallet.id));
    assert_eq!(listed[2].contract, None);
}

#[test]
fn the_book_names_signers_with_their_wallet_and_offline_wallets_without() {
    let mut book = AddressBook::default();
    let a = hot("A");
    name_signers(&mut book, std::slice::from_ref(&a));
    let entry = book.get(&a.pubkey).unwrap();
    assert_eq!(
        (entry.label.as_str(), entry.wallet_id),
        ("A", Some(a.wallet.id))
    );

    name_offline_wallet(&mut book, pk(1), "Cold");
    let entry = book.get(&pk(1)).unwrap();
    assert_eq!((entry.label.as_str(), entry.wallet_id), ("Cold", None));

    // An address the book already names keeps its entry.
    name_offline_wallet(&mut book, a.pubkey, "Renamed");
    let entry = book.get(&a.pubkey).unwrap();
    assert_eq!(
        (entry.label.as_str(), entry.wallet_id),
        ("A", Some(a.wallet.id))
    );
}

#[test]
fn a_site_is_shown_keys_and_contracts_and_a_locked_vault_shows_what_the_book_ties_to_a_wallet() {
    let mut data = VaultData::default();
    let mut workspace = Workspace::default();
    let a = hot("A");
    data.add_signers(std::slice::from_ref(&a)).unwrap();
    name_signers(&mut workspace.book, std::slice::from_ref(&a));
    let signer = WalletRecord::programmatic("Signer", pk(2), None);
    let address = data.add_contract_wallet(signer).unwrap().unwrap();
    data.add_offline_wallet("Cold", pk(3), None).unwrap();
    name_offline_wallet(&mut workspace.book, pk(3), "Cold");
    workspace.book.insert_contact(pk(4), "Friend");
    let quantum = WalletRecord::vector(
        "Quantum",
        SchemeId::Ed25519,
        pk(50),
        pk(5).to_bytes().to_vec(),
        Authority::Outside,
    );
    let vector_address = quantum.address().unwrap();
    data.wallets.push(quantum);

    // Vector wallets are off until turned on, and are then shown too.
    let shown = |workspace: &Workspace| -> Vec<(Pubkey, bool)> {
        shareable_accounts(Some(&data), workspace)
            .iter()
            .map(|account| (account.pubkey, account.contract))
            .collect()
    };
    assert_eq!(shown(&workspace), [(a.pubkey, false), (address, true)]);
    workspace.vector_wallets = true;
    assert_eq!(
        shown(&workspace),
        [(a.pubkey, false), (address, true), (vector_address, true)]
    );
    assert!(!shareable_accounts(Some(&data), &workspace)[0]
        .summary
        .is_empty());

    // Locked: only the key the book ties to a wallet, with no summary.
    let locked = shareable_accounts(None, &workspace);
    assert_eq!(locked.len(), 1);
    assert_eq!(
        (locked[0].pubkey, locked[0].summary.as_str()),
        (a.pubkey, "")
    );
}

#[test]
fn a_refresh_reads_owners_contacts_and_wallets_once_each_and_the_known_nonces() {
    let mut data = VaultData::default();
    let mut book = AddressBook::default();
    let a = hot("A");
    data.add_signers(std::slice::from_ref(&a)).unwrap();
    name_signers(&mut book, std::slice::from_ref(&a));
    // A wallet the book does not name is read all the same.
    let signer = WalletRecord::programmatic("Signer", pk(2), None);
    let address = data.add_contract_wallet(signer).unwrap().unwrap();
    book.insert_contact(pk(4), "Friend");
    let mut nonce = book.insert_contact(pk(5), "Nonce").clone();
    nonce.kind = AddressKind::NonceAccount;
    book.upsert(nonce);

    let (owners, nonces) = refresh_targets(Some(&data), &book);
    let mut expected = vec![a.pubkey, address, pk(4)];
    let mut found = owners.clone();
    expected.sort();
    found.sort();
    assert_eq!(found, expected);
    assert_eq!(nonces, [pk(5)]);

    let (locked, _) = refresh_targets(None, &book);
    assert!(!locked.contains(&address));
    assert_eq!(locked.len(), 2);
}

#[test]
fn removing_a_wallet_takes_it_out_of_every_list_of_wallets_that_approve_together() {
    let mut data = VaultData::default();
    let (acting, gone, stays) = (hot("Acting"), hot("Gone"), hot("Stays"));
    data.add_signers(&[acting.clone(), gone.clone(), stays.clone()])
        .unwrap();
    let nonce = pk(70);
    let mut multisig = squad("Team", vec![acting.pubkey, gone.pubkey, stays.pubkey]);
    multisig.kind.set_member_wallet(Some(acting.wallet.id));
    multisig
        .kind
        .set_approves_with(vec![gone.wallet.id, stays.wallet.id]);
    multisig.kind.set_approval_nonce(Some(nonce));
    data.add_contract_wallet(multisig).unwrap();

    data.remove_wallet(gone.wallet.id);

    let Some(WalletKind::Squads {
        member_wallet,
        approves_with,
        approval_nonce,
        ..
    }) = data.wallets.last().map(|wallet| &wallet.kind)
    else {
        panic!("the multisig is still there");
    };
    assert_eq!(*member_wallet, Some(acting.wallet.id));
    assert_eq!(*approves_with, vec![stays.wallet.id]);
    // The nonce account is not a wallet, so it stays.
    assert_eq!(*approval_nonce, Some(nonce));
}

#[test]
fn an_offline_wallet_becomes_the_authority_of_a_vector_account_made_for_its_ed25519_key() {
    let mut data = VaultData::default();
    let account = |scheme: SchemeId, key: Vec<u8>| {
        WalletRecord::vector("Vector", scheme, pk(70), key, Authority::Outside)
    };
    let waiting = account(SchemeId::Ed25519, pk(1).to_bytes().to_vec());
    // The same bytes under another scheme are another key.
    let other = account(
        SchemeId::Xmss,
        [pk(1).to_bytes().as_slice(), &[1; 9]].concat(),
    );
    let (waiting_id, other_id) = (waiting.id, other.id);
    data.add_contract_wallet(waiting).unwrap();
    data.add_contract_wallet(other).unwrap();

    let cold = data.add_offline_wallet("Cold", pk(1), None).unwrap();
    let authority = |id| {
        let wallet = data.wallets.iter().find(|wallet| wallet.id == id).unwrap();
        match &wallet.kind {
            WalletKind::Programmatic { authority, .. } => authority.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(authority(waiting_id), Authority::Wallet { wallet: cold });
    assert_eq!(authority(other_id), Authority::Outside);
}

#[test]
fn a_programmatic_signer_that_would_keep_its_key_in_its_record_is_not_stored() {
    let mut data = VaultData::default();
    let held = Authority::Held {
        secret: crate::SecretBytes(vec![1; 32]),
        next_leaf: 0,
    };
    let mut signer = WalletRecord::programmatic("Signer", pk(2), None);
    if let WalletKind::Programmatic { authority, .. } = &mut signer.kind {
        *authority = held.clone();
    }
    assert_eq!(
        data.add_contract_wallet(signer),
        Err(RosterError::KeyInRecord)
    );
    // A Vector account may, and one whose key names no account is left out.
    let vector = WalletRecord::vector("Vector", SchemeId::MlDsa44, pk(70), vec![7; 1312], held);
    assert!(data.add_contract_wallet(vector).unwrap().is_some());
    let broken = WalletRecord::vector(
        "Broken",
        SchemeId::MlDsa44,
        pk(70),
        vec![7; 12],
        Authority::Outside,
    );
    assert_eq!(data.add_contract_wallet(broken), Ok(None));
    assert_eq!(data.wallets.len(), 1);
    // Its key signs for the account, not for an address of its own.
    assert!(data.local_signer_pubkeys().is_empty());
}

#[test]
fn an_offline_wallet_hands_over_to_its_signer_only_when_that_is_its_default() {
    let mut data = VaultData::default();
    let cold = data.add_offline_wallet("Cold", pk(1), None).unwrap();
    let signer = WalletRecord::programmatic("Cold signer", pk(1), Some(cold));
    let signer_id = signer.id;
    data.add_contract_wallet(signer).unwrap();
    assert_eq!(acting_wallet(&data.wallets, cold), cold);

    let set = |data: &mut VaultData, path| {
        let wallet = data.wallets.iter_mut().find(|w| w.id == cold).unwrap();
        wallet.kind.set_default_path(path);
    };
    set(&mut data, Some(OfflinePath::DurableNonce));
    assert_eq!(acting_wallet(&data.wallets, cold), cold);
    set(&mut data, Some(OfflinePath::Programmatic));
    assert_eq!(acting_wallet(&data.wallets, cold), signer_id);
    assert_eq!(
        signer_of(&data.wallets, pk(1)).map(|wallet| wallet.id),
        Some(signer_id)
    );

    // Without its signer the key acts as itself again, and forgets the path.
    data.remove_wallet(signer_id);
    assert_eq!(acting_wallet(&data.wallets, cold), cold);
    let wallet = data.wallets.iter().find(|w| w.id == cold).unwrap();
    assert_eq!(wallet.kind.default_path(), None);
}

#[test]
fn a_summary_of_an_offline_wallet_carries_its_nonce_and_its_default_path() {
    let mut data = VaultData::default();
    let cold = data.add_offline_wallet("Cold", pk(1), None).unwrap();
    let wallet = data.wallets.iter_mut().find(|w| w.id == cold).unwrap();
    wallet.kind.set_durable_nonce(Some(pk(4)));
    wallet
        .kind
        .default_path_unless_chosen(OfflinePath::DurableNonce);
    wallet
        .kind
        .default_path_unless_chosen(OfflinePath::Programmatic);
    let summary = data.summaries().pop().unwrap();
    assert_eq!(summary.durable_nonce, Some(pk(4)));
    assert_eq!(summary.default_path, Some(OfflinePath::DurableNonce));
    assert_eq!(summary.contract, None);
}
