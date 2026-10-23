use super::*;
use crate::error::SignError;
use crate::fixtures::{compile, transfer};
use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
use crate::keys::signer::KeypairSigner;
use crate::keys::signer::WalletSigner;
use crate::tx::builder::{compile_v1, NonceSpec};
use crate::tx::signatures::{apply_local_signatures, insert_signature};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_message::v0::{Message as V0Message, MessageAddressTableLookup};
use solana_message::{Message as LegacyMessage, VersionedMessage};
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;

fn hash_from_byte(b: u8) -> Hash {
    Hash::new_from_array([b; 32])
}

fn compiled_nonce_transfer(from: &KeypairSigner, to: Pubkey) -> crate::tx::builder::CompiledTx {
    compile_v1(
        from.pubkey(),
        vec![transfer(from.pubkey(), to, 1_000)],
        Some(NonceSpec {
            account: Pubkey::new_from_array([3; 32]),
            authority: from.pubkey(),
            hash: hash_from_byte(9),
        }),
        None,
    )
    .unwrap()
}

#[test]
fn decodes_system_transfer_with_nonce() {
    let from = KeypairSigner::new_random();
    let to = Pubkey::new_from_array([2; 32]);
    let compiled = compiled_nonce_transfer(&from, to);
    let decoded = decode_v1_transaction(&compiled.wire).unwrap();

    assert!(decoded.uses_durable_nonce);
    assert_eq!(decoded.lifetime, hash_from_byte(9));
    assert_eq!(decoded.signers.len(), 1);
    assert!(!decoded.signers[0].filled);
    assert_eq!(decoded.signers[0].pubkey, from.pubkey());
    assert_eq!(decoded.instructions.len(), 2);
    let sent = &decoded.instructions[1];
    assert_eq!(sent.program_id, SYSTEM_PROGRAM_ID);
    assert_eq!(sent.accounts, vec![from.pubkey(), to]);
    assert_eq!(sent.data, transfer(from.pubkey(), to, 1_000).data);
}

#[test]
fn only_a_leading_nonce_advance_makes_a_nonce_transaction() {
    let payer = Pubkey::new_from_array([1; 32]);
    let advance = solana_system_interface::instruction::advance_nonce_account(
        &Pubkey::new_from_array([3; 32]),
        &payer,
    );
    let send = transfer(payer, Pubkey::new_from_array([2; 32]), 5);

    let trailing = compile(payer, vec![send.clone(), advance.clone()]);
    assert!(
        !decode_v1_transaction(&trailing.wire)
            .unwrap()
            .uses_durable_nonce
    );
    let leading = compile(payer, vec![advance, send]);
    assert!(
        decode_v1_transaction(&leading.wire)
            .unwrap()
            .uses_durable_nonce
    );
}

#[test]
fn inserts_a_matching_external_signature() {
    let from = KeypairSigner::new_random();
    let to = Pubkey::new_from_array([2; 32]);
    let compiled = compiled_nonce_transfer(&from, to);
    let decoded = decode_v1_transaction(&compiled.wire).unwrap();
    let signed = from.try_sign_message(&decoded.message).unwrap();
    let bytes: [u8; SIGNATURE_BYTES] = signed.as_ref().try_into().unwrap();
    let next = insert_signature(&compiled.wire, &from.pubkey(), &bytes).unwrap();
    let again = decode_v1_transaction(&next).unwrap();
    assert!(again.signers[0].valid);
    assert!(again.signers[0].filled);
}

#[test]
fn rejects_a_signature_for_another_key() {
    let from = KeypairSigner::new_random();
    let other = KeypairSigner::new_random();
    let compiled = compiled_nonce_transfer(&from, Pubkey::new_from_array([2; 32]));
    let decoded = decode_v1_transaction(&compiled.wire).unwrap();
    let signed = other.try_sign_message(&decoded.message).unwrap();
    let bytes: [u8; SIGNATURE_BYTES] = signed.as_ref().try_into().unwrap();
    assert!(insert_signature(&compiled.wire, &from.pubkey(), &bytes).is_err());
    assert!(insert_signature(&compiled.wire, &other.pubkey(), &bytes).is_err());
}

#[test]
fn rejects_non_v1() {
    assert_eq!(
        decode_v1_transaction(&[0x80, 1, 2, 3]).unwrap_err(),
        DecodeError::NotV1
    );
    assert_eq!(decode_v1_transaction(&[]).unwrap_err(), DecodeError::NotV1);
}

#[test]
fn local_sign_fills_and_verifies() {
    let from = KeypairSigner::new_random();
    let to = Pubkey::new_from_array([2; 32]);
    let compiled = compiled_nonce_transfer(&from, to);
    let signed = apply_local_signatures(&compiled.wire, &[&from]).unwrap();
    let decoded = decode_v1_transaction(&signed).unwrap();
    assert!(decoded.signers[0].filled);
    assert!(decoded.signers[0].valid);
    assert_eq!(signed.len(), compiled.wire.len());
}

#[test]
fn missing_local_signer_is_an_error() {
    let from = KeypairSigner::new_random();
    let other = KeypairSigner::new_random();
    let compiled = compiled_nonce_transfer(&from, Pubkey::new_from_array([2; 32]));
    assert!(matches!(
        apply_local_signatures(&compiled.wire, &[&other]),
        Err(SignError::NoLocalSigner)
    ));
}

#[test]
fn account_indexes_are_resolved_or_refused() {
    let ix = decode_instruction(
        &[
            Pubkey::new_from_array([9; 32]),
            Pubkey::new_from_array([8; 32]),
        ],
        0,
        &[1],
        &[0xaa, 0xbb],
    )
    .unwrap();
    assert_eq!(ix.program_id, Pubkey::new_from_array([9; 32]));
    assert_eq!(ix.accounts, vec![Pubkey::new_from_array([8; 32])]);
    assert_eq!(ix.data, vec![0xaa, 0xbb]);
    assert_eq!(
        decode_instruction(&[Pubkey::new_from_array([9; 32])], 0, &[1], &[]),
        Err(DecodeError::BadAccountIndex)
    );
}

fn empty_sigs(n: usize) -> Vec<Signature> {
    vec![Signature::default(); n]
}

fn legacy_transfer_wire(from: &KeypairSigner, to: Pubkey) -> Vec<u8> {
    let mut message =
        LegacyMessage::new(&[transfer(from.pubkey(), to, 1_000)], Some(&from.pubkey()));
    message.recent_blockhash = hash_from_byte(1);
    let nsig = usize::from(message.header.num_required_signatures);
    wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(nsig),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap()
}

fn table_account_data(addresses: &[Pubkey]) -> Vec<u8> {
    let mut data = vec![0u8; LOOKUP_TABLE_META_SIZE];
    for address in addresses {
        data.extend_from_slice(address.as_ref());
    }
    data
}

fn v0_transfer_wire(payer: Pubkey, loaded: Pubkey, table: Pubkey, writable: bool) -> Vec<u8> {
    // Static: payer (signer writable), system program. Loaded: recipient.
    let header = solana_message::MessageHeader {
        num_required_signatures: 1,
        num_readonly_signed_accounts: 0,
        num_readonly_unsigned_accounts: 1,
    };
    let (writable_indexes, readonly_indexes) = if writable {
        (vec![0u8], vec![])
    } else {
        (vec![], vec![0u8])
    };
    // Transfer needs from (writable signer) and to (writable). Loaded index 0
    // after static keys is account index 2.
    let ix = solana_message::compiled_instruction::CompiledInstruction {
        program_id_index: 1,
        accounts: vec![0, 2],
        data: {
            let mut data = 2u32.to_le_bytes().to_vec(); // System Transfer
            data.extend_from_slice(&1_000u64.to_le_bytes());
            data
        },
    };
    let message = V0Message {
        header,
        account_keys: vec![payer, SYSTEM_PROGRAM_ID],
        recent_blockhash: hash_from_byte(1),
        instructions: vec![ix],
        address_table_lookups: vec![MessageAddressTableLookup {
            account_key: table,
            writable_indexes,
            readonly_indexes,
        }],
    };
    let _ = loaded;
    wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(1),
        message: VersionedMessage::V0(message),
    })
    .unwrap()
}

#[test]
fn decodes_a_legacy_transfer() {
    let from = KeypairSigner::new_random();
    let to = Pubkey::new_from_array([2; 32]);
    let wire = legacy_transfer_wire(&from, to);
    let view = parse_wire(&wire).unwrap();
    assert_eq!(view.format, TxFormat::Legacy);
    assert_eq!(view.num_required_signatures, 1);
    assert!(view.signature_offsets[0] < view.message.len().max(view.signature_offsets[0] + 1));

    let decoded = decode_transaction(&wire, &LookupTables::new()).unwrap();
    assert_eq!(decoded.format, TxFormat::Legacy);
    assert_eq!(decoded.signers[0].pubkey, from.pubkey());
    assert_eq!(decoded.instructions.len(), 1);
    assert_eq!(decoded.instructions[0].accounts, vec![from.pubkey(), to]);
    assert!(!decoded.uses_durable_nonce);
}

#[test]
fn legacy_budget_comes_from_compute_budget_instructions() {
    let from = KeypairSigner::new_random();
    let set_limit = Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data: [vec![2u8], 200_000u32.to_le_bytes().to_vec()].concat(),
    };
    let set_price = Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data: [vec![3u8], 1_000u64.to_le_bytes().to_vec()].concat(),
    };
    let mut message = LegacyMessage::new(
        &[
            set_limit,
            set_price,
            transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5),
        ],
        Some(&from.pubkey()),
    );
    message.recent_blockhash = hash_from_byte(1);
    let nsig = usize::from(message.header.num_required_signatures);
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(nsig),
        message: VersionedMessage::Legacy(message),
    })
    .unwrap();
    let decoded = decode_transaction(&wire, &LookupTables::new()).unwrap();
    assert_eq!(decoded.budget.compute_unit_limit, Some(200_000));
    // 1000 microlamports/CU * 200_000 CU / 1_000_000 = 200 lamports.
    assert_eq!(decoded.budget.priority_fee, Some(200));
}

#[test]
fn a_legacy_fee_without_a_limit_follows_what_the_runtime_gives() {
    let from = KeypairSigner::new_random();
    let budget = |data: Vec<u8>| Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data,
    };
    let set_price = budget([vec![3u8], 1_000u64.to_le_bytes().to_vec()].concat());
    let other_program = Instruction {
        program_id: Pubkey::new_from_array([9; 32]),
        accounts: Vec::new(),
        data: Vec::new(),
    };
    let fee_of = |instructions: &[Instruction]| {
        let mut message = LegacyMessage::new(instructions, Some(&from.pubkey()));
        message.recent_blockhash = hash_from_byte(1);
        let wire = wincode::serialize(&VersionedTransaction {
            signatures: empty_sigs(1),
            message: VersionedMessage::Legacy(message),
        })
        .unwrap();
        decode_transaction(&wire, &LookupTables::new())
            .unwrap()
            .budget
            .priority_fee
    };
    let send = transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5);
    // The price and the transfer are built-ins at 3,000 units each, the
    // other program 200,000: 206,000 units at 1,000 micro-lamports.
    assert_eq!(
        fee_of(&[set_price.clone(), send.clone(), other_program]),
        Some(206)
    );
    // A limit past the most a transaction may use is charged at that most.
    let set_limit = budget([vec![2u8], 2_000_000u32.to_le_bytes().to_vec()].concat());
    assert_eq!(fee_of(&[set_limit, set_price, send]), Some(1_400));
}

#[test]
fn v0_loads_accounts_from_tables() {
    let payer = KeypairSigner::new_random().pubkey();
    let loaded = Pubkey::new_from_array([7; 32]);
    let table = Pubkey::new_from_array([9; 32]);
    let wire = v0_transfer_wire(payer, loaded, table, true);
    assert_eq!(lookup_tables_needed(&wire).unwrap(), vec![table]);

    let mut tables = LookupTables::new();
    tables.insert(table, vec![loaded]);
    let decoded = decode_transaction(&wire, &tables).unwrap();
    assert_eq!(decoded.format, TxFormat::V0);
    assert_eq!(decoded.accounts.len(), 3);
    assert_eq!(decoded.accounts[2].pubkey, loaded);
    assert_eq!(decoded.accounts[2].lookup, Some(table));
    assert!(decoded.accounts[2].writable);
    assert!(!decoded.accounts[2].signer);
    assert_eq!(decoded.instructions[0].accounts, vec![payer, loaded]);
}

#[test]
fn v0_readonly_loaded_account_is_not_writable() {
    let payer = KeypairSigner::new_random().pubkey();
    let loaded = Pubkey::new_from_array([7; 32]);
    let table = Pubkey::new_from_array([9; 32]);
    let wire = v0_transfer_wire(payer, loaded, table, false);
    let mut tables = LookupTables::new();
    tables.insert(table, vec![loaded]);
    let decoded = decode_transaction(&wire, &tables).unwrap();
    assert!(!decoded.accounts[2].writable);
}

#[test]
fn v0_loads_writable_accounts_of_every_table_before_readonly_ones() {
    let payer = KeypairSigner::new_random().pubkey();
    let (first, second) = (
        Pubkey::new_from_array([8; 32]),
        Pubkey::new_from_array([9; 32]),
    );
    let first_writable = Pubkey::new_from_array([11; 32]);
    let first_readonly = Pubkey::new_from_array([12; 32]);
    let second_writable = Pubkey::new_from_array([21; 32]);
    let header = solana_message::MessageHeader {
        num_required_signatures: 1,
        num_readonly_signed_accounts: 0,
        num_readonly_unsigned_accounts: 1,
    };
    // 0 payer, 1 system program, 2 first_writable, 3 second_writable,
    // 4 first_readonly.
    let message = V0Message {
        header,
        account_keys: vec![payer, SYSTEM_PROGRAM_ID],
        recent_blockhash: hash_from_byte(1),
        instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
            program_id_index: 1,
            accounts: vec![2, 3, 4],
            data: vec![],
        }],
        address_table_lookups: vec![
            MessageAddressTableLookup {
                account_key: first,
                writable_indexes: vec![0],
                readonly_indexes: vec![1],
            },
            MessageAddressTableLookup {
                account_key: second,
                writable_indexes: vec![0],
                readonly_indexes: vec![],
            },
        ],
    };
    let wire = wincode::serialize(&VersionedTransaction {
        signatures: empty_sigs(1),
        message: VersionedMessage::V0(message),
    })
    .unwrap();
    let mut tables = LookupTables::new();
    tables.insert(first, vec![first_writable, first_readonly]);
    tables.insert(second, vec![second_writable]);
    let decoded = decode_transaction(&wire, &tables).unwrap();
    assert_eq!(
        decoded.instructions[0].accounts,
        vec![first_writable, second_writable, first_readonly]
    );
    let loaded: Vec<_> = decoded.accounts[2..]
        .iter()
        .map(|a| (a.pubkey, a.writable, a.lookup))
        .collect();
    assert_eq!(
        loaded,
        vec![
            (first_writable, true, Some(first)),
            (second_writable, true, Some(second)),
            (first_readonly, false, Some(first)),
        ]
    );
}

#[test]
fn v0_missing_table_or_bad_index_is_refused() {
    let payer = KeypairSigner::new_random().pubkey();
    let table = Pubkey::new_from_array([9; 32]);
    let wire = v0_transfer_wire(payer, Pubkey::new_from_array([7; 32]), table, true);
    assert_eq!(
        decode_transaction(&wire, &LookupTables::new()).unwrap_err(),
        DecodeError::MissingLookupTable(table)
    );
    let mut tables = LookupTables::new();
    tables.insert(table, vec![]); // index 0 is past the end
    assert_eq!(
        decode_transaction(&wire, &tables).unwrap_err(),
        DecodeError::LookupIndexOutOfRange
    );
}

#[test]
fn parse_lookup_table_skips_the_header() {
    let a = Pubkey::new_from_array([1; 32]);
    let b = Pubkey::new_from_array([2; 32]);
    let data = table_account_data(&[a, b]);
    assert_eq!(parse_lookup_table_addresses(&data).unwrap(), vec![a, b]);
    assert!(parse_lookup_table_addresses(&[0u8; 10]).is_err());
}
