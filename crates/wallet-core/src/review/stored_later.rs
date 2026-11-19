//! What a transaction stores for a Squads vault and leaves to run later.
//!
//! A transaction that creates a proposal (a v4 `vaultTransactionCreate`, a
//! smart account `createTransaction` with a transaction payload) carries the
//! instructions the vault will run once it is approved. Unless the same
//! transaction also executes it, its own simulation says nothing of what
//! they do, so Review and Sign simulates them too (see
//! `squads::VaultRun`). They are found at any depth this app wraps them in
//! for a member without a key: a programmatic signer's `Submit`, a Vector
//! `Passthrough`. What a stored transaction carries is not looked into: it
//! runs some other time. A change to the multisig itself holds actions, not
//! instructions, and a transaction stored through a buffer or as a batch is
//! not read here.

use super::blind::{squads_call, SquadsCall};
use crate::edit::{Carrier, CarrierKind};
use crate::tx::decode::DecodedTx;
use crate::wallets::squads::{SquadsProgram, StoredTransaction};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// How many levels of wrapping are looked through.
const MAX_DEPTH: usize = 4;

/// Each vault transaction `tx` stores and does not execute, first stored
/// first.
pub fn stored_for_later(tx: &DecodedTx) -> Vec<StoredTransaction> {
    let mut stored = Vec::new();
    let mut executed = Vec::new();
    walk(&tx.native_instructions(), 0, &mut stored, &mut executed);
    stored
        .into_iter()
        .filter(|(account, _)| !executed.contains(account))
        .map(|(_, transaction)| transaction)
        .collect()
}

/// `stored`: each stored transaction with the account it is stored in.
/// `executed`: the transaction accounts an execution names.
fn walk(
    list: &[Instruction],
    depth: usize,
    stored: &mut Vec<(Pubkey, StoredTransaction)>,
    executed: &mut Vec<Pubkey>,
) {
    for instruction in list {
        if let Some(program) = SquadsProgram::from_program_id(&instruction.program_id) {
            let account = |at: usize| instruction.accounts.get(at).map(|meta| meta.pubkey);
            match squads_call(program, &instruction.data) {
                Some(SquadsCall::Stores) => {
                    if let (Some(transaction), Some(account)) =
                        (StoredTransaction::parse(instruction), account(1))
                    {
                        stored.push((account, transaction));
                    }
                }
                Some(SquadsCall::Runs {
                    transaction: Some(at),
                    ..
                }) => executed.extend(account(at)),
                _ => {}
            }
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Some((carrier, inner)) = Carrier::open(instruction) {
            if matches!(
                carrier.kind,
                CarrierKind::Programmatic { .. } | CarrierKind::Passthrough { .. }
            ) {
                walk(&inner, depth + 1, stored, executed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::builder::compile_v1;
    use crate::tx::decode::decode_v1_transaction;
    use crate::wallets::squads::Squad;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn decoded(payer: Pubkey, instructions: Vec<Instruction>) -> DecodedTx {
        let compiled = compile_v1(
            payer,
            instructions,
            None,
            Some(solana_hash::Hash::new_from_array([3; 32])),
        )
        .unwrap();
        decode_v1_transaction(&compiled.wire).unwrap()
    }

    fn pay(from: Pubkey, to: Pubkey) -> Instruction {
        solana_system_interface::instruction::transfer(&from, &to, 5)
    }

    #[test]
    fn a_creation_is_found_and_an_execution_beside_it_drops_it() {
        for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
            let squad = Squad {
                program,
                address: pk(40),
            };
            let member = pk(1);
            let carried = vec![pay(squad.vault(0), pk(9))];
            let create = squad
                .create_transaction(7, 0, &carried, &member, &member)
                .unwrap();
            let open = squad.create_proposal(7, &member, &member);
            let approve = squad.approve(7, &member);

            let found = stored_for_later(&decoded(
                member,
                vec![create.clone(), open.clone(), approve.clone()],
            ));
            assert_eq!(found.len(), 1, "{program:?}");
            assert_eq!(found[0].instructions, carried);
            assert_eq!(found[0].vault, squad.vault(0));

            let execute = squad.execute(7, 0, &carried, &member);
            let found = stored_for_later(&decoded(member, vec![create, open, approve, execute]));
            assert!(found.is_empty(), "{program:?}");
        }
    }

    #[test]
    fn a_vote_or_a_change_stores_nothing_to_simulate() {
        let squad = Squad {
            program: SquadsProgram::V4,
            address: pk(40),
        };
        assert!(stored_for_later(&decoded(pk(1), vec![squad.approve(3, &pk(1))])).is_empty());
        let change = squad
            .create_change(
                3,
                &[crate::wallets::squads::ConfigAction::ChangeThreshold(1)],
                &pk(1),
                &pk(1),
            )
            .unwrap();
        assert!(stored_for_later(&decoded(pk(1), vec![change])).is_empty());
    }
}
