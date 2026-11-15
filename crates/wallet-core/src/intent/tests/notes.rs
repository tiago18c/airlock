//! Notes read from the JSON a site sends, and checked against its
//! transactions.

use super::*;
use base64::Engine;

#[test]
fn a_request_reads_as_the_reference_writes_it() {
    let site = Site::new();
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let json = serde_json::json!({
        "id": "mint-1",
        "account": site.owner.to_string(),
        "chain": "solana:devnet",
        "transactions": [b64(&site.wire(Format::Legacy))],
        "delivery": "cosign",
        "accounts": {
            site.mint.to_string(): { "kind": "new", "replaceable": true },
            site.ata().to_string(): { "kind": "derived", "pda": {
                "program": ASSOCIATED_TOKEN_PROGRAM_ID.to_string(),
                "seeds": [
                    { "account": site.owner.to_string() },
                    { "bytes": b64(TOKEN_PROGRAM_ID.as_ref()) },
                    { "account": site.mint.to_string() },
                ],
            } },
            site.payer.to_string(): { "kind": "feePayer", "replaceable": true, "signs": "app" },
            pk(9).to_string(): { "kind": "signer", "signs": "app" },
            pk(8).to_string(): { "kind": "derived", "withSeed": {
                "base": site.owner.to_string(), "seed": "stake:0", "owner": pk(7).to_string(),
            } },
        },
        "data": [
            { "transaction": 0, "instruction": 1, "offset": 1, "account": site.mint.to_string() },
            { "transaction": 0, "instruction": 3, "offset": 1, "bumpOf": site.ata().to_string() },
        ],
    });
    let input: ResolveInput = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(input.id.as_deref(), Some("mint-1"));
    assert_eq!(input.order, Order::Serial);
    assert_eq!(input.delivery, Delivery::Cosign);
    assert_eq!(input.transactions, vec![site.wire(Format::Legacy)]);
    let expected = site.notes();
    assert_eq!(input.notes.data, expected.data);
    for (address, note) in expected.accounts.iter() {
        assert_eq!(input.notes.accounts.get(address), Some(note));
    }
    assert_eq!(
        input.notes.accounts.get(&pk(9)),
        Some(&AccountNote::Signer {
            signs: SignsBy::App
        })
    );
    assert!(matches!(
        input.notes.accounts.get(&pk(8)),
        Some(AccountNote::Derived(Derivation::WithSeed(WithSeed { seed, .. }))) if seed == "stake:0"
    ));

    // Written back, it reads the same.
    let again: ResolveInput =
        serde_json::from_value(serde_json::to_value(&input).unwrap()).unwrap();
    assert_eq!(again, input);
    let written = serde_json::to_value(&input).unwrap();
    assert_eq!(
        written["accounts"][site.ata().to_string()],
        json["accounts"][site.ata().to_string()]
    );
    assert_eq!(written["data"], json["data"]);
}

#[test]
fn what_a_request_leaves_out_takes_its_default() {
    let json = serde_json::json!({
        "account": pk(2).to_string(),
        "chain": "solana:mainnet",
        "transactions": [],
    });
    let input: ResolveInput = serde_json::from_value(json).unwrap();
    assert_eq!(input.id, None);
    assert_eq!(input.order, Order::Serial);
    assert_eq!(input.delivery, Delivery::Send);
    assert_eq!(input.notes, Notes::default());
}

#[test]
fn an_address_that_is_not_one_is_named() {
    let json = serde_json::json!({ "not-an-address": { "kind": "new", "replaceable": true } });
    let err = serde_json::from_value::<AccountNotes>(json).unwrap_err();
    assert!(err.to_string().contains("not-an-address is not an address"));
}

#[test]
fn ids_are_letters_digits_and_dashes() {
    assert!(valid_id("mint-1"));
    assert!(valid_id(&"a".repeat(80)));
    assert!(!valid_id(""));
    assert!(!valid_id(&"a".repeat(81)));
    assert!(!valid_id("mint_1"));
}

#[test]
fn true_notes_pass_on_every_format() {
    let site = Site::new();
    for format in FORMATS {
        let tx = decoded(&site.wire(format));
        assert_eq!(
            check_notes(&[tx], &site.notes(), &site.owner),
            Ok(()),
            "{format:?}"
        );
    }
}

fn check(notes: &Notes) -> Result<(), NoteError> {
    let site = Site::new();
    check_notes(&[decoded(&site.wire(Format::Legacy))], notes, &site.owner)
}

#[test]
fn a_derivation_that_gives_another_address_is_refused() {
    let site = Site::new();
    let mut notes = site.notes();
    // The seeds in the wrong order.
    notes.accounts.0[1].1 = AccountNote::Derived(Derivation::Pda(Pda {
        program: ASSOCIATED_TOKEN_PROGRAM_ID,
        seeds: vec![
            Seed::Account(site.mint),
            Seed::Bytes(TOKEN_PROGRAM_ID.to_bytes().to_vec()),
            Seed::Account(site.owner),
        ],
    }));
    assert!(matches!(
        check(&notes),
        Err(NoteError::WrongDerivation { address, .. }) if address == site.ata()
    ));
}

#[test]
fn a_new_account_that_does_not_sign_is_refused() {
    let site = Site::new();
    let mut notes = site.notes();
    notes
        .accounts
        .0
        .push((site.ata(), AccountNote::New { replaceable: true }));
    notes.accounts.0.remove(1);
    notes.data.remove(1);
    assert_eq!(
        check(&notes),
        Err(NoteError::NewDoesNotSign {
            address: site.ata()
        })
    );
}

#[test]
fn an_address_with_two_notes_is_refused() {
    let site = Site::new();
    let mut notes = site.notes();
    notes
        .accounts
        .0
        .push((site.mint, AccountNote::New { replaceable: false }));
    assert_eq!(
        check(&notes),
        Err(NoteError::TwoNotes { address: site.mint })
    );
    // Read from JSON, the second stays to be refused.
    let json = format!(
        r#"{{"{0}":{{"kind":"new","replaceable":true}},"{0}":{{"kind":"new","replaceable":false}}}}"#,
        site.mint
    );
    let read: AccountNotes = serde_json::from_str(&json).unwrap();
    assert_eq!(read.0.len(), 2);
}

#[test]
fn the_account_acted_as_carries_no_note() {
    let site = Site::new();
    // As the fee payer, or as anything else.
    for note in [
        AccountNote::FeePayer {
            replaceable: true,
            signs: None,
        },
        AccountNote::FeePayer {
            replaceable: false,
            signs: Some(SignsBy::App),
        },
        AccountNote::Signer {
            signs: SignsBy::App,
        },
    ] {
        let mut notes = site.notes();
        notes.accounts.0.push((site.owner, note));
        let err = check(&notes).unwrap_err();
        assert_eq!(
            err,
            NoteError::OnAccount {
                address: site.owner
            }
        );
        assert_eq!(err.code(), "bad_notes");
    }
    // The account paying for itself with no note is the wallet's affair.
    let own = wire_of(Format::V1, &site.instructions(), &site.owner, &[]);
    let mut notes = site.notes();
    notes
        .accounts
        .0
        .retain(|(address, _)| *address != site.payer);
    assert_eq!(check_notes(&[decoded(&own)], &notes, &site.owner), Ok(()));
}

#[test]
fn a_note_on_an_address_no_transaction_lists_is_refused() {
    let mut notes = Site::new().notes();
    notes.accounts.0.push((
        pk(60),
        AccountNote::Signer {
            signs: SignsBy::App,
        },
    ));
    assert_eq!(
        check(&notes),
        Err(NoteError::NotInTransactions { address: pk(60) })
    );
}

#[test]
fn a_note_on_an_account_read_from_a_lookup_table_is_refused() {
    let site = Site::new();
    let table = AddressLookupTableAccount {
        key: pk(50),
        addresses: vec![site.ata()],
    };
    let wire = wire_of(
        Format::V0,
        &site.instructions(),
        &site.payer,
        std::slice::from_ref(&table),
    );
    let mut tables = LookupTables::new();
    tables.insert(table.key, table.addresses);
    let tx = decode_transaction(&wire, &tables).unwrap();
    assert!(tx
        .accounts
        .iter()
        .any(|account| account.pubkey == site.ata() && account.lookup == Some(pk(50))));
    assert_eq!(
        check_notes(&[tx], &site.notes(), &site.owner),
        Err(NoteError::FromLookupTable {
            transaction: 0,
            address: site.ata()
        })
    );
}

#[test]
fn a_data_entry_out_of_range_or_naming_other_bytes_is_refused() {
    let site = Site::new();
    let with_entry = |entry: DataNote| {
        let mut notes = site.notes();
        notes.data[0] = entry;
        check(&notes)
    };
    let entry = |instruction, offset, holds| DataNote {
        transaction: 0,
        instruction,
        offset,
        holds,
    };
    // The init instruction's data is 33 bytes.
    assert!(matches!(
        with_entry(entry(1, 2, DataHolds::Account(site.mint))),
        Err(NoteError::OutOfRange { entry: 0, .. })
    ));
    assert!(matches!(
        with_entry(entry(1, usize::MAX, DataHolds::Account(site.mint))),
        Err(NoteError::OutOfRange { entry: 0, .. })
    ));
    // The bytes there are the mint, not the owner.
    assert_eq!(
        with_entry(entry(1, 1, DataHolds::Account(site.owner))),
        Err(NoteError::NotThatAddress {
            entry: 0,
            offset: 1,
            address: site.owner
        })
    );
    assert!(matches!(
        with_entry(entry(9, 0, DataHolds::Account(site.mint))),
        Err(NoteError::NoSuchInstruction {
            entry: 0,
            count: 4,
            ..
        })
    ));
    assert!(matches!(
        with_entry(DataNote {
            transaction: 1,
            ..entry(1, 1, DataHolds::Account(site.mint))
        }),
        Err(NoteError::NoSuchTransaction {
            entry: 0,
            count: 1,
            ..
        })
    ));

    // A bump at the wrong byte, and the bump of what has no pda note.
    let mut notes = site.notes();
    notes.data[1].offset = 0;
    assert!(matches!(
        check(&notes),
        Err(NoteError::NotThatBump { entry: 1, .. })
    ));
    let mut notes = site.notes();
    notes.data[1].holds = DataHolds::BumpOf(site.mint);
    assert_eq!(
        check(&notes),
        Err(NoteError::BumpOfNotPda {
            entry: 1,
            address: site.mint
        })
    );
}

#[test]
fn a_replaceable_key_in_data_no_entry_names_is_refused() {
    let site = Site::new();
    let mut notes = site.notes();
    notes.data.remove(0);
    assert_eq!(
        check(&notes),
        Err(NoteError::UndeclaredUse {
            address: site.mint,
            transaction: 0,
            instruction: 1,
            offset: 1
        })
    );
    // Not replaceable, the app signs for it and nothing has to be declared.
    notes.accounts.0[0].1 = AccountNote::New { replaceable: false };
    assert_eq!(check(&notes), Ok(()));
}

#[test]
fn a_derived_account_on_a_replaceable_one_has_its_uses_declared_too() {
    let site = Site::new();
    let mut instructions = site.instructions();
    instructions[3].data.extend_from_slice(site.ata().as_ref());
    let tx = decoded(&wire_of(Format::V1, &instructions, &site.payer, &[]));
    assert_eq!(
        check_notes(&[tx], &site.notes(), &site.owner),
        Err(NoteError::UndeclaredUse {
            address: site.ata(),
            transaction: 0,
            instruction: 3,
            offset: 2
        })
    );
}

#[test]
fn every_error_names_what_is_at_fault() {
    let err = NoteError::UndeclaredUse {
        address: pk(3),
        transaction: 0,
        instruction: 1,
        offset: 4,
    };
    assert!(err.to_string().contains(&pk(3).to_string()));
    assert_eq!(err.code(), "bad_notes");
    let entry = NoteError::NotThatAddress {
        entry: 2,
        offset: 4,
        address: pk(3),
    };
    assert!(entry.to_string().starts_with("data[2]"));
}
