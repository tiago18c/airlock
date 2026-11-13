use super::*;
use crate::fixtures::{self, compile};
use crate::idl::catalog::{IdlStore, MemoryBodies};
use crate::ids::{
    COMPUTE_BUDGET_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID,
};
use crate::tx::builder::compile_v1;
use crate::tx::decode::decode_v1_transaction;
use codama_dynamic_client::Value;
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

/// Every IDL the app ships, on devnet.
fn builtins() -> IdlStore<MemoryStore, MemoryBodies> {
    crate::fixtures::shipped_catalog(crate::IdlNetwork::Devnet)
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
    use crate::fixtures::RAYDIUM_CLMM_PROGRAM_ID;

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
    assert_eq!(field(&idl, "Amount"), "6,708,044,446,768 base units");
    assert_eq!(
        field(&idl, "Minimum Out or Maximum In"),
        "1,622,897,189 base units"
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
    use crate::fixtures::RAYDIUM_CP_SWAP_PROGRAM_ID;

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
                "{} swaps 2,500,000 base units of {} for at least 1,900,000 base units of {} in pool {}",
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

/// `initialize_v2` creates a pool like `initialize`, one account shorter
/// (no rent sysvar), with an optional creator fee after the amounts.
#[test]
fn a_raydium_cp_pool_with_a_creator_fee_names_its_creator_and_fee() {
    use crate::fixtures::RAYDIUM_CP_SWAP_PROGRAM_ID;

    let pool = |creator_fee: &[u8]| {
        let mut data = hex::decode("4399af27da102620").unwrap();
        data.extend(1_000_000u64.to_le_bytes());
        data.extend(2_000_000u64.to_le_bytes());
        data.extend(0u64.to_le_bytes());
        data.extend(creator_fee);
        let mut accounts = vec![AccountMeta::new(pk(1), true)];
        accounts.extend((2..=19).map(|byte| AccountMeta::new(pk(byte), false)));
        Instruction {
            program_id: RAYDIUM_CP_SWAP_PROGRAM_ID,
            accounts,
            data,
        }
    };

    let idl = explain_one_ix(&builtins(), pk(1), pool(&[1, 1]));
    assert_eq!(idl.intent, "Create pool");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{} creates pool {} for {} and {} under AMM config {}, depositing 1,000,000 base units and 2,000,000 base units",
                pk(1),
                pk(4),
                pk(5),
                pk(6),
                pk(2)
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Creator Fee"), "On token 0 only");
    assert_eq!(idl.unnamed_accounts, 0);

    let without = explain_one_ix(&builtins(), pk(1), pool(&[0]));
    assert_eq!(without.intent, "Create pool");
    assert_eq!(field(&without, "Creator Fee"), "None");
}

/// Setting a CP swap config's owner (`param` 3) reads the new owner from the
/// account after the named ones, so review shows it rather than flagging it.
#[test]
fn a_raydium_cp_config_owner_change_shows_the_new_owner() {
    use crate::fixtures::RAYDIUM_CP_SWAP_PROGRAM_ID;

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

/// An instruction of a devnet router transaction: its data and its
/// accounts as `(address, signer, writable)`.
fn router_ix(data: &str, accounts: &[(&str, bool, bool)]) -> Instruction {
    use crate::fixtures::RAYDIUM_ROUTER_PROGRAM_ID;

    Instruction {
        program_id: RAYDIUM_ROUTER_PROGRAM_ID,
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    }
}

const ROUTER_TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const ROUTER_TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const ROUTER_ATA: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const ROUTER_SYSTEM: &str = "11111111111111111111111111111111";
const ROUTER_WSOL: &str = "So11111111111111111111111111111111111111112";
const ROUTER_CPMM: &str = "DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb";

/// Devnet `CreateSyncNative` and `CloseTokenAccount` around a routed swap
/// (transaction `2PxY15kC…`).
#[test]
fn router_wsol_utilities_name_the_owner_and_accounts() {
    let owner = "98hqb3kvXZVv4KY3DgAD1o5Ve1McqCafqzNjrWveYnkQ";
    let wsol_account = "C9GBvKnJ3mtYdbmwW3ZXayyNibZTYYyCvKYU2ufHpHGR";
    let wrap = router_ix(
        "058096980000000000",
        &[
            (owner, true, true),
            (wsol_account, false, true),
            (ROUTER_WSOL, false, true),
            (ROUTER_TOKEN, false, false),
            (ROUTER_ATA, false, false),
            (ROUTER_SYSTEM, false, false),
        ],
    );
    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), wrap);
    assert_eq!(idl.intent, "Wrap SOL");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(format!("{owner} wraps 0.01 SOL into {wsol_account}").as_str())
    );
    assert_eq!(idl.unnamed_accounts, 0);

    let close = router_ix(
        "06",
        &[
            (owner, true, true),
            (wsol_account, false, true),
            (owner, true, true),
            (ROUTER_TOKEN, false, false),
            (ROUTER_ATA, false, false),
            (ROUTER_SYSTEM, false, false),
        ],
    );
    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), close);
    assert_eq!(idl.intent, "Close token account");
    assert_eq!(field(&idl, "Token Account"), wsol_account);
    assert_eq!(field(&idl, "Rent To"), owner);
}

/// A devnet one-hop `SwapBaseInWithUserAccount` (tag 0) through a CP swap
/// pool, with no CLMM price limits after the amounts.
#[test]
fn a_legacy_routed_swap_names_its_source_and_destination() {
    let owner = "98hqb3kvXZVv4KY3DgAD1o5Ve1McqCafqzNjrWveYnkQ";
    let source = "C9GBvKnJ3mtYdbmwW3ZXayyNibZTYYyCvKYU2ufHpHGR";
    let destination = "79MJFSDL9GyNRyh4MoY9Wzbq7oqXwwTZjH39qNRo4pHj";
    let mut accounts = vec![
        (ROUTER_TOKEN, false, false),
        (ROUTER_TOKEN_2022, false, false),
        (ROUTER_ATA, false, false),
        (ROUTER_SYSTEM, false, false),
        (owner, true, true),
        (source, false, true),
        (destination, false, true),
        (ROUTER_CPMM, false, false),
        (destination, false, true),
        (ROUTER_WSOL, false, true),
    ];
    accounts.extend(
        [
            "81DsRSjULjx3KyKkNorbZSAFqvZQNBqi6AoYLtfPXK3y",
            "CXniRufdq5xL8t8jZAPxsPZDpuudwuJSPWnbcD5Y5Nxq",
            "5MxLgy9oPdTC3YgkiePHqr3EoCRD9uLVYRQS2ANAs7wy",
            "4YoEuDXc2323brXqCGu9GnycvaoR7uX3374dJUkqPUJ3",
            "8HrFFuasANF3aeuj2wynDF6FNXL5aZV7kEDesdUnKNTe",
            "9PfKT4v79wMUZnnaBKjP6tPfWTozZ5ArMj9QzJCugBy4",
            "8qWPcBYXpaXUAEQUtd6NgpAdEWcYrQApfsnYgTPxF155",
        ]
        .map(|address| (address, false, true)),
    );
    let swap = router_ix("0080969800000000002555b70515000000", &accounts);

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), swap);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps 10,000,000 base units from {source} for at least \
                 90,290,214,181 base units into {destination} through a route of pools"
            )
            .as_str()
        )
    );
    // The hop's accounts are read by the program, so none is unnamed, and
    // neither they nor the programs are listed.
    assert_eq!(idl.unnamed_accounts, 0);
    assert!(idl
        .fields
        .iter()
        .all(|field| field.value != ROUTER_CPMM && field.value != ROUTER_TOKEN));
}

/// A devnet one-hop `SwapBaseOutWithUserAccount` (tag 1): its hops come right
/// after the owner.
#[test]
fn a_legacy_routed_exact_output_swap_reads_its_limits() {
    let owner = "CvmHcd7BXZ2st5Lpz8RYRMkmEhg1UDHqS4ozkMLEks11";
    let mut accounts = vec![
        (ROUTER_TOKEN, false, false),
        (ROUTER_TOKEN_2022, false, false),
        (ROUTER_ATA, false, false),
        (ROUTER_SYSTEM, false, false),
        (owner, true, true),
        (ROUTER_CPMM, false, false),
    ];
    accounts.extend(
        [
            "YEbUSJVtBzzJFYfR7psrmvs5eTYkxvnV8qZrxfD2PuC",
            "yMYVydeK55KvP2i3xtL88jMGPmxJCoPV8gvt8krw1zD",
            ROUTER_WSOL,
            "5h67oDwpdfMTgZNJxYMaKyX3eN8Kev9PZ5E999KyncMM",
            "CXniRufdq5xL8t8jZAPxsPZDpuudwuJSPWnbcD5Y5Nxq",
            "5MxLgy9oPdTC3YgkiePHqr3EoCRD9uLVYRQS2ANAs7wy",
            "E6XavFGuKZxqSfvjF6zPt9mGxgRbPd9oyJUvou93QBKz",
            "7JwyHCeEuMGXipEWwcXTTavhBc6kZnKdCLUwVkdCHy9A",
            "9vpKw36RYDGmfXqiGmFrPLgDAhrgpE7F7M3aRacjJwPX",
            "D2vfTg1ufpdrGYouEyF9yg7ZSDmZ6ybWQ8kZx5G8do9E",
        ]
        .map(|address| (address, false, true)),
    );
    let swap = router_ix("015f3909000000000000c029f73d540500", &accounts);

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), swap);
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} receives 1,500,000,000,000,000 base units for at most 604,511 base units \
                 through a route of pools"
            )
            .as_str()
        )
    );
    assert_eq!(idl.unnamed_accounts, 0);
}

/// A devnet two-hop `SwapBaseIn` (tag 8, transaction `29YXEce3…`): SOL to an
/// intermediate token to a third, both through CP swap pools.
#[test]
fn a_routed_swap_names_its_input_and_first_hop() {
    let owner = "538RWC8Z25WenqyHw73hffdHm4ADbvMzdH7ikYoGgmrL";
    let source = "n5XKKgR8YhYztzFwLMANC9R8vZaBnnWptSJWdzB6f8C";
    let intermediate = "17bizZywyucCoaR2So5yPnWM3wkYXERDzG9ptviPVFK";
    let intermediate_mint = "8uve3VTzdfzjfgHz3MeK6g1z1KTVAVp5QoQtHDLP3kHD";
    let pool_accounts = |accounts: &mut Vec<(&'static str, bool, bool)>,
                         pool: [&'static str; 6]| {
        accounts.extend(pool.map(|address| (address, false, true)));
        accounts.extend([(ROUTER_TOKEN, false, false), (ROUTER_TOKEN, false, false)]);
    };
    let mut accounts = vec![
        (owner, true, true),
        (ROUTER_TOKEN, false, false),
        (ROUTER_CPMM, false, false),
        (source, false, true),
        (intermediate, false, true),
        (ROUTER_WSOL, false, false),
        (intermediate_mint, false, false),
    ];
    pool_accounts(
        &mut accounts,
        [
            "CXniRufdq5xL8t8jZAPxsPZDpuudwuJSPWnbcD5Y5Nxq",
            "5MxLgy9oPdTC3YgkiePHqr3EoCRD9uLVYRQS2ANAs7wy",
            "3QCKmmcVYggAwmf1WbZVjcPCrkoJJmT2Eti9J1KyERap",
            "J6vKZ2upAA1B1dmvzqQT5n7CrDR8eBM9gjiJX7RvfTni",
            "4VcWR8u4Xwm9Fe5oyNqosPFnJE84ZkGJGqkrDsAxJqGo",
            "Cewcj8fYGK1UQUAt8N5UJZBcNt5231n7mXREpbvCbob6",
        ],
    );
    accounts.extend([
        (ROUTER_CPMM, false, false),
        (intermediate, false, true),
        ("EpxUZ8s3ixsz461MDj8ZaXbncN68oLWR1nCiPJSWqGJX", false, true),
        (intermediate_mint, false, false),
        ("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU", false, false),
    ]);
    pool_accounts(
        &mut accounts,
        [
            "CXniRufdq5xL8t8jZAPxsPZDpuudwuJSPWnbcD5Y5Nxq",
            "5MxLgy9oPdTC3YgkiePHqr3EoCRD9uLVYRQS2ANAs7wy",
            "3TgF7Y5kMSMyWokpZk6kKWymvCpF8CW11T3imooQd5Tm",
            "CnBC7M18ZNhnvyvtq8dXmowzPvqM88bvVc2jojmLQoAc",
            "AnidHeNrevJEMCwG9Fhtzv4dJFL5hGQTbkzqhgdRNADk",
            "885LwCgbCo1Lx62BauVEYbatRiimwP2nnmQPsE1vKSzX",
        ],
    );
    let swap = router_ix("08964ac8dc000000005df1950000000000", &accounts);

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), swap);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps 3,704,113,814 base units of {ROUTER_WSOL} from {source} through a \
                 route of pools, for at least 9,826,653 base units of its last token"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "First Hop To"), intermediate);
    assert_eq!(field(&idl, "First Hop Output Mint"), intermediate_mint);
    assert_eq!(idl.unnamed_accounts, 0);
}

/// An instruction of the Whirlpool program: its data and its accounts as
/// `(address, signer, writable)`.
fn whirlpool_ix(data: &str, accounts: &[(&str, bool, bool)]) -> Instruction {
    use crate::fixtures::ORCA_WHIRLPOOL_PROGRAM_ID;

    Instruction {
        program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    }
}

/// A devnet `swap_v2` (transaction `22pTxZ77…`) whose `remainingAccountsInfo`
/// passes two supplemental tick arrays after the named accounts.
#[test]
fn a_whirlpool_swap_names_its_mints_and_reads_its_extra_tick_arrays() {
    let owner = "8cMnAcFSSVrfYCsCshyHE2dZ5ZKUS8MnTNPKSWJU7xuQ";
    let pool = "3KBZiL2g8C7tiJ32hTv5v3KM7aK9htpqTw4cTXz1HvPt";
    let mint_b = "BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k";
    let account_a = "5L5SwrK3jPZwq6yhoCSnneHLdzVQNFX63Fo4W3dJeqxK";
    let account_b = "5AgrPE66YREecQsiYvyUNfuCneEyeYWZ5Nq4UtwETA36";
    let mut accounts = vec![
        (ROUTER_TOKEN, false, false),
        (ROUTER_TOKEN, false, false),
        ("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr", false, false),
        (owner, true, true),
        (pool, false, true),
        (ROUTER_WSOL, false, false),
        (mint_b, false, false),
        (account_a, true, true),
        ("C9zLV5zWF66j3rZj3uuhDqvfuA8esJyWnruGzDW9qEj2", false, true),
        (account_b, false, true),
    ];
    accounts.extend(
        [
            "7DM3RMz2yzUB8yPRQM3FMZgdFrwZGMsabsfsKopWktoX",
            "7knZZ461yySGbSEHeBUwEpg3VtAkQy8B9tp78RGgyUHE",
            "CpoSFo3ajrizueggtJr2ZjvYgdtkgugXtvhqcwkyCkKP",
            "9iGzy4mQtJadZDuH8seBFQGiqcb6wyp2KW4M6NKHvsAW",
            "2KEWNc3b6EfqoWQpfKQMHh4mhRyKXYRdPbtGRTJX3Cip",
            "3aBJJLAR3QxGcGsesNXeW3f64Rv3TckF7EQ6sXtAuvGM",
            "A1vrG379E5ttoaWmyQBiunsMdyrpoUp7mSQwu8DgLcip",
        ]
        .map(|address| (address, false, true)),
    );
    let swap = whirlpool_ix(
        "2b04ed0b1ac91e62a086010000000000ce27440000000000000000000000000000000000000000000100\
         01010000000602",
        &accounts,
    );

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), swap);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps between {ROUTER_WSOL} and {mint_b} in pool {pool}, with \
                 {account_a} and {account_b}"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Amount"), "100,000 base units");
    assert_eq!(
        field(&idl, "Minimum Out or Maximum In"),
        "4,466,638 base units"
    );
    assert_eq!(field(&idl, "Exact Input"), "true");
    assert_eq!(field(&idl, "Token A For Token B"), "false");
    // The supplemental tick arrays are read by the program, so they are not
    // unnamed, and neither they nor the programs are listed.
    assert_eq!(idl.unnamed_accounts, 0);
    assert!(idl.fields.iter().all(|field| field.value
        != "A1vrG379E5ttoaWmyQBiunsMdyrpoUp7mSQwu8DgLcip"
        && field.value != ROUTER_TOKEN));
}

/// A devnet `initialize_pool_v2` (transaction `5DgksRSW…`) of a splash pool.
/// Both vaults are new keypairs that sign. The client appended the program
/// id after the named accounts; the program does not read it, so review
/// counts it as unnamed.
#[test]
fn a_whirlpool_pool_creation_names_its_mints_and_vaults() {
    let funder = "2Ggmdr2qpuMsd7sGiCwPRBzoA3uXn6Tf6oTycg3fxAPB";
    let pool = "AqNYrACHNgPdpWwCS1GtAJPDWWTjvAVUTJj9MNYdMaZK";
    let mint_a = "46PWVv2z2FAT5q4PSMCvDSiuEF9CNncXZ1DhzHfA8i3U";
    let mint_b = "8Q2UVRq3aHagoowWUraEdBhbPy8kJivo43LhYHTtBpi3";
    let config = "FcrweFY1G9HJAHG5inkGB6pKg1HZ6x9UC2WioAfWrGkR";
    let vault_a = "GUdGdGqBFG1PESeRLaEBbnA3G8343b3LfB9sNt9MV9BS";
    let vault_b = "H9qVTMWRWjwfNW1feHgduZmfKngsyHoEqFEsVfANxXAL";
    let create = whirlpool_ix(
        "cf2d57f21b3fcc438080008046c945970b000a00000000000000",
        &[
            (config, false, false),
            (mint_a, false, false),
            (mint_b, false, false),
            ("EPpeSV8bS5CqnL9bJ4YR4BaUScmovVHD2hYNMCVyN8eP", false, false),
            ("GWehUaYqR61ZcGkVp64i154hSm6qmV313VC8fmpiFTGf", false, false),
            (funder, true, true),
            (pool, false, true),
            (vault_a, true, true),
            (vault_b, true, true),
            ("8EHtyN3DBseSZzHYkxXuT2GDoPmqmKEtbbaNrabFZhdL", false, false),
            (ROUTER_TOKEN, false, false),
            (ROUTER_TOKEN_2022, false, false),
            (ROUTER_SYSTEM, false, false),
            ("SysvarRent111111111111111111111111111111111", false, false),
            ("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc", false, false),
        ],
    );

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(funder), create);
    assert_eq!(idl.intent, "Create pool");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{funder} creates pool {pool} for {mint_a} and {mint_b} with tick spacing \
                 32896 in config {config}"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Pool Vault A"), vault_a);
    assert_eq!(field(&idl, "Pool Vault B"), vault_b);
    assert_eq!(idl.unnamed_accounts, 1);
}

/// The three memo programs read the same data, the memo as UTF-8, and name
/// the signers passed after it.
#[test]
fn a_memo_reads_its_text_on_every_memo_program() {
    for program in [
        "Memo4c2pN8afCj432Lb7RMVKi9PbQnnW7ewFFaV3oAH",
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
        "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo",
    ] {
        let memo = Instruction {
            program_id: Pubkey::from_str_const(program),
            accounts: vec![AccountMeta::new_readonly(pk(1), true)],
            data: b"gm, airlock".to_vec(),
        };
        let idl = explain_one_ix(&builtins(), pk(1), memo);
        assert_eq!(idl.intent, "Add memo", "{program}");
        assert_eq!(
            idl.interpolated_intent.as_deref(),
            Some("Add memo \"gm, airlock\""),
            "{program}"
        );
        assert_eq!(idl.unnamed_accounts, 0, "{program}");
    }
}

/// Bytes that are not UTF-8 are no memo: the program fails on them, and
/// review shows them raw.
#[test]
fn a_memo_that_is_not_utf8_stays_raw() {
    let memo = Instruction {
        program_id: Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
        accounts: vec![],
        data: vec![0xff, 0xfe],
    };
    let tx = decode_v1_transaction(&compile(pk(1), vec![memo]).wire).unwrap();
    let explained = explain_transaction(&builtins(), &tx);
    assert!(explained.instructions[0].idl.is_none());
    assert_eq!(explained.instructions[0].data, vec![0xff, 0xfe]);
}

/// An instruction of the SPL Governance program Realms runs: its data and its
/// accounts as `(address, signer, writable)`.
fn realms_ix(data: &str, accounts: &[(&str, bool, bool)]) -> Instruction {
    use crate::fixtures::REALMS_GOVERNANCE_PROGRAM_ID;

    Instruction {
        program_id: REALMS_GOVERNANCE_PROGRAM_ID,
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    }
}

const REALMS_REALM: &str = "By2sVGZXwfQq6rAiAM3rNPJ9iQfb5e2QhnF4YjJ4Bip";
const REALMS_REALM_CONFIG: &str = "CH3w5UZF4TLvsvB6ARHg12SKCpkZfsDoR1Mh6HPfp5Qj";

/// A mainnet deposit (transaction `35zbZeSs…`).
#[test]
fn a_realms_deposit_names_its_owner_and_amount() {
    let owner = "2kHzF6eexNuefF8tnKKQ5eux2LQFUhv2iwHAjfCMPVwi";
    let source = "3k1YS6rW1GPz8keNHC389qbURntXivcRJx8dr9uh9RPy";
    let deposit = realms_ix(
        "0100d0ed902e000000",
        &[
            (REALMS_REALM, false, false),
            ("AuWECxm8YT89i4jvcHsy6rL4KRToUrKv7jwLmLPJPtHf", false, true),
            (source, false, true),
            (owner, true, true),
            (owner, true, true),
            ("D3xvu3EnAzAmh4Ku5bBR4CF2iw1bHnV28o6vKQgwZw6Z", false, true),
            (owner, true, true),
            (ROUTER_SYSTEM, false, false),
            (ROUTER_TOKEN, false, false),
            (REALMS_REALM_CONFIG, false, false),
        ],
    );

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), deposit);
    assert_eq!(idl.intent, "Deposit governing tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} deposits 200,000,000,000 base units from {source} into realm {REALMS_REALM}"
            )
            .as_str()
        )
    );
    assert_eq!(idl.unnamed_accounts, 0);
}

/// A mainnet proposal (transaction `NnP3bUp2…`) in a realm without a voter
/// weight plugin: the deposit account comes where the voter weight record
/// would, so it is read as part of the group and not named as that record.
#[test]
fn a_realms_proposal_does_not_name_its_deposit_as_a_voter_weight_record() {
    let owner = "2kHzF6eexNuefF8tnKKQ5eux2LQFUhv2iwHAjfCMPVwi";
    let proposal = "GMC8qjrirxRznFkhoWifQyAvWVpjEMXQdTtEkhpgZaPZ";
    let governance = "oyznFCoGLEr8mWWfroGRbhtPE7wUVMMy1X9Rhv3RvqD";
    let deposit = "93EfVQaJ2EQ71QswAzyr3fz7SXgonfvg4xHs8aS4euwH";
    let create = realms_ix(
        "06320000005661756c74204d616e6167656d656e742055706772616465202d204175746f6d617465\
         64205969656c6420526f7574696e67930000004465706c6f79206175746f6d617465642074726561\
         73757279206d616e6167656d656e7420666f72207969656c64206f7074696d697a6174696f6e2061\
         63726f737320446546692070726f746f636f6c732e20506861736520313a20696e697469616c697a\
         65207661756c7420636f6e66696720616e642072756e20696e697469616c20706f7274666f6c696f\
         2073796e632e000100000007000000417070726f76650101bffb70a26823cfdf8c5125c3ef0f7b4b\
         f8c9f0bb47f21e6478bacc535053ac",
        &[
            (REALMS_REALM, false, false),
            (proposal, false, true),
            (governance, false, true),
            ("D3xvu3EnAzAmh4Ku5bBR4CF2iw1bHnV28o6vKQgwZw6Z", false, true),
            ("8upjSpvjcdpuzhfR1zriwg5NXkwDruejqNE9WNbPRtyA", false, false),
            (owner, true, true),
            (owner, true, true),
            (ROUTER_SYSTEM, false, false),
            (REALMS_REALM_CONFIG, false, false),
            (deposit, false, true),
        ],
    );

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(owner), create);
    assert_eq!(idl.intent, "Create proposal");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} creates proposal Vault Management Upgrade - Automated Yield Routing \
                 at {proposal} in governance {governance}"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Voter Weight Record or Deposit"), deposit);
    assert!(idl
        .fields
        .iter()
        .all(|field| field.label != "Voter Weight Record"));
    assert_eq!(field(&idl, "Vote Type"), "Single choice");
    assert_eq!(field(&idl, "Options"), "Approve");
    assert_eq!(idl.unnamed_accounts, 0);
}

/// A mainnet council veto (transaction `5SJ1BySf…`).
#[test]
fn a_realms_vote_says_how_it_votes() {
    let voter = "9f8uuHm4VFzsHnBoCUZxF2cuQJzJteX4tzAqL1sPsqXQ";
    let proposal = "GMC8qjrirxRznFkhoWifQyAvWVpjEMXQdTtEkhpgZaPZ";
    let vote = realms_ix(
        "0d03",
        &[
            (REALMS_REALM, false, false),
            ("oyznFCoGLEr8mWWfroGRbhtPE7wUVMMy1X9Rhv3RvqD", false, true),
            (proposal, false, true),
            ("D3xvu3EnAzAmh4Ku5bBR4CF2iw1bHnV28o6vKQgwZw6Z", false, true),
            ("HbkNdykLNJU41y41TFFpJYYgwGjRZLTQa2qaoCT5v5de", false, true),
            (voter, true, true),
            ("4N2AgNDTGtsMryUFvpKBfqgrdRBkBuGFnVR9V8ftcn4d", false, true),
            ("5weUku8SQw9Yiiar7Qtv42R4fGzkgeGcGvC5Jiz15ssN", false, false),
            (voter, true, true),
            (ROUTER_SYSTEM, false, false),
            (REALMS_REALM_CONFIG, false, false),
        ],
    );

    let idl = explain_one_ix(&builtins(), Pubkey::from_str_const(voter), vote);
    assert_eq!(idl.intent, "Vote on proposal");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(format!("{voter} votes Veto on proposal {proposal}").as_str())
    );
    assert_eq!(idl.unnamed_accounts, 0);
}

/// The accounts of a devnet `InsertTransaction` (transaction `49VWdVrb…`).
fn realms_insert(data: &str) -> Instruction {
    let author = "At1CNUW4LaiVV8gFhcHkUAEt1a8R6VuK8gsvwnmESfUF";
    realms_ix(
        data,
        &[
            ("UUqdy9qMnfEjgzMo5xsijZ2iZhLo66fnfkMyTUYKSFz", false, false),
            ("BYbRt5XhHoagQv7mnGdDsEGbVaC5eeM4oFQhr9P4KjYa", false, true),
            ("BarM4bouviowiVdB71dHVLwwcknLPeJ7X6s3KtAu5myu", false, false),
            (author, true, true),
            ("6AFtFZvFKeBrRebJK8TRYxTaUwAaqcaMjuZSNzJ9XHz2", false, true),
            (author, true, true),
            (ROUTER_SYSTEM, false, false),
            ("SysvarRent111111111111111111111111111111111", false, false),
        ],
    )
}

/// The second transaction of the devnet proposal `BYbRt5Xh…`: a token
/// transfer the governance's treasury signs once the proposal passes.
const REALMS_INSERT_TRANSFER: &str = "09000100000000000100000006ddf6e1d765a193d9cbe146ceeb79ac1cb4\
     85ed5f5b37913a8cf5857eff00a9040000000f3b53f07edb5d8db314d5bf2b85ef0c4b638456d6c8c8b7bb273e\
     9ce4db6f2600019323c44b18fe83e5279c6c93ef3cddc028a94b7981ce5c7c96d76d1d08d9afb2000037f663db\
     6099732f62f62a024c412211b8e1b38f453d51d1d8aac1115a64df8900015bc23274bb4904732133affcad2376\
     49e2ca951d63604c92ff8ceae870bda45e01000a0000000c0010a5d4e800000006";

#[test]
fn a_realms_proposal_transaction_is_opened_and_each_instruction_explained() {
    let author = Pubkey::from_str_const("At1CNUW4LaiVV8gFhcHkUAEt1a8R6VuK8gsvwnmESfUF");
    let insert = realms_insert(REALMS_INSERT_TRANSFER);
    let tx = decode_v1_transaction(&compile(author, vec![insert]).wire).unwrap();
    let explained = explain_transaction(&builtins(), &tx);
    let added = &explained.instructions[0];

    let idl = added.idl.as_ref().unwrap();
    assert_eq!(idl.intent, "Add proposal transaction");
    // The carried instructions are explained below, not dumped as a field.
    assert!(idl.fields.iter().all(|field| field.label != "Instructions"));
    let inner = added.inner.as_ref().unwrap();
    assert_eq!(
        inner.kind,
        InnerKind::ProposalTransaction {
            governance: Pubkey::from_str_const("UUqdy9qMnfEjgzMo5xsijZ2iZhLo66fnfkMyTUYKSFz"),
            proposal: Pubkey::from_str_const("BYbRt5XhHoagQv7mnGdDsEGbVaC5eeM4oFQhr9P4KjYa"),
            option_index: 0,
            transaction_index: 1,
        }
    );
    assert_eq!(inner.instructions.len(), 1);
    let transfer = &inner.instructions[0];
    assert_eq!(transfer.program_id, TOKEN_PROGRAM_ID);
    // The treasury signs it when the governance runs it, not this transaction.
    let treasury = Pubkey::from_str_const("7BBrGMFu5a49LqyyESQYvSL3GYwESQR2j2UVrXSuf1CH");
    assert_eq!(transfer.accounts[3], treasury);
    let worded = transfer.idl.as_ref().unwrap();
    assert_eq!(worded.intent, "Transfer tokens");
    assert_eq!(
        worded.interpolated_intent.as_deref(),
        Some(
            "Transfer 1,000,000 AuNZhucaYASPKmjCZLhTfhigadRMq4q6qXHbkSjvy4Q1 from \
             22Tax896iNLNZbsCRExWomRmS6uJnrMT1vF4FGzvM4JH to \
             4mTLFrGiJCX7ACKtxnBJRH9TivTHXfQyK3VgkveijVaY"
        )
    );
    assert_eq!(field(worded, "Authority"), treasury.to_string());
    assert!(explained
        .programs
        .iter()
        .any(|line| line.program_id == TOKEN_PROGRAM_ID));
}

/// An `InsertTransaction` whose bytes run past what it declares is not
/// opened: what it carries cannot be told.
#[test]
fn a_realms_proposal_transaction_with_extra_bytes_is_not_opened() {
    let author = Pubkey::from_str_const("At1CNUW4LaiVV8gFhcHkUAEt1a8R6VuK8gsvwnmESfUF");
    for data in [
        format!("{REALMS_INSERT_TRANSFER}00"),
        REALMS_INSERT_TRANSFER[..REALMS_INSERT_TRANSFER.len() - 2].to_string(),
    ] {
        let insert = realms_insert(&data);
        let tx = decode_v1_transaction(&compile(author, vec![insert]).wire).unwrap();
        assert!(explain_transaction(&builtins(), &tx).instructions[0]
            .inner
            .is_none());
    }
}

/// The four calls a Squads vault made to publish program-metadata's own IDL
/// on devnet (transaction `3vJooqCq…`): allocate the metadata account, copy
/// the IDL into it from a buffer, initialize it, and close the buffer.
#[test]
fn publishing_an_idl_reads_as_program_metadata_calls() {
    const PROGRAM_METADATA: &str = "ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S";
    let metadata = "9zzVPVwZojagC4mBCMrTi17Zu9KvXKoTvqnBL1vxYDyM";
    let authority = "AXF4hqmBaXMwkpPwpbdJVPmq5KZ1mbF7e8sKqF2rrAPF";
    let program_data = "F8hZ6cQFCNZ3UdNkhUfjgxxRnUD8HiYPobpk2qRVD7aB";
    let buffer = "6fPLewV4HaCXUJtaeifu1qiHh4xrNnDuLNSgGhjs1KPR";
    let recipient = "DULamQmz3KUf4ntxARS9qjD3okBRD3Aiw5VeuzBR2TZE";
    let ix = |data: &str, accounts: &[(&str, bool, bool)]| Instruction {
        program_id: Pubkey::from_str_const(PROGRAM_METADATA),
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    };
    let explain = |ix| explain_one_ix(&builtins(), Pubkey::from_str_const(authority), ix);

    let allocate = explain(ix(
        "0769646c00000000000000000000000000",
        &[
            (metadata, false, true),
            (authority, true, false),
            (PROGRAM_METADATA, false, false),
            (program_data, false, false),
            ("11111111111111111111111111111111", false, false),
        ],
    ));
    assert_eq!(allocate.intent, "Allocate buffer");
    assert_eq!(
        allocate.interpolated_intent.as_deref(),
        Some(format!("Allocate buffer {metadata}").as_str())
    );
    assert_eq!(field(&allocate, "Seed"), "idl");

    let write = explain(ix(
        "0000000000",
        &[
            (metadata, false, true),
            (authority, true, false),
            (buffer, false, false),
        ],
    ));
    assert_eq!(
        write.interpolated_intent.as_deref(),
        Some(format!("Write to buffer {metadata} at offset 0").as_str())
    );
    assert_eq!(field(&write, "Copy From"), buffer);

    // The program's own address stands in for the system program it was
    // not passed.
    let initialize = explain(ix(
        "0169646c0000000000000000000000000001020000",
        &[
            (metadata, false, true),
            (authority, true, false),
            (PROGRAM_METADATA, false, false),
            (program_data, false, false),
            (PROGRAM_METADATA, false, false),
        ],
    ));
    assert_eq!(initialize.intent, "Create program metadata");
    assert_eq!(
        initialize.interpolated_intent.as_deref(),
        Some(
            format!("Create metadata {metadata} with seed \"idl\" for program {PROGRAM_METADATA}")
                .as_str()
        )
    );
    assert_eq!(field(&initialize, "Encoding"), "UTF-8");
    assert_eq!(field(&initialize, "Compression"), "zlib");
    assert_eq!(field(&initialize, "Format"), "None");
    assert_eq!(field(&initialize, "Data Source"), "Direct");
    assert_eq!(initialize.unnamed_accounts, 0);

    let accounts = &[
        (metadata, false, true),
        (authority, true, false),
        (PROGRAM_METADATA, false, false),
        (program_data, false, false),
    ];
    let new_authority = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
    let bytes = bs58::decode(new_authority).into_vec().unwrap();
    let set = explain(ix(&format!("0201{}", hex::encode(bytes)), accounts));
    assert_eq!(
        set.interpolated_intent.as_deref(),
        Some(format!("Change the authority of {metadata} to {new_authority}").as_str())
    );
    let remove = explain(ix("0200", accounts));
    assert_eq!(
        remove.interpolated_intent.as_deref(),
        Some(format!("Change the authority of {metadata} to None").as_str())
    );

    let close = explain(ix(
        "06",
        &[
            (buffer, false, true),
            (authority, true, false),
            (PROGRAM_METADATA, false, false),
            (program_data, false, false),
            (recipient, false, true),
        ],
    ));
    assert_eq!(
        close.interpolated_intent.as_deref(),
        Some(format!("Close {buffer} and send its balance to {recipient}").as_str())
    );
}

/// Calls of OtterSec's verify program as mainnet executed them: `solana-verify`
/// recording the build of a program (transaction `4HWbv76z…`), updating a
/// record (`4JkaiaHH…`) and closing one (`LESvNbth…`).
#[test]
fn recording_a_programs_build_reads_as_a_sentence() {
    const OTTER_VERIFY: &str = "verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC";
    const SYSTEM: &str = "11111111111111111111111111111111";
    let ix = |data: &str, accounts: &[(&str, bool, bool)]| Instruction {
        program_id: Pubkey::from_str_const(OTTER_VERIFY),
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    };
    let explain =
        |signer: &str, ix| explain_one_ix(&builtins(), Pubkey::from_str_const(signer), ix);

    let signer = "9wFWwL8A9g4dSM7or8Nyp7cJCTa5wDYUUcrYYJtuJZhb";
    let program = "PerpX2pWbVffrbJfb4zbDtGkJeWGyucH2HVPkvdpDit";
    let initialize = explain(
        signer,
        ix(
            "afaf6d1f0d989bed05000000302e352e322f00000068747470733a2f2f6769746875622e636f6d2f706572706574756178797a2f70657270657475612d70726f6772616d2800000036396165376237653232653862333539336261316138333334363664393963376230313834663933020000000e0000002d2d6c6962726172792d6e616d650800000070657270657475611cf5e81b00000000",
            &[
                ("72vh8hZUrtgzUVFpKcMkzU592jThonVHa5MYS2w2hjEi", false, true),
                (signer, true, true),
                (program, false, false),
                (SYSTEM, false, false),
            ],
        ),
    );
    assert_eq!(initialize.intent, "Record a program's build");
    assert_eq!(
        initialize.interpolated_intent.as_deref(),
        Some(
            format!(
                "{signer} states that program {program} is built from \
                 https://github.com/perpetuaxyz/perpetua-program at commit \
                 69ae7b7e22e8b3593ba1a833466d99c7b0184f93"
            )
            .as_str()
        )
    );
    assert_eq!(field(&initialize, "solana-verify Version"), "0.5.2");
    assert!(field(&initialize, "Build Arguments").contains("--library-name"));
    assert!(field(&initialize, "Build Arguments").contains("perpetua"));
    assert_eq!(field(&initialize, "Deploy Slot"), "468,251,932");
    assert_eq!(initialize.unnamed_accounts, 0);

    let signer = "A4rJLoFB5pxCKvQ6UTb4tHgG8vEPBmz21LrSav5ho5Yj";
    let program = "DCcHikMZLtykhHRToKGSTYTeyvogsYTkEhFKmHf2NQQr";
    let update = explain(
        signer,
        ix(
            "dbc858b09e3ffd7f05000000302e352e322a00000068747470733a2f2f6769746875622e636f6d2f6f7675737061642f736d6172742d636f6e7472616374730700000063636663656566070000000e0000002d2d6c6962726172792d6e616d650a0000006c61756e63685f7061640c0000002d2d626173652d696d6167652e000000736f6c616e61666f756e646174696f6e2f736f6c616e612d76657269666961626c652d6275696c643a342e312e30020000002d2d0a0000002d2d6665617475726573060000006e6f2d69646c00afe91b00000000",
            &[
                ("BDcRASt8LSj7nFZ5Tj4Qd7fHwsP5TuUFNtzWuvBKDjvR", false, true),
                (signer, true, true),
                (program, false, false),
                (SYSTEM, false, false),
            ],
        ),
    );
    assert_eq!(update.intent, "Update a program's build record");
    assert_eq!(
        update.interpolated_intent.as_deref(),
        Some(
            format!(
                "{signer} now states that program {program} is built from \
                 https://github.com/ovuspad/smart-contracts at commit ccfceef"
            )
            .as_str()
        )
    );
    assert!(field(&update, "Build Arguments").contains("no-idl"));

    let signer = "7bCwrqTG91caMtckUsYJsGLL9fQaDmPpseKo1hKVkLaC";
    let record = "7q8k26n4MJpvK3fGngN8g5u26VxVRi2sQbg7jdWDYTXt";
    let program = "2vsF3TTo7QwSfqPAwtFV69974QE1bcQJRZKP4VxtsFZt";
    let close = explain(
        signer,
        ix(
            "62a5c9b16c41ce60",
            &[
                (record, false, true),
                (signer, true, true),
                (program, false, false),
            ],
        ),
    );
    assert_eq!(
        close.interpolated_intent.as_deref(),
        Some(
            format!(
                "{signer} closes its build record {record} for program {program} \
                 and takes back its rent"
            )
            .as_str()
        )
    );
}

/// A Token-2022 mint on devnet with 6 decimals and the metadata pointer and
/// token metadata extensions, as `getAccountInfo` returned it.
const MINT_WITH_METADATA: &[u8] =
    include_bytes!("../../../tests/fixtures/token-2022-mint-with-metadata.bin");

fn mint_to(program: Pubkey, mint: Pubkey, token: Pubkey, amount: u64) -> DecodedTx {
    let mut data = vec![7];
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction {
        program_id: program,
        accounts: vec![
            AccountMeta::new(mint, false),
            AccountMeta::new(token, false),
            AccountMeta::new(pk(1), true),
        ],
        data,
    };
    decode_v1_transaction(&compile(pk(1), vec![ix]).wire).unwrap()
}

fn raw_account(owner: Pubkey, data: &[u8]) -> crate::chain::RawAccount {
    crate::chain::RawAccount {
        lamports: 1,
        data: data.to_vec(),
        owner,
        executable: false,
        rent_epoch: 0,
    }
}

#[test]
fn a_mint_to_reads_as_a_sentence_once_the_mints_bytes_are_fetched() {
    let store = builtins();
    let tx = mint_to(TOKEN_2022_PROGRAM_ID, pk(7), pk(8), 123_456_000_000);
    let bare = explain_transaction(&store, &tx);
    let idl = bare.instructions[0].idl.as_ref().unwrap();
    assert_eq!(idl.interpolated_intent, None);
    assert_eq!(field(idl, "Amount"), "123,456,000,000 (raw)");
    let needed = bare.accounts_needed();
    assert_eq!(needed, vec![(pk(7), TOKEN_2022_PROGRAM_ID)]);

    let fetched = [Some(raw_account(TOKEN_2022_PROGRAM_ID, MINT_WITH_METADATA))];
    let note = DisplayNote {
        accounts: display_accounts(&needed, &fetched),
        ..DisplayNote::default()
    };
    let explained = explain_with_note(&store, &tx, &note);
    assert_eq!(
        explained.instructions[0]
            .idl
            .as_ref()
            .unwrap()
            .interpolated_intent
            .as_deref(),
        Some(format!("Mint 123,456 {} to {}", pk(7), pk(8)).as_str())
    );
}

#[test]
fn fetched_bytes_count_only_from_an_account_the_reading_program_owns() {
    let needed = [(pk(7), TOKEN_2022_PROGRAM_ID), (pk(9), TOKEN_PROGRAM_ID)];
    let fetched = [Some(raw_account(pk(3), MINT_WITH_METADATA)), None];
    assert!(display_accounts(&needed, &fetched).is_empty());
}

/// An instruction of a mainnet Jupiter transaction: its data and its accounts
/// as `(address, signer, writable)`.
fn jupiter_ix(data: &str, accounts: &[(&str, bool, bool)]) -> Instruction {
    use crate::fixtures::JUPITER_PROGRAM_ID;

    Instruction {
        program_id: JUPITER_PROGRAM_ID,
        accounts: accounts
            .iter()
            .map(|&(address, signer, writable)| AccountMeta {
                pubkey: Pubkey::from_str_const(address),
                is_signer: signer,
                is_writable: writable,
            })
            .collect(),
        data: hex::decode(data).unwrap(),
    }
}

const JUP: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
const JUP_EVENTS: &str = "D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf";

/// The 82 bytes of an initialized Token mint with `decimals`.
fn token_mint(decimals: u8) -> Vec<u8> {
    let mut data = vec![0; 82];
    data[44] = decimals;
    data[45] = 1;
    data
}

/// How a Jupiter instruction reads on mainnet, the only network it runs on,
/// once the node has given the accounts its sentence asks for: `mints`, each
/// a Token mint with its decimals.
fn jupiter_explained(payer: &str, ix: Instruction, mints: &[(&str, u8)]) -> IdlExplanation {
    let store = crate::fixtures::shipped_catalog(crate::IdlNetwork::Mainnet);
    let tx = decode_v1_transaction(&compile(Pubkey::from_str_const(payer), vec![ix]).wire).unwrap();
    let needed = explain_transaction(&store, &tx).accounts_needed();
    let fetched: Vec<_> = needed
        .iter()
        .map(|(address, _)| {
            mints
                .iter()
                .find(|(mint, _)| Pubkey::from_str_const(mint) == *address)
                .map(|(_, decimals)| raw_account(TOKEN_PROGRAM_ID, &token_mint(*decimals)))
        })
        .collect();
    let note = DisplayNote {
        accounts: display_accounts(&needed, &fetched),
        ..DisplayNote::default()
    };
    explain_with_note(&store, &tx, &note).instructions[0]
        .idl
        .clone()
        .expect("explained from an IDL")
}

/// A mainnet `route_v2` that sends its output to another account and takes a
/// platform fee (transaction `5aH3BnmS…`), with the first accounts of its
/// route plan.
#[test]
fn a_jupiter_swap_names_its_mints_quote_slippage_and_fee() {
    let owner = "AER6GKt4reeBqQaQDzU7aHNqhVT3SnfZRFD1gBT4nZVr";
    let input = "HwZSULZAMKyFyLaSLVVAAKGvgbau2ScnWEX7ozF7pump";
    let output = "CASHx9KJUStyftLFWGvEVf59SGeG9sh5FfcnZMVPCASH";
    let token_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
    let swap = jupiter_ix(
        "bb64facc31c4af14a7b3724111010000fe1d25010000000070085500000004000000c0001027000130ea040103a1010000000026220103740010270304",
        &[
            (owner, true, true),
            ("61c1ZXYghfB1jipxy6W8xc3QmujERmq5RRRq5MziZma5", false, true),
            ("FNuFCd7CFHniMyt7eYjbVsQkMAT9hmAVL9VKpqguDWJq", false, true),
            (input, false, false),
            (output, false, false),
            (token_2022, false, false),
            (token_2022, false, false),
            ("79hbfuh1Y6zGrXkdGxCCxoZeUvsDAZwpcnjBCcqir8qm", false, true),
            (JUP_EVENTS, false, false),
            (JUP, false, false),
            ("8ZjzUhmxDREvwhYfoGYsfsvhLHoKy2BLA5iLCm1GZGVT", false, true),
            ("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P", false, false),
            ("4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf", false, false),
        ],
    );
    let idl = jupiter_explained(owner, swap, &[(input, 6), (output, 6)]);
    assert_eq!(idl.intent, "Swap tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps 1,173,624.107943 of {input} for a quoted 19.209726 of \
                 {output}, accepting up to 21.6 % less"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Platform Fee"), "0.85 %");
    assert_eq!(field(&idl, "Positive Slippage"), "0 %");
    assert_eq!(
        field(&idl, "From"),
        "61c1ZXYghfB1jipxy6W8xc3QmujERmq5RRRq5MziZma5"
    );
    assert_eq!(
        field(&idl, "To"),
        "FNuFCd7CFHniMyt7eYjbVsQkMAT9hmAVL9VKpqguDWJq"
    );
    assert_eq!(
        field(&idl, "Output Sent To"),
        "79hbfuh1Y6zGrXkdGxCCxoZeUvsDAZwpcnjBCcqir8qm"
    );
    // The route plan's accounts are a declared group, not unnamed extras.
    assert_eq!(idl.unnamed_accounts, 0);
    assert!(idl
        .fields
        .iter()
        .all(|field| field.value != token_2022 && field.value != JUP_EVENTS));
}

/// A mainnet `route` that passes neither an output account nor a platform fee
/// account (transaction `4dwR9Fv5…`): Anchor puts the program's own address in
/// their place, and review leaves them out rather than show it. `route` names
/// no input mint, so its input stays in base units.
#[test]
fn a_jupiter_swap_leaves_out_the_optional_accounts_it_does_not_pass() {
    let owner = "3VTMBUmb6Bm8q1hhAWNRYqEadtLdj5UcBiNFkcC56h2L";
    let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let swap = jupiter_ix(
        "e517cb977ae3ad2a01000000b501640001e097c1110000000053da7c0100000000320000",
        &[
            ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false, false),
            (owner, true, true),
            ("2n3hGfs6QTfrJbPsrsBFv8XdFuG8kXzueC5Zt1vgVp41", false, true),
            ("9FPcwpMEi4A4jJ8csQizHTviJcnUzRcUcDke1TExw6Zb", false, true),
            (JUP, false, false),
            (usdc, false, false),
            (JUP, false, false),
            (JUP_EVENTS, false, false),
            (JUP, false, false),
            ("3TK9D8aoBFYjYZtKCjciPrVrRStsnvo7KmpcJqDavpaU", false, false),
        ],
    );
    let idl = jupiter_explained(owner, swap, &[(usdc, 6)]);
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps 297,900,000 base units from \
                 2n3hGfs6QTfrJbPsrsBFv8XdFuG8kXzueC5Zt1vgVp41 for a quoted 24.959571 of \
                 {usdc}, accepting up to 0.5 % less"
            )
            .as_str()
        )
    );
    assert_eq!(
        field(&idl, "To"),
        "9FPcwpMEi4A4jJ8csQizHTviJcnUzRcUcDke1TExw6Zb"
    );
    assert!(idl.fields.iter().all(|field| field.value != JUP));
    assert!(idl
        .fields
        .iter()
        .all(|field| field.label != "Output Sent To" && field.label != "Platform Fee Account"));
}

/// An `exact_out_route_v2` names the amount bought and the most it may cost.
#[test]
fn a_jupiter_exact_out_swap_names_what_it_buys_and_the_most_it_pays() {
    let owner = pk(1).to_string();
    let source = pk(4).to_string();
    let destination = pk(5).to_string();
    let mut data = hex::decode("9d8ab85215f4f324").unwrap();
    data.extend(5_000_000u64.to_le_bytes());
    data.extend(2_000_000_000u64.to_le_bytes());
    data.extend(50u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    // One step: `saber` (variant 0, no data), all of it, token 0 to token 1.
    data.extend(1u32.to_le_bytes());
    data.extend([0, 0x10, 0x27, 0, 1]);
    let token = TOKEN_PROGRAM_ID.to_string();
    let swap = jupiter_ix(
        &hex::encode(data),
        &[
            (&owner, true, false),
            (&pk(2).to_string(), false, true),
            (&pk(3).to_string(), false, true),
            (&source, false, false),
            (&destination, false, false),
            (&token, false, false),
            (&token, false, false),
            (JUP, false, false),
            (JUP_EVENTS, false, false),
            (JUP, false, false),
        ],
    );
    let idl = jupiter_explained(&owner, swap, &[(&destination, 6), (&source, 9)]);
    assert_eq!(idl.intent, "Buy tokens");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} buys 5 of {destination} for a quoted 2 of {source}, paying up to \
                 0.5 % more"
            )
            .as_str()
        )
    );
    assert_eq!(idl.unnamed_accounts, 0);
}

/// Until the node gives a swap's mints, its amounts have no scale: no sentence,
/// and the fields say so, as for any amount whose decimals are not known yet.
#[test]
fn a_jupiter_swap_reads_once_its_mints_are_fetched() {
    let owner = pk(1).to_string();
    let (input, output) = (pk(4).to_string(), pk(5).to_string());
    let mut data = hex::decode("bb64facc31c4af14").unwrap();
    data.extend(1_500_000u64.to_le_bytes());
    data.extend(2_000_000_000u64.to_le_bytes());
    data.extend(50u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(1u32.to_le_bytes());
    data.extend([0, 0x10, 0x27, 0, 1]);
    let data = hex::encode(data);
    let (from, to) = (pk(2).to_string(), pk(3).to_string());
    let token = TOKEN_PROGRAM_ID.to_string();
    let swap = || {
        jupiter_ix(
            &data,
            &[
                (&owner, true, false),
                (&from, false, true),
                (&to, false, true),
                (&input, false, false),
                (&output, false, false),
                (&token, false, false),
                (&token, false, false),
                (JUP, false, false),
                (JUP_EVENTS, false, false),
                (JUP, false, false),
            ],
        )
    };

    let bare = jupiter_explained(&owner, swap(), &[]);
    assert_eq!(bare.interpolated_intent, None);
    assert_eq!(field(&bare, "Amount In"), "1,500,000 (raw)");

    let read = jupiter_explained(&owner, swap(), &[(&input, 6), (&output, 9)]);
    assert_eq!(
        read.interpolated_intent.as_deref(),
        Some(
            format!(
                "{owner} swaps 1.5 of {input} for a quoted 2 of {output}, accepting up to \
                 0.5 % less"
            )
            .as_str()
        )
    );
}

/// A mint is read for any program that names it, as a swap names the mints it
/// trades: only the Token programs write an initialized mint they own.
#[test]
fn a_mint_is_read_whichever_program_reads_it() {
    let reader = pk(3);
    let needed = [(pk(7), reader), (pk(8), reader)];
    let fetched = [
        Some(raw_account(TOKEN_PROGRAM_ID, &token_mint(6))),
        Some(raw_account(TOKEN_2022_PROGRAM_ID, MINT_WITH_METADATA)),
    ];
    let kept = display_accounts(&needed, &fetched);
    assert_eq!(kept.get(&pk(7)), Some(&token_mint(6)));
    assert_eq!(
        kept.get(&pk(8)).map(Vec::as_slice),
        Some(MINT_WITH_METADATA)
    );
}

/// Nothing else the Token programs own is taken for a mint: not a token
/// account, a multisig, or a mint not yet initialized. Nor is a mint's shape
/// owned by any other program.
#[test]
fn only_an_initialized_token_mint_is_read_for_another_program() {
    let reader = pk(3);
    let mut uninitialized = token_mint(6);
    uninitialized[45] = 0;
    let mut extended_account = vec![0; 170];
    extended_account[45] = 1;
    extended_account[165] = 2;
    let mut multisig = vec![1; 355];
    multisig[165] = 1;
    let cases = [
        raw_account(TOKEN_PROGRAM_ID, &uninitialized),
        raw_account(TOKEN_PROGRAM_ID, &[1; 165]),
        raw_account(TOKEN_PROGRAM_ID, &[1; 355]),
        raw_account(TOKEN_2022_PROGRAM_ID, &extended_account),
        raw_account(TOKEN_2022_PROGRAM_ID, &multisig),
        raw_account(pk(9), &token_mint(6)),
    ];
    for account in cases {
        let kept = display_accounts(&[(pk(7), reader)], &[Some(account.clone())]);
        assert!(
            kept.is_empty(),
            "{} bytes owned by {}",
            account.data.len(),
            account.owner
        );
    }
}

fn clear_signing_client() -> ProgramClient {
    let idl = crate::idl::builtin::builtin_idl(
        crate::IdlNetwork::Devnet,
        &crate::ids::SQUADS_CLEAR_SIGNING_PROGRAM_ID,
    )
    .expect("the clear signing IDL is in the core on devnet");
    ProgramClient::from_json(idl.json).unwrap()
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::struct_from(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    )
}

/// One instruction a vault transaction stores, as `verifyProposal` carries
/// it, and its bytes as the program reads them (Borsh).
fn expected_instruction(ix: &Instruction) -> (Value, Vec<u8>) {
    let value = record([
        ("programId", Value::PublicKey(ix.program_id.to_bytes())),
        (
            "accounts",
            Value::Array(
                ix.accounts
                    .iter()
                    .map(|meta| {
                        record([
                            ("pubkey", Value::PublicKey(meta.pubkey.to_bytes())),
                            ("isSigner", Value::Bool(meta.is_signer)),
                            ("isWritable", Value::Bool(meta.is_writable)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("data", Value::Bytes(ix.data.clone())),
    ]);
    let mut bytes = ix.program_id.to_bytes().to_vec();
    bytes.extend_from_slice(&(ix.accounts.len() as u32).to_le_bytes());
    for meta in &ix.accounts {
        bytes.extend_from_slice(meta.pubkey.as_ref());
        bytes.push(meta.is_signer as u8);
        bytes.push(meta.is_writable as u8);
    }
    bytes.extend_from_slice(&(ix.data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&ix.data);
    (value, bytes)
}

/// `verifyProposal` built from the IDL lays out its data as the program
/// reads it, and reads as a sentence that names the proposal, the
/// transaction, the multisig, the member and the vote. The instructions it
/// carries are not a field: review opens them.
#[test]
fn a_clear_signed_vote_names_its_proposal_member_and_vote() {
    let (proposal, transaction, multisig, member) = (pk(3), pk(4), pk(5), pk(6));
    let stored = [
        fixtures::transfer(pk(7), pk(8), 1_000_000),
        Instruction {
            program_id: crate::ids::MEMO_PROGRAM_ID,
            accounts: vec![AccountMeta::new_readonly(pk(7), true)],
            data: b"payroll".to_vec(),
        },
    ];
    let (values, encoded): (Vec<Value>, Vec<Vec<u8>>) =
        stored.iter().map(expected_instruction).unzip();
    let verify = clear_signing_client()
        .method("verifyProposal")
        .unwrap()
        .account("proposal", proposal)
        .account("transaction", transaction)
        .argument(
            "args",
            record([
                ("multisig", Value::PublicKey(multisig.to_bytes())),
                ("transactionIndex", Value::from(12u64)),
                ("vaultIndex", Value::from(0u8)),
                ("numEphemeralSigners", Value::from(0u8)),
                ("member", Value::PublicKey(member.to_bytes())),
                ("action", Value::unit_enum("approve")),
                ("instructions", Value::Array(values)),
            ]),
        )
        .build()
        .unwrap();

    let mut data = hex::decode("368c3946f39d1e7c").unwrap();
    data.extend_from_slice(multisig.as_ref());
    data.extend_from_slice(&12u64.to_le_bytes());
    data.extend_from_slice(&[0, 0]);
    data.extend_from_slice(member.as_ref());
    data.push(0); // Approve
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend(encoded.concat());
    assert_eq!(verify.data, data);
    assert_eq!(
        verify.accounts,
        [
            AccountMeta::new_readonly(proposal, false),
            AccountMeta::new_readonly(transaction, false),
            AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR, false),
        ]
    );

    let idl = explain_one_ix(&builtins(), member, verify.clone());
    assert_eq!(idl.intent, "Verify proposal");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "Checks that proposal {proposal}, transaction #12 of multisig {multisig}, holds \
                 exactly the instructions shown, for {member} to approve it"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Vault Index"), "0");
    assert_eq!(field(&idl, "Ephemeral Signers"), "0");
    assert_eq!(field(&idl, "Vote"), "approve");
    assert!(
        !idl.fields
            .iter()
            .any(|field| field.label.contains("Instructions")),
        "{:?}",
        idl.fields
    );
    assert_eq!(idl.unnamed_accounts, 0);

    // A lookup table the stored message names comes after the named accounts.
    let mut with_table = verify;
    with_table
        .accounts
        .push(AccountMeta::new_readonly(pk(9), false));
    let idl = explain_one_ix(&builtins(), member, with_table);
    assert_eq!(idl.unnamed_accounts, 0);
    assert_eq!(field(&idl, "Lookup Table"), pk(9).to_string());
}

const INSTRUCTIONS_SYSVAR: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// The hash a buffer is checked against is shown whole: it is what the
/// signer compares with `solana-verify get-buffer-hash`.
#[test]
fn a_buffer_hash_check_shows_the_whole_hash() {
    let (buffer, member) = (pk(3), pk(6));
    let hash = "7ff737ad44ad241dc6cb45dae1010ba61774c6d55b662876282dbc40a9efe573";
    let mut data = hex::decode("45610c8e75ad7c1f").unwrap();
    data.extend_from_slice(buffer.as_ref());
    data.extend_from_slice(&(hash.len() as u32).to_le_bytes());
    data.extend_from_slice(hash.as_bytes());
    let built = clear_signing_client()
        .method("verifyBufferHash")
        .unwrap()
        .account("buffer", buffer)
        .argument(
            "args",
            record([
                ("buffer", Value::PublicKey(buffer.to_bytes())),
                ("expectedHash", Value::String(hash.into())),
            ]),
        )
        .build()
        .unwrap();
    assert_eq!(built.data, data);
    assert_eq!(built.accounts, [AccountMeta::new_readonly(buffer, false)]);

    let idl = explain_one_ix(&builtins(), member, built);
    assert_eq!(idl.intent, "Verify buffer hash");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(format!("Checks that buffer {buffer} holds the build whose hash is {hash}").as_str())
    );
    assert_eq!(field(&idl, "Buffer Hash"), hash);
}

fn loader_ix(tag: u32, args: &[u8], accounts: Vec<AccountMeta>) -> Instruction {
    let mut data = tag.to_le_bytes().to_vec();
    data.extend_from_slice(args);
    Instruction {
        program_id: crate::program_code::BPF_LOADER_UPGRADEABLE_ID,
        accounts,
        data,
    }
}

const RENT: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");
const CLOCK: Pubkey = Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

/// Who upgrades which program from which buffer, and where the buffer's
/// spare lamports go, in the account order of
/// `solana_loader_v3_interface::instruction::upgrade`.
#[test]
fn a_program_upgrade_names_the_program_buffer_and_spill() {
    let (program_data, program, buffer, spill, authority) = (pk(2), pk(3), pk(4), pk(5), pk(6));
    let idl = explain_one_ix(
        &builtins(),
        authority,
        loader_ix(
            3,
            &[],
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new(program, false),
                AccountMeta::new(buffer, false),
                AccountMeta::new(spill, false),
                AccountMeta::new_readonly(RENT, false),
                AccountMeta::new_readonly(CLOCK, false),
                AccountMeta::new_readonly(authority, true),
            ],
        ),
    );
    assert_eq!(idl.intent, "Upgrade program");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "{authority} upgrades program {program} to the build in buffer {buffer}, and \
                 sends the buffer's spare lamports to {spill}"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Program Data"), program_data.to_string());
    assert!(!idl
        .fields
        .iter()
        .any(|field| field.value == RENT.to_string() || field.value == CLOCK.to_string()));
}

/// Leaving the new authority out makes the program or buffer final. A
/// sentence cannot name an account the instruction leaves out, so it says
/// so either way, and the new authority, when there is one, is a field.
#[test]
fn setting_a_loader_authority_says_that_leaving_it_out_is_final() {
    let (program_data, current, new) = (pk(2), pk(3), pk(4));
    let sentence = format!(
        "{current} hands over the authority of {program_data}. Without a new authority, it can \
         never change again"
    );
    let handed = explain_one_ix(
        &builtins(),
        current,
        loader_ix(
            4,
            &[],
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new_readonly(current, true),
                AccountMeta::new_readonly(new, false),
            ],
        ),
    );
    assert_eq!(handed.intent, "Set authority");
    assert_eq!(
        handed.interpolated_intent.as_deref(),
        Some(sentence.as_str())
    );
    assert_eq!(field(&handed, "New Authority"), new.to_string());

    let finalized = explain_one_ix(
        &builtins(),
        current,
        loader_ix(
            4,
            &[],
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new_readonly(current, true),
            ],
        ),
    );
    assert_eq!(
        finalized.interpolated_intent.as_deref(),
        Some(sentence.as_str())
    );
    assert!(!finalized
        .fields
        .iter()
        .any(|field| field.label == "New Authority"));
    assert_eq!(finalized.unnamed_accounts, 0);

    // The checked form names the new authority, which signs.
    let checked = explain_one_ix(
        &builtins(),
        current,
        loader_ix(
            7,
            &[],
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new_readonly(current, true),
                AccountMeta::new_readonly(new, true),
            ],
        ),
    );
    assert_eq!(
        checked.interpolated_intent.as_deref(),
        Some(format!("{current} makes {new} the authority of {program_data}").as_str())
    );
}

/// A write names its buffer and offset. The bytes it writes, a slice of an
/// executable, are not a field.
#[test]
fn a_buffer_write_names_the_buffer_and_offset_and_hides_the_bytes() {
    let (buffer, authority) = (pk(2), pk(3));
    let chunk = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
    let mut args = 4096u32.to_le_bytes().to_vec();
    args.extend_from_slice(&(chunk.len() as u64).to_le_bytes());
    args.extend_from_slice(&chunk);
    let idl = explain_one_ix(
        &builtins(),
        authority,
        loader_ix(
            1,
            &args,
            vec![
                AccountMeta::new(buffer, false),
                AccountMeta::new_readonly(authority, true),
            ],
        ),
    );
    assert_eq!(idl.intent, "Write to buffer");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!("{authority} writes program bytes into buffer {buffer} at offset 4096")
                .as_str()
        )
    );
    assert_eq!(
        idl.fields
            .iter()
            .map(|field| field.label.as_str())
            .collect::<Vec<_>>(),
        ["Offset (bytes)", "Buffer", "Buffer Authority"]
    );
}

/// Extending a program reads its size in bytes.
#[test]
fn extending_a_program_says_by_how_many_bytes() {
    let (program_data, program, payer) = (pk(2), pk(3), pk(4));
    let idl = explain_one_ix(
        &builtins(),
        payer,
        loader_ix(
            6,
            &10_240u32.to_le_bytes(),
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new(program, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new(payer, true),
            ],
        ),
    );
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(format!("Extends program {program} by 10,240 bytes").as_str())
    );
    assert_eq!(field(&idl, "Payer"), payer.to_string());
}

/// A Squads vote with the check that pins what its proposal holds: the check
/// opens to the instructions it carries, read with their own IDLs, run as the
/// vault. They are read from the check's bytes, so they open with no IDL for
/// the check itself, as on a network whose catalog has none.
#[test]
fn a_checked_vote_opens_to_what_the_proposal_holds() {
    use crate::wallets::squads::{CheckForm, ProposalCheck, Squad, VoteAction};
    let squad = Squad::v4(&pk(50));
    let (vault, member) = (squad.vault(1), pk(1));
    let (program_data, program, buffer, spill) = (pk(2), pk(3), pk(4), pk(5));
    let held = vec![
        fixtures::transfer(vault, pk(8), 1_000_000),
        loader_ix(
            3,
            &[],
            vec![
                AccountMeta::new(program_data, false),
                AccountMeta::new(program, false),
                AccountMeta::new(buffer, false),
                AccountMeta::new(spill, false),
                AccountMeta::new_readonly(RENT, false),
                AccountMeta::new_readonly(CLOCK, false),
                AccountMeta::new_readonly(vault, true),
            ],
        ),
    ];
    let check = ProposalCheck {
        form: CheckForm::TopLevel,
        squad,
        transaction_index: 12,
        vault_index: 1,
        vault,
        ephemeral_signers: 0,
        member,
        action: VoteAction::Approve,
        instructions: held.clone(),
    };
    let verify = check.instruction(&[]);
    let wire = compile(member, vec![verify.clone(), squad.approve(12, &member)]).wire;
    let tx = decode_v1_transaction(&wire).unwrap();

    // What the transaction carries comes back exactly: the same metas, flags
    // and data the program compares with what is stored.
    let native = tx.native_instructions();
    assert_eq!(ProposalCheck::parse(&native[0]).unwrap().instructions, held);

    for (catalog, worded) in [
        (builtins(), true),
        (
            crate::fixtures::shipped_catalog(crate::IdlNetwork::Mainnet),
            false,
        ),
        (
            IdlStore::open(MemoryStore::new(), MemoryBodies::new()),
            false,
        ),
    ] {
        let explained = explain_transaction(&catalog, &tx);
        let opened = &explained.instructions[0];
        assert_eq!(opened.idl.is_some(), worded);
        let inner = opened.inner.as_ref().expect("the check opens");
        assert_eq!(
            inner.kind,
            InnerKind::SquadsCheck {
                multisig: squad.address,
                transaction_index: 12,
                vault,
                vault_index: 1,
                member,
                action: VoteAction::Approve,
            }
        );
        assert_eq!(inner.instructions.len(), 2);
        for (shown, held) in inner.instructions.iter().zip(&held) {
            assert_eq!(shown.program_id, held.program_id);
            assert_eq!(
                shown.accounts,
                held.accounts
                    .iter()
                    .map(|meta| meta.pubkey)
                    .collect::<Vec<_>>()
            );
            assert_eq!(shown.data, held.data);
            assert!(shown.inner.is_none());
        }
        // The vote itself is not opened: it holds nothing.
        assert!(explained.instructions[1].inner.is_none());
        // Every program inside is listed, so a missing IDL is looked for.
        let listed: Vec<Pubkey> = explained
            .programs
            .iter()
            .map(|line| line.program_id)
            .collect();
        assert!(listed.contains(&SYSTEM_PROGRAM_ID));
        assert!(listed.contains(&crate::program_code::BPF_LOADER_UPGRADEABLE_ID));
    }

    // With the IDLs the app ships, the payment and the upgrade read as
    // sentences, and the vault is what signs the upgrade.
    let explained = explain_transaction(&builtins(), &tx);
    let inner = &explained.instructions[0]
        .inner
        .as_ref()
        .unwrap()
        .instructions;
    let pay = inner[0].idl.as_ref().unwrap();
    assert_eq!(pay.intent, "Transfer SOL");
    let upgrade = inner[1].idl.as_ref().unwrap();
    assert_eq!(upgrade.intent, "Upgrade program");
    let signer = upgrade.accounts.iter().find(|line| line.signer).unwrap();
    assert_eq!(signer.pubkey, vault);
    // The vault is no signer of the transaction itself: only the member is.
    assert_eq!(tx.signers.len(), 1);
    assert_eq!(tx.signers[0].pubkey, member);

    // A check that does not read to its last byte is not opened.
    let mut longer = verify;
    longer.data.push(0);
    let tx = decode_v1_transaction(&compile(member, vec![longer]).wire).unwrap();
    assert!(explain_transaction(&builtins(), &tx).instructions[0]
        .inner
        .is_none());

    // The same check for a member that signs through a program opens the
    // same way, wherever it sits.
    let by_cpi = ProposalCheck {
        form: CheckForm::Cpi,
        ..check
    };
    let carried = crate::review::explain::explain_instructions(
        &IdlStore::open(MemoryStore::new(), MemoryBodies::new()),
        &[by_cpi.instruction(&[])],
    );
    let inner = carried[0].inner.as_ref().expect("the check opens");
    assert_eq!(
        inner.kind,
        InnerKind::SquadsCheck {
            multisig: squad.address,
            transaction_index: 12,
            vault,
            vault_index: 1,
            member,
            action: VoteAction::Approve,
        }
    );
    assert_eq!(
        inner
            .instructions
            .iter()
            .map(|shown| shown.data.clone())
            .collect::<Vec<_>>(),
        held.iter().map(|ix| ix.data.clone()).collect::<Vec<_>>()
    );
}

/// `verifyProposalCpi` built from the IDL lays out its data as
/// `verifyProposal` does under its own tag, names the member as a signer
/// after the instructions sysvar, and reads as the same check for a member
/// that signs through a program.
#[test]
fn a_clear_signed_vote_by_cpi_reads_as_the_same_check() {
    let (proposal, transaction, multisig, member) = (pk(3), pk(4), pk(5), pk(6));
    let stored = [fixtures::transfer(pk(7), pk(8), 1_000_000)];
    let (values, encoded): (Vec<Value>, Vec<Vec<u8>>) =
        stored.iter().map(expected_instruction).unzip();
    let verify = clear_signing_client()
        .method("verifyProposalCpi")
        .unwrap()
        .account("proposal", proposal)
        .account("transaction", transaction)
        .account("member", member)
        .argument(
            "args",
            record([
                ("multisig", Value::PublicKey(multisig.to_bytes())),
                ("transactionIndex", Value::from(12u64)),
                ("vaultIndex", Value::from(0u8)),
                ("numEphemeralSigners", Value::from(0u8)),
                ("member", Value::PublicKey(member.to_bytes())),
                ("action", Value::unit_enum("approve")),
                ("instructions", Value::Array(values)),
            ]),
        )
        .build()
        .unwrap();

    let mut data = hex::decode("dc969bbf04fb8ca8").unwrap();
    assert_eq!(data, crate::wallets::squads::VERIFY_PROPOSAL_CPI);
    data.extend_from_slice(multisig.as_ref());
    data.extend_from_slice(&12u64.to_le_bytes());
    data.extend_from_slice(&[0, 0]);
    data.extend_from_slice(member.as_ref());
    data.push(0); // Approve
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend(encoded.concat());
    assert_eq!(verify.data, data);
    assert_eq!(
        verify.accounts,
        [
            AccountMeta::new_readonly(proposal, false),
            AccountMeta::new_readonly(transaction, false),
            AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR, false),
            AccountMeta::new_readonly(member, true),
        ]
    );
    // What the app builds for the same check is these bytes.
    let parsed = crate::wallets::squads::ProposalCheck::from_data(&verify.data).unwrap();
    assert_eq!(parsed.form, crate::wallets::squads::CheckForm::Cpi);

    let idl = explain_one_ix(&builtins(), member, verify.clone());
    assert_eq!(idl.intent, "Verify proposal by CPI");
    assert_eq!(
        idl.interpolated_intent.as_deref(),
        Some(
            format!(
                "Checks that proposal {proposal}, transaction #12 of multisig {multisig}, holds \
                 exactly the instructions shown, for {member} to approve it, signing through a \
                 program"
            )
            .as_str()
        )
    );
    assert_eq!(field(&idl, "Vote"), "approve");
    assert!(
        !idl.fields
            .iter()
            .any(|field| field.label.contains("Instructions")),
        "{:?}",
        idl.fields
    );
    assert_eq!(idl.unnamed_accounts, 0);

    // A lookup table the stored message names comes after the member.
    let mut with_table = verify;
    with_table
        .accounts
        .push(AccountMeta::new_readonly(pk(9), false));
    let idl = explain_one_ix(&builtins(), member, with_table);
    assert_eq!(idl.unnamed_accounts, 0);
    assert_eq!(field(&idl, "Lookup Table"), pk(9).to_string());
}

/// One file of `tests/fixtures/pack`: instructions a cluster executed, with
/// what Review and Sign says about each.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedFile {
    /// The network the instructions ran on, whose IDLs read them.
    network: String,
    /// What a node answers for the accounts the sentences read (a mint, for
    /// its decimals), by address.
    #[serde(default)]
    accounts: std::collections::BTreeMap<String, RecordedAccount>,
    instructions: Vec<RecordedIx>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedAccount {
    owner: String,
    /// The account's data, in base64.
    data: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedIx {
    /// The transaction's signature, then `#` and the instruction's place in
    /// it, from 1: `#3` is its third instruction, `#3.2` the second call the
    /// third made into another program.
    from: String,
    program_id: String,
    /// Each account's address, then `:s` for a signer, `:w` for writable or
    /// `:sw` for both.
    accounts: Vec<String>,
    /// The instruction's data, in hex.
    data: String,
    intent: String,
    /// `None` for an instruction whose IDL gives it no sentence.
    sentence: Option<String>,
    /// Every field under the sentence, as `[label, value]`, in order.
    fields: Vec<(String, String)>,
    #[serde(default)]
    unnamed_accounts: usize,
}

impl RecordedIx {
    fn instruction(&self) -> Instruction {
        Instruction {
            program_id: Pubkey::from_str_const(&self.program_id),
            accounts: self
                .accounts
                .iter()
                .map(|account| {
                    let (address, flags) = account.split_once(':').unwrap_or((account, ""));
                    AccountMeta {
                        pubkey: Pubkey::from_str_const(address),
                        is_signer: flags.contains('s'),
                        is_writable: flags.contains('w'),
                    }
                })
                .collect(),
            data: hex::decode(&self.data).expect("hex data"),
        }
    }
}

/// How the app reads `ix` on its own in a transaction, once the node has
/// answered for the accounts its sentence reads with what `chain` holds.
fn recorded_explanation(
    store: &IdlStore<MemoryStore, MemoryBodies>,
    ix: Instruction,
    chain: &HashMap<Pubkey, crate::chain::RawAccount>,
) -> Option<IdlExplanation> {
    let payer = ix
        .accounts
        .iter()
        .find(|account| account.is_signer)
        .map_or(pk(1), |account| account.pubkey);
    let tx = decode_v1_transaction(&compile(payer, vec![ix]).wire).unwrap();
    let needed = explain_transaction(store, &tx).accounts_needed();
    let fetched: Vec<_> = needed
        .iter()
        .map(|(address, _)| chain.get(address).cloned())
        .collect();
    let note = DisplayNote {
        accounts: display_accounts(&needed, &fetched),
        ..DisplayNote::default()
    };
    explain_with_note(store, &tx, &note).instructions[0]
        .idl
        .clone()
}

/// Every instruction recorded in `tests/fixtures/pack` reads as recorded: its
/// title, its sentence, each field under it, and no account left without a
/// name. A file there is named after the IDL it exercises, and holds
/// instructions as a cluster executed them, so an IDL of the pack is checked
/// against what its program accepted and not against bytes built from the IDL
/// itself.
#[test]
fn every_recorded_instruction_of_the_pack_reads_as_recorded() {
    use base64::Engine;

    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pack");
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .expect("tests/fixtures/pack")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty());

    let mut failures = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let file: RecordedFile = serde_json::from_slice(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        let network = crate::IdlNetwork::ALL
            .into_iter()
            .find(|network| network.to_string() == file.network)
            .unwrap_or_else(|| panic!("{name}: no network {}", file.network));
        let store = crate::fixtures::shipped_catalog(network);
        let chain: HashMap<Pubkey, crate::chain::RawAccount> = file
            .accounts
            .iter()
            .map(|(address, account)| {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(&account.data)
                    .unwrap_or_else(|err| panic!("{name}: {address}: {err}"));
                (
                    Pubkey::from_str_const(address),
                    raw_account(Pubkey::from_str_const(&account.owner), &data),
                )
            })
            .collect();
        assert!(!file.instructions.is_empty(), "{name} records nothing");
        for recorded in &file.instructions {
            let at = format!("{name}: {}", recorded.from);
            let Some(idl) = recorded_explanation(&store, recorded.instruction(), &chain) else {
                failures.push(format!("{at}: no IDL reads it"));
                continue;
            };
            let fields: Vec<(String, String)> = idl
                .fields
                .iter()
                .map(|field| (field.label.clone(), field.value.clone()))
                .collect();
            if idl.intent != recorded.intent {
                failures.push(format!("{at}: intent {:?}", idl.intent));
            }
            if idl.interpolated_intent != recorded.sentence {
                failures.push(format!("{at}: sentence {:?}", idl.interpolated_intent));
            }
            if fields != recorded.fields {
                failures.push(format!("{at}: fields {fields:?}"));
            }
            if idl.unnamed_accounts != recorded.unnamed_accounts {
                failures.push(format!("{at}: {} unnamed accounts", idl.unnamed_accounts));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
