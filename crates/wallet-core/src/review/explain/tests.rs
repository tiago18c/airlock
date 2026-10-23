use super::*;
use crate::fixtures::{self, compile};
use crate::idl::catalog::{IdlStore, MemoryBodies};
use crate::ids::{
    COMPUTE_BUDGET_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use crate::tx::builder::compile_v1;
use crate::tx::decode::decode_v1_transaction;
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_system_interface::instruction::transfer;
use wallet_vault::store::MemoryStore;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn store_with(program: Pubkey) -> IdlStore<MemoryStore, MemoryBodies> {
    let mut store = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
    let bytes = format!(
        r#"{{
            "name": "pinger",
            "version": "0.1.0",
            "metadata": {{ "address": "{program}", "origin": "shank" }},
            "instructions": [{{
                "name": "ping",
                "accounts": [{{ "name": "payer", "isMut": true, "isSigner": true }}],
                "args": [{{ "name": "amount", "type": "u64" }}]
            }}]
        }}"#
    );
    store.import(bytes.as_bytes(), None).unwrap();
    store
}

fn message_with(program: Pubkey, data: Vec<u8>) -> DecodedTx {
    let payer = pk(1);
    let dest = pk(2);
    let ping = Instruction {
        program_id: program,
        accounts: vec![AccountMeta {
            pubkey: payer,
            is_signer: true,
            is_writable: true,
        }],
        data,
    };
    let compiled = compile_v1(
        payer,
        vec![transfer(&payer, &dest, 5), ping],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    decode_v1_transaction(&compiled.wire).unwrap()
}

#[test]
fn imported_idl_explains_only_the_unknown_program() {
    let program = pk(7);
    let store = store_with(program);
    let mut data = vec![0u8];
    data.extend_from_slice(&42u64.to_le_bytes());
    let tx = message_with(program, data);
    let explained = explain_transaction(&store, &tx);

    assert_eq!(explained.programs.len(), 2);
    assert_eq!(explained.programs[0].program_id, SYSTEM_PROGRAM_ID);
    assert!(explained.programs[0].name.is_none());
    assert!(explained.programs[0].origin.is_none());
    assert_eq!(explained.programs[1].origin, Some(IdlOrigin::File));
    assert_eq!(explained.programs[1].name.as_deref(), Some("pinger"));
    assert!(explained.programs[1].sha256.is_some());
    assert_eq!(explained.programs[1].status, IdlPinStatus::NotChecked);

    // This store has no System IDL, so the transfer stays raw.
    assert!(explained.instructions[0].idl.is_none());
    assert_eq!(explained.instructions[0].program_id, SYSTEM_PROGRAM_ID);
    assert_eq!(explained.instructions[0].accounts.len(), 2);

    let idl = explained.instructions[1].idl.clone().unwrap();
    assert_eq!(idl.intent, "Ping");
    assert!(idl.interpolated_intent.is_none());
    assert!(idl
        .fields
        .iter()
        .any(|field| field.label == "Amount" && field.value == "42"));
    assert_eq!(idl.accounts.len(), 1);
    assert_eq!(idl.accounts[0].name, "payer");
    assert!(idl.accounts[0].signer && idl.accounts[0].writable);
    assert_eq!(idl.idl_sha256, explained.programs[1].sha256.unwrap());
}

#[test]
fn a_remote_mismatch_does_not_change_the_explanation() {
    let program = pk(7);
    let mut store = store_with(program);
    let mut data = vec![0u8];
    data.extend_from_slice(&42u64.to_le_bytes());
    let tx = message_with(program, data);
    let before = explain_transaction(&store, &tx);
    let check = store
        .record_check(&program, br#"{"name":"other"}"#)
        .unwrap();
    assert!(!check.same);
    let after = explain_transaction(&store, &tx);
    assert_eq!(after.programs[1].status, IdlPinStatus::Differs);
    assert_eq!(after.instructions[1].idl, before.instructions[1].idl);
}

#[test]
fn a_mismatched_pin_does_not_supply_account_bytes() {
    let program = pk(7);
    let store = store_with(program);
    let local = store.records()[0].sha256;
    let mut data = vec![0u8];
    data.extend_from_slice(&42u64.to_le_bytes());
    let tx = message_with(program, data);
    let mint = pk(4);
    let mut accounts = std::collections::HashMap::new();
    accounts.insert(mint, vec![9u8]);
    let differs = DisplayNote {
        pins: vec![(program, [0; 32])],
        accounts: accounts.clone(),
        schemes: Default::default(),
        keys: Default::default(),
    };
    assert!(!note_supplies_accounts(&program, Some(local), &differs));
    let explained = explain_with_note(&store, &tx, &differs);
    assert_eq!(
        explained.programs[1].context_pin,
        Some(ContextPinMatch::Differs)
    );
    let same = DisplayNote {
        pins: vec![(program, local)],
        accounts,
        schemes: Default::default(),
        keys: Default::default(),
    };
    assert!(note_supplies_accounts(&program, Some(local), &same));
    let matched = explain_with_note(&store, &tx, &same);
    assert_eq!(matched.programs[1].context_pin, Some(ContextPinMatch::Same));
    assert_eq!(matched.instructions[1].idl, explained.instructions[1].idl);
    assert!(note_supplies_accounts(
        &SYSTEM_PROGRAM_ID,
        None,
        &DisplayNote::default()
    ));
}

#[test]
fn bytes_the_idl_cannot_parse_stay_raw() {
    let program = pk(7);
    let store = store_with(program);
    let data = vec![9, 0, 0, 0, 0, 0, 0, 0, 0];
    let tx = message_with(program, data.clone());
    let explained = explain_transaction(&store, &tx);
    let raw = &explained.instructions[1];
    assert!(raw.idl.is_none());
    assert_eq!(raw.program_id, program);
    assert_eq!(raw.accounts, vec![pk(1)]);
    assert_eq!(raw.data, data);
}

fn builtins() -> IdlStore<MemoryStore, MemoryBodies> {
    IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins()
}

fn explain_one_ix(
    store: &IdlStore<MemoryStore, MemoryBodies>,
    payer: Pubkey,
    ix: Instruction,
) -> IdlExplanation {
    let tx = decode_v1_transaction(&compile(payer, vec![ix]).wire).unwrap();
    explain_transaction(store, &tx).instructions[0]
        .idl
        .clone()
        .expect("explained from an IDL")
}

fn field<'a>(idl: &'a IdlExplanation, label: &str) -> &'a str {
    idl.fields
        .iter()
        .find(|field| field.label == label)
        .map(|field| field.value.as_str())
        .unwrap_or_else(|| panic!("no {label} in {:?}", idl.fields))
}

#[test]
fn a_system_transfer_reads_as_a_sentence_with_its_amount_in_sol() {
    let idl = explain_one_ix(
        &builtins(),
        pk(1),
        fixtures::transfer(pk(1), pk(2), 1_500_000_000),
    );
    assert_eq!(idl.intent, "Transfer SOL");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(format!("Transfer 1.5 SOL from {} to {}", pk(1), pk(2)).as_str())
    );
    assert_eq!(field(&idl, "Amount"), "1.5 SOL");
    assert_eq!(idl.unnamed_accounts, 0);
}

#[test]
fn a_token_transfer_names_its_mint_authority_and_scaled_amount() {
    for program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
        let ix =
            fixtures::token_transfer_checked(program, pk(4), pk(6), pk(5), pk(1), 1_500_000, 6);
        let idl = explain_one_ix(&builtins(), pk(1), ix);
        assert_eq!(idl.intent, "Transfer tokens");
        assert_eq!(field(&idl, "Amount"), "1.5");
        assert_eq!(field(&idl, "From"), pk(4).to_string());
        assert_eq!(field(&idl, "To"), pk(5).to_string());
        assert_eq!(field(&idl, "Mint"), pk(6).to_string());
        assert_eq!(field(&idl, "Authority"), pk(1).to_string());
        assert_eq!(idl.unnamed_accounts, 0);
    }
}

#[test]
fn creating_an_associated_account_names_the_wallet_and_mint() {
    // Read from the associated-token program the Token IDL carries.
    for token_program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
        let ix = fixtures::create_ata_idempotent(pk(1), pk(8), pk(9), token_program);
        let idl = explain_one_ix(&builtins(), pk(1), ix);
        assert_eq!(idl.intent, "Create associated token account");
        assert_eq!(field(&idl, "Owner"), pk(8).to_string());
        assert_eq!(field(&idl, "Mint"), pk(9).to_string());
        assert_eq!(field(&idl, "Token Program"), token_program.to_string());
    }
}

#[test]
fn compute_budget_instructions_are_explained() {
    let payer = pk(1);
    // SetComputeUnitLimit: tag 2, units as u32 LE.
    let set_limit = Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data: [vec![2u8], 200_000u32.to_le_bytes().to_vec()].concat(),
    };
    let compiled = compile_v1(
        payer,
        vec![set_limit, fixtures::transfer(payer, pk(2), 5)],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    let tx = decode_v1_transaction(&compiled.wire).unwrap();
    let explained = explain_transaction(&builtins(), &tx);
    let limit = explained.instructions[0].idl.as_ref().expect("explained");
    assert!(
        limit.fields.iter().any(|field| field.value.contains("200")),
        "{:?}",
        limit.fields
    );
    assert!(explained.instructions[1].idl.is_some());
    assert!(explained
        .programs
        .iter()
        .all(|line| line.origin == Some(IdlOrigin::Builtin)));
}

/// The embedded stake IDL describes the layout without sysvar accounts.
/// Older clients send three more, the clock, the stake history and the
/// unused stake config, and then the IDL's names land on the wrong accounts.
#[test]
fn an_instruction_with_more_accounts_than_its_idl_names_is_flagged() {
    let (stake, vote, authority) = (pk(11), pk(12), pk(1));
    let newer = fixtures::stake_delegate(stake, authority, vote);
    assert_eq!(newer.accounts.len(), 3);
    let idl = explain_one_ix(&builtins(), authority, newer.clone());
    assert_eq!(idl.intent, "Delegate stake");
    assert_eq!(idl.unnamed_accounts, 0);
    assert_eq!(field(&idl, "Stake Authority"), authority.to_string());

    let legacy = [
        "SysvarC1ock11111111111111111111111111111111",
        "SysvarStakeHistory1111111111111111111111111",
        "StakeConfig11111111111111111111111111111111",
    ]
    .map(|id| AccountMeta::new_readonly(Pubkey::from_str_const(id), false));
    let older = Instruction {
        accounts: [&newer.accounts[..2], &legacy, &newer.accounts[2..]].concat(),
        ..newer
    };
    let idl = explain_one_ix(&builtins(), authority, older);
    assert_eq!(idl.unnamed_accounts, 3);
    assert_ne!(field(&idl, "Stake Authority"), authority.to_string());
}

#[test]
fn an_imported_idl_takes_over_from_the_embedded_one_and_says_so() {
    let mut store = builtins();
    let bytes = format!(
        r#"{{
            "name": "imported-system",
            "metadata": {{ "address": "{SYSTEM_PROGRAM_ID}", "origin": "shank" }},
            "instructions": [{{ "name": "ping", "accounts": [], "args": [] }}]
        }}"#
    );
    store.import(bytes.as_bytes(), None).unwrap();
    let tx = decode_v1_transaction(&compile(pk(1), vec![fixtures::transfer(pk(1), pk(2), 5)]).wire)
        .unwrap();
    let explained = explain_transaction(&store, &tx);
    assert_eq!(explained.programs[0].origin, Some(IdlOrigin::File));
    assert_eq!(
        explained.programs[0].name.as_deref(),
        Some("importedSystem")
    );
    // The imported file does not describe a transfer, so the row is raw.
    assert!(explained.instructions[0].idl.is_none());
}

/// A hot wallet relays what a programmatic signer authorized: storing a
/// payment for a Squads vault. The payment is three levels down.
#[test]
fn instructions_inside_instructions_are_opened_and_explained() {
    use crate::wallets::programmatic::{
        programmatic_signer_address, ExecutorNonce, MESSAGE_EXECUTOR_PROGRAM_ID,
    };
    use crate::wallets::relay::wrap_for_programmatic;
    use crate::wallets::squads::Squad;

    let (hot, authority) = (pk(1), pk(5));
    let member = programmatic_signer_address(&authority);
    let squad = Squad::v4(&pk(6));
    let vault = squad.vault(0);
    let payment = transfer(&vault, &pk(2), 1_000_000_000);
    let built = compile_v1(
        hot,
        vec![squad
            .create_transaction(1, 0, std::slice::from_ref(&payment), &member, &hot)
            .unwrap()],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    let wrapped = wrap_for_programmatic(
        &decode_v1_transaction(&built.wire).unwrap(),
        &[authority],
        ExecutorNonce {
            account: pk(8),
            authority: member,
            value: Hash::new_from_array([3; 32]),
        },
        hot,
        Hash::new_from_array([9; 32]),
    )
    .unwrap();
    let tx = decode_v1_transaction(&wrapped).unwrap();
    let store = IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins();
    let explained = explain_transaction(&store, &tx);

    assert_eq!(explained.instructions.len(), 1);
    let submit = explained.instructions[0].inner.as_ref().unwrap();
    assert_eq!(
        submit.kind,
        InnerKind::Authorization {
            authorities: vec![AuthorizingKey {
                authority,
                address: member,
                signed: false,
            }],
            forwarded: vec![hot],
        }
    );
    let execute = &submit.instructions[0];
    assert_eq!(execute.program_id, MESSAGE_EXECUTOR_PROGRAM_ID);
    let execution = execute.inner.as_ref().unwrap();
    assert!(matches!(
        execution.kind,
        InnerKind::Execution { nonce_account, nonce_authority, .. }
            if nonce_account == pk(8) && nonce_authority == member
    ));
    let store_tx = &execution.instructions[0];
    assert_eq!(store_tx.program_id, squad.program.program_id());
    let worded = store_tx.idl.as_ref().unwrap();
    assert_eq!(
        worded.interpolated_intent.as_deref(),
        Some(
            format!(
                "Vault transaction {} created by {member} for multisig {}",
                squad.transaction(1),
                squad.address
            )
            .as_str()
        )
    );
    // Neither the stored message bytes nor the System program are listed.
    assert!(worded
        .fields
        .iter()
        .all(|field| field.label != "Transaction Message"
            && field.value != SYSTEM_PROGRAM_ID.to_string()));
    let stored = store_tx.inner.as_ref().unwrap();
    assert_eq!(
        stored.kind,
        InnerKind::SquadsTransaction {
            vault,
            vault_index: 0,
        }
    );
    // The payment itself, read by the System program's IDL.
    let pay = &stored.instructions[0];
    assert_eq!(pay.program_id, SYSTEM_PROGRAM_ID);
    assert_eq!(pay.data, payment.data);
    assert!(pay.idl.is_some());
    assert!(pay.inner.is_none());

    // Every program on the way down is listed. The Squads IDL ships with
    // the app, so it is not one to fetch.
    let listed: Vec<Pubkey> = explained
        .programs
        .iter()
        .map(|line| line.program_id)
        .collect();
    assert_eq!(
        listed,
        vec![
            ED25519_SIGNER_PROGRAM_ID,
            MESSAGE_EXECUTOR_PROGRAM_ID,
            squad.program.program_id(),
            SYSTEM_PROGRAM_ID,
        ]
    );
    let missing = missing_catalog_programs(&store, &tx);
    assert!(!missing.contains(&squad.program.program_id()));
    assert!(!missing.contains(&SYSTEM_PROGRAM_ID));
}

/// A Vector wrap opens its Passthrough from its bytes alone, pasted or
/// scanned with no note; what it carries reads the same as explaining those
/// instructions on their own.
#[test]
fn a_vector_passthrough_opens_to_what_it_carries() {
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::wallets::relay::{wrap_for_vector, VectorWrap};
    use crate::wallets::vector::{address, parse_passthrough, SchemeId, PROGRAM_ID};

    let payer = pk(1);
    let key = KeypairSigner::new_random();
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, key.pubkey().as_ref()).unwrap();
    let payment = transfer(&account, &pk(2), 1_000_000_000);
    let built = compile_v1(
        payer,
        vec![payment],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    let site = decode_v1_transaction(&built.wire).unwrap();
    let wrap = VectorWrap {
        account,
        scheme: SchemeId::Ed25519,
        key: key.pubkey().to_bytes().to_vec(),
        next_key: None,
    };
    let wire = wrap_for_vector(&site, &[wrap], payer, Hash::new_from_array([2; 32])).unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    let store = IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins();

    let explained = explain_transaction(&store, &tx);
    assert_eq!(explained.instructions[0].program_id, PROGRAM_ID);
    let advance = explained.instructions[0].idl.as_ref().unwrap();
    assert_eq!(
        advance.interpolated_intent.as_deref(),
        Some(format!("Vector {account} signs and authorizes the nonce advance").as_str())
    );
    // The signature blob and the sysvar are not shown.
    assert_eq!(advance.fields.len(), 1);

    let passthrough = explained.instructions[1].inner.as_ref().unwrap();
    assert_eq!(
        passthrough.kind,
        InnerKind::Passthrough {
            signers: vec![RunsAs {
                account,
                system: None,
                next_key: None,
            }],
        }
    );
    let carried = parse_passthrough(&tx.native_instructions()[1]).unwrap();
    let bare = explain_instructions(&store, &carried.instructions);
    assert_eq!(passthrough.instructions[0].idl, bare[0].idl);
    assert!(passthrough.instructions[0].idl.is_some());
}

/// Squads sentences read fields inside the `args` struct by dotted path.
#[test]
fn squads_sentences_name_values_from_inside_their_args() {
    use crate::wallets::squads::{Squad, SquadsMember, SquadsSetup, PERMISSION_ALL};

    let (creator, treasury) = (pk(1), pk(9));
    let setup = SquadsSetup {
        members: vec![
            SquadsMember {
                key: creator,
                permissions: PERMISSION_ALL,
            },
            SquadsMember {
                key: pk(2),
                permissions: PERMISSION_ALL,
            },
        ],
        threshold: 2,
        time_lock: 0,
    };
    let (squad, create) = Squad::create_v4(&pk(3), &creator, &treasury, &setup);
    let store = builtins();

    let created = explain_one_ix(&store, creator, create);
    assert_eq!(
        created.interpolated_intent.as_deref(),
        Some(
            format!(
                "Multisig {} created by {creator} with threshold 2",
                squad.address
            )
            .as_str()
        )
    );

    let proposal = explain_one_ix(
        &store,
        creator,
        squad.create_proposal(7, &creator, &creator),
    );
    assert_eq!(
        proposal.interpolated_intent.as_deref(),
        Some(
            format!(
                "Proposal {} for transaction #7 created by {creator} for multisig {}",
                squad.proposal(7),
                squad.address
            )
            .as_str()
        )
    );
}

/// What an `Advance` does to a hash-based key is read from its layout: by
/// length alone for one account, or with the schemes a wallet record gives.
/// An XMSS account that names the key it holds keeps it.
#[test]
fn the_key_an_advance_hands_a_hash_based_account_to_is_shown_on_its_passthrough() {
    use crate::wallets::relay::{wrap_for_vector, VectorContext, VectorWrap};
    use crate::wallets::vector::{address, SchemeId, PROGRAM_ID};
    use crate::wallets::{Authority, VaultData, WalletRecord};

    let payer = pk(1);
    let store = IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins();
    let next_key_of =
        |explained: &ExplainedTx| match &explained.instructions[1].inner.as_ref().unwrap().kind {
            InnerKind::Passthrough { signers } => signers[0].next_key.clone(),
            other => panic!("not a Passthrough: {other:?}"),
        };

    for scheme in [SchemeId::Winternitz, SchemeId::Xmss] {
        let first = vec![5u8; scheme.key_len()];
        let next = vec![6u8; scheme.key_len()];
        let account = address(&PROGRAM_ID, scheme, &first).unwrap();
        let built = compile_v1(
            payer,
            vec![transfer(&account, &pk(2), 1_000)],
            None,
            Some(Hash::new_from_array([9; 32])),
        )
        .unwrap();
        let site = decode_v1_transaction(&built.wire).unwrap();
        let wrapped = |next_key: &[u8]| {
            let wrap = VectorWrap {
                account,
                scheme,
                key: first.clone(),
                next_key: Some(next_key.to_vec()),
            };
            let wire =
                wrap_for_vector(&site, &[wrap], payer, Hash::new_from_array([2; 32])).unwrap();
            decode_v1_transaction(&wire).unwrap()
        };
        let rotates = wrapped(&next);

        // With nothing known, the Advance of one account is laid out by its
        // length alone, which tells Winternitz from XMSS.
        let expected = match scheme {
            SchemeId::Winternitz => NextKey::Replaces(next.clone()),
            // Nothing says which key the account holds now.
            _ => NextKey::Names(next.clone()),
        };
        assert_eq!(
            next_key_of(&explain_transaction(&store, &rotates)),
            Some(expected.clone())
        );

        let data = VaultData {
            wallets: vec![WalletRecord::vector(
                "Watched",
                scheme,
                PROGRAM_ID,
                first.clone(),
                Authority::of(None),
            )],
            selected: None,
        };
        let from_wallets = DisplayNote::default().with_wallet_schemes(&data.summaries());
        let shown = next_key_of(&explain_with_note(&store, &rotates, &from_wallets));
        // The record does not say which key the account holds now either.
        assert_eq!(shown, Some(expected), "{scheme:?}");

        if scheme == SchemeId::Xmss {
            let read = |current: &[u8]| {
                DisplayNote::default().with_vector_schemes(&[VectorContext {
                    account,
                    scheme,
                    nonce: None,
                    key: Some(current.to_vec()),
                    leaf_floor: 0,
                }])
            };
            let keeps = wrapped(&first);
            assert_eq!(
                next_key_of(&explain_with_note(&store, &keeps, &read(&first))),
                None
            );
            assert_eq!(
                next_key_of(&explain_with_note(&store, &rotates, &read(&first))),
                Some(NextKey::Replaces(next.clone()))
            );
        }
    }
}

/// The data of a devnet `swap_v2`, with the pool's bitmap extension and two
/// tick arrays after the named accounts.
#[test]
fn a_raydium_swap_names_its_mints_pool_and_limits() {
    use crate::idl::builtin::RAYDIUM_CLMM_PROGRAM_ID;

    let mut data = hex::decode("2b04ed0b1ac91e62").unwrap();
    data.extend(6_708_044_446_768u64.to_le_bytes());
    data.extend(1_622_897_189u64.to_le_bytes());
    data.extend(0u128.to_le_bytes());
    data.push(1);
    let mut accounts = vec![AccountMeta::new_readonly(pk(1), true)];
    accounts.extend((2..=8).map(|byte| AccountMeta::new(pk(byte), false)));
    accounts.extend([
        AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        AccountMeta::new_readonly(
            Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
            false,
        ),
        AccountMeta::new_readonly(pk(9), false),
        AccountMeta::new_readonly(pk(10), false),
    ]);
    accounts.extend((11..=13).map(|byte| AccountMeta::new(pk(byte), false)));
    let swap = Instruction {
        program_id: RAYDIUM_CLMM_PROGRAM_ID,
        accounts,
        data,
    };

    let idl = explain_one_ix(&builtins(), pk(1), swap);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{} swaps {} for {} in pool {}, from {} to {}",
                pk(1),
                pk(9),
                pk(10),
                pk(3),
                pk(4),
                pk(5)
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Amount"), "6708044446768 base units");
    assert_eq!(
        field(&idl, "Minimum Out or Maximum In"),
        "1622897189 base units"
    );
    assert_eq!(field(&idl, "Exact Input"), "true");
    // The tick arrays are read by the program, so they are not unnamed, and
    // neither they nor the programs are listed.
    assert_eq!(idl.unnamed_accounts, 0);
    assert!(idl
        .fields
        .iter()
        .all(|field| field.value != pk(11).to_string()
            && field.value != TOKEN_PROGRAM_ID.to_string()));
}

/// A `swap_base_input` on the CP swap program, with the accounts in the
/// order its IDL lists them.
#[test]
fn a_raydium_cp_swap_names_its_mints_pool_and_limits() {
    use crate::idl::builtin::RAYDIUM_CP_SWAP_PROGRAM_ID;

    let mut data = hex::decode("8fbe5adac41e33de").unwrap();
    data.extend(2_500_000u64.to_le_bytes());
    data.extend(1_900_000u64.to_le_bytes());
    let mut accounts = vec![
        AccountMeta::new_readonly(pk(1), true),
        AccountMeta::new_readonly(pk(2), false),
        AccountMeta::new_readonly(pk(3), false),
    ];
    accounts.extend((4..=8).map(|byte| AccountMeta::new(pk(byte), false)));
    accounts.extend([
        AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        AccountMeta::new_readonly(pk(9), false),
        AccountMeta::new_readonly(pk(10), false),
        AccountMeta::new(pk(11), false),
    ]);
    let swap = Instruction {
        program_id: RAYDIUM_CP_SWAP_PROGRAM_ID,
        accounts,
        data,
    };

    let idl = explain_one_ix(&builtins(), pk(1), swap);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{} swaps 2500000 base units of {} for at least 1900000 base units of {} in pool {}",
                pk(1),
                pk(9),
                pk(10),
                pk(4)
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "From"), pk(5).to_string());
    assert_eq!(field(&idl, "To"), pk(6).to_string());
    assert_eq!(idl.unnamed_accounts, 0);
    assert!(idl
        .fields
        .iter()
        .all(|field| field.value != TOKEN_PROGRAM_ID.to_string()
            && field.value != TOKEN_2022_PROGRAM_ID.to_string()));
}

/// Setting a CP swap config's owner (`param` 3) reads the new owner from the
/// account after the named ones, so review shows it rather than flagging it.
#[test]
fn a_raydium_cp_config_owner_change_shows_the_new_owner() {
    use crate::idl::builtin::RAYDIUM_CP_SWAP_PROGRAM_ID;

    let admin = Pubkey::from_str_const("DRayqG9RXYi8WHgWEmRQGrUWRWbhjYWYkCRJDd6JBBak");
    let mut data = hex::decode("313cae889a1c74c8").unwrap();
    data.push(3);
    data.extend(0u64.to_le_bytes());
    let update = Instruction {
        program_id: RAYDIUM_CP_SWAP_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(admin, true),
            AccountMeta::new(pk(2), false),
            AccountMeta::new_readonly(pk(3), false),
        ],
        data,
    };

    let idl = explain_one_ix(&builtins(), admin, update);
    assert_eq!(idl.intent, "Update AMM config");
    assert_eq!(field(&idl, "New Owner"), pk(3).to_string());
    assert_eq!(idl.unnamed_accounts, 0);
}
