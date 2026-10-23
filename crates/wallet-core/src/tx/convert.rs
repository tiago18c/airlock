//! Rewriting a legacy or v0 transaction as v1 so an offline device can read it.

use crate::error::BuildError;
use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
use crate::tx::builder::{
    compile_message, runtime_default_cu_limit, ComputeBudgetSpec,
    DEFAULT_LOADED_ACCOUNTS_DATA_SIZE, MAX_CU_LIMIT,
};
use crate::tx::compose::with_margin;
use crate::tx::decode::{compute_budget_values, DecodedTx, TxFormat};
use crate::tx::renew::swap_in_data;
use solana_instruction::Instruction;
use solana_message::v1::{MAX_ADDRESSES, MAX_INSTRUCTIONS, MAX_SIGNATURES};
use solana_pubkey::Pubkey;
use thiserror::Error;

/// What [`to_v1`](to_v1) produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub wire: Vec<u8>,
    /// Signatures the older wire already held that no longer apply.
    pub voided: usize,
    pub from: TxFormat,
    /// Accounts the older wire loaded from lookup tables.
    pub loaded: usize,
    /// Lookup tables the older wire loaded accounts from.
    pub tables: usize,
    /// The compute limit in the v1 header, and where it came from.
    pub cu_limit: u32,
    pub limit_from: LimitFrom,
    /// One-time keys the site had signed with, each with the key that takes
    /// its place in the rewrite and still has to sign it.
    pub replaced: Vec<(Pubkey, Pubkey)>,
}

/// Where the limits of a rewrite come from, for those the site left unset.
/// A limit the site set is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Limits {
    /// What the runtime would have given the transaction as the site sent it.
    #[default]
    Runtime,
    /// The highest limits and no priority fee, to simulate the rewrite and
    /// measure what it uses.
    Probe,
    /// What a simulation of the probe measured, plus a margin. A value the
    /// node did not report falls back to [`Limits::Runtime`].
    Measured {
        units: Option<u64>,
        loaded_accounts: Option<u32>,
    },
}

/// Where the compute limit of a rewrite came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LimitFrom {
    /// The site's SetComputeUnitLimit.
    Site,
    /// What the runtime gives a transaction that sets no limit.
    #[default]
    Runtime,
    /// A simulation of the rewrite, plus a margin.
    Simulated,
    /// The highest limit, for a probe that is only simulated.
    Probe,
}

impl LimitFrom {
    /// A sentence for a notice about the rewrite.
    pub fn describe(&self, cu_limit: u32) -> String {
        match self {
            Self::Site => format!(
                "The compute limit, {cu_limit} units, is the one the transaction set."
            ),
            Self::Runtime => format!(
                "The transaction set no compute limit, so it is {cu_limit} units, what the network would have given it."
            ),
            Self::Simulated => format!(
                "The transaction set no compute limit, so it is {cu_limit} units: what a simulation used, plus 5%."
            ),
            Self::Probe => format!("The compute limit is {cu_limit} units, to measure it."),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConvertError {
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(
        "A signature from {0} is already on this transaction, and that key is not a wallet of this device, so the rewritten transaction cannot be completed here."
    )]
    OutsideSignature(Pubkey),
    #[error("{0}")]
    Message(String),
}

/// Rewrite `tx` as v1 with every account listed inline. Keeps the site's
/// blockhash or durable nonce. `known` is every wallet pubkey this device
/// holds: a filled signature for any other key stops the conversion. Limits
/// the site left unset are what the runtime would have given it.
pub fn to_v1(tx: &DecodedTx, known: &[Pubkey]) -> Result<Converted, ConvertError> {
    to_v1_with(tx, known, Limits::Runtime)
}

/// [`to_v1`], with the unset limits taken from `limits`. The priority fee is
/// the site's price per unit times the compute limit, so a tighter limit
/// keeps the site's priority and costs less.
pub fn to_v1_with(
    tx: &DecodedTx,
    known: &[Pubkey],
    limits: Limits,
) -> Result<Converted, ConvertError> {
    to_v1_swapping(tx, known, limits, &[])
}

/// [`to_v1_with`], with each `(old, new)` of `swaps` put in place of a
/// one-time key the site made and signed with, such as a new mint. Its
/// signature, if it had one, no longer applies, so it is not refused like
/// another outside signature, and `new` has to sign the rewrite instead.
///
/// A swapped key has to sign `tx`. Where an instruction's data names it, as
/// a mint's metadata pointer names the mint, `new` is written there too: the
/// key was made for this transaction, so it means the same new account. An
/// address derived from it is not updated: simulate the rewrite before using
/// it.
pub fn to_v1_swapping(
    tx: &DecodedTx,
    known: &[Pubkey],
    limits: Limits,
    swaps: &[(Pubkey, Pubkey)],
) -> Result<Converted, ConvertError> {
    if tx.format == TxFormat::V1 {
        return Err(ConvertError::Message(
            "This transaction is already v1.".into(),
        ));
    }

    for (old, new) in swaps {
        if !tx.signers.iter().any(|slot| slot.pubkey == *old) {
            return Err(ConvertError::Message(format!(
                "{old} does not sign this transaction, so it cannot be replaced."
            )));
        }
        if tx.accounts.iter().any(|account| account.pubkey == *new) {
            return Err(ConvertError::Message(format!(
                "{new} is already in this transaction."
            )));
        }
    }
    let swapped = |key: &Pubkey| {
        swaps
            .iter()
            .find(|(old, _)| old == key)
            .map_or(*key, |(_, new)| *new)
    };

    let mut voided = 0usize;
    for slot in &tx.signers {
        if !slot.filled {
            continue;
        }
        let one_time = swaps.iter().any(|(old, _)| *old == slot.pubkey);
        if !known.contains(&slot.pubkey) && !one_time {
            // TODO: log the refused outside signature (pubkey, format) once
            // app logging is in place.
            return Err(ConvertError::OutsideSignature(slot.pubkey));
        }
        voided += 1;
    }

    let fee_payer = tx
        .fee_payer()
        .map(|payer| swapped(&payer))
        .ok_or_else(|| ConvertError::Message("This transaction has no fee payer.".into()))?;

    let body_instructions: Vec<Instruction> = tx
        .instructions
        .iter()
        .filter(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
        .map(|raw| Instruction {
            program_id: raw.program_id,
            accounts: raw
                .accounts
                .iter()
                .map(|pubkey| {
                    let known = tx.accounts.iter().find(|a| a.pubkey == *pubkey);
                    solana_instruction::AccountMeta {
                        pubkey: swapped(pubkey),
                        is_signer: known.is_some_and(|a| a.signer),
                        is_writable: known.is_some_and(|a| a.writable),
                    }
                })
                .collect(),
            data: swap_in_data(&raw.data, swaps),
        })
        .collect();

    if body_instructions.len() > usize::from(MAX_INSTRUCTIONS) {
        return Err(BuildError::Message(format!(
            "The rewritten transaction would have {} instructions; v1 holds at most {MAX_INSTRUCTIONS}.",
            body_instructions.len()
        ))
        .into());
    }
    if tx.accounts.len() > usize::from(MAX_ADDRESSES) {
        return Err(BuildError::Message(format!(
            "The rewritten transaction would list {} accounts; v1 holds at most {MAX_ADDRESSES}.",
            tx.accounts.len()
        ))
        .into());
    }
    if tx.signers.len() > usize::from(MAX_SIGNATURES) {
        return Err(BuildError::Message(format!(
            "The rewritten transaction would need {} signatures; v1 holds at most {MAX_SIGNATURES}.",
            tx.signers.len()
        ))
        .into());
    }

    let loaded = tx
        .accounts
        .iter()
        .filter(|account| account.lookup.is_some())
        .count();
    let tables = tx
        .accounts
        .iter()
        .filter_map(|account| account.lookup)
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    let site = compute_budget_values(&tx.instructions);
    let runtime_cu_limit =
        || runtime_default_cu_limit(body_instructions.iter().map(|ix| &ix.program_id));
    let (cu_limit, limit_from) = match (site.unit_limit, limits) {
        (Some(limit), _) => (limit.min(MAX_CU_LIMIT), LimitFrom::Site),
        (None, Limits::Probe) => (MAX_CU_LIMIT, LimitFrom::Probe),
        (
            None,
            Limits::Measured {
                units: Some(units), ..
            },
        ) => (with_margin(units, MAX_CU_LIMIT), LimitFrom::Simulated),
        (None, _) => (runtime_cu_limit(), LimitFrom::Runtime),
    };
    let loaded_accounts = match (site.loaded_accounts_data_size, limits) {
        (Some(size), _) => size.min(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE),
        (
            None,
            Limits::Measured {
                loaded_accounts: Some(size),
                ..
            },
        ) => with_margin(u64::from(size), DEFAULT_LOADED_ACCOUNTS_DATA_SIZE),
        (None, _) => DEFAULT_LOADED_ACCOUNTS_DATA_SIZE,
    };
    let mut config = ComputeBudgetSpec {
        cu_limit: Some(cu_limit),
        cu_price_micro_lamports: site
            .unit_price_micro_lamports
            .filter(|_| limits != Limits::Probe),
        loaded_accounts_data_size: Some(loaded_accounts),
    }
    .config(body_instructions.len());
    if let Some(heap) = site.heap_size {
        config = config.with_heap_size(heap);
    }

    let compiled = compile_message(&fee_payer, &body_instructions, tx.lifetime, config)?;
    Ok(Converted {
        wire: compiled.wire,
        voided,
        from: tx.format,
        loaded,
        tables,
        cu_limit,
        limit_from,
        replaced: swaps.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::transfer;
    use crate::ids::COMPUTE_BUDGET_PROGRAM_ID;
    use crate::keys::signer::KeypairSigner;
    use crate::keys::signer::WalletSigner;
    use crate::tx::decode::{
        decode_transaction, decode_v1_transaction, DecodedAccount, DecodedSigner, LookupTables,
        RawInstruction,
    };
    use crate::tx::signatures::apply_local_signatures;
    use solana_hash::Hash;
    use solana_instruction::Instruction;
    use solana_message::v0::{Message as V0Message, MessageAddressTableLookup};
    use solana_message::{Message as LegacyMessage, VersionedMessage};
    use solana_signature::Signature;
    use solana_transaction::versioned::VersionedTransaction;

    fn hash_from_byte(b: u8) -> Hash {
        Hash::new_from_array([b; 32])
    }

    fn empty_sigs(n: usize) -> Vec<Signature> {
        vec![Signature::default(); n]
    }

    fn legacy_wire(instructions: &[Instruction], payer: &Pubkey) -> Vec<u8> {
        let mut message = LegacyMessage::new(instructions, Some(payer));
        message.recent_blockhash = hash_from_byte(1);
        let nsig = usize::from(message.header.num_required_signatures);
        wincode::serialize(&VersionedTransaction {
            signatures: empty_sigs(nsig),
            message: VersionedMessage::Legacy(message),
        })
        .unwrap()
    }

    #[test]
    fn compute_budget_moves_into_the_header_and_is_removed() {
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
        let wire = legacy_wire(
            &[
                set_limit,
                set_price,
                transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5),
            ],
            &from.pubkey(),
        );
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        let converted = to_v1(&tx, &[from.pubkey()]).unwrap();
        assert_eq!(converted.from, TxFormat::Legacy);
        assert_eq!(converted.voided, 0);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert_eq!(v1.format, TxFormat::V1);
        assert_eq!(v1.budget.compute_unit_limit, Some(200_000));
        assert_eq!(v1.budget.priority_fee, Some(200));
        assert!(v1
            .instructions
            .iter()
            .all(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID));
        assert_eq!(v1.lifetime, hash_from_byte(1));
    }

    fn budget_ix(tag: u8, value: &[u8]) -> Instruction {
        Instruction {
            program_id: COMPUTE_BUDGET_PROGRAM_ID,
            accounts: Vec::new(),
            data: [vec![tag], value.to_vec()].concat(),
        }
    }

    fn other_program(from: Pubkey) -> Instruction {
        Instruction {
            program_id: Pubkey::new_from_array([9; 32]),
            accounts: vec![solana_instruction::AccountMeta::new(from, true)],
            data: Vec::new(),
        }
    }

    #[test]
    fn without_a_limit_each_instruction_gets_what_the_runtime_gives() {
        let from = KeypairSigner::new_random();
        let wire = legacy_wire(
            &[
                budget_ix(3, &1_000u64.to_le_bytes()),
                transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5),
                other_program(from.pubkey()),
            ],
            &from.pubkey(),
        );
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        let converted = to_v1(&tx, &[from.pubkey()]).unwrap();
        // The transfer is a built-in at 3,000 units, the other program 200,000.
        assert_eq!(converted.cu_limit, 203_000);
        assert_eq!(converted.limit_from, LimitFrom::Runtime);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert_eq!(v1.budget.compute_unit_limit, Some(203_000));
        assert_eq!(v1.budget.priority_fee, Some(203));
        assert_eq!(
            v1.budget.loaded_accounts_data_size_limit,
            Some(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE)
        );
    }

    #[test]
    fn a_probe_has_the_highest_limit_and_no_fee_and_a_measure_sets_both_limits() {
        let from = KeypairSigner::new_random();
        let wire = legacy_wire(
            &[
                budget_ix(3, &1_000u64.to_le_bytes()),
                transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5),
            ],
            &from.pubkey(),
        );
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();

        let probe = to_v1_with(&tx, &[from.pubkey()], Limits::Probe).unwrap();
        let budget = decode_v1_transaction(&probe.wire).unwrap().budget;
        assert_eq!(budget.compute_unit_limit, Some(MAX_CU_LIMIT));
        assert_eq!(budget.priority_fee, None);

        let measured = Limits::Measured {
            units: Some(150),
            loaded_accounts: Some(1_000),
        };
        let fitted = to_v1_with(&tx, &[from.pubkey()], measured).unwrap();
        assert_eq!(fitted.limit_from, LimitFrom::Simulated);
        let budget = decode_v1_transaction(&fitted.wire).unwrap().budget;
        assert_eq!(budget.compute_unit_limit, Some(158));
        assert_eq!(budget.loaded_accounts_data_size_limit, Some(1_050));
        // 158 units at the site's 1,000 micro-lamports, rounded up.
        assert_eq!(budget.priority_fee, Some(1));

        // A node that measured nothing leaves the runtime's limits.
        let unmeasured = Limits::Measured {
            units: None,
            loaded_accounts: None,
        };
        let fallback = to_v1_with(&tx, &[from.pubkey()], unmeasured).unwrap();
        assert_eq!(fallback.limit_from, LimitFrom::Runtime);
        assert_eq!(fallback.cu_limit, 3_000);
    }

    #[test]
    fn the_sites_limits_are_kept_within_the_most_allowed_and_the_heap_is_carried() {
        let from = KeypairSigner::new_random();
        let wire = legacy_wire(
            &[
                budget_ix(1, &(64 * 1024u32).to_le_bytes()),
                budget_ix(2, &2_000_000u32.to_le_bytes()),
                budget_ix(4, &4_096u32.to_le_bytes()),
                transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5),
            ],
            &from.pubkey(),
        );
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        let measured = Limits::Measured {
            units: Some(150),
            loaded_accounts: Some(1_000),
        };
        let converted = to_v1_with(&tx, &[from.pubkey()], measured).unwrap();
        assert_eq!(converted.limit_from, LimitFrom::Site);
        let budget = decode_v1_transaction(&converted.wire).unwrap().budget;
        assert_eq!(budget.compute_unit_limit, Some(MAX_CU_LIMIT));
        assert_eq!(budget.loaded_accounts_data_size_limit, Some(4_096));
        assert_eq!(budget.heap_size, Some(64 * 1024));
    }

    #[test]
    fn a_filled_local_signature_is_voided() {
        let from = KeypairSigner::new_random();
        let wire = legacy_wire(
            &[transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5)],
            &from.pubkey(),
        );
        let signed = apply_local_signatures(&wire, &[&from as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert!(tx.signers[0].filled);
        let converted = to_v1(&tx, &[from.pubkey()]).unwrap();
        assert_eq!(converted.voided, 1);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert!(!v1.signers[0].filled);
    }

    #[test]
    fn a_filled_outside_signature_is_refused() {
        let from = KeypairSigner::new_random();
        let outsider = KeypairSigner::new_random();
        // `from` signed, and this device knows only `outsider`.
        let wire = legacy_wire(
            &[transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5)],
            &from.pubkey(),
        );
        let signed = apply_local_signatures(&wire, &[&from as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        assert_eq!(
            to_v1(&tx, &[outsider.pubkey()]).unwrap_err(),
            ConvertError::OutsideSignature(from.pubkey())
        );
    }

    #[test]
    fn a_one_time_key_the_site_signed_with_is_swapped_for_a_new_one() {
        let payer = KeypairSigner::new_random();
        let mint = KeypairSigner::new_random();
        let fresh = KeypairSigner::new_random();
        let owner = Pubkey::new_from_array([7; 32]);
        let create = solana_system_interface::instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            1_000,
            82,
            &owner,
        );
        let wire = legacy_wire(&[create], &payer.pubkey());
        let signed = apply_local_signatures(&wire, &[&mint as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        let swaps = [(mint.pubkey(), fresh.pubkey())];
        let converted = to_v1_swapping(&tx, &[payer.pubkey()], Limits::Runtime, &swaps).unwrap();
        assert_eq!(converted.voided, 1);
        assert_eq!(converted.replaced, swaps);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        let signers: Vec<Pubkey> = v1.signers.iter().map(|slot| slot.pubkey).collect();
        assert_eq!(signers, vec![payer.pubkey(), fresh.pubkey()]);
        assert!(v1.signers.iter().all(|slot| !slot.filled));
        assert!(v1.accounts.iter().all(|a| a.pubkey != mint.pubkey()));
        let resigned =
            apply_local_signatures(&converted.wire, &[&payer as &dyn WalletSigner, &fresh])
                .unwrap();
        assert!(decode_v1_transaction(&resigned).unwrap().fully_signed());
    }

    #[test]
    fn a_one_time_key_in_instruction_data_is_swapped_there_too() {
        let payer = KeypairSigner::new_random();
        let mint = KeypairSigner::new_random();
        let mut ix = transfer(payer.pubkey(), Pubkey::new_from_array([2; 32]), 5);
        ix.accounts
            .push(solana_instruction::AccountMeta::new(mint.pubkey(), true));
        ix.data.extend_from_slice(mint.pubkey().as_ref());
        let wire = legacy_wire(&[ix], &payer.pubkey());
        let signed = apply_local_signatures(&wire, &[&mint as &dyn WalletSigner]).unwrap();
        let tx = decode_transaction(&signed, &LookupTables::new()).unwrap();
        let fresh = Pubkey::new_unique();
        let converted = to_v1_swapping(
            &tx,
            &[payer.pubkey()],
            Limits::Runtime,
            &[(mint.pubkey(), fresh)],
        )
        .unwrap();
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        let data = &v1.instructions[0].data;
        assert_eq!(&data[data.len() - 32..], fresh.as_ref());
        assert!(!data
            .windows(32)
            .any(|window| window == mint.pubkey().as_ref()));
    }

    #[test]
    fn table_accounts_are_inlined() {
        let payer = KeypairSigner::new_random().pubkey();
        let loaded = Pubkey::new_from_array([7; 32]);
        let table = Pubkey::new_from_array([9; 32]);
        let header = solana_message::MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        };
        let ix = solana_message::compiled_instruction::CompiledInstruction {
            program_id_index: 1,
            accounts: vec![0, 2],
            data: {
                let mut data = 2u32.to_le_bytes().to_vec();
                data.extend_from_slice(&1_000u64.to_le_bytes());
                data
            },
        };
        let message = V0Message {
            header,
            account_keys: vec![payer, crate::ids::SYSTEM_PROGRAM_ID],
            recent_blockhash: hash_from_byte(1),
            instructions: vec![ix],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![0],
                readonly_indexes: vec![],
            }],
        };
        let wire = wincode::serialize(&VersionedTransaction {
            signatures: empty_sigs(1),
            message: VersionedMessage::V0(message),
        })
        .unwrap();
        let mut tables = LookupTables::new();
        tables.insert(table, vec![loaded]);
        let tx = decode_transaction(&wire, &tables).unwrap();
        let converted = to_v1(&tx, &[payer]).unwrap();
        assert_eq!(converted.tables, 1);
        assert_eq!(converted.loaded, 1);
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert!(v1
            .accounts
            .iter()
            .any(|a| a.pubkey == loaded && a.lookup.is_none()));
        assert_eq!(v1.lifetime, hash_from_byte(1));
    }

    #[test]
    fn nonce_advance_stays_first() {
        let from = KeypairSigner::new_random();
        let nonce = Pubkey::new_from_array([3; 32]);
        let advance =
            solana_system_interface::instruction::advance_nonce_account(&nonce, &from.pubkey());
        let send = transfer(from.pubkey(), Pubkey::new_from_array([2; 32]), 5);
        let wire = legacy_wire(&[advance, send], &from.pubkey());
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        assert!(tx.uses_durable_nonce);
        let converted = to_v1(&tx, &[from.pubkey()]).unwrap();
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert!(v1.uses_durable_nonce);
        assert_eq!(v1.lifetime, hash_from_byte(1));
        assert_eq!(v1.instructions[0].program_id, crate::ids::SYSTEM_PROGRAM_ID);
        assert!(v1.instructions[0].data.starts_with(&[4, 0, 0, 0]));
    }

    #[test]
    fn header_and_writability_are_kept() {
        let payer = KeypairSigner::new_random().pubkey();
        let cosigner = Pubkey::new_from_array([4; 32]);
        let loaded_writable = Pubkey::new_from_array([7; 32]);
        let loaded_readonly = Pubkey::new_from_array([8; 32]);
        let table = Pubkey::new_from_array([9; 32]);
        let header = solana_message::MessageHeader {
            num_required_signatures: 2,
            num_readonly_signed_accounts: 1,
            num_readonly_unsigned_accounts: 1,
        };
        let message = V0Message {
            header,
            account_keys: vec![payer, cosigner, crate::ids::SYSTEM_PROGRAM_ID],
            recent_blockhash: hash_from_byte(1),
            instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 3, 1, 4],
                data: {
                    let mut data = 2u32.to_le_bytes().to_vec();
                    data.extend_from_slice(&1_000u64.to_le_bytes());
                    data
                },
            }],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![0],
                readonly_indexes: vec![1],
            }],
        };
        let wire = wincode::serialize(&VersionedTransaction {
            signatures: empty_sigs(2),
            message: VersionedMessage::V0(message),
        })
        .unwrap();
        let mut tables = LookupTables::new();
        tables.insert(table, vec![loaded_writable, loaded_readonly]);
        let tx = decode_transaction(&wire, &tables).unwrap();
        assert_eq!(tx.signers.len(), 2);
        assert!(tx
            .accounts
            .iter()
            .any(|a| a.pubkey == loaded_writable && a.writable && a.lookup == Some(table)));
        assert!(tx
            .accounts
            .iter()
            .any(|a| a.pubkey == loaded_readonly && !a.writable && a.lookup == Some(table)));

        let converted = to_v1(&tx, &[payer, cosigner]).unwrap();
        let v1 = decode_v1_transaction(&converted.wire).unwrap();
        assert_eq!(v1.signers.len(), 2);
        assert_eq!(v1.signers[0].pubkey, payer);
        assert_eq!(v1.signers[1].pubkey, cosigner);
        let writable = v1
            .accounts
            .iter()
            .find(|a| a.pubkey == loaded_writable)
            .unwrap();
        assert!(writable.writable);
        assert!(writable.lookup.is_none());
        let readonly = v1
            .accounts
            .iter()
            .find(|a| a.pubkey == loaded_readonly)
            .unwrap();
        assert!(!readonly.writable);
    }

    fn stub_legacy(accounts: usize, instructions: usize, signer_count: usize) -> DecodedTx {
        let keys: Vec<Pubkey> = (0..accounts)
            .map(|i| Pubkey::new_from_array([(i as u8).wrapping_add(1); 32]))
            .collect();
        let signers: Vec<DecodedSigner> = (0..signer_count)
            .map(|i| DecodedSigner {
                pubkey: keys[i],
                filled: false,
                valid: false,
            })
            .collect();
        let instructions = (0..instructions)
            .map(|_| RawInstruction {
                program_id: crate::ids::SYSTEM_PROGRAM_ID,
                accounts: vec![keys[0], keys[1.min(accounts - 1)]],
                data: {
                    let mut data = 2u32.to_le_bytes().to_vec();
                    data.extend_from_slice(&1u64.to_le_bytes());
                    data
                },
            })
            .collect();
        DecodedTx {
            wire: Vec::new(),
            message: Vec::new(),
            format: TxFormat::Legacy,
            lifetime: hash_from_byte(1),
            uses_durable_nonce: false,
            budget: crate::tx::decode::TxBudget::default(),
            accounts: keys
                .into_iter()
                .enumerate()
                .map(|(i, pubkey)| DecodedAccount {
                    pubkey,
                    signer: i < signer_count,
                    writable: i < 2,
                    lookup: None,
                })
                .collect(),
            instructions,
            signers,
        }
    }

    #[test]
    fn each_limit_is_refused() {
        let too_many_ix = stub_legacy(3, usize::from(MAX_INSTRUCTIONS) + 1, 1);
        assert!(matches!(
            to_v1(&too_many_ix, &[]).unwrap_err(),
            ConvertError::Build(BuildError::Message(_))
        ));

        let too_many_accounts = stub_legacy(usize::from(MAX_ADDRESSES) + 1, 1, 1);
        assert!(matches!(
            to_v1(&too_many_accounts, &[]).unwrap_err(),
            ConvertError::Build(BuildError::Message(_))
        ));

        let too_many_sigs = stub_legacy(20, 1, usize::from(MAX_SIGNATURES) + 1);
        assert!(matches!(
            to_v1(&too_many_sigs, &[]).unwrap_err(),
            ConvertError::Build(BuildError::Message(_))
        ));

        // Oversize: many instructions with large data, past the early counts.
        let from = KeypairSigner::new_random();
        let big = Instruction {
            program_id: Pubkey::new_from_array([9; 32]),
            accounts: vec![solana_instruction::AccountMeta::new(from.pubkey(), true)],
            data: vec![0u8; 700],
        };
        let wire = legacy_wire(&vec![big; 7], &from.pubkey());
        let tx = decode_transaction(&wire, &LookupTables::new()).unwrap();
        assert!(matches!(
            to_v1(&tx, &[from.pubkey()]).unwrap_err(),
            ConvertError::Build(BuildError::TooLarge { .. })
        ));
    }
}
