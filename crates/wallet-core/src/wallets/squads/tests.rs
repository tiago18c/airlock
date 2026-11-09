use super::*;
use crate::fixtures::{pk, transfer};

#[test]
fn the_vault_is_a_different_address_from_the_multisig() {
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        assert_ne!(squad.vault(0), squad.address);
        assert_ne!(squad.vault(0), squad.vault(1));
        assert_ne!(squad.transaction(1), squad.proposal(1));
        assert!(!squad.vault(0).is_on_curve());
    }
}

/// Program config addresses as they exist on devnet and mainnet.
#[test]
fn program_configs_are_at_their_known_addresses() {
    assert_eq!(
        SquadsProgram::V4.program_config().to_string(),
        "BSTq9w3kZwNwpBXJEvTZz2G9ZTNyKBvoSeXMvwb4cNZr"
    );
    assert_eq!(
        SquadsProgram::SmartAccount.program_config().to_string(),
        "GmY9kVi3FhrCUn2MJkzzpE6C5618YoHuGsgqHU78cKus"
    );
}

#[test]
fn a_stored_message_lists_the_vault_first_and_marks_what_it_writes() {
    let squad = Squad::v4(&pk(1));
    let vault = squad.vault(0);
    let message = vault_message(&vault, &[transfer(vault, pk(2), 5)]);
    assert_eq!(message.account_keys, vec![vault, pk(2), SYSTEM_PROGRAM_ID]);
    assert_eq!(
        (
            message.num_signers,
            message.num_writable_signers,
            message.num_writable_non_signers
        ),
        (1, 1, 1)
    );
    assert!(message.is_writable(0));
    assert!(message.is_writable(1));
    assert!(!message.is_writable(2));

    let execute = squad.execute(1, 0, &[transfer(vault, pk(2), 5)], &pk(3));
    // The vault signs inside the program, never in the transaction.
    assert!(execute
        .accounts
        .iter()
        .all(|meta| !meta.is_signer || meta.pubkey == pk(3)));
    assert_eq!(execute.accounts.len(), 4 + 3);
}

#[test]
fn a_stored_transaction_reads_back() {
    let transfer = |from: Pubkey| transfer(from, pk(9), 1_000);
    for squad in [Squad::v4(&pk(1)), Squad::smart_account(7)] {
        let vault = squad.vault(0);
        let stored = squad
            .create_transaction(1, 0, &[transfer(vault)], &pk(2), &pk(3))
            .unwrap();
        let read = StoredTransaction::parse(&stored).unwrap();
        assert_eq!(
            (read.squad, read.vault, read.vault_index),
            (squad, vault, 0)
        );
        assert_eq!(read.instructions.len(), 1);
        assert_eq!(read.instructions[0].data, transfer(vault).data);
        let from = &read.instructions[0].accounts[0];
        assert!(from.pubkey == vault && from.is_signer && from.is_writable);
        assert!(!read.instructions[0].accounts[1].is_signer);

        // Other instructions of the same program hold no transaction.
        assert_eq!(StoredTransaction::parse(&squad.approve(1, &pk(2))), None);
        let mut cut = stored.clone();
        cut.data.truncate(cut.data.len() - 40);
        assert_eq!(StoredTransaction::parse(&cut), None);
    }
}

#[test]
fn a_truncated_account_is_not_read() {
    let squad = Squad::v4(&pk(1));
    assert_eq!(squad.parse_state(&[]), None);
    assert_eq!(squad.parse_state(&[0u8; 80]), None);
}

/// Accounts the two programs wrote on a local chain: a transfer of
/// 0x11223344 lamports from vault 0, stored as transaction 1 and approved
/// by its creator, in a multisig of two that needs both.
pub(crate) struct Written {
    pub(crate) squad: Squad,
    creator: &'static str,
    other: &'static str,
    to: &'static str,
    settings: &'static str,
    pub(crate) transaction: &'static str,
    pub(crate) proposal: &'static str,
}

pub(crate) fn unhex(text: &str) -> Vec<u8> {
    hex::decode(text).unwrap()
}

fn address(text: &str) -> Pubkey {
    Pubkey::new_from_array(unhex(text).try_into().unwrap())
}

pub(crate) fn written() -> [Written; 2] {
    [
        Written {
            squad: Squad {
                program: SquadsProgram::V4,
                address: address(
                    "d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b7",
                ),
            },
            creator: "35a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d3",
            other: "6f2e948d7147340ef1ab4ce3ac07755bf96228beaa657ac4be0e56c0b270357e",
            to: "e2649cd17279322c1945b9c13b9d8f063e52f3723d3e70d81b9f339357aa3670",
            settings: "e07479ba44a14feca9aede94bc029ca330129bff392aadfa1cbbd60bea33b056b6082897ad166afa00000000000000000000000000000000000000000000000000000000000000000200000000000100000000000000000000000000000000ff0200000035a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d3076f2e948d7147340ef1ab4ce3ac07755bf96228beaa657ac4be0e56c0b270357e070000000000000000000000000000000000000000000000000000000000000000",
            transaction: "a8faa264510ea2cfd9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b735a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d30100000000000000ff00ff0000000001010103000000cd14d8623ad6e0db012f89806e133cc3326376b5c15ff369d4e7df8821d6eb07e2649cd17279322c1945b9c13b9d8f063e52f3723d3e70d81b9f339357aa3670000000000000000000000000000000000000000000000000000000000000000001000000020200000000010c00000002000000443322110000000000000000",
            proposal: "1a5ebdbb74883521d9a5e44daa78c870fe9cc9c9cd9b18266e9f23d17ba3babdb928f35f539137b70100000000000000010000000000000000ff0100000035a8380093a2d2386c1b25d1024507dbc5ae3ac66a3e65a3f6f192dee20896d30000000000000000",
        },
        Written {
            squad: Squad {
                program: SquadsProgram::SmartAccount,
                address: address(
                    "35641fda3e34d347bb748d0087fb15f32b6444c31c68177a4be5cdd07b23c9e8",
                ),
            },
            creator: "cd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6d",
            other: "d17285104604cf41453534dc4d28a22c7ffa8e578fc48a62b13b40f49bb5ad2a",
            to: "94d7dcc100934e331db3969500dfba27422fd329a2d68057226ca525a6016517",
            settings: "dfb3a3beb1e043ad2a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020000000000010000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000ff02000000cd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6d07d17285104604cf41453534dc4d28a22c7ffa8e578fc48a62b13b40f49bb5ad2a070001000000000000000000",
            transaction: "0b18ae81cb75f21735641fda3e34d347bb748d0087fb15f32b6444c31c68177a4be5cdd07b23c9e8cd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6dcd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6d010000000000000000000000000001010103000000e787fb9ee299e40eb35f1bf7dbd5c864687b4fd8a6c9e7fefe6d5061bb1b2dfe94d7dcc100934e331db3969500dfba27422fd329a2d68057226ca525a6016517000000000000000000000000000000000000000000000000000000000000000001000000020200000000010c000000020000004433221100000000000000000000",
            proposal: "1a5ebdbb7488352135641fda3e34d347bb748d0087fb15f32b6444c31c68177a4be5cdd07b23c9e80100000000000000cd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6d010000000000000000ff01000000cd2711eaa245a4348d63359da144d66c37f9129534e9d1f91f894eb417cdde6d0000000000000000",
        },
    ]
}

#[test]
fn accounts_the_programs_wrote_read_back() {
    for w in written() {
        let squad = w.squad;
        let (creator, other, to) = (address(w.creator), address(w.other), address(w.to));
        let vault = squad.vault(0);
        let payment = transfer(vault, to, 0x1122_3344);

        assert_eq!(
            squad.parse_state(&unhex(w.settings)),
            Some(SquadState {
                threshold: 2,
                time_lock: 0,
                transaction_index: 1,
                stale_transaction_index: 0,
                members: [creator, other]
                    .map(|key| SquadsMember {
                        key,
                        permissions: PERMISSION_ALL
                    })
                    .to_vec(),
            }),
            "{:?}",
            squad.program
        );

        let proposal = squad.parse_proposal(&unhex(w.proposal)).unwrap();
        assert_eq!(
            proposal,
            ProposalState {
                index: 1,
                status: ProposalStatus::Active,
                since: 0,
                approved: vec![creator],
                rejected: Vec::new(),
                cancelled: Vec::new(),
            },
            "{:?}",
            squad.program
        );

        let data = unhex(w.transaction);
        let Some(StoredAccount::Vault(stored)) = squad.parse_transaction(&data) else {
            panic!("{:?}: not read as a vault transaction", squad.program);
        };
        assert_eq!((stored.creator, stored.index), (creator, 1));
        assert_eq!((stored.vault_index, stored.vault), (0, vault));
        assert_eq!(stored.instructions, vec![payment.clone()]);
        // Executing from the account is executing what was stored.
        assert_eq!(
            squad.execute_stored(&stored, &pk(3)),
            squad.execute(1, 0, &[payment], &pk(3))
        );

        // Each is told apart from the other, and from another multisig.
        assert_eq!(squad.parse_proposal(&data), None);
        assert_eq!(squad.parse_state(&data), None);
        assert_eq!(
            squad.parse_transaction(&unhex(w.proposal)),
            Some(StoredAccount::Other)
        );
        let elsewhere = Squad {
            program: squad.program,
            address: pk(1),
        };
        assert_eq!(elsewhere.parse_transaction(&data), None);
        assert_eq!(elsewhere.parse_proposal(&unhex(w.proposal)), None);
        assert_eq!(squad.parse_transaction(&data[..data.len() / 2]), None);
        assert_eq!(squad.parse_transaction(&[]), None);
    }
}

#[test]
fn a_stored_transaction_that_names_lookup_tables_is_not_resolved() {
    let w = &written()[0];
    let mut data = unhex(w.transaction);
    // One table: its address, and no index into it either way.
    let at = data.len() - 4;
    data[at] = 1;
    data.extend_from_slice(&[9u8; 32]);
    data.extend_from_slice(&[0u8; 8]);
    assert_eq!(
        w.squad.parse_transaction(&data),
        Some(StoredAccount::LookupTables)
    );
}

#[test]
fn an_execution_at_once_lists_the_members_then_the_stored_accounts() {
    let squad = Squad::smart_account(7);
    let vault = squad.vault(1);
    let payment = transfer(vault, pk(9), 1_000);
    let members = [pk(2), pk(3)];
    let execute = squad
        .execute_sync(1, std::slice::from_ref(&payment), &members)
        .unwrap();
    assert_eq!(execute.program_id, SQUADS_SMART_ACCOUNT_PROGRAM_ID);
    let listed: Vec<(Pubkey, bool, bool)> = execute
        .accounts
        .iter()
        .map(|meta| (meta.pubkey, meta.is_signer, meta.is_writable))
        .collect();
    assert_eq!(
        listed,
        vec![
            (squad.address, false, false),
            (SQUADS_SMART_ACCOUNT_PROGRAM_ID, false, false),
            (pk(2), true, false),
            (pk(3), true, false),
            // The vault signs inside the program, never in the transaction.
            (vault, false, true),
            (pk(9), false, true),
            (SYSTEM_PROGRAM_ID, false, false),
        ]
    );
    // Tag, vault 1, two members, then the instructions as a sized list,
    // numbered from the start of the members: the system program is 4, the
    // vault 2 and the recipient 3.
    let mut expected = hex::decode("2b66f859e7616886").unwrap();
    expected.extend_from_slice(&[1, 2]);
    let mut list = vec![1, 4, 2, 2, 3];
    list.extend_from_slice(&(payment.data.len() as u16).to_le_bytes());
    list.extend_from_slice(&payment.data);
    expected.extend_from_slice(&(list.len() as u32).to_le_bytes());
    expected.extend_from_slice(&list);
    assert_eq!(execute.data, expected);

    let read = SyncExecution::parse(&execute).unwrap();
    assert_eq!(
        (
            read.squad,
            read.vault,
            read.vault_index,
            read.signers.clone()
        ),
        (squad, vault, 1, members.to_vec())
    );
    assert_eq!(read.instructions, vec![payment.clone()]);

    // Written again around other instructions, for the same members.
    let other = transfer(vault, pk(4), 5);
    let rewritten = SyncExecution::rewrite(&execute, std::slice::from_ref(&other)).unwrap();
    assert_eq!(
        rewritten,
        squad.execute_sync(1, &[other], &members).unwrap()
    );

    // Other instructions, and v4, are not it.
    assert_eq!(SyncExecution::parse(&squad.approve(1, &pk(2))), None);
    assert_eq!(
        Squad::v4(&pk(1)).execute_sync(0, &[payment], &members),
        None
    );
}

#[test]
fn a_member_named_inside_is_not_a_signer_there_and_the_multisig_is_not_written() {
    let squad = Squad::smart_account(7);
    let vault = squad.vault(0);
    // Pays the member, and names the multisig.
    let mut payment = transfer(vault, pk(2), 5);
    payment
        .accounts
        .push(AccountMeta::new(squad.address, false));
    let mut execute = squad
        .execute_sync(0, std::slice::from_ref(&payment), &[pk(2)])
        .unwrap();
    // As a decoded message flags it: the member signs everywhere it appears.
    for meta in &mut execute.accounts {
        if meta.pubkey == pk(2) {
            meta.is_signer = true;
        }
    }
    let read = SyncExecution::parse(&execute).unwrap();
    let accounts = &read.instructions[0].accounts;
    assert!(accounts[0].is_signer, "the vault signs");
    assert!(!accounts[1].is_signer, "the member does not, inside");
    assert!(!accounts[2].is_writable, "the multisig is passed read-only");
}

#[test]
fn an_execution_at_once_numbers_at_most_256_accounts() {
    let squad = Squad::smart_account(7);
    let vault = squad.vault(0);
    let many: Vec<Instruction> = (0..250u8)
        .map(|n| transfer(vault, Pubkey::new_from_array([n; 32]), 1))
        .collect();
    assert!(squad.execute_sync(0, &many, &[pk(2)]).is_some());
    let members: Vec<Pubkey> = (0..10).map(|n| pk(100 + n)).collect();
    assert_eq!(squad.execute_sync(0, &many, &members), None);
}

/// The smart account program config as mainnet held it on 2026-11-09: what
/// creating one there reads for its treasury and its number.
#[test]
fn the_mainnet_smart_account_config_reads() {
    let data = unhex("c4d25ae790958c3fc7590b000000000000000000000000000ab482e5c90a9d95bf7edc0d49a413de6939ed6f69ad52069a876563c8ac47d100000000000000000ab482e5c90a9d95bf7edc0d49a413de6939ed6f69ad52069a876563c8ac47d100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
    assert_eq!(SquadsProgram::smart_account_count(&data), Some(743_879));
    assert_eq!(
        SquadsProgram::SmartAccount.treasury(&data),
        Some(address(
            "0ab482e5c90a9d95bf7edc0d49a413de6939ed6f69ad52069a876563c8ac47d1"
        ))
    );
}
