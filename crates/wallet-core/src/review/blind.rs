//! Instructions that say yes to something the transaction does not hold.
//!
//! An approval signs the address of a proposal. What the proposal would run is
//! in an account of its own, so nothing in the bytes being signed says what it
//! is, and a device with no network cannot go and look. Another device can send
//! the account along, and [`stored_in`] checks that it fits the address being
//! approved. That does not make it what the chain holds, so what comes that way
//! is only ever shown as what the other device says.
//!
//! Like [`super::risk`], these are told apart by program and instruction, from
//! the bytes the program will read. A proposal the same list of instructions
//! stores and opens is not blind, since the transaction then holds what is
//! approved. Only the programs named here are known: another program can keep
//! what it approves elsewhere too, and reads as whatever its IDL says.

use crate::ids::REALMS_GOVERNANCE_PROGRAM_ID;
use crate::review::explain::{ExplainedIx, ExplainedTx};
use crate::wallets::squads::{Squad, SquadsProgram, StoredAccount, VaultTransaction};
use solana_pubkey::Pubkey;
use std::collections::HashMap;

const SQUADS_APPROVAL: &str =
    "Approves a Squads proposal. What it would run is stored on chain, not in this transaction.";
const SQUADS_RUN: &str =
    "Runs a transaction a Squads multisig stored on chain. What it does is not in this transaction.";
const REALMS_VOTE: &str =
    "Votes on a Realms proposal. What it would run is stored on chain, not in this transaction.";

/// Anchor's tag for each instruction that matters here. The tests hold them to
/// what the IDLs the app ships build, where the app builds that instruction.
const V4_STORE: [u8; 8] = [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3];
const V4_STORE_CONFIG: [u8; 8] = [0x9b, 0xec, 0x57, 0xe4, 0x89, 0x4b, 0x51, 0x27];
const V4_OPEN: [u8; 8] = [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f];
const V4_APPROVE: [u8; 8] = [0x90, 0x25, 0xa4, 0x88, 0xbc, 0xd8, 0x2a, 0xf8];
const V4_RUN: [u8; 8] = [0xc2, 0x08, 0xa1, 0x57, 0x99, 0xa4, 0x19, 0xab];
const V4_RUN_CONFIG: [u8; 8] = [0x72, 0x92, 0xf4, 0xbd, 0xfc, 0x8c, 0x24, 0x28];
const V4_RUN_BATCH: [u8; 8] = [0xac, 0x2c, 0xb3, 0x98, 0x15, 0x7f, 0xea, 0xb4];
const SMART_STORE: [u8; 8] = [0xe3, 0xc1, 0x35, 0xef, 0x37, 0x7e, 0x70, 0x69];
const SMART_STORE_SETTINGS: [u8; 8] = [0x65, 0xa8, 0xfe, 0xcb, 0xde, 0x66, 0x5f, 0xc0];
const SMART_OPEN: [u8; 8] = [0x84, 0x74, 0x44, 0xae, 0xd8, 0xa0, 0xc6, 0x16];
const SMART_APPROVE: [u8; 8] = [0x88, 0x6c, 0x66, 0x55, 0x62, 0x72, 0x07, 0x93];
const SMART_RUN: [u8; 8] = [0xe7, 0xad, 0x31, 0x5b, 0xeb, 0x18, 0x44, 0x13];
const SMART_RUN_SETTINGS: [u8; 8] = [0x83, 0xd2, 0x1b, 0x58, 0x1b, 0xcc, 0x8f, 0xbd];
const SMART_RUN_BATCH: [u8; 8] = [0xed, 0x43, 0xc9, 0xad, 0x21, 0x82, 0x58, 0x86];

/// spl-governance's `CastVote`.
const REALMS_CAST_VOTE: u8 = 13;

/// What a Squads instruction is to this module, and where it names the accounts
/// that matter. Every one of them names the multisig first.
enum SquadsCall {
    /// Stores what its own bytes say, as the account at position 1.
    Stores,
    /// Opens voting on a stored transaction, the proposal at position 1. Its
    /// number follows the tag.
    Opens,
    Approves {
        proposal: usize,
    },
    /// `transaction` is `None` when what it runs is not a vault transaction,
    /// which this app does not read.
    Runs {
        proposal: usize,
        transaction: Option<usize>,
    },
}

fn squads_call(program: SquadsProgram, data: &[u8]) -> Option<SquadsCall> {
    let tag: [u8; 8] = data.get(..8)?.try_into().ok()?;
    let runs = |proposal, transaction| {
        Some(SquadsCall::Runs {
            proposal,
            transaction,
        })
    };
    match (program, tag) {
        (SquadsProgram::V4, V4_STORE | V4_STORE_CONFIG)
        | (SquadsProgram::SmartAccount, SMART_STORE | SMART_STORE_SETTINGS) => {
            Some(SquadsCall::Stores)
        }
        (SquadsProgram::V4, V4_OPEN) | (SquadsProgram::SmartAccount, SMART_OPEN) => {
            Some(SquadsCall::Opens)
        }
        (SquadsProgram::V4, V4_APPROVE) | (SquadsProgram::SmartAccount, SMART_APPROVE) => {
            Some(SquadsCall::Approves { proposal: 2 })
        }
        (SquadsProgram::V4, V4_RUN) | (SquadsProgram::SmartAccount, SMART_RUN) => runs(1, Some(2)),
        (SquadsProgram::V4, V4_RUN_CONFIG)
        | (SquadsProgram::SmartAccount, SMART_RUN_SETTINGS)
        | (SquadsProgram::V4, V4_RUN_BATCH)
        | (SquadsProgram::SmartAccount, SMART_RUN_BATCH) => runs(2, None),
        _ => None,
    }
}

/// The stored transaction an approval acts on, for reading it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StoredTarget {
    pub squad: Squad,
    pub proposal: Pubkey,
    /// The stored transaction, when the instruction names it.
    pub transaction: Option<Pubkey>,
}

/// One instruction that says yes to something the transaction does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlindApproval {
    /// What it does, in words.
    pub what: &'static str,
    /// `None` when this app cannot read what it acts on.
    pub target: Option<StoredTarget>,
}

/// The approvals in `explained`, each once, at any depth the app could open.
pub fn blind_in(explained: &ExplainedTx) -> Vec<BlindApproval> {
    let mut found = Vec::new();
    walk(&explained.instructions, &mut found);
    found
}

fn walk(list: &[ExplainedIx], found: &mut Vec<BlindApproval>) {
    // Stored and opened earlier in this list, which they succeed or fail with.
    // A list carried inside an instruction is a list of its own: it may only be
    // stored, to run some other time, so it says nothing of the accounts here.
    let mut stored = Vec::new();
    let mut opened = Vec::new();
    for instruction in list {
        if let Some(approval) = blind_one(instruction, &mut stored, &mut opened) {
            if !found.contains(&approval) {
                found.push(approval);
            }
        }
        if let Some(inner) = &instruction.inner {
            walk(&inner.instructions, found);
        }
    }
}

fn blind_one(
    instruction: &ExplainedIx,
    stored: &mut Vec<Pubkey>,
    opened: &mut Vec<Pubkey>,
) -> Option<BlindApproval> {
    if instruction.program_id == REALMS_GOVERNANCE_PROGRAM_ID {
        return (instruction.data.first() == Some(&REALMS_CAST_VOTE)).then_some(BlindApproval {
            what: REALMS_VOTE,
            target: None,
        });
    }
    let program = SquadsProgram::from_program_id(&instruction.program_id)?;
    let account = |at: usize| instruction.accounts.get(at).copied();
    let target = |proposal: Option<Pubkey>, transaction: Option<Pubkey>| {
        Some(StoredTarget {
            squad: Squad {
                program,
                address: account(0)?,
            },
            proposal: proposal?,
            transaction,
        })
    };
    let (what, proposal, target) = match squads_call(program, &instruction.data)? {
        SquadsCall::Stores => {
            stored.extend(account(1));
            return None;
        }
        SquadsCall::Opens => {
            let number = u64::from_le_bytes(instruction.data.get(8..16)?.try_into().ok()?);
            let squad = Squad {
                program,
                address: account(0)?,
            };
            let proposal = account(1)?;
            if squad.proposal(number) == proposal && stored.contains(&squad.transaction(number)) {
                opened.push(proposal);
            }
            return None;
        }
        SquadsCall::Approves { proposal } => {
            let proposal = account(proposal);
            (SQUADS_APPROVAL, proposal, target(proposal, None))
        }
        SquadsCall::Runs {
            proposal,
            transaction: Some(transaction),
        } => {
            let proposal = account(proposal);
            let transaction = account(transaction);
            let target = target(proposal, transaction).filter(|_| transaction.is_some());
            (SQUADS_RUN, proposal, target)
        }
        SquadsCall::Runs {
            proposal,
            transaction: None,
        } => (SQUADS_RUN, account(proposal), None),
    };
    if proposal.is_some_and(|proposal| opened.contains(&proposal)) {
        return None;
    }
    Some(BlindApproval { what, target })
}

/// The transaction `target` acts on, if `accounts` holds one that fits it.
/// Fitting says it could be that transaction: anyone can write bytes for an
/// address, so where they came from decides what they are worth.
pub fn stored_in(
    target: &StoredTarget,
    accounts: &HashMap<Pubkey, Vec<u8>>,
) -> Option<VaultTransaction> {
    accounts.iter().find_map(|(address, data)| {
        let StoredAccount::Vault(stored) = target.squad.parse_transaction(data)? else {
            return None;
        };
        let fits = target.squad.transaction(stored.index) == *address
            && target.squad.proposal(stored.index) == target.proposal
            && target.transaction.is_none_or(|named| named == *address);
        fits.then_some(stored)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{compile, pk, transfer};
    use crate::idl::catalog::{IdlStore, MemoryBodies};
    use crate::ids::SQUADS_V4_PROGRAM_ID;
    use crate::review::explain::explain_transaction;
    use crate::tx::decode::decode_v1_transaction;
    use crate::wallets::squads::tests::{unhex, written};
    use solana_instruction::Instruction;
    use wallet_vault::store::MemoryStore;

    fn squads() -> [Squad; 2] {
        [Squad::v4(&pk(50)), Squad::smart_account(7)]
    }

    fn explained(instructions: Vec<Instruction>) -> ExplainedTx {
        let wire = compile(pk(1), instructions).wire;
        let catalog = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
        explain_transaction(&catalog, &decode_v1_transaction(&wire).unwrap())
    }

    fn whats(instructions: Vec<Instruction>) -> Vec<&'static str> {
        blind_in(&explained(instructions))
            .into_iter()
            .map(|approval| approval.what)
            .collect()
    }

    fn raw(program: Pubkey, data: Vec<u8>, accounts: Vec<Pubkey>) -> ExplainedIx {
        ExplainedIx {
            program_id: program,
            accounts,
            data,
            idl: None,
            inner: None,
        }
    }

    fn one(instruction: ExplainedIx) -> Vec<&'static str> {
        blind_in(&ExplainedTx {
            programs: Vec::new(),
            instructions: vec![instruction],
        })
        .into_iter()
        .map(|approval| approval.what)
        .collect()
    }

    /// The tag and the account names the IDL at `text` gives an instruction.
    fn in_idl(text: &str, name: &str) -> (serde_json::Value, Vec<String>) {
        let idl: serde_json::Value = serde_json::from_str(text).unwrap();
        let instruction = idl["program"]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|instruction| instruction["name"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .clone();
        let tag = instruction["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|argument| argument["name"] == "discriminator")
            .unwrap()["defaultValue"]
            .clone();
        let accounts = instruction["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|account| account["name"].as_str().unwrap().to_owned())
            .collect();
        (tag, accounts)
    }

    #[test]
    fn every_tag_and_position_is_the_one_the_idls_the_app_ships_give() {
        let v4 = include_str!("../../idls/squads-multisig.json");
        let smart = include_str!("../../idls/squads-smart-account.json");
        for (program, idl, name, tag) in [
            (SquadsProgram::V4, v4, "vaultTransactionCreate", V4_STORE),
            (
                SquadsProgram::V4,
                v4,
                "configTransactionCreate",
                V4_STORE_CONFIG,
            ),
            (SquadsProgram::V4, v4, "proposalCreate", V4_OPEN),
            (SquadsProgram::V4, v4, "proposalApprove", V4_APPROVE),
            (SquadsProgram::V4, v4, "vaultTransactionExecute", V4_RUN),
            (
                SquadsProgram::V4,
                v4,
                "configTransactionExecute",
                V4_RUN_CONFIG,
            ),
            (
                SquadsProgram::V4,
                v4,
                "batchExecuteTransaction",
                V4_RUN_BATCH,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "createTransaction",
                SMART_STORE,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "createSettingsTransaction",
                SMART_STORE_SETTINGS,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "createProposal",
                SMART_OPEN,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "approveProposal",
                SMART_APPROVE,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "executeTransaction",
                SMART_RUN,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "executeSettingsTransaction",
                SMART_RUN_SETTINGS,
            ),
            (
                SquadsProgram::SmartAccount,
                smart,
                "executeBatchTransaction",
                SMART_RUN_BATCH,
            ),
        ] {
            let (want, accounts) = in_idl(idl, name);
            assert_eq!(want["data"], hex::encode(tag), "{name}");
            assert!(
                ["multisig", "consensusAccount", "settings"].contains(&accounts[0].as_str()),
                "{name}"
            );
            match squads_call(program, &tag).unwrap() {
                SquadsCall::Stores => assert_eq!(accounts[1], "transaction", "{name}"),
                SquadsCall::Opens => assert_eq!(accounts[1], "proposal", "{name}"),
                SquadsCall::Approves { proposal } => {
                    assert_eq!(accounts[proposal], "proposal", "{name}")
                }
                SquadsCall::Runs {
                    proposal,
                    transaction,
                } => {
                    assert_eq!(accounts[proposal], "proposal", "{name}");
                    if let Some(transaction) = transaction {
                        assert_eq!(accounts[transaction], "transaction", "{name}");
                    }
                }
            }
        }
        let realms = include_str!("../../../../resources/idls/realms-governance.json");
        let (want, _) = in_idl(realms, "castVote");
        assert_eq!(want["number"], f64::from(REALMS_CAST_VOTE));
    }

    #[test]
    fn approving_and_running_what_is_stored_elsewhere_is_told_on_either_program() {
        for squad in squads() {
            let vault = squad.vault(0);
            let found = blind_in(&explained(vec![squad.approve(3, &pk(1))]));
            assert_eq!(
                found,
                [BlindApproval {
                    what: SQUADS_APPROVAL,
                    target: Some(StoredTarget {
                        squad,
                        proposal: squad.proposal(3),
                        transaction: None,
                    }),
                }],
                "{:?}",
                squad.program
            );
            let run = squad.execute(3, 0, &[transfer(vault, pk(2), 5)], &pk(1));
            let found = blind_in(&explained(vec![run]));
            assert_eq!(
                found,
                [BlindApproval {
                    what: SQUADS_RUN,
                    target: Some(StoredTarget {
                        squad,
                        proposal: squad.proposal(3),
                        transaction: Some(squad.transaction(3)),
                    }),
                }],
                "{:?}",
                squad.program
            );
        }
    }

    #[test]
    fn what_only_config_and_batch_runs_hold_is_told_with_nothing_to_read() {
        for (program, tag) in [
            (SquadsProgram::V4, V4_RUN_CONFIG),
            (SquadsProgram::V4, V4_RUN_BATCH),
            (SquadsProgram::SmartAccount, SMART_RUN_SETTINGS),
            (SquadsProgram::SmartAccount, SMART_RUN_BATCH),
        ] {
            let accounts = (1..6).map(pk).collect();
            let found = blind_in(&ExplainedTx {
                programs: Vec::new(),
                instructions: vec![raw(program.program_id(), tag.to_vec(), accounts)],
            });
            assert_eq!(
                found,
                [BlindApproval {
                    what: SQUADS_RUN,
                    target: None
                }]
            );
        }
    }

    #[test]
    fn a_vote_is_told_by_its_tag_and_nothing_else_realms_does_is() {
        let realms = |tag: u8| one(raw(REALMS_GOVERNANCE_PROGRAM_ID, vec![tag, 0], Vec::new()));
        assert_eq!(realms(REALMS_CAST_VOTE), [REALMS_VOTE]);
        // Add a transaction, sign off, finalize, and run: anyone may run what
        // has passed, so a signature on that says yes to nothing.
        for other in [9, 12, 14, 16] {
            assert!(realms(other).is_empty());
        }
    }

    #[test]
    fn what_the_same_list_stores_and_opens_is_not_blind() {
        for squad in squads() {
            let vault = squad.vault(0);
            let payment = [transfer(vault, pk(2), 5)];
            let store = squad
                .create_transaction(4, 0, &payment, &pk(1), &pk(1))
                .unwrap();
            let open = squad.create_proposal(4, &pk(1), &pk(1));
            let vote = squad.approve(4, &pk(1));
            let run = squad.execute(4, 0, &payment, &pk(1));

            let all = vec![store.clone(), open.clone(), vote.clone(), run.clone()];
            assert!(whats(all).is_empty(), "{:?}", squad.program);

            // Voted on before it is stored: the list fails, or the proposal
            // was there already and may hold something else.
            assert_eq!(
                whats(vec![vote.clone(), store.clone(), open.clone()]),
                [SQUADS_APPROVAL]
            );
            // Stored but not opened here: the proposal may hold something else.
            assert_eq!(whats(vec![store.clone(), vote.clone()]), [SQUADS_APPROVAL]);
            // Opened but not stored here: the same.
            assert_eq!(whats(vec![open, vote.clone()]), [SQUADS_APPROVAL]);
            // Another number stored and opened here says nothing of number 4.
            let elsewhere = vec![
                squad
                    .create_transaction(5, 0, &payment, &pk(1), &pk(1))
                    .unwrap(),
                squad.create_proposal(5, &pk(1), &pk(1)),
                vote,
            ];
            assert_eq!(whats(elsewhere), [SQUADS_APPROVAL]);
        }
    }

    #[test]
    fn a_list_only_stored_for_later_does_not_vouch_for_the_one_around_it() {
        // A stored transaction whose own list stores and opens number 4, beside
        // an approval of number 4 that takes effect now, whatever number 4 is.
        let squad = Squad::v4(&pk(50));
        let other = Squad::v4(&pk(51));
        let payment = [transfer(squad.vault(0), pk(2), 5)];
        let vote = squad.approve(4, &pk(1));
        let carried = vec![
            squad
                .create_transaction(4, 0, &payment, &pk(1), &pk(1))
                .unwrap(),
            squad.create_proposal(4, &pk(1), &pk(1)),
        ];
        let carrying = other
            .create_transaction(1, 0, &carried, &pk(1), &pk(1))
            .unwrap();
        assert_eq!(whats(vec![carrying, vote]), [SQUADS_APPROVAL]);
    }

    #[test]
    fn what_an_instruction_carries_is_read_too_and_each_thing_is_said_once() {
        let squad = Squad::v4(&pk(50));
        let other = Squad::v4(&pk(51));
        let carrying = other
            .create_transaction(1, 0, &[squad.approve(4, &pk(9))], &pk(1), &pk(1))
            .unwrap();
        let again = squad.approve(4, &pk(9));
        assert_eq!(whats(vec![carrying, again]), [SQUADS_APPROVAL]);
    }

    #[test]
    fn an_approval_from_several_members_is_said_once() {
        let squad = Squad::v4(&pk(50));
        let votes = vec![squad.approve(4, &pk(1)), squad.approve(4, &pk(2))];
        assert_eq!(whats(votes), [SQUADS_APPROVAL]);
    }

    #[test]
    fn a_short_or_foreign_instruction_is_not_one_and_does_not_panic() {
        let v4 = SQUADS_V4_PROGRAM_ID;
        assert!(one(raw(v4, V4_APPROVE[..7].to_vec(), Vec::new())).is_empty());
        assert!(one(raw(v4, Vec::new(), Vec::new())).is_empty());
        assert!(one(raw(REALMS_GOVERNANCE_PROGRAM_ID, Vec::new(), Vec::new())).is_empty());
        // The same tag on another program, or on the other Squads program.
        assert!(one(raw(pk(9), V4_APPROVE.to_vec(), Vec::new())).is_empty());
        let smart = SquadsProgram::SmartAccount.program_id();
        assert!(one(raw(smart, V4_APPROVE.to_vec(), Vec::new())).is_empty());
        // An open with no number stores nothing and says nothing.
        assert!(one(raw(v4, V4_OPEN.to_vec(), vec![pk(1), pk(2)])).is_empty());
    }

    #[test]
    fn an_approval_short_of_accounts_is_still_told_with_nothing_to_read() {
        let found = blind_in(&ExplainedTx {
            programs: Vec::new(),
            instructions: vec![raw(SQUADS_V4_PROGRAM_ID, V4_APPROVE.to_vec(), vec![pk(1)])],
        });
        assert_eq!(
            found,
            [BlindApproval {
                what: SQUADS_APPROVAL,
                target: None
            }]
        );
    }

    #[test]
    fn accounts_the_programs_wrote_are_taken_only_for_what_they_fit() {
        for w in written() {
            let squad = w.squad;
            let data = unhex(w.transaction);
            let target = StoredTarget {
                squad,
                proposal: squad.proposal(1),
                transaction: None,
            };
            let held = |address: Pubkey| HashMap::from([(address, data.clone())]);

            let stored = stored_in(&target, &held(squad.transaction(1))).unwrap();
            assert_eq!(stored.index, 1, "{:?}", squad.program);
            assert_eq!(stored.instructions.len(), 1);

            // Under the wrong address, for another proposal, or naming another
            // stored transaction than the instruction does.
            assert_eq!(stored_in(&target, &held(pk(9))), None);
            let other_proposal = StoredTarget {
                proposal: squad.proposal(2),
                ..target
            };
            assert_eq!(
                stored_in(&other_proposal, &held(squad.transaction(1))),
                None
            );
            let other_named = StoredTarget {
                transaction: Some(squad.transaction(2)),
                ..target
            };
            assert_eq!(stored_in(&other_named, &held(squad.transaction(1))), None);
            let same_named = StoredTarget {
                transaction: Some(squad.transaction(1)),
                ..target
            };
            assert!(stored_in(&same_named, &held(squad.transaction(1))).is_some());

            // Of another multisig, or not a stored transaction at all.
            let elsewhere = Squad {
                program: squad.program,
                address: pk(1),
            };
            let target_elsewhere = StoredTarget {
                squad: elsewhere,
                proposal: elsewhere.proposal(1),
                transaction: None,
            };
            assert_eq!(
                stored_in(&target_elsewhere, &held(elsewhere.transaction(1))),
                None
            );
            let other_account = HashMap::from([(squad.transaction(1), unhex(w.proposal))]);
            assert_eq!(stored_in(&target, &other_account), None);
            assert_eq!(stored_in(&target, &HashMap::new()), None);
        }
    }
}
