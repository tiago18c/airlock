use super::*;
use crate::idl::catalog::{IdlStore, MemoryBodies};
use crate::ids::SYSTEM_PROGRAM_ID;
use crate::tx::builder::{compile_v1, compile_v1_budget, ComputeBudgetSpec};
use crate::tx::decode::decode_v1_transaction;
use crate::wallets::programmatic::{
    message_instructions, programmatic_signer_address, Execution, ExecutorNonce, Submission,
};
use crate::wallets::relay::{wrap_for_programmatic, wrap_for_vector, VectorWrap};
use crate::wallets::squads::{Squad, StoredTransaction};
use solana_hash::Hash;
use solana_instruction::AccountMeta;
use solana_system_interface::instruction::transfer;
use wallet_vault::store::MemoryStore;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn builtins() -> IdlStore<MemoryStore, MemoryBodies> {
    IdlStore::open(MemoryStore::new(), MemoryBodies::new()).with_builtins()
}

/// What Build compiles a block into.
fn build(store: &IdlStore<MemoryStore, MemoryBodies>, opened: &OpenedIx) -> Instruction {
    match opened {
        OpenedIx::Idl(draft) => {
            let client = store.program_client(&draft.program_id.unwrap()).unwrap();
            draft.clone().build(&client).unwrap()
        }
        OpenedIx::Raw(raw) => raw.build().unwrap(),
        OpenedIx::Carried { carrier, inner } => {
            let inner: Vec<Instruction> = inner.iter().map(|ix| build(store, ix)).collect();
            carrier.wrap(&inner).unwrap()
        }
    }
}

fn idl(opened: &OpenedIx) -> &IdlDraft {
    match opened {
        OpenedIx::Idl(draft) => draft,
        other => panic!("not read by an IDL: {other:?}"),
    }
}

fn carried(opened: &OpenedIx) -> (&Carrier, &[OpenedIx]) {
    match opened {
        OpenedIx::Carried { carrier, inner } => (carrier, inner),
        other => panic!("carries nothing: {other:?}"),
    }
}

#[test]
fn a_transfer_reads_back_onto_its_form_and_the_budget_onto_build() {
    let store = builtins();
    let (payer, dest) = (pk(1), pk(2));
    let payment = transfer(&payer, &dest, 5_000);
    let compiled = compile_v1_budget(
        payer,
        vec![payment.clone()],
        None,
        Some(Hash::new_from_array([9; 32])),
        ComputeBudgetSpec {
            cu_limit: Some(30_000),
            cu_price_micro_lamports: Some(1_234),
            loaded_accounts_data_size: Some(64_000),
        },
    )
    .unwrap();
    let tx = decode_v1_transaction(&compiled.wire).unwrap();
    let opened = open_for_edit(&store, &tx);

    assert_eq!(opened.fee_payer, Some(payer));
    assert_eq!(opened.nonce, None);
    assert_eq!(opened.cu_limit, Some(30_000));
    assert_eq!(opened.loaded_accounts, Some(64_000));
    // The header states a total. The price read back comes to the same one.
    let price = opened.cu_price_micro_lamports.unwrap();
    assert_eq!(
        (u128::from(price) * 30_000).div_ceil(1_000_000),
        u128::from(tx.budget.priority_fee.unwrap())
    );

    assert_eq!(opened.instructions.len(), 1);
    let draft = idl(&opened.instructions[0]);
    assert_eq!(draft.program_id, Some(SYSTEM_PROGRAM_ID));
    assert_eq!(draft.instruction, "transferSol");
    assert_eq!(draft.argument("amount"), "5000");
    assert_eq!(draft.account("source"), Some(payer));
    assert_eq!(draft.account("destination"), Some(dest));
    assert_eq!(build(&store, &opened.instructions[0]), payment);
}

#[test]
fn what_no_idl_reads_stays_accounts_and_bytes() {
    let store = builtins();
    let odd = Instruction {
        program_id: pk(7),
        accounts: vec![
            AccountMeta::new(pk(1), true),
            AccountMeta::new_readonly(pk(3), false),
        ],
        data: vec![0xde, 0xad, 0xbe, 0xef],
    };
    // A System instruction its IDL does not know.
    let unknown_system = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(pk(1), true)],
        data: vec![0xff; 4],
    };
    let compiled = compile_v1(
        pk(1),
        vec![odd.clone(), unknown_system.clone()],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    let tx = decode_v1_transaction(&compiled.wire).unwrap();
    let opened = open_for_edit(&store, &tx);

    let OpenedIx::Raw(raw) = &opened.instructions[0] else {
        panic!("not raw: {:?}", opened.instructions[0]);
    };
    assert_eq!(raw.data, "deadbeef");
    assert_eq!(raw.accounts.len(), 2);
    assert!(raw.accounts[0].signer && raw.accounts[0].writable);
    assert!(!raw.accounts[1].signer && !raw.accounts[1].writable);
    assert_eq!(build(&store, &opened.instructions[0]), odd);
    assert!(matches!(opened.instructions[1], OpenedIx::Raw(_)));
    assert_eq!(build(&store, &opened.instructions[1]), unknown_system);
}

#[test]
fn a_durable_nonce_goes_back_to_the_nonce_fields() {
    use crate::tx::builder::NonceSpec;
    let store = builtins();
    let nonce = NonceSpec {
        account: pk(4),
        authority: pk(1),
        hash: Hash::new_from_array([5; 32]),
    };
    let compiled = compile_v1(
        pk(1),
        vec![transfer(&pk(1), &pk(2), 1)],
        Some(nonce.clone()),
        None,
    )
    .unwrap();
    let tx = decode_v1_transaction(&compiled.wire).unwrap();
    let opened = open_for_edit(&store, &tx);
    assert_eq!(opened.nonce, Some(nonce));
    assert_eq!(opened.instructions.len(), 1);
}

/// A payment from a Squads vault, stored by a programmatic signer's
/// authorization: three levels, each opened and written again.
#[test]
fn a_payment_inside_a_squads_store_inside_a_submit_is_edited_in_place() {
    let store = builtins();
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
    let submit = tx.native_instructions().remove(0);
    let mut opened = open_for_edit(&store, &tx);

    let (carrier, inner) = carried(&opened.instructions[0]);
    assert_eq!(
        carrier.kind,
        CarrierKind::Programmatic {
            authorities: vec![authority],
            nonce_account: pk(8),
        }
    );
    let (stored, payments) = carried(&inner[0]);
    assert_eq!(
        stored.kind,
        CarrierKind::Squads {
            vault,
            vault_index: 0,
        }
    );
    assert_eq!(idl(&payments[0]).argument("amount"), "1000000000");
    // Unchanged, every level gives back its bytes.
    assert_eq!(build(&store, &opened.instructions[0]), submit);

    // Pay half as much.
    let OpenedIx::Carried { inner, .. } = &mut opened.instructions[0] else {
        unreachable!()
    };
    let OpenedIx::Carried {
        inner: payments, ..
    } = &mut inner[0]
    else {
        unreachable!()
    };
    let OpenedIx::Idl(draft) = &mut payments[0] else {
        unreachable!()
    };
    draft.set_argument("amount", "500000000".into());
    let edited = build(&store, &opened.instructions[0]);
    assert_ne!(edited, submit);

    let submission = Submission::parse(&edited.data).unwrap();
    // The old authorization signature does not cover this.
    assert!(submission.authorities().all(|(_, signed)| !signed));
    let [execute] = message_instructions(&submission.message)
        .try_into()
        .unwrap();
    let execution = Execution::parse(&execute).unwrap();
    assert_eq!(execution.nonce_account, pk(8));
    let [store_tx] = execution.instructions().try_into().unwrap();
    let stored = StoredTransaction::parse(&store_tx).unwrap();
    assert_eq!(stored.vault, vault);
    assert_eq!(
        stored.instructions,
        vec![transfer(&vault, &pk(2), 500_000_000)]
    );
}

#[test]
fn a_vector_passthrough_is_written_again_around_its_edit() {
    use crate::keys::signer::{KeypairSigner, WalletSigner};
    use crate::wallets::vector::{address, parse_passthrough, SchemeId, PROGRAM_ID};

    let store = builtins();
    let payer = pk(1);
    let key = KeypairSigner::new_random();
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, key.pubkey().as_ref()).unwrap();
    let built = compile_v1(
        payer,
        vec![transfer(&account, &pk(2), 1_000)],
        None,
        Some(Hash::new_from_array([9; 32])),
    )
    .unwrap();
    let wrap = VectorWrap {
        account,
        scheme: SchemeId::Ed25519,
        key: key.pubkey().to_bytes().to_vec(),
        next_key: None,
    };
    let wire = wrap_for_vector(
        &decode_v1_transaction(&built.wire).unwrap(),
        &[wrap],
        payer,
        Hash::new_from_array([2; 32]),
    )
    .unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    let original = tx.native_instructions()[1].clone();
    let mut opened = open_for_edit(&store, &tx);

    let (carrier, inner) = carried(&opened.instructions[1]);
    assert!(matches!(carrier.kind, CarrierKind::Passthrough { .. }));
    // A transfer from the account runs as the program's `withdraw`.
    assert_eq!(idl(&inner[0]).instruction, "withdraw");
    assert_eq!(idl(&inner[0]).argument("lamports"), "1000");
    assert_eq!(build(&store, &opened.instructions[1]), original);

    let OpenedIx::Carried { inner, .. } = &mut opened.instructions[1] else {
        unreachable!()
    };
    inner.push(OpenedIx::Raw(RawDraft::from_instruction(&transfer(
        &account,
        &pk(3),
        7,
    ))));
    let edited = build(&store, &opened.instructions[1]);
    let parsed = parse_passthrough(&edited).unwrap();
    let before = parse_passthrough(&original).unwrap();
    assert_eq!(parsed.signers, before.signers);
    assert_eq!(parsed.instructions[0], before.instructions[0]);
    assert_eq!(parsed.instructions[1], transfer(&account, &pk(3), 7));
    assert_eq!(parsed.instructions.len(), 2);
}
