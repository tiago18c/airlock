//! The request rewritten with the replaced addresses.

use super::*;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::tx::decode::parse_wire;
use crate::tx::signatures::apply_local_signatures;
use std::collections::BTreeMap;

#[test]
fn a_replaced_mint_gives_a_new_token_account_and_bump_on_every_format() {
    let site = Site::new();
    let fresh = pk(40);
    let payer = pk(41);
    let new_ata = associated_token_address(&site.owner, &fresh, &TOKEN_PROGRAM_ID);
    let new_bump = Pubkey::find_program_address(
        &[
            site.owner.as_ref(),
            TOKEN_PROGRAM_ID.as_ref(),
            fresh.as_ref(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .1;
    for format in FORMATS {
        let wire = site.wire(format);
        let before = parse_wire(&wire).unwrap();
        let replaced = BTreeMap::from([(site.mint, fresh), (site.payer, payer)]);
        let (wires, replacements) =
            apply_to_request(std::slice::from_ref(&wire), &site.notes(), &replaced).unwrap();

        assert_eq!(
            replacements.addresses,
            BTreeMap::from([
                (site.mint, fresh),
                (site.payer, payer),
                (site.ata(), new_ata)
            ]),
            "{format:?}"
        );
        assert_eq!(replacements.bumps, BTreeMap::from([(site.ata(), new_bump)]));

        let after = parse_wire(&wires[0]).unwrap();
        assert_eq!(after.format, before.format);
        // The same accounts in the same places, with the new addresses.
        let expected: Vec<Pubkey> = before
            .static_keys
            .iter()
            .map(|key| *replacements.addresses.get(key).unwrap_or(key))
            .collect();
        assert_eq!(after.static_keys, expected, "{format:?}");
        assert_eq!(
            after.num_required_signatures,
            before.num_required_signatures
        );

        let tx = decoded(&wires[0]);
        assert_eq!(&tx.instructions[1].data[1..33], fresh.as_ref());
        assert_eq!(tx.instructions[3].data, vec![9, new_bump]);
        assert!(tx.signers.iter().all(|slot| !slot.filled));
        assert_eq!(tx.fee_payer(), Some(payer));
    }
}

#[test]
fn signatures_made_over_the_old_message_are_dropped() {
    let site = Site::new();
    let mint = KeypairSigner::new_random();
    let site = Site {
        mint: mint.pubkey(),
        ..site
    };
    let signed =
        apply_local_signatures(&site.wire(Format::V1), &[&mint as &dyn WalletSigner]).unwrap();
    assert!(decoded(&signed).signers.iter().any(|slot| slot.valid));
    let (wires, _) = apply_to_request(&[signed], &site.notes(), &BTreeMap::new()).unwrap();
    assert!(decoded(&wires[0]).signers.iter().all(|slot| !slot.filled));
}

#[test]
fn only_an_address_a_note_lets_the_wallet_replace_is_replaced() {
    let site = Site::new();
    let replaced = BTreeMap::from([(site.owner, pk(40))]);
    assert_eq!(
        replacements(&site.notes(), &replaced),
        Err(NoteError::NotReplaceable {
            address: site.owner
        })
    );
    let mut notes = site.notes();
    notes.accounts.0[0].1 = AccountNote::New { replaceable: false };
    let replaced = BTreeMap::from([(site.mint, pk(40))]);
    assert_eq!(
        replacements(&notes, &replaced),
        Err(NoteError::NotReplaceable { address: site.mint })
    );
}

#[test]
fn two_accounts_with_one_address_are_refused() {
    let site = Site::new();
    let replaced = BTreeMap::from([(site.mint, site.owner)]);
    assert_eq!(
        apply_to_request(&[site.wire(Format::Legacy)], &site.notes(), &replaced).unwrap_err(),
        NoteError::SameAddress {
            transaction: 0,
            address: site.owner
        }
    );
}

#[test]
fn a_replaced_key_left_in_data_is_refused() {
    let site = Site::new();
    let mut notes = site.notes();
    notes.data.remove(0);
    let replaced = BTreeMap::from([(site.mint, pk(40))]);
    assert_eq!(
        apply_to_request(&[site.wire(Format::V0)], &notes, &replaced).unwrap_err(),
        NoteError::UndeclaredUse {
            address: site.mint,
            transaction: 0,
            instruction: 1,
            offset: 1
        }
    );
}

#[test]
fn derived_accounts_are_derived_again_to_any_depth() {
    let site = Site::new();
    // A program-derived account of the site's on the token account, which
    // rests on the mint.
    let deeper =
        |ata: &Pubkey| Pubkey::find_program_address(&[b"vault", ata.as_ref()], &SITE_PROGRAM).0;
    let mut notes = site.notes();
    notes.accounts.0.push((
        deeper(&site.ata()),
        AccountNote::Derived(Derivation::Pda(Pda {
            program: SITE_PROGRAM,
            seeds: vec![Seed::Bytes(b"vault".to_vec()), Seed::Account(site.ata())],
        })),
    ));
    let fresh = pk(40);
    let replacements = replacements(&notes, &BTreeMap::from([(site.mint, fresh)])).unwrap();
    let new_ata = associated_token_address(&site.owner, &fresh, &TOKEN_PROGRAM_ID);
    assert_eq!(replacements.addresses.get(&site.ata()), Some(&new_ata));
    assert_eq!(
        replacements.addresses.get(&deeper(&site.ata())),
        Some(&deeper(&new_ata))
    );
}

#[test]
fn derivations_in_a_loop_are_refused() {
    let derived_from = |other: Pubkey| {
        AccountNote::Derived(Derivation::Pda(Pda {
            program: SITE_PROGRAM,
            seeds: vec![Seed::Account(other)],
        }))
    };
    let notes = Notes {
        accounts: AccountNotes(vec![
            (pk(10), derived_from(pk(11))),
            (pk(11), derived_from(pk(10))),
        ]),
        data: Vec::new(),
    };
    assert!(matches!(
        replacements(&notes, &BTreeMap::new()),
        Err(NoteError::Loop { .. })
    ));
}

#[test]
fn a_with_seed_account_on_a_replaced_base_moves_with_it() {
    let base = pk(12);
    let owner = pk(13);
    let old = Pubkey::create_with_seed(&base, "stake:0", &owner).unwrap();
    let notes = Notes {
        accounts: AccountNotes(vec![
            (base, AccountNote::New { replaceable: true }),
            (
                old,
                AccountNote::Derived(Derivation::WithSeed(WithSeed {
                    base,
                    seed: "stake:0".into(),
                    owner,
                })),
            ),
        ]),
        data: Vec::new(),
    };
    let replacements = replacements(&notes, &BTreeMap::from([(base, pk(14))])).unwrap();
    assert_eq!(
        replacements.addresses.get(&old),
        Some(&Pubkey::create_with_seed(&pk(14), "stake:0", &owner).unwrap())
    );
    assert!(replacements.bumps.is_empty());
}
