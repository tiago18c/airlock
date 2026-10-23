//! Reading a transaction wire in any Solana format: legacy, v0 or v1.

use crate::error::DecodeError;
use crate::ids::{COMPUTE_BUDGET_PROGRAM_ID, SYSTEM_PROGRAM_ID};
use crate::tx::builder::{
    runtime_default_cu_limit, DEFAULT_CU_PER_INSTRUCTION, DEFAULT_LOADED_ACCOUNTS_DATA_SIZE,
    MAX_CU_LIMIT,
};
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::v1::{TransactionConfig, V1_PREFIX};
use solana_message::{MessageHeader, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::{Signature, SIGNATURE_BYTES};
use solana_transaction::versioned::VersionedTransaction;
use std::collections::HashMap;

#[cfg(test)]
mod tests;

/// How a transaction was laid out on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxFormat {
    Legacy,
    V0,
    V1,
}

/// What [`parse_wire`](parse_wire) reads without a network: enough to find
/// signature slots and the message bytes, and to list lookup tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireView {
    pub format: TxFormat,
    pub wire: Vec<u8>,
    /// Bytes the signatures cover.
    pub message: Vec<u8>,
    /// Byte offset of each required signature slot in [`wire`](Self::wire).
    pub signature_offsets: Vec<usize>,
    pub static_keys: Vec<Pubkey>,
    pub num_required_signatures: usize,
    /// Address lookup tables named by a v0 message, in order.
    pub lookup_table_keys: Vec<Pubkey>,
}

impl WireView {
    /// Whether slot `index` holds a valid signature over the message.
    pub fn slot_valid(&self, index: usize) -> bool {
        let (Some(offset), Some(pubkey)) = (
            self.signature_offsets.get(index),
            self.static_keys.get(index),
        ) else {
            return false;
        };
        let Some(Ok(slot)) = self
            .wire
            .get(*offset..*offset + SIGNATURE_BYTES)
            .map(<[u8; SIGNATURE_BYTES]>::try_from)
        else {
            return false;
        };
        slot.iter().any(|b| *b != 0) && Signature::from(slot).verify(pubkey.as_ref(), &self.message)
    }

    pub fn valid_signatures(&self) -> usize {
        (0..self.num_required_signatures)
            .filter(|&index| self.slot_valid(index))
            .count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedAccount {
    pub pubkey: Pubkey,
    pub signer: bool,
    pub writable: bool,
    /// The lookup table this account was loaded from, when it was.
    pub lookup: Option<Pubkey>,
}

/// One instruction with its account indexes resolved. What it means is read
/// from the program's IDL, in `explain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawInstruction {
    pub program_id: Pubkey,
    pub accounts: Vec<Pubkey>,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedSigner {
    pub pubkey: Pubkey,
    pub filled: bool,
    pub valid: bool,
}

/// Limits and priority fee. On v1 they come from the header. On legacy and v0
/// they are read from ComputeBudget instructions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxBudget {
    pub compute_unit_limit: Option<u32>,
    /// A total in lamports, on top of the base fee.
    pub priority_fee: Option<u64>,
    pub loaded_accounts_data_size_limit: Option<u32>,
    pub heap_size: Option<u32>,
}

impl TxBudget {
    /// The header that states this budget again, as it was.
    pub(crate) fn config(&self) -> TransactionConfig {
        let mut config = TransactionConfig::empty();
        if let Some(limit) = self.compute_unit_limit {
            config = config.with_compute_unit_limit(limit);
        }
        if let Some(limit) = self.loaded_accounts_data_size_limit {
            config = config.with_loaded_accounts_data_size_limit(limit);
        }
        self.carry_fee_and_heap(config)
    }

    /// The header of a transaction rebuilt around these instructions:
    /// `limit` compute units, at most [`MAX_CU_LIMIT`], and this priority fee
    /// and heap size. The loaded accounts limit is the default. The one
    /// stated was measured for the instructions alone, and what wraps them
    /// loads more programs and accounts. See `relay::fit_loaded_accounts`.
    pub(crate) fn rebuilt_config(&self, limit: u32) -> TransactionConfig {
        let config = TransactionConfig::empty()
            .with_compute_unit_limit(limit.min(MAX_CU_LIMIT))
            .with_loaded_accounts_data_size_limit(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE);
        self.carry_fee_and_heap(config)
    }

    fn carry_fee_and_heap(&self, mut config: TransactionConfig) -> TransactionConfig {
        if let Some(fee) = self.priority_fee {
            config = config.with_priority_fee(fee);
        }
        if let Some(heap) = self.heap_size {
            config = config.with_heap_size(heap);
        }
        config
    }
}

/// What a cluster charges for each signature of a transaction.
pub const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

/// Address lookup table account: 56-byte meta, then 32-byte addresses.
pub const LOOKUP_TABLE_META_SIZE: usize = 56;

/// Tables a v0 decode needs: table address to the addresses it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookupTables {
    tables: HashMap<Pubkey, Vec<Pubkey>>,
}

impl LookupTables {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, table: Pubkey, addresses: Vec<Pubkey>) {
        self.tables.insert(table, addresses);
    }

    pub fn get(&self, table: &Pubkey) -> Option<&[Pubkey]> {
        self.tables.get(table).map(|v| v.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTx {
    pub wire: Vec<u8>,
    pub message: Vec<u8>,
    pub format: TxFormat,
    pub lifetime: Hash,
    pub uses_durable_nonce: bool,
    pub budget: TxBudget,
    pub accounts: Vec<DecodedAccount>,
    pub instructions: Vec<RawInstruction>,
    pub signers: Vec<DecodedSigner>,
}

impl DecodedTx {
    /// The account that pays the fee: the first signer.
    pub fn fee_payer(&self) -> Option<Pubkey> {
        self.signers.first().map(|slot| slot.pubkey)
    }

    /// The most this transaction costs its fee payer in fees: the base fee
    /// for each signature plus the priority fee.
    pub fn fee_lamports(&self) -> u64 {
        (self.signers.len() as u64)
            .saturating_mul(LAMPORTS_PER_SIGNATURE)
            .saturating_add(self.budget.priority_fee.unwrap_or(0))
    }

    /// The compute limit this transaction stated, or the default its
    /// instructions get.
    pub fn stated_cu_limit(&self) -> u32 {
        self.budget.compute_unit_limit.unwrap_or_else(|| {
            let count = u32::try_from(self.instructions.len().max(1)).unwrap_or(u32::MAX);
            DEFAULT_CU_PER_INSTRUCTION.saturating_mul(count)
        })
    }

    /// Every required signature is present and valid.
    pub fn fully_signed(&self) -> bool {
        self.signers.iter().all(|slot| slot.valid)
    }

    /// Each instruction with its accounts flagged the way the message flags them.
    pub fn native_instructions(&self) -> Vec<Instruction> {
        self.instructions
            .iter()
            .map(|raw| Instruction {
                program_id: raw.program_id,
                accounts: raw
                    .accounts
                    .iter()
                    .map(|pubkey| {
                        let known = self.accounts.iter().find(|a| a.pubkey == *pubkey);
                        AccountMeta {
                            pubkey: *pubkey,
                            is_signer: known.is_some_and(|a| a.signer),
                            is_writable: known.is_some_and(|a| a.writable),
                        }
                    })
                    .collect(),
                data: raw.data.clone(),
            })
            .collect()
    }
}

/// Read signature slots and the message from any format, without lookup tables.
pub fn parse_wire(wire: &[u8]) -> Result<WireView, DecodeError> {
    if wire.is_empty() {
        return Err(DecodeError::Truncated);
    }
    let tx: VersionedTransaction =
        wincode::deserialize(wire).map_err(|_| DecodeError::Truncated)?;
    let format = match &tx.message {
        VersionedMessage::Legacy(_) => TxFormat::Legacy,
        VersionedMessage::V0(_) => TxFormat::V0,
        VersionedMessage::V1(_) => TxFormat::V1,
    };
    let message = tx.message.serialize();
    let nsig = usize::from(tx.message.header().num_required_signatures);
    if tx.signatures.len() != nsig {
        return Err(DecodeError::Truncated);
    }
    let static_keys: Vec<Pubkey> = tx
        .message
        .static_account_keys()
        .iter()
        .map(|key| Pubkey::new_from_array(key.to_bytes()))
        .collect();
    if nsig > static_keys.len() {
        return Err(DecodeError::Truncated);
    }
    let lookup_table_keys = tx
        .message
        .address_table_lookups()
        .unwrap_or(&[])
        .iter()
        .map(|lookup| Pubkey::new_from_array(lookup.account_key.to_bytes()))
        .collect();

    let signature_offsets = match format {
        TxFormat::V1 => {
            if !wire.starts_with(&message) {
                return Err(DecodeError::Truncated);
            }
            let sig_start = message.len();
            if wire.len() != sig_start + nsig * SIGNATURE_BYTES {
                return Err(DecodeError::Truncated);
            }
            (0..nsig).map(|i| sig_start + i * SIGNATURE_BYTES).collect()
        }
        TxFormat::Legacy | TxFormat::V0 => {
            let prefix = short_u16_len(nsig);
            let sig_start = prefix;
            let message_start = prefix + nsig * SIGNATURE_BYTES;
            if wire.len() != message_start + message.len()
                || &wire[message_start..] != message.as_slice()
            {
                return Err(DecodeError::Truncated);
            }
            (0..nsig).map(|i| sig_start + i * SIGNATURE_BYTES).collect()
        }
    };

    Ok(WireView {
        format,
        wire: wire.to_vec(),
        message,
        signature_offsets,
        static_keys,
        num_required_signatures: nsig,
        lookup_table_keys,
    })
}

/// Lookup table addresses a v0 wire names. Empty for legacy and v1.
pub fn lookup_tables_needed(wire: &[u8]) -> Result<Vec<Pubkey>, DecodeError> {
    Ok(parse_wire(wire)?.lookup_table_keys)
}

/// Addresses stored in an address lookup table account. A deactivated table
/// still resolves until it is closed, so this does not refuse one.
pub fn parse_lookup_table_addresses(data: &[u8]) -> Result<Vec<Pubkey>, DecodeError> {
    if data.len() < LOOKUP_TABLE_META_SIZE {
        return Err(DecodeError::Message(
            "Address lookup table account is too short.".into(),
        ));
    }
    let raw = &data[LOOKUP_TABLE_META_SIZE..];
    let (addresses, rest) = raw.as_chunks::<32>();
    if !rest.is_empty() {
        return Err(DecodeError::Message(
            "Address lookup table account data is truncated.".into(),
        ));
    }
    Ok(addresses
        .iter()
        .map(|bytes| Pubkey::new_from_array(*bytes))
        .collect())
}

/// Decode any format. Legacy and v1 need no tables. V0 needs every table it
/// names in `tables`.
pub fn decode_transaction(wire: &[u8], tables: &LookupTables) -> Result<DecodedTx, DecodeError> {
    let view = parse_wire(wire)?;
    let tx: VersionedTransaction =
        wincode::deserialize(wire).map_err(|_| DecodeError::Truncated)?;

    let signers = (0..view.num_required_signatures)
        .map(|i| {
            let start = view.signature_offsets[i];
            let slot: [u8; SIGNATURE_BYTES] = wire[start..start + SIGNATURE_BYTES]
                .try_into()
                .map_err(|_| DecodeError::Truncated)?;
            let filled = slot.iter().any(|b| *b != 0);
            let signature = Signature::from(slot);
            let valid = filled && signature.verify(view.static_keys[i].as_ref(), &view.message);
            Ok(DecodedSigner {
                pubkey: view.static_keys[i],
                filled,
                valid,
            })
        })
        .collect::<Result<Vec<_>, DecodeError>>()?;

    let (accounts, instructions, lifetime, budget) = match &tx.message {
        VersionedMessage::V1(message) => {
            let accounts = view
                .static_keys
                .iter()
                .enumerate()
                .map(|(i, pk)| DecodedAccount {
                    pubkey: *pk,
                    signer: i < view.num_required_signatures,
                    writable: is_writable(message.header, i, view.static_keys.len()),
                    lookup: None,
                })
                .collect();
            let mut instructions = Vec::with_capacity(message.instructions.len());
            for ix in &message.instructions {
                instructions.push(decode_instruction(
                    &view.static_keys,
                    ix.program_id_index,
                    &ix.accounts,
                    &ix.data,
                )?);
            }
            (
                accounts,
                instructions,
                message.lifetime_specifier,
                TxBudget {
                    compute_unit_limit: message.config.compute_unit_limit,
                    priority_fee: message.config.priority_fee,
                    loaded_accounts_data_size_limit: message.config.loaded_accounts_data_size_limit,
                    heap_size: message.config.heap_size,
                },
            )
        }
        VersionedMessage::Legacy(message) => {
            let accounts = view
                .static_keys
                .iter()
                .enumerate()
                .map(|(i, pk)| DecodedAccount {
                    pubkey: *pk,
                    signer: i < view.num_required_signatures,
                    writable: is_writable(message.header, i, view.static_keys.len()),
                    lookup: None,
                })
                .collect::<Vec<_>>();
            let mut instructions = Vec::with_capacity(message.instructions.len());
            for ix in &message.instructions {
                instructions.push(decode_instruction(
                    &view.static_keys,
                    ix.program_id_index,
                    &ix.accounts,
                    &ix.data,
                )?);
            }
            let budget = budget_from_compute_budget(&instructions);
            (accounts, instructions, message.recent_blockhash, budget)
        }
        VersionedMessage::V0(message) => {
            let loaded = load_v0_accounts(message, tables)?;
            let all_keys: Vec<Pubkey> = loaded.iter().map(|a| a.pubkey).collect();
            let mut instructions = Vec::with_capacity(message.instructions.len());
            for ix in &message.instructions {
                instructions.push(decode_instruction(
                    &all_keys,
                    ix.program_id_index,
                    &ix.accounts,
                    &ix.data,
                )?);
            }
            let budget = budget_from_compute_budget(&instructions);
            (loaded, instructions, message.recent_blockhash, budget)
        }
    };

    let uses_durable_nonce = instructions.first().is_some_and(is_advance_nonce);

    Ok(DecodedTx {
        wire: wire.to_vec(),
        message: view.message,
        format: view.format,
        lifetime,
        uses_durable_nonce,
        budget,
        accounts,
        instructions,
        signers,
    })
}

/// Strict v1 decode. The offline device, QRS1 handoff and transport stay on this.
pub fn decode_v1_transaction(wire: &[u8]) -> Result<DecodedTx, DecodeError> {
    if wire.first().copied() != Some(V1_PREFIX) {
        return Err(DecodeError::NotV1);
    }
    let tx = decode_transaction(wire, &LookupTables::new())?;
    if tx.format != TxFormat::V1 {
        return Err(DecodeError::NotV1);
    }
    Ok(tx)
}

fn load_v0_accounts(
    message: &solana_message::v0::Message,
    tables: &LookupTables,
) -> Result<Vec<DecodedAccount>, DecodeError> {
    let nsig = usize::from(message.header.num_required_signatures);
    let static_keys: Vec<Pubkey> = message
        .account_keys
        .iter()
        .map(|key| Pubkey::new_from_array(key.to_bytes()))
        .collect();
    let mut accounts: Vec<DecodedAccount> = static_keys
        .iter()
        .enumerate()
        .map(|(i, pk)| DecodedAccount {
            pubkey: *pk,
            signer: i < nsig,
            writable: is_writable(message.header, i, static_keys.len()),
            lookup: None,
        })
        .collect();

    // The runtime lists every table's writable accounts before any table's
    // readonly ones, and instruction indexes count in that order.
    for writable in [true, false] {
        for lookup in &message.address_table_lookups {
            let table = Pubkey::new_from_array(lookup.account_key.to_bytes());
            let addresses = tables
                .get(&table)
                .ok_or(DecodeError::MissingLookupTable(table))?;
            let indexes = if writable {
                &lookup.writable_indexes
            } else {
                &lookup.readonly_indexes
            };
            push_loaded(&mut accounts, table, addresses, indexes, writable)?;
        }
    }
    Ok(accounts)
}

fn push_loaded(
    accounts: &mut Vec<DecodedAccount>,
    table: Pubkey,
    addresses: &[Pubkey],
    indexes: &[u8],
    writable: bool,
) -> Result<(), DecodeError> {
    for &index in indexes {
        let pubkey = *addresses
            .get(usize::from(index))
            .ok_or(DecodeError::LookupIndexOutOfRange)?;
        accounts.push(DecodedAccount {
            pubkey,
            signer: false,
            writable,
            lookup: Some(table),
        });
    }
    Ok(())
}

fn decode_instruction(
    keys: &[Pubkey],
    program_id_index: u8,
    accounts: &[u8],
    data: &[u8],
) -> Result<RawInstruction, DecodeError> {
    let key = |index: u8| {
        keys.get(usize::from(index))
            .copied()
            .ok_or(DecodeError::BadAccountIndex)
    };
    Ok(RawInstruction {
        program_id: key(program_id_index)?,
        accounts: accounts
            .iter()
            .map(|index| key(*index))
            .collect::<Result<_, _>>()?,
        data: data.to_vec(),
    })
}

/// What the ComputeBudget instructions of a legacy or v0 message set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComputeBudgetValues {
    pub heap_size: Option<u32>,
    pub unit_limit: Option<u32>,
    pub unit_price_micro_lamports: Option<u64>,
    pub loaded_accounts_data_size: Option<u32>,
}

pub(crate) fn compute_budget_values(instructions: &[RawInstruction]) -> ComputeBudgetValues {
    let mut values = ComputeBudgetValues::default();
    let le_u32 = |data: &[u8]| {
        data.get(1..5)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let le_u64 = |data: &[u8]| {
        data.get(1..9)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    };
    for ix in instructions {
        if ix.program_id != COMPUTE_BUDGET_PROGRAM_ID {
            continue;
        }
        match ix.data.first() {
            Some(1) => values.heap_size = le_u32(&ix.data).or(values.heap_size),
            Some(2) => values.unit_limit = le_u32(&ix.data).or(values.unit_limit),
            Some(3) => {
                values.unit_price_micro_lamports =
                    le_u64(&ix.data).or(values.unit_price_micro_lamports)
            }
            Some(4) => {
                values.loaded_accounts_data_size =
                    le_u32(&ix.data).or(values.loaded_accounts_data_size)
            }
            _ => {}
        }
    }
    values
}

/// Read budget fields from ComputeBudget instructions in older formats.
fn budget_from_compute_budget(instructions: &[RawInstruction]) -> TxBudget {
    let ComputeBudgetValues {
        heap_size: heap,
        unit_limit: limit,
        unit_price_micro_lamports: price_micro,
        loaded_accounts_data_size: loaded,
    } = compute_budget_values(instructions);
    let priority_fee = price_micro.filter(|p| *p > 0).map(|price| {
        let units = limit.map_or_else(
            || runtime_default_cu_limit(instructions.iter().map(|ix| &ix.program_id)),
            |limit| limit.min(MAX_CU_LIMIT),
        );
        let fee = (u128::from(price) * u128::from(units)).div_ceil(1_000_000);
        u64::try_from(fee).unwrap_or(u64::MAX)
    });
    TxBudget {
        compute_unit_limit: limit,
        priority_fee,
        loaded_accounts_data_size_limit: loaded,
        heap_size: heap,
    }
}

/// The System program's `AdvanceNonceAccount`: enum tag 4 as a little-endian u32.
const ADVANCE_NONCE_TAG: [u8; 4] = [4, 0, 0, 0];

fn is_advance_nonce(instruction: &RawInstruction) -> bool {
    instruction.program_id == SYSTEM_PROGRAM_ID
        && instruction.data.starts_with(&ADVANCE_NONCE_TAG)
        && !instruction.accounts.is_empty()
}

fn is_writable(header: MessageHeader, index: usize, nkeys: usize) -> bool {
    let nsig = usize::from(header.num_required_signatures);
    let n_ro_signed = usize::from(header.num_readonly_signed_accounts);
    let n_ro_unsigned = usize::from(header.num_readonly_unsigned_accounts);
    if index >= nkeys {
        return false;
    }
    if index < nsig {
        index < nsig.saturating_sub(n_ro_signed)
    } else {
        index < nkeys.saturating_sub(n_ro_unsigned)
    }
}

fn short_u16_len(n: usize) -> usize {
    match n {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        _ => 3,
    }
}
