//! Round-trip the upstream System program IDL through the dynamic client, and
//! compare each instruction and its sentence with the system interface crate.

use crate::ids::SYSTEM_PROGRAM_ID;
use codama_dynamic_client::{get_instruction_display, InstructionDisplay, ProgramClient};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_system_interface::instruction::{
    advance_nonce_account, allocate, allocate_with_seed, assign, assign_with_seed,
    authorize_nonce_account, create_account, create_account_allow_prefund,
    create_account_with_seed, create_nonce_account, transfer, transfer_with_seed,
    upgrade_nonce_account, withdraw_nonce_account,
};
use std::collections::HashMap;

const LAMPORTS: u64 = 1_500_000_000;
const SPACE: u64 = 165;
const SEED: &str = "vault";

fn system() -> ProgramClient {
    ProgramClient::from_json(include_str!("../../tests/fixtures/system-idl.json"))
        .expect("system IDL")
}

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn key(pubkey: Pubkey) -> [u8; 32] {
    pubkey.to_bytes()
}

fn round_trip(name: &str, ix: &Instruction) -> InstructionDisplay {
    let client = system();
    assert_eq!(ix.program_id, SYSTEM_PROGRAM_ID);
    let parsed = client
        .parse_instruction(ix)
        .unwrap_or_else(|err| panic!("{name} parse failed: {err}"))
        .unwrap_or_else(|| panic!("{name} was not identified"));
    get_instruction_display(client.root(), &parsed, &HashMap::new(), &HashMap::new())
        .unwrap_or_else(|| panic!("{name} has no display"))
}

fn assert_same_instruction(idl: &Instruction, reference: &Instruction) {
    assert_eq!(idl.program_id, reference.program_id);
    assert_eq!(idl.accounts, reference.accounts);
    assert_eq!(idl.data, reference.data);
}

fn expect(name: &str, ix: &Instruction, reference: &Instruction, intent: &str, sentence: &str) {
    assert_same_instruction(ix, reference);
    let display = round_trip(name, ix);
    assert_eq!(display.intent, intent, "{:?}", display.fields);
    assert_eq!(
        display.interpolated_intent.as_deref(),
        Some(sentence),
        "{:?}",
        display.fields
    );
}

#[test]
fn idl_lists_every_system_instruction() {
    assert_eq!(
        system().instruction_names(),
        [
            "createAccount",
            "assign",
            "transferSol",
            "createAccountWithSeed",
            "advanceNonceAccount",
            "withdrawNonceAccount",
            "initializeNonceAccount",
            "authorizeNonceAccount",
            "allocate",
            "allocateWithSeed",
            "assignWithSeed",
            "transferSolWithSeed",
            "upgradeNonceAccount",
            "createAccountAllowPrefund",
        ]
    );
}

#[test]
fn transfer_sol_matches_the_system_interface() {
    let source = pk(1);
    let destination = pk(2);

    let ix = system()
        .method("transferSol")
        .unwrap()
        .account("source", source)
        .account("destination", destination)
        .argument("amount", LAMPORTS)
        .build()
        .unwrap();
    let ours = &transfer(&source, &destination, LAMPORTS);
    expect(
        "transferSol",
        &ix,
        ours,
        "Transfer SOL",
        &format!("Transfer 1.5 SOL from {source} to {destination}"),
    );
    assert_same_instruction(&ix, &transfer(&source, &destination, LAMPORTS));
}

#[test]
fn create_account_matches_the_system_interface() {
    let payer = pk(1);
    let new_account = pk(2);
    let owner = pk(9);
    let ix = system()
        .method("createAccount")
        .unwrap()
        .account("payer", payer)
        .account("newAccount", new_account)
        .argument("lamports", LAMPORTS)
        .argument("space", SPACE)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    let ours = &create_account(&payer, &new_account, LAMPORTS, SPACE, &owner);
    expect(
        "createAccount",
        &ix,
        ours,
        "Create account",
        &format!("Create account {new_account} with 1.5 SOL"),
    );
    assert_same_instruction(
        &ix,
        &create_account(&payer, &new_account, LAMPORTS, SPACE, &owner),
    );
}

#[test]
fn assign_matches_the_system_interface() {
    let account = pk(2);
    let owner = pk(9);
    let ix = system()
        .method("assign")
        .unwrap()
        .account("account", account)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    expect(
        "assign",
        &ix,
        &assign(&account, &owner),
        "Assign account",
        &format!("Assign {account} to program {owner}"),
    );
}

#[test]
fn create_account_with_seed_matches_the_system_interface() {
    let payer = pk(1);
    let new_account = pk(2);
    let base = pk(3);
    let owner = pk(9);
    let ix = system()
        .method("createAccountWithSeed")
        .unwrap()
        .account("payer", payer)
        .account("newAccount", new_account)
        .account("baseAccount", base)
        .argument("base", key(base))
        .argument("seed", SEED)
        .argument("amount", LAMPORTS)
        .argument("space", SPACE)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    expect(
        "createAccountWithSeed",
        &ix,
        &create_account_with_seed(&payer, &new_account, &base, SEED, LAMPORTS, SPACE, &owner),
        "Create account with seed",
        &format!("Create account {new_account} with 1.5 SOL"),
    );
}

#[test]
fn advance_nonce_matches_the_system_interface() {
    let nonce = pk(4);
    let authority = pk(1);
    let ix = system()
        .method("advanceNonceAccount")
        .unwrap()
        .account("nonceAccount", nonce)
        .account("nonceAuthority", authority)
        .build()
        .unwrap();
    expect(
        "advanceNonceAccount",
        &ix,
        &advance_nonce_account(&nonce, &authority),
        "Advance nonce account",
        &format!("Advance nonce account {nonce}"),
    );
}

#[test]
fn withdraw_nonce_matches_the_system_interface() {
    let nonce = pk(4);
    let recipient = pk(2);
    let authority = pk(1);
    let ix = system()
        .method("withdrawNonceAccount")
        .unwrap()
        .account("nonceAccount", nonce)
        .account("recipientAccount", recipient)
        .account("nonceAuthority", authority)
        .argument("withdrawAmount", LAMPORTS)
        .build()
        .unwrap();
    let ours = &withdraw_nonce_account(&nonce, &authority, &recipient, LAMPORTS);
    expect(
        "withdrawNonceAccount",
        &ix,
        ours,
        "Withdraw from nonce account",
        &format!("Withdraw 1.5 SOL from {nonce} to {recipient}"),
    );
    assert_same_instruction(
        &ix,
        &withdraw_nonce_account(&nonce, &authority, &recipient, LAMPORTS),
    );
}

#[test]
fn initialize_nonce_matches_the_system_interface() {
    let payer = pk(1);
    let nonce = pk(4);
    let authority = pk(5);
    let ix = system()
        .method("initializeNonceAccount")
        .unwrap()
        .account("nonceAccount", nonce)
        .argument("nonceAuthority", key(authority))
        .build()
        .unwrap();
    let ours = &create_nonce_account(&payer, &nonce, &authority, LAMPORTS)[1];
    expect(
        "initializeNonceAccount",
        &ix,
        ours,
        "Initialize nonce account",
        &format!("Initialize nonce account {nonce} with authority {authority}"),
    );
}

#[test]
fn authorize_nonce_matches_the_system_interface() {
    let nonce = pk(4);
    let current = pk(1);
    let new_authority = pk(5);
    let ix = system()
        .method("authorizeNonceAccount")
        .unwrap()
        .account("nonceAccount", nonce)
        .account("nonceAuthority", current)
        .argument("newNonceAuthority", key(new_authority))
        .build()
        .unwrap();
    let ours = &authorize_nonce_account(&nonce, &current, &new_authority);
    expect(
        "authorizeNonceAccount",
        &ix,
        ours,
        "Authorize nonce account",
        &format!("Set the authority of nonce account {nonce} to {new_authority}"),
    );
}

#[test]
fn allocate_matches_the_system_interface() {
    let account = pk(2);
    let ix = system()
        .method("allocate")
        .unwrap()
        .account("newAccount", account)
        .argument("space", SPACE)
        .build()
        .unwrap();
    expect(
        "allocate",
        &ix,
        &allocate(&account, SPACE),
        "Allocate space",
        &format!("Allocate 165 bytes for {account}"),
    );
}

#[test]
fn allocate_with_seed_matches_the_system_interface() {
    let account = pk(2);
    let base = pk(3);
    let owner = pk(9);
    let ix = system()
        .method("allocateWithSeed")
        .unwrap()
        .account("newAccount", account)
        .account("baseAccount", base)
        .argument("base", key(base))
        .argument("seed", SEED)
        .argument("space", SPACE)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    expect(
        "allocateWithSeed",
        &ix,
        &allocate_with_seed(&account, &base, SEED, SPACE, &owner),
        "Allocate space with seed",
        &format!("Allocate 165 bytes for {account}"),
    );
}

#[test]
fn assign_with_seed_matches_the_system_interface() {
    let account = pk(2);
    let base = pk(3);
    let owner = pk(9);
    let ix = system()
        .method("assignWithSeed")
        .unwrap()
        .account("account", account)
        .account("baseAccount", base)
        .argument("base", key(base))
        .argument("seed", SEED)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    expect(
        "assignWithSeed",
        &ix,
        &assign_with_seed(&account, &base, SEED, &owner),
        "Assign account with seed",
        &format!("Assign {account} to program {owner}"),
    );
}

#[test]
fn transfer_with_seed_matches_the_system_interface() {
    let source = pk(2);
    let base = pk(3);
    let destination = pk(6);
    let owner = pk(9);
    let ix = system()
        .method("transferSolWithSeed")
        .unwrap()
        .account("source", source)
        .account("baseAccount", base)
        .account("destination", destination)
        .argument("amount", LAMPORTS)
        .argument("fromSeed", SEED)
        .argument("fromOwner", key(owner))
        .build()
        .unwrap();
    expect(
        "transferSolWithSeed",
        &ix,
        &transfer_with_seed(
            &source,
            &base,
            SEED.to_string(),
            &owner,
            &destination,
            LAMPORTS,
        ),
        "Transfer SOL with seed",
        &format!("Transfer 1.5 SOL from {source} to {destination}"),
    );
}

#[test]
fn upgrade_nonce_matches_the_system_interface() {
    let nonce = pk(4);
    let ix = system()
        .method("upgradeNonceAccount")
        .unwrap()
        .account("nonceAccount", nonce)
        .build()
        .unwrap();
    expect(
        "upgradeNonceAccount",
        &ix,
        &upgrade_nonce_account(nonce),
        "Upgrade nonce account",
        &format!("Upgrade nonce account {nonce}"),
    );
}

#[test]
fn create_account_allow_prefund_matches_the_system_interface() {
    let new_account = pk(2);
    let payer = pk(1);
    let owner = pk(9);
    let ix = system()
        .method("createAccountAllowPrefund")
        .unwrap()
        .account("newAccount", new_account)
        .account("payer", payer)
        .argument("lamports", LAMPORTS)
        .argument("space", SPACE)
        .argument("programAddress", key(owner))
        .build()
        .unwrap();
    expect(
        "createAccountAllowPrefund",
        &ix,
        &create_account_allow_prefund(&new_account, Some((&payer, LAMPORTS)), SPACE, &owner),
        "Create account",
        &format!("Create account {new_account} with 1.5 SOL"),
    );
}
