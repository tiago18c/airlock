//! A transaction read back into what Build edits.
//!
//! Each instruction becomes the block Build would have made it from: the
//! form of its program's IDL, filled in, when that form builds the same
//! bytes and accounts again; its program, accounts and bytes otherwise. An
//! instruction that carries others (a Squads stored transaction, a Vector
//! `Passthrough`, a programmatic signer's `Submit`) is opened, and what it
//! carries is read the same way, level by level. The nonce advance and the
//! compute budget go back to Build's own fields.
//!
//! Nothing here keeps a signature. Build compiles a new transaction.

use crate::idl::catalog::IdlCatalog;
use crate::idl::draft::IdlDraft;
use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
use crate::tx::builder::NonceSpec;
use crate::tx::decode::{compute_budget_values, DecodedTx, TxFormat};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

mod carrier;
mod idl;
mod raw;

#[cfg(test)]
mod tests;

pub use carrier::{Carrier, CarrierKind};
pub use idl::idl_draft_for;
pub use raw::{RawAccountDraft, RawDraft};

/// How many levels of instructions inside instructions are opened.
const MAX_DEPTH: usize = 4;

/// One instruction, as Build edits it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenedIx {
    Idl(IdlDraft),
    Raw(RawDraft),
    Carried {
        carrier: Carrier,
        inner: Vec<OpenedIx>,
    },
}

/// A transaction, as Build edits it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenedTx {
    pub fee_payer: Option<Pubkey>,
    pub nonce: Option<NonceSpec>,
    pub cu_limit: Option<u32>,
    pub cu_price_micro_lamports: Option<u64>,
    pub loaded_accounts: Option<u32>,
    pub instructions: Vec<OpenedIx>,
}

pub fn open_for_edit(catalog: &dyn IdlCatalog, tx: &DecodedTx) -> OpenedTx {
    let mut native = tx.native_instructions();
    let mut nonce = None;
    if tx.uses_durable_nonce {
        let advance = native.remove(0);
        match advance.accounts.get(2) {
            Some(authority) => {
                nonce = Some(NonceSpec {
                    account: advance.accounts[0].pubkey,
                    authority: authority.pubkey,
                    hash: tx.lifetime,
                })
            }
            None => native.insert(0, advance),
        }
    }
    native.retain(|instruction| !is_budget_setting(instruction));
    OpenedTx {
        fee_payer: tx.fee_payer(),
        nonce,
        cu_limit: tx.budget.compute_unit_limit,
        cu_price_micro_lamports: unit_price(tx),
        loaded_accounts: tx.budget.loaded_accounts_data_size_limit,
        instructions: native
            .iter()
            .map(|instruction| open_instruction(catalog, instruction))
            .collect(),
    }
}

/// One instruction, opened as deep as it goes.
pub fn open_instruction(catalog: &dyn IdlCatalog, instruction: &Instruction) -> OpenedIx {
    open_at(catalog, instruction, 0)
}

fn open_at(catalog: &dyn IdlCatalog, instruction: &Instruction, depth: usize) -> OpenedIx {
    if depth < MAX_DEPTH {
        if let Some((carrier, inner)) = Carrier::open(instruction) {
            return OpenedIx::Carried {
                carrier,
                inner: inner
                    .iter()
                    .map(|inner| open_at(catalog, inner, depth + 1))
                    .collect(),
            };
        }
    }
    let program_id = instruction.program_id;
    catalog
        .records()
        .iter()
        .find(|record| record.program_id == program_id)
        .and_then(|record| {
            let client = catalog.program_client(&program_id)?;
            idl_draft_for(&client, &record.name, instruction)
        })
        .map(OpenedIx::Idl)
        .unwrap_or_else(|| OpenedIx::Raw(RawDraft::from_instruction(instruction)))
}

/// A well-formed ComputeBudget instruction that Build has a field for.
fn is_budget_setting(instruction: &Instruction) -> bool {
    instruction.program_id == COMPUTE_BUDGET_PROGRAM_ID
        && matches!(
            (instruction.data.first(), instruction.data.len()),
            (Some(2 | 4), 5) | (Some(3), 9)
        )
}

/// The priority fee as a price per compute unit. A v1 header states a total,
/// so the price is the least that comes to that total at the stated limit.
fn unit_price(tx: &DecodedTx) -> Option<u64> {
    if tx.format != TxFormat::V1 {
        return compute_budget_values(&tx.instructions).unit_price_micro_lamports;
    }
    let fee = tx.budget.priority_fee.filter(|fee| *fee > 0)?;
    let limit = u128::from(tx.stated_cu_limit().max(1));
    let total = |price: u128| (price * limit).div_ceil(1_000_000);
    let mut price = (u128::from(fee) * 1_000_000) / limit;
    if total(price) < u128::from(fee) {
        price += 1;
    }
    u64::try_from(price).ok()
}
