//! Vector wallets on the path the app takes, against the deployed program:
//! a site's transaction is rebuilt for the account, its key signs on this
//! device or another, a hot wallet pays, and the exact bytes land.

use crate::support::vector::{device_signs, note, run, Desk};
use crate::support::SOL;
use solana_instruction::AccountMeta;
use solana_pubkey::Pubkey;
use wallet_core::protocol::{decode_object, encode_object, QrObject};
use wallet_core::relay::{
    authority_slots, fit_loaded_accounts, with_blockhash, with_compute_unit_limit, AuthContext,
    PASSTHROUGH_CU,
};
use wallet_core::vector::{
    parse_passthrough, passthrough, withdraw, SchemeId, VectorAccount, ADVANCE_DISCRIMINATOR,
    PROGRAM_ID,
};
use wallet_core::{
    decode_v1_transaction, Instruction, KeypairSigner, WalletSigner, SYSTEM_PROGRAM_ID,
    TOKEN_PROGRAM_ID,
};
use wallet_flows::compile::compile_fresh;

fn transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(from, to, lamports)
}

fn nonce_of(desk: &Desk, account: &Pubkey) -> [u8; 32] {
    let raw = desk.chain().account(account).unwrap();
    VectorAccount::parse(&PROGRAM_ID, &raw).unwrap().nonce
}

#[test]
fn a_transfer_a_site_asked_of_a_vector_wallet_is_rebuilt_signed_here_and_lands() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", true);
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    // The site made the account pay, as it would for any wallet.
    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    assert_eq!(adapted.fee_payer, desk.payer.pubkey());
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    let signers: Vec<Pubkey> = tx.signers.iter().map(|slot| slot.pubkey).collect();
    assert_eq!(signers, vec![desk.payer.pubkey()]);
    // The Advance, then the transfer as a Withdraw inside the account's
    // Passthrough.
    assert_eq!(tx.instructions.len(), 2);
    assert_eq!(tx.instructions[0].data[0], ADVANCE_DISCRIMINATOR);
    let inner = parse_passthrough(&tx.native_instructions()[1]).unwrap();
    assert_eq!(inner.instructions.len(), 1);
    assert_eq!(inner.instructions[0].program_id, PROGRAM_ID);

    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    assert!(signed.authorized);
    let landed = desk.land(&signed.wire);
    println!(
        "Ed25519 transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    // What the rebuild adds to the limit covers what the account's part
    // costs.
    let added = SchemeId::Ed25519.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
    // The key holds no lamports and signed no transaction.
    assert_eq!(desk.chain().balance(&account.key.pubkey()), 0);
}

#[test]
fn a_sites_new_mint_for_a_vector_wallet_gets_a_key_made_here_that_signs_after_the_authorization() {
    use solana_message::{Message as LegacyMessage, VersionedMessage};
    use solana_transaction::versioned::VersionedTransaction;
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", true);
    let receiver = KeypairSigner::new_random().pubkey();
    // The site's new mint, left unsigned. A hot wallet pays its rent, since
    // a Vector account only sends lamports as a plain transfer, and the
    // account controls it and pays something too.
    let mint = KeypairSigner::new_random();
    let mut initialize = vec![20u8, 6];
    initialize.extend_from_slice(account.address.as_ref());
    initialize.push(0);
    let instructions = [
        solana_system_interface::instruction::create_account(
            &desk.payer.pubkey(),
            &mint.pubkey(),
            desk.chain().rent(82),
            82,
            &TOKEN_PROGRAM_ID,
        ),
        Instruction {
            program_id: TOKEN_PROGRAM_ID,
            accounts: vec![AccountMeta::new(mint.pubkey(), false)],
            data: initialize,
        },
        transfer(&account.address, &receiver, SOL / 4),
    ];
    let mut message = LegacyMessage::new(&instructions, Some(&account.address));
    message.recent_blockhash = desk.chain().latest_blockhash();
    let signatures = usize::from(message.header.num_required_signatures);
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: vec![solana_signature::Signature::default(); signatures],
        message: VersionedMessage::Legacy(message),
    })
    .unwrap();
    let tx = wallet_core::decode_transaction(&wire, &wallet_core::LookupTables::new()).unwrap();

    let known: Vec<Pubkey> = desk
        .wallets
        .iter()
        .filter_map(wallet_core::WalletRecord::address)
        .collect();
    let routed = run(wallet_flows::request::route(
        Some(&desk.node),
        &tx,
        &[desk.payer.pubkey()],
        &known,
        true,
        false,
    ))
    .unwrap();
    let wallet_flows::request::Route::Converted(converted, keys) = routed else {
        panic!("a Vector wallet is converted");
    };
    let fresh = keys.pubkeys();
    assert_eq!(converted.replaced, vec![(mint.pubkey(), fresh[0])]);
    let adapted = desk.rebuild_wire(&converted.wire);
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert!(rebuilt.signers.iter().any(|slot| slot.pubkey == fresh[0]));

    // The authorization and the payer first, as Sign does over two presses, then
    // the key kept for the mint.
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!(signed.authorizations, 1);
    let signed = wallet_core::apply_local_signatures(&signed.wire, &keys.signers()).unwrap();
    assert!(decode_v1_transaction(&signed).unwrap().fully_signed());
    desk.land(&signed);
    let made = desk.chain().account(&fresh[0]).expect("the mint is made");
    assert_eq!(made.owner, TOKEN_PROGRAM_ID);
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
}

#[test]
fn a_token_transfer_owned_by_a_vector_wallet_lands() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", true);
    let mint = KeypairSigner::new_random().pubkey();
    let (source, destination) = (
        KeypairSigner::new_random().pubkey(),
        KeypairSigner::new_random().pubkey(),
    );
    {
        let mut chain = desk.chain();
        chain.plant(mint, TOKEN_PROGRAM_ID, mint_data(6));
        chain.plant(
            source,
            TOKEN_PROGRAM_ID,
            token_data(&mint, &account.address, 1_000),
        );
        let other = KeypairSigner::new_random().pubkey();
        chain.plant(destination, TOKEN_PROGRAM_ID, token_data(&mint, &other, 0));
    }
    let mut data = vec![12];
    data.extend_from_slice(&400u64.to_le_bytes());
    data.push(6);
    let transfer_checked = Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(source, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new(destination, false),
            AccountMeta::new_readonly(account.address, true),
        ],
        data,
    };
    let adapted = desk.rebuild(desk.payer.pubkey(), vec![transfer_checked]);
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    desk.land(&signed.wire);
    let amount = |address: &Pubkey| {
        let data = desk.chain().data(address);
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    };
    assert_eq!((amount(&source), amount(&destination)), (600, 400));
}

#[test]
fn instructions_around_the_accounts_own_stay_where_the_site_put_them() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", true);
    let payer = desk.payer.pubkey();
    let [first, second, third] = [(); 3].map(|_| KeypairSigner::new_random().pubkey());
    let adapted = desk.rebuild(
        payer,
        vec![
            transfer(&payer, &first, SOL),
            transfer(&account.address, &second, SOL / 2),
            transfer(&payer, &third, SOL),
        ],
    );
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    let programs: Vec<Pubkey> = tx.instructions.iter().map(|ix| ix.program_id).collect();
    assert_eq!(
        programs,
        vec![PROGRAM_ID, SYSTEM_PROGRAM_ID, PROGRAM_ID, SYSTEM_PROGRAM_ID]
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    desk.land(&signed.wire);
    let chain = desk.chain();
    assert_eq!(
        [first, second, third].map(|key| chain.balance(&key)),
        [SOL, SOL / 2, SOL]
    );
}

#[test]
fn two_vector_wallets_on_two_devices_sign_one_advance_in_either_order() {
    for second_first in [false, true] {
        let mut desk = Desk::new();
        let a = desk.add_vector("A", false);
        let b = desk.add_vector("B", false);
        let receiver = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(
            desk.payer.pubkey(),
            vec![
                transfer(&a.address, &receiver, 1_000_000),
                transfer(&b.address, &receiver, 2_000_000),
            ],
        );
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        // One Advance for both, and one Passthrough that runs as both.
        assert_eq!(tx.instructions.len(), 2);
        let slots = authority_slots(&tx);
        assert_eq!(slots.len(), 2);
        assert_eq!(
            slots.iter().map(|slot| slot.address).collect::<Vec<_>>(),
            vec![a.address, b.address]
        );
        // Both are Ed25519, so the length of the data tells where each goes
        // without any context.
        assert!(slots.iter().all(|slot| slot.place.is_some()));

        // Each device sees the transaction and the note, never the other's key.
        let shown = note(&adapted.context);
        let from_a = device_signs(&adapted.wire, &a.key, &shown);
        let from_b = device_signs(&adapted.wire, &b.key, &shown);
        let order = if second_first {
            [from_b, from_a]
        } else {
            [from_a, from_b]
        };
        let mut wire = adapted.wire.clone();
        for object in order {
            wire = desk.accept(&wire, &object, &adapted.context);
        }
        let signed = desk.sign(&wire, &adapted.context).unwrap();
        assert_eq!((signed.authorizations, signed.signatures), (0, 1));
        let landed = desk.land(&signed.wire);
        println!("Two Ed25519 accounts: {} CU", landed.compute_units);
        let added = 2 * SchemeId::Ed25519.advance_cu() + PASSTHROUGH_CU;
        assert!(landed.compute_units < u64::from(added), "{landed:?}");
        assert_eq!(desk.chain().balance(&receiver), 3_000_000);
    }
}

#[test]
fn a_new_blockhash_and_new_limits_after_signing_still_land() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", false);
    let receiver = KeypairSigner::new_random().pubkey();
    let adapted = desk.rebuild(
        desk.payer.pubkey(),
        vec![transfer(&account.address, &receiver, SOL / 2)],
    );
    let object = device_signs(&adapted.wire, &account.key, &note(&adapted.context));
    let wire = desk.accept(&adapted.wire, &object, &adapted.context);

    // Other transactions land while it is away, and it comes back to a new
    // blockhash. The authorization does not cover the blockhash or the
    // TransactionConfig limits.
    let wire = {
        let mut chain = desk.chain();
        let before = chain.latest_blockhash();
        let treasury = chain.treasury;
        chain
            .send(
                &[&desk.payer],
                vec![transfer(&desk.payer.pubkey(), &treasury, 1)],
            )
            .unwrap();
        assert_ne!(chain.latest_blockhash(), before);
        with_blockhash(&wire, chain.latest_blockhash()).unwrap()
    };
    let before = decode_v1_transaction(&wire).unwrap();
    // A rebuild measured before the authorization was signed leaves compute
    // at the network maximum, so the limit moves down from there.
    let limit = before.budget.compute_unit_limit.unwrap_or(200_000);
    let changed = if limit > 1_350_000 {
        limit - 50_000
    } else {
        limit + 50_000
    };
    let wire = with_compute_unit_limit(&wire, changed).unwrap();
    let wire = fit_loaded_accounts(&wire, 800_000).unwrap();
    let after = decode_v1_transaction(&wire).unwrap();
    assert_eq!(after.budget.compute_unit_limit, Some(changed));
    assert!(authority_slots(&after).iter().all(|slot| slot.signed));
    let signed = desk.sign(&wire, &adapted.context).unwrap();
    desk.land(&signed.wire);
    assert_eq!(desk.chain().balance(&receiver), SOL / 2);
}

#[test]
fn a_signature_does_not_outlive_its_nonce_or_its_instructions() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", false);
    let receiver = KeypairSigner::new_random().pubkey();
    let pay = |lamports| vec![transfer(&account.address, &receiver, lamports)];
    // Two transactions rebuilt and signed at the same nonce.
    let first = desk.rebuild(desk.payer.pubkey(), pay(SOL / 10));
    let second = desk.rebuild(desk.payer.pubkey(), pay(SOL / 5));
    let signed: Vec<Vec<u8>> = [&first, &second]
        .map(|adapted| {
            let object = device_signs(&adapted.wire, &account.key, &note(&adapted.context));
            desk.accept(&adapted.wire, &object, &adapted.context)
        })
        .to_vec();

    // The first, changed after it was signed to pay more: Sign finds that
    // the signature no longer verifies.
    let key = account.key.pubkey().to_bytes();
    let mut changed = decode_v1_transaction(&signed[0])
        .unwrap()
        .native_instructions();
    let more = [withdraw(SchemeId::Ed25519, &key, &receiver, SOL / 2).unwrap()];
    changed[1] = passthrough(SchemeId::Ed25519, &key, &more).unwrap();
    let (changed, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), changed)).unwrap();
    let err = desk.sign(&changed, &first.context).unwrap_err();
    assert!(err.contains("no longer verifies"), "{err}");

    let done = desk.sign(&signed[0], &first.context).unwrap();
    desk.land(&done.wire);
    // The nonce moved: the second no longer verifies, and Sign says so
    // before the payer signs.
    let err = desk.sign(&signed[1], &second.context).unwrap_err();
    assert!(err.contains("no longer verifies"), "{err}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 10);
}

#[test]
fn a_payer_no_instruction_names_can_be_replaced_after_the_account_signed() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", false);
    let receiver = KeypairSigner::new_random().pubkey();
    let adapted = desk.rebuild(
        desk.payer.pubkey(),
        vec![transfer(&account.address, &receiver, SOL / 2)],
    );
    let object = device_signs(&adapted.wire, &account.key, &note(&adapted.context));
    let wire = desk.accept(&adapted.wire, &object, &adapted.context);

    // The message the account signed does not name the payer, so another
    // one can pay.
    let other = KeypairSigner::new_random();
    desk.chain().fund(&other.pubkey(), SOL);
    let tx = decode_v1_transaction(&wire).unwrap();
    let swap = wallet_core::adapt::PayerSwap {
        from: desk.payer.pubkey(),
        to: other.pubkey(),
    };
    let swapped = wallet_core::adapt::swap_fee_payer(&tx, &swap).unwrap();
    let swapped = wallet_core::apply_local_signatures(&swapped, &[&other]).unwrap();
    desk.land(&swapped);
    assert_eq!(desk.chain().balance(&receiver), SOL / 2);
}

#[test]
fn swapping_to_a_payer_an_instruction_names_voids_the_authorization() {
    let mut desk = Desk::new();
    let account = desk.add_vector("Quantum", false);
    let named = KeypairSigner::new_random();
    desk.chain().fund(&named.pubkey(), SOL);
    // A top-level transfer that names `named`. It stays outside the
    // Passthrough, so the digest covers its account flags.
    let adapted = desk.rebuild(
        desk.payer.pubkey(),
        vec![
            transfer(&account.address, &desk.payer.pubkey(), SOL / 10),
            transfer(&desk.payer.pubkey(), &named.pubkey(), 1),
        ],
    );
    let object = device_signs(&adapted.wire, &account.key, &note(&adapted.context));
    let wire = desk.accept(&adapted.wire, &object, &adapted.context);
    let tx = decode_v1_transaction(&wire).unwrap();
    assert!(authority_slots(&tx).iter().all(|slot| slot.signed));

    // Making `named` the fee payer changes the flags the account signed.
    let swap = wallet_core::adapt::PayerSwap {
        from: desk.payer.pubkey(),
        to: named.pubkey(),
    };
    let swapped = wallet_core::adapt::swap_fee_payer(&tx, &swap).unwrap();
    let err = desk.sign(&swapped, &adapted.context).unwrap_err();
    assert!(err.contains("no longer verifies"), "{err}");
}

#[test]
fn a_secp256k1_vector_wallet_signs_a_transfer_that_lands() {
    let mut desk = Desk::new();
    let account = desk.add_secp("Secp");
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    let landed = desk.land(&signed.wire);
    println!(
        "secp256k1 transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::Secp256k1.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
}

#[test]
fn an_eip191_vector_wallet_signs_a_transfer_that_lands() {
    let mut desk = Desk::new();
    let account = desk.add_eip191("Eth");
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    let landed = desk.land(&signed.wire);
    println!(
        "EIP-191 transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::Eip191.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
}

#[test]
fn an_mldsa44_vector_wallet_signs_a_transfer_that_lands() {
    let mut desk = Desk::new();
    let account = desk.add_mldsa("Dilithium");
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    let landed = desk.land(&signed.wire);
    println!(
        "ML-DSA-44 transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::MlDsa44.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
}

#[test]
fn a_falcon512_vector_wallet_signs_a_transfer_that_lands() {
    let mut desk = Desk::new();
    let account = desk.add_falcon("Falcon");
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    let landed = desk.land(&signed.wire);
    println!(
        "Falcon-512 transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::Falcon512.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
}

#[test]
fn an_xmss_vector_wallet_signs_a_transfer_that_lands() {
    let mut desk = Desk::new();
    let account = desk.add_xmss("Tree");
    let receiver = KeypairSigner::new_random().pubkey();
    let before = nonce_of(&desk, &account.address);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (1, 1));
    // A second press over the same digest reuses the leaf (retry cache).
    let again = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!(signed.wire, again.wire);
    let landed = desk.land(&signed.wire);
    println!(
        "XMSS transfer, rebuilt: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::Xmss.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    assert_ne!(nonce_of(&desk, &account.address), before);
    assert_eq!(
        VectorAccount::parse(
            &PROGRAM_ID,
            &desk.chain().account(&account.address).unwrap()
        )
        .unwrap()
        .next_leaf(),
        Some(1)
    );
}

#[test]
fn the_key_device_rotates_an_xmss_account_without_a_node_and_signs_with_the_next_key() {
    let mut desk = Desk::new();
    let account = desk.add_xmss("Tree");
    let receiver = KeypairSigner::new_random().pubkey();

    // One transaction lands, and a second is signed and never sent: the
    // account may be at either nonce, as far as the key device knows.
    let first = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    let signed = desk.sign(&first.wire, &first.context).unwrap();
    desk.land(&signed.wire);
    let second = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    desk.sign(&second.wire, &second.context).unwrap();

    // The key device signs the rotation at both, and the QR carries it.
    let rotation = desk.sign_rotation(account.wallet);
    assert_eq!(rotation.signed.len(), 2);
    // Pressed again, it signs at the same nonces, not at its own digests.
    let again = desk.sign_rotation(account.wallet);
    let nonces = |r: &wallet_core::protocol::KeyRotation| {
        r.signed.iter().map(|s| s.nonce).collect::<Vec<_>>()
    };
    assert_eq!(nonces(&again), nonces(&rotation));
    assert_eq!(again.next_key, rotation.next_key);
    let carried =
        match decode_object(&encode_object(&QrObject::KeyRotation(rotation.clone())).unwrap())
            .unwrap()
        {
            QrObject::KeyRotation(rotation) => rotation,
            other => panic!("{other:?}"),
        };
    let first_key = desk
        .wallets
        .iter()
        .find(|wallet| wallet.id == account.wallet)
        .and_then(|wallet| wallet.kind.vector_settings())
        .unwrap()
        .key
        .to_vec();

    // The online device picks the one at the account's nonce, and pays.
    let prepared = run(wallet_flows::vector::rotate(
        &desk.node,
        desk.payer.pubkey(),
        SchemeId::Xmss,
        &first_key,
        &carried,
    ))
    .unwrap();
    let paid = desk.sign(&prepared.wire, &[]).unwrap();
    assert_eq!((paid.authorizations, paid.signatures), (0, 1));
    let landed = desk.land(&paid.wire);
    println!(
        "XMSS rotation: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let held = VectorAccount::parse(
        &PROGRAM_ID,
        &desk.chain().account(&account.address).unwrap(),
    )
    .unwrap();
    assert_eq!(held.current_key(), Some(rotation.next_key.as_slice()));
    assert_eq!(held.next_leaf(), Some(0));

    // Sent again, it is refused: the account holds the key already.
    assert!(run(wallet_flows::vector::rotate(
        &desk.node,
        desk.payer.pubkey(),
        SchemeId::Xmss,
        &first_key,
        &carried,
    ))
    .is_err());

    // The next transaction's nonce is the rotation's digest, so the key
    // device installs the next key before it signs, and the transfer lands.
    let third = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    let signed = desk.sign(&third.wire, &third.context).unwrap();
    assert_eq!(signed.authorizations, 1);
    desk.land(&signed.wire);
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
    let settings = desk
        .wallets
        .iter()
        .find(|wallet| wallet.id == account.wallet)
        .unwrap();
    assert_eq!(settings.vector_prepared_next_key(), None);
}

#[test]
fn a_rotation_signed_before_the_account_moved_on_is_refused_online() {
    let mut desk = Desk::new();
    let account = desk.add_xmss("Tree");
    let receiver = KeypairSigner::new_random().pubkey();
    let first = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    desk.sign(&first.wire, &first.context).unwrap();
    // Made while the first transaction is out, at both nonces.
    let rotation = desk.sign_rotation(account.wallet);
    let first_key = desk
        .wallets
        .iter()
        .find(|wallet| wallet.id == account.wallet)
        .and_then(|wallet| wallet.kind.vector_settings())
        .unwrap()
        .key
        .to_vec();
    // Land the first, then another the online device built without the
    // key device: neither of the rotation's nonces is the account's now.
    let signed = desk.sign(&first.wire, &first.context).unwrap();
    desk.land(&signed.wire);
    let second = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    let signed = desk.sign(&second.wire, &second.context).unwrap();
    desk.land(&signed.wire);
    assert!(run(wallet_flows::vector::rotate(
        &desk.node,
        desk.payer.pubkey(),
        SchemeId::Xmss,
        &first_key,
        &rotation,
    ))
    .is_err());
}

const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// The rotation the key device signs, sent and paid for online, landed.
fn rotate_and_land(desk: &mut Desk, wallet: wallet_core::WalletId) -> Vec<u8> {
    let rotation = desk.sign_rotation(wallet);
    let first_key = desk
        .wallets
        .iter()
        .find(|w| w.id == wallet)
        .and_then(|w| w.kind.vector_settings())
        .unwrap()
        .key
        .to_vec();
    let prepared = run(wallet_flows::vector::rotate(
        &desk.node,
        desk.payer.pubkey(),
        SchemeId::Xmss,
        &first_key,
        &rotation,
    ))
    .unwrap();
    let paid = desk.sign(&prepared.wire, &[]).unwrap();
    desk.land(&paid.wire);
    rotation.next_key
}

fn phrase_index(desk: &Desk, wallet: wallet_core::WalletId) -> (u32, bool) {
    let phrase = desk
        .wallets
        .iter()
        .find(|w| w.id == wallet)
        .and_then(|w| w.vector_phrase())
        .unwrap();
    (phrase.index, phrase.restored)
}

#[test]
fn an_xmss_wallet_from_a_phrase_rotates_into_the_phrases_next_key() {
    let mut desk = Desk::new();
    let account = desk.add_xmss_phrase("Tree", PHRASE);
    let receiver = KeypairSigner::new_random().pubkey();
    let send = |desk: &mut Desk| {
        let adapted = desk.rebuild(
            account.address,
            vec![transfer(&account.address, &receiver, SOL / 8)],
        );
        let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
        desk.land(&signed.wire);
    };
    send(&mut desk);

    let next = rotate_and_land(&mut desk, account.wallet);
    assert_eq!(
        next,
        wallet_core::vector::phrase_key(PHRASE, SchemeId::Xmss, 1)
            .unwrap()
            .0
    );
    send(&mut desk);
    assert_eq!(phrase_index(&desk, account.wallet), (1, false));
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
}

#[test]
fn a_wallet_restored_from_its_phrase_finds_its_key_and_rotates_at_the_last_leaf() {
    let mut desk = Desk::new();
    let account = desk.add_xmss_phrase("Tree", PHRASE);
    let receiver = KeypairSigner::new_random().pubkey();
    let send = |desk: &mut Desk| -> Result<(), String> {
        let adapted = desk.rebuild(
            account.address,
            vec![transfer(&account.address, &receiver, SOL / 8)],
        );
        let signed = desk.sign(&adapted.wire, &adapted.context)?;
        desk.land(&signed.wire);
        Ok(())
    };
    send(&mut desk).unwrap();
    rotate_and_land(&mut desk, account.wallet);
    send(&mut desk).unwrap();

    // The device is lost. Typed in again, the phrase holds key 0 and has
    // no idea which leaves the lost copy used: it signs nothing.
    desk.restore_from_phrase(account.wallet, PHRASE);
    assert_eq!(phrase_index(&desk, account.wallet), (0, true));
    let refused = send(&mut desk).unwrap_err();
    assert!(refused.contains("restored"), "{refused}");

    // The online device shows the account's state; the key it holds is the
    // phrase's key 1.
    let adopted = desk.adopt_state(account.wallet, &account.address);
    assert_eq!(adopted.index, Some(1));

    // One signature, at the last leaf of key 1, moves it to key 2.
    let rotation = desk.sign_rotation(account.wallet);
    assert_eq!(rotation.signed.len(), 1);
    assert_eq!(
        u32::from_be_bytes(rotation.signed[0].signature[..4].try_into().unwrap()),
        wallet_core::vector::XMSS_LEAVES - 1
    );
    let first_key = wallet_core::vector::phrase_key(PHRASE, SchemeId::Xmss, 0)
        .unwrap()
        .0;
    let prepared = run(wallet_flows::vector::rotate(
        &desk.node,
        desk.payer.pubkey(),
        SchemeId::Xmss,
        &first_key,
        &rotation,
    ))
    .unwrap();
    let paid = desk.sign(&prepared.wire, &[]).unwrap();
    desk.land(&paid.wire);

    // The next transfer shows the rotation landed: key 2 is installed, the
    // wallet is whole again, and it lands.
    send(&mut desk).unwrap();
    assert_eq!(phrase_index(&desk, account.wallet), (2, false));
    assert_eq!(desk.chain().balance(&receiver), 3 * SOL / 8);
}

#[test]
fn a_winternitz_vector_wallet_does_not_sign() {
    // A Winternitz key signs once, and a transaction that fails on chain
    // leaves the account on it: anything else would be a second signature.
    // So the scheme is turned off, and a wallet of it never signs.
    let mut desk = Desk::new();
    let account = desk.add_winternitz("Once");
    let receiver = KeypairSigner::new_random().pubkey();
    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 4)],
    );
    match desk.sign(&adapted.wire, &adapted.context) {
        Err(_) => {}
        Ok(signed) => assert_eq!(signed.authorizations, 0),
    }
}

#[test]
fn an_xmss_wallet_may_skip_a_leaf_and_cannot_reuse_a_lower_one() {
    let mut desk = Desk::new();
    let account = desk.add_xmss("Skip");
    desk.set_xmss_leaf(&account.address, 3);
    let receiver = KeypairSigner::new_random().pubkey();

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    assert_eq!(adapted.context[0].leaf_floor, 3);
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    let landed = desk.land(&signed.wire);
    assert!(landed.compute_units > 0);
    assert_eq!(
        VectorAccount::parse(
            &PROGRAM_ID,
            &desk.chain().account(&account.address).unwrap()
        )
        .unwrap()
        .next_leaf(),
        Some(4)
    );

    // A signature made at leaf 0 while the chain wants 4 cannot land.
    desk.set_xmss_leaf(&account.address, 4);
    let stale = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 16)],
    );
    // Force leaf 0 into the Advance without updating the record past 4.
    let mut keys = wallet_core::vector::local_vector_keys(&desk.wallets);
    keys[0].leaf = Some(0);
    let ctx = AuthContext {
        vector: &stale.context,
    };
    let forged = wallet_core::relay::sign_vector_authorizations(&stale.wire, &keys, &ctx).unwrap();
    let mut empty = wallet_flows::sign::VectorSigning {
        keys: &[],
        known: &stale.context,
        cache: None,
        known_is_current: false,
    };
    let payer_signed = run(wallet_flows::sign::sign_with(
        Some(&desk.node),
        &forged.wire,
        &[&desk.payer as &dyn WalletSigner],
        &mut empty,
    ))
    .unwrap();
    assert!(desk.chain().send_wire(&payer_signed.wire).is_err());
}

#[test]
fn the_last_xmss_leaf_must_rotate_and_rotation_resets_the_chain() {
    let mut desk = Desk::new();
    let account = desk.add_xmss("Last");
    desk.set_xmss_leaf(&account.address, wallet_core::vector::XMSS_LEAVES - 1);
    let receiver = KeypairSigner::new_random().pubkey();

    // Without a next key, rebuild refuses past the force-rotate threshold.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        desk.rebuild(
            account.address,
            vec![transfer(&account.address, &receiver, SOL / 8)],
        )
    }));
    assert!(refused.is_err());

    let next_public = desk.prepare_xmss_rotation(account.wallet);

    let adapted = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 8)],
    );
    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    desk.land(&signed.wire);
    let read = VectorAccount::parse(
        &PROGRAM_ID,
        &desk.chain().account(&account.address).unwrap(),
    )
    .unwrap();
    assert_eq!(read.next_leaf(), Some(0));
    assert_eq!(read.current_key(), Some(next_public.as_slice()));

    // The new seed is installed once the chain holds its key, and the new
    // key signs from its own first leaf: the old key's marks do not carry
    // over.
    desk.install_rotation(account.wallet, &account.address);
    let wallet = desk
        .wallets
        .iter()
        .find(|w| w.id == account.wallet)
        .unwrap();
    assert!(wallet.vector_next_pubkey().is_none());
    let again = desk.rebuild(
        account.address,
        vec![transfer(&account.address, &receiver, SOL / 16)],
    );
    let signed = desk.sign(&again.wire, &again.context).unwrap();
    desk.land(&signed.wire);
    let read = VectorAccount::parse(
        &PROGRAM_ID,
        &desk.chain().account(&account.address).unwrap(),
    )
    .unwrap();
    assert_eq!(read.next_leaf(), Some(1));
    assert_eq!(desk.chain().balance(&receiver), SOL / 8 + SOL / 16);
}

#[test]
fn an_ed25519_and_a_secp256k1_account_sign_one_advance_together() {
    let mut desk = Desk::new();
    let ed = desk.add_vector("Ed", true);
    let secp = desk.add_secp("Secp");
    let receiver = KeypairSigner::new_random().pubkey();

    let adapted = desk.rebuild(
        desk.payer.pubkey(),
        vec![
            transfer(&ed.address, &receiver, SOL / 8),
            transfer(&secp.address, &receiver, SOL / 8),
        ],
    );
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    // One Advance for both, then one Passthrough that runs as both.
    assert_eq!(tx.instructions[0].data[0], ADVANCE_DISCRIMINATOR);
    assert_eq!(tx.instructions.len(), 2);

    let signed = desk.sign(&adapted.wire, &adapted.context).unwrap();
    assert_eq!((signed.authorizations, signed.signatures), (2, 1));
    let landed = desk.land(&signed.wire);
    println!(
        "Ed25519+secp256k1 Advance: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    let added = SchemeId::Ed25519.advance_cu() + SchemeId::Secp256k1.advance_cu() + PASSTHROUGH_CU;
    assert!(landed.compute_units < u64::from(added), "{landed:?}");
    assert_eq!(desk.chain().balance(&receiver), SOL / 4);
}

/// An initialized mint with no authorities.
fn mint_data(decimals: u8) -> Vec<u8> {
    let mut data = vec![0u8; 82];
    data[44] = decimals;
    data[45] = 1;
    data
}

/// An initialized token account of `mint` that `owner` controls.
fn token_data(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut data = vec![0u8; 165];
    data[..32].copy_from_slice(mint.as_ref());
    data[32..64].copy_from_slice(owner.as_ref());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    data[108] = 1;
    data
}
