//! A transaction a vault stores, put in a transaction of its own so a node
//! can say what it would do if it ran now.
//!
//! Simulating the Squads transaction around it says nothing of that:
//! storing a transaction only creates accounts, and an execution can be
//! simulated only once the proposal is approved on chain. So the stored
//! instructions themselves are compiled into a new transaction, with a fee
//! payer of the caller's choosing in front. The vault, the transaction's
//! ephemeral signers and any other signer of the stored message stay in its
//! signer section, as the stored instructions mark them. A node that
//! simulates with signature verification off, as this app always asks it
//! to, takes every address there as having signed, so a program-derived
//! address signs as the Squads program would sign for it.
//!
//! What can still differ from the real execution, accepted here:
//!
//! - The instructions run one call shallower, at the top of the transaction
//!   rather than inside the Squads program. Only a program that checks how
//!   deep it was called, or reads the instructions sysvar, can tell.
//! - The fee payer is another account, which also pays a fee for each of
//!   the vault's and other signers' empty signatures. [`VaultRun::read`]
//!   takes the fee out of what it says the fee payer loses.
//! - The compute limit is the most a transaction may ask for, and the heap
//!   the most it may have, as this app's executions ask for: a simulation
//!   should not fail for want of either where the real execution would not.
//!   The Squads program's own work is not counted.
//! - A compute budget instruction stored among the others is left out: run
//!   inside the program it does nothing.
//!
//! It is a v1 transaction when it fits one (4,096 bytes, 64 accounts, 12
//! signatures), else a v0 transaction that reads the stored transaction's
//! own lookup tables (1,232 bytes). One that fits neither is refused.

use super::*;
use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
use crate::simulate::Simulation;
use crate::tx::builder::{compile_message, DEFAULT_LOADED_ACCOUNTS_DATA_SIZE, MAX_CU_LIMIT};
use crate::tx::decode::{
    decode_transaction, decode_v1_transaction, DecodedTx, LookupTables, PACKET_DATA_SIZE,
};
use solana_message::v1::{self, TransactionConfig};

/// What a vault would run, as a transaction to simulate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultRun {
    /// The transaction, unsigned. A node asked to simulate it gives it a
    /// blockhash of its own.
    pub tx: DecodedTx,
    pub fee_payer: Pubkey,
    /// For each instruction of `tx`, its place among the stored
    /// instructions; `None` for one added here.
    places: Vec<Option<usize>>,
}

/// Why a stored transaction cannot be simulated.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CannotRun {
    #[error("Too large to simulate.")]
    TooLarge,
}

impl VaultTransaction {
    /// What this transaction would do run now as the vault, with `fee_payer`
    /// paying the fee.
    pub fn run_now(&self, fee_payer: Pubkey) -> Result<VaultRun, CannotRun> {
        VaultRun::new(&self.instructions, &self.tables, fee_payer)
    }
}

impl StoredTransaction {
    /// What the transaction this instruction stores would do run now as the
    /// vault, with `fee_payer` paying the fee.
    pub fn run_now(&self, fee_payer: Pubkey) -> Result<VaultRun, CannotRun> {
        VaultRun::new(&self.instructions, &[], fee_payer)
    }
}

impl VaultRun {
    fn new(
        stored: &[Instruction],
        tables: &[(Pubkey, Vec<Pubkey>)],
        fee_payer: Pubkey,
    ) -> Result<Self, CannotRun> {
        let (kept, places): (Vec<Instruction>, Vec<Option<usize>>) = stored
            .iter()
            .enumerate()
            .filter(|(_, instruction)| instruction.program_id != COMPUTE_BUDGET_PROGRAM_ID)
            .map(|(place, instruction)| (instruction.clone(), Some(place)))
            .unzip();
        let mut tx = match as_v1(&kept, fee_payer) {
            Some(tx) => Self {
                tx,
                fee_payer,
                places,
            },
            None => {
                let budget = [
                    compute_budget(2, MAX_CU_LIMIT),
                    compute_budget(1, v1::MAX_HEAP_SIZE),
                ];
                let instructions: Vec<Instruction> = budget.into_iter().chain(kept).collect();
                Self {
                    tx: as_v0(&instructions, tables, fee_payer).ok_or(CannotRun::TooLarge)?,
                    fee_payer,
                    places: [None, None].into_iter().chain(places).collect(),
                }
            }
        };
        // Its first instruction may advance a nonce, which would have the
        // node keep the blockhash it was given, and none was.
        tx.tx.uses_durable_nonce = false;
        Ok(tx)
    }

    /// A node's simulation of [`Self::tx`], told in terms of the stored
    /// transaction: the fee payer's loss without the fee, which the real
    /// execution does not charge it, and each instruction by its place among
    /// the stored ones.
    pub fn read(&self, mut simulation: Simulation) -> Simulation {
        let fee = self.tx.fee_lamports();
        for change in &mut simulation.balance_changes {
            if change.pubkey == self.fee_payer {
                change.after = change.after.saturating_add(fee);
            }
        }
        simulation
            .balance_changes
            .retain(|change| change.before != change.after);
        let place = |at: usize| self.places.get(at).copied().flatten();
        if let Some(error) = &mut simulation.error {
            error.instruction = error.instruction.and_then(place);
        }
        simulation.inner = simulation
            .inner
            .into_iter()
            .filter_map(|mut group| {
                group.instruction = place(group.instruction)?;
                Some(group)
            })
            .collect();
        simulation
    }
}

/// A Compute Budget instruction: `tag` 2 sets the compute limit, 1 asks for
/// a heap of `value` bytes.
fn compute_budget(tag: u8, value: u32) -> Instruction {
    let mut data = vec![tag];
    data.extend_from_slice(&value.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: Vec::new(),
        data,
    }
}

/// `instructions` as a v1 transaction with the most compute and heap one
/// may ask for, when they fit one.
fn as_v1(instructions: &[Instruction], fee_payer: Pubkey) -> Option<DecodedTx> {
    let config = TransactionConfig::empty()
        .with_compute_unit_limit(MAX_CU_LIMIT)
        .with_loaded_accounts_data_size_limit(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE)
        .with_heap_size(v1::MAX_HEAP_SIZE);
    let compiled = compile_message(
        &fee_payer,
        instructions,
        solana_hash::Hash::default(),
        config,
    )
    .ok()?;
    let tx = decode_v1_transaction(&compiled.wire).ok()?;
    let fits = tx.signers.len() <= usize::from(v1::MAX_SIGNATURES)
        && tx.accounts.len() <= usize::from(v1::MAX_ADDRESSES)
        && tx.instructions.len() <= usize::from(v1::MAX_INSTRUCTIONS);
    fits.then_some(tx)
}

/// `instructions` as a v0 transaction that reads `tables`, when it fits one.
fn as_v0(
    instructions: &[Instruction],
    tables: &[(Pubkey, Vec<Pubkey>)],
    fee_payer: Pubkey,
) -> Option<DecodedTx> {
    let accounts: Vec<solana_message::AddressLookupTableAccount> = tables
        .iter()
        .map(
            |(key, addresses)| solana_message::AddressLookupTableAccount {
                key: *key,
                addresses: addresses.clone(),
            },
        )
        .collect();
    let message = solana_message::v0::Message::try_compile(
        &fee_payer,
        instructions,
        &accounts,
        solana_hash::Hash::default(),
    )
    .ok()?;
    let signatures = usize::from(message.header.num_required_signatures);
    let wire = wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: vec![solana_signature::Signature::default(); signatures],
        message: solana_message::VersionedMessage::V0(message),
    })
    .ok()?;
    if wire.len() > PACKET_DATA_SIZE {
        return None;
    }
    let mut loaded = LookupTables::new();
    for (key, addresses) in tables {
        loaded.insert(*key, addresses.clone());
    }
    decode_transaction(&wire, &loaded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulate::{BalanceChange, InnerCalls, SimulationError};
    use crate::tx::decode::TxFormat;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    fn squad() -> Squad {
        Squad {
            program: SquadsProgram::V4,
            address: pk(40),
        }
    }

    fn pay(from: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
        solana_system_interface::instruction::transfer(&from, &to, lamports)
    }

    fn signer_of(run: &VaultRun, key: &Pubkey) -> Option<(bool, bool)> {
        run.tx
            .accounts
            .iter()
            .find(|account| account.pubkey == *key)
            .map(|account| (account.signer, account.writable))
    }

    #[test]
    fn the_vault_signs_and_another_wallet_pays_the_fee() {
        let squad = squad();
        let vault = squad.vault(0);
        let stored = VaultTransaction::for_test(&squad, 3, 0, &[pay(vault, pk(9), 5_000)]);
        let run = stored.run_now(pk(1)).unwrap();
        assert_eq!(run.tx.format, TxFormat::V1);
        assert_eq!(run.tx.fee_payer(), Some(pk(1)));
        assert_eq!(run.fee_payer, pk(1));
        // The vault, written to and signing; the recipient written to only.
        assert_eq!(signer_of(&run, &vault), Some((true, true)));
        assert_eq!(signer_of(&run, &pk(9)), Some((false, true)));
        assert_eq!(run.tx.signers.len(), 2);
        // The most compute and heap, and nothing to fit a nonce.
        assert_eq!(run.tx.budget.compute_unit_limit, Some(MAX_CU_LIMIT));
        assert_eq!(run.tx.budget.heap_size, Some(v1::MAX_HEAP_SIZE));
        assert!(!run.tx.uses_durable_nonce);
        assert_eq!(run.tx.native_instructions(), stored.instructions);
    }

    #[test]
    fn ephemeral_and_other_signers_sign_too() {
        let squad = squad();
        let vault = squad.vault(0);
        let ephemeral = squad.ephemeral_signer(3, 0);
        let cosigner = pk(7);
        // The vault pays for a new account an ephemeral signer is, and a
        // key the stored message names signs a transfer of its own.
        let create = solana_system_interface::instruction::create_account(
            &vault,
            &ephemeral,
            1_000_000,
            0,
            &pk(30),
        );
        let stored = VaultTransaction::for_test(&squad, 3, 0, &[create, pay(cosigner, vault, 1)]);
        let run = stored.run_now(pk(1)).unwrap();
        for key in [vault, ephemeral, cosigner] {
            assert_eq!(signer_of(&run, &key), Some((true, true)), "{key}");
        }
        assert_eq!(run.tx.signers.len(), 4);
    }

    #[test]
    fn a_vault_that_also_pays_the_fee_signs_once() {
        let squad = squad();
        let vault = squad.vault(0);
        let stored = VaultTransaction::for_test(&squad, 3, 0, &[pay(vault, pk(9), 5)]);
        let run = stored.run_now(vault).unwrap();
        assert_eq!(run.tx.signers.len(), 1);
        assert_eq!(run.tx.fee_payer(), Some(vault));
    }

    #[test]
    fn a_stored_compute_budget_is_left_out_and_places_kept() {
        let squad = squad();
        let vault = squad.vault(0);
        let stored = VaultTransaction::for_test(
            &squad,
            3,
            0,
            &[compute_budget(2, 50_000), pay(vault, pk(9), 5)],
        );
        let run = stored.run_now(pk(1)).unwrap();
        assert_eq!(run.tx.instructions.len(), 1);
        let failed = Simulation {
            error: Some(SimulationError {
                instruction: Some(0),
                message: "x".into(),
                custom: None,
            }),
            inner: vec![InnerCalls {
                instruction: 0,
                calls: Vec::new(),
            }],
            ..Simulation::default()
        };
        let read = run.read(failed);
        // The transfer is the second stored instruction.
        assert_eq!(read.error.unwrap().instruction, Some(1));
        assert_eq!(read.inner[0].instruction, 1);
    }

    #[test]
    fn the_fee_is_not_counted_as_the_fee_payer_s_loss() {
        let squad = squad();
        let vault = squad.vault(0);
        let stored = VaultTransaction::for_test(&squad, 3, 0, &[pay(vault, pk(1), 50_000)]);
        let run = stored.run_now(pk(2)).unwrap();
        // Two signatures: the fee payer's and the vault's.
        assert_eq!(run.tx.fee_lamports(), 10_000);
        let change = |pubkey, before, after| BalanceChange {
            pubkey,
            before,
            after,
        };
        let simulation = Simulation {
            balance_changes: vec![
                change(pk(2), 1_000_000, 990_000),
                change(vault, 100_000, 50_000),
                change(pk(1), 0, 50_000),
            ],
            ..Simulation::default()
        };
        assert_eq!(
            run.read(simulation).balance_changes,
            vec![change(vault, 100_000, 50_000), change(pk(1), 0, 50_000)]
        );

        // A fee payer the stored instructions pay keeps what they give it.
        let paid = stored.run_now(pk(1)).unwrap();
        let simulation = Simulation {
            balance_changes: vec![
                change(pk(1), 1_000_000, 1_040_000),
                change(vault, 100_000, 50_000),
            ],
            ..Simulation::default()
        };
        assert_eq!(
            paid.read(simulation).balance_changes[0],
            change(pk(1), 1_000_000, 1_050_000)
        );
    }

    #[test]
    fn what_reads_lookup_tables_and_does_not_fit_v1_reads_them_as_v0() {
        let squad = squad();
        let vault = squad.vault(0);
        // 70 recipients: more accounts than a v1 transaction may list.
        let recipients: Vec<Pubkey> = (0..70u8)
            .map(|n| Pubkey::new_from_array([n.wrapping_add(100); 32]))
            .collect();
        let table = pk(60);
        // One instruction of some program that writes to all of them.
        let spread = Instruction {
            program_id: pk(30),
            accounts: std::iter::once(AccountMeta::new(vault, true))
                .chain(recipients.iter().map(|to| AccountMeta::new(*to, false)))
                .collect(),
            data: vec![1],
        };
        let mut stored = VaultTransaction::for_test(&squad, 3, 0, &[spread]);
        stored.lookup_tables = vec![table];
        stored.tables = vec![(table, recipients.clone())];
        let run = stored.run_now(pk(1)).unwrap();
        assert_eq!(run.tx.format, TxFormat::V0);
        assert!(run.tx.wire.len() <= PACKET_DATA_SIZE);
        assert!(run
            .tx
            .accounts
            .iter()
            .filter(|account| recipients.contains(&account.pubkey))
            .all(|account| account.lookup == Some(table) && account.writable));
        assert_eq!(signer_of(&run, &vault), Some((true, true)));
        assert_eq!(run.tx.budget.compute_unit_limit, Some(MAX_CU_LIMIT));
        assert_eq!(run.tx.budget.heap_size, Some(v1::MAX_HEAP_SIZE));
        // The budget it adds is not one of the stored instructions.
        let failed = Simulation {
            error: Some(SimulationError {
                instruction: Some(2),
                message: "x".into(),
                custom: None,
            }),
            ..Simulation::default()
        };
        assert_eq!(run.read(failed).error.unwrap().instruction, Some(0));
        let in_budget = Simulation {
            error: Some(SimulationError {
                instruction: Some(1),
                message: "x".into(),
                custom: None,
            }),
            ..Simulation::default()
        };
        assert_eq!(run.read(in_budget).error.unwrap().instruction, None);
    }

    #[test]
    fn what_fits_neither_is_refused() {
        let squad = squad();
        let vault = squad.vault(0);
        // 70 recipients and no table to read them from: too many accounts
        // for v1, too many bytes for v0.
        let payments: Vec<Instruction> = (0..70u8)
            .map(|n| pay(vault, Pubkey::new_from_array([n.wrapping_add(100); 32]), 1))
            .collect();
        let stored = VaultTransaction::for_test(&squad, 3, 0, &payments);
        assert_eq!(stored.run_now(pk(1)), Err(CannotRun::TooLarge));
        assert_eq!(CannotRun::TooLarge.to_string(), "Too large to simulate.");
    }

    #[test]
    fn what_a_creation_carries_runs_the_same_way() {
        let squad = squad();
        let vault = squad.vault(0);
        let create = squad
            .create_transaction(3, 0, &[pay(vault, pk(9), 5)], &pk(2), &pk(2))
            .unwrap();
        let stored = StoredTransaction::parse(&create).unwrap();
        let run = stored.run_now(pk(2)).unwrap();
        assert_eq!(signer_of(&run, &vault), Some((true, true)));
        assert_eq!(run.tx.native_instructions(), stored.instructions);
    }
}
