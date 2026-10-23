//! Compile instructions into a versioned transaction v1.
//!
//! Instructions themselves come from a program's IDL (`idl_draft`). This
//! module only orders them behind the nonce advance, sets the limits a v1
//! message carries in its header, and lays out the wire.

use crate::error::BuildError;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_message::v1::{TransactionConfig, MAX_TRANSACTION_SIZE, V1_PREFIX};
use solana_message::{v1, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::SIGNATURE_BYTES;
use solana_system_interface::instruction::advance_nonce_account;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonceSpec {
    pub account: Pubkey,
    pub authority: Pubkey,
    pub hash: Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledTx {
    /// Versioned transaction v1 wire bytes: `0x81` message, signatures at the tail.
    pub wire: Vec<u8>,
    pub message: Vec<u8>,
    pub required_signers: Vec<Pubkey>,
    pub uses_durable_nonce: bool,
}

/// Compute units a transaction gets per instruction when no limit is given.
pub const DEFAULT_CU_PER_INSTRUCTION: u32 = 200_000;
/// Compute units a legacy or v0 transaction that sets no limit gets for each
/// instruction of a program the runtime runs natively.
pub const BUILTIN_CU_PER_INSTRUCTION: u32 = 3_000;
/// The most compute units one transaction may ask for.
pub const MAX_CU_LIMIT: u32 = 1_400_000;
/// Bytes of account data a transaction may load when no limit is given.
pub const DEFAULT_LOADED_ACCOUNTS_DATA_SIZE: u32 = 64 * 1024 * 1024;

/// Programs the runtime runs natively, and the signature precompiles. Stake,
/// Config and Address Lookup Table are left out: they are moving to on-chain
/// programs, which the runtime budgets as any other program, so counting
/// them here could give a rewrite less than it needs.
const BUILTIN_PROGRAMS: [Pubkey; 9] = [
    crate::ids::SYSTEM_PROGRAM_ID,
    crate::ids::COMPUTE_BUDGET_PROGRAM_ID,
    Pubkey::from_str_const("Vote111111111111111111111111111111111111111"),
    Pubkey::from_str_const("BPFLoader1111111111111111111111111111111111"),
    Pubkey::from_str_const("BPFLoader2111111111111111111111111111111111"),
    Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111"),
    Pubkey::from_str_const("LoaderV411111111111111111111111111111111111"),
    Pubkey::from_str_const("Ed25519SigVerify111111111111111111111111111"),
    Pubkey::from_str_const("KeccakSecp256k11111111111111111111111111111"),
];

/// The compute limit the runtime gives a legacy or v0 transaction that sets
/// none, from the program of each of its instructions.
pub fn runtime_default_cu_limit<'a>(programs: impl IntoIterator<Item = &'a Pubkey>) -> u32 {
    programs
        .into_iter()
        .map(|program| {
            if BUILTIN_PROGRAMS.contains(program) {
                BUILTIN_CU_PER_INSTRUCTION
            } else {
                DEFAULT_CU_PER_INSTRUCTION
            }
        })
        .fold(0u32, u32::saturating_add)
        .min(MAX_CU_LIMIT)
}

/// Limits and priority fee.
///
/// A v1 message states these in its header. An unset limit there means zero,
/// not a default, and Compute Budget instructions are not how a v1
/// transaction sets them. So a blank field here is filled with the default
/// that older transaction formats applied silently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputeBudgetSpec {
    pub cu_limit: Option<u32>,
    /// Price per compute unit. The header holds a total in lamports, which is
    /// this times the limit, rounded up.
    pub cu_price_micro_lamports: Option<u64>,
    pub loaded_accounts_data_size: Option<u32>,
}

impl ComputeBudgetSpec {
    pub(crate) fn config(&self, instruction_count: usize) -> TransactionConfig {
        let cu_limit = self.cu_limit.unwrap_or_else(|| {
            let count = u32::try_from(instruction_count.max(1)).unwrap_or(u32::MAX);
            DEFAULT_CU_PER_INSTRUCTION
                .saturating_mul(count)
                .min(MAX_CU_LIMIT)
        });
        let mut config = TransactionConfig::empty()
            .with_compute_unit_limit(cu_limit)
            .with_loaded_accounts_data_size_limit(
                self.loaded_accounts_data_size
                    .unwrap_or(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE),
            );
        if let Some(price) = self.cu_price_micro_lamports.filter(|price| *price > 0) {
            let fee = (u128::from(price) * u128::from(cu_limit)).div_ceil(1_000_000);
            config = config.with_priority_fee(u64::try_from(fee).unwrap_or(u64::MAX));
        }
        config
    }
}

/// Compile extra instructions into a v1 tx. Durable nonce is prepended as ix 0.
pub fn compile_v1(
    fee_payer: Pubkey,
    extra: Vec<Instruction>,
    nonce: Option<NonceSpec>,
    recent_blockhash: Option<Hash>,
) -> Result<CompiledTx, BuildError> {
    compile_v1_budget(
        fee_payer,
        extra,
        nonce,
        recent_blockhash,
        ComputeBudgetSpec::default(),
    )
}

/// Like [`compile_v1`], with explicit limits and priority fee.
pub fn compile_v1_budget(
    fee_payer: Pubkey,
    extra: Vec<Instruction>,
    nonce: Option<NonceSpec>,
    recent_blockhash: Option<Hash>,
    budget: ComputeBudgetSpec,
) -> Result<CompiledTx, BuildError> {
    let mut instructions = Vec::new();
    let lifetime;
    let uses_durable_nonce;

    if let Some(nonce) = nonce {
        instructions.push(advance_nonce_account(&nonce.account, &nonce.authority));
        lifetime = nonce.hash;
        uses_durable_nonce = true;
    } else {
        lifetime = recent_blockhash.ok_or(BuildError::BlockhashOffline)?;
        uses_durable_nonce = false;
    }
    instructions.extend(extra);

    let config = budget.config(instructions.len());
    let mut compiled = compile_message(&fee_payer, &instructions, lifetime, config)?;
    compiled.uses_durable_nonce = uses_durable_nonce;
    Ok(compiled)
}

/// Compile `instructions` into a v1 message with the limits in `config`, and
/// lay out its unsigned wire.
pub(crate) fn compile_message(
    fee_payer: &Pubkey,
    instructions: &[Instruction],
    lifetime: Hash,
    config: TransactionConfig,
) -> Result<CompiledTx, BuildError> {
    let message = v1::Message::try_compile_with_config(fee_payer, instructions, lifetime, config)
        .map_err(|e| BuildError::Message(e.to_string()))?;
    wire_of(message)
}

/// Lay out the unsigned wire of a finished message: the message, then one
/// empty place for each required signature.
pub(crate) fn wire_of(message: v1::Message) -> Result<CompiledTx, BuildError> {
    let versioned = VersionedMessage::V1(message);
    let nsig = versioned.header().num_required_signatures as usize;
    let keys = versioned.static_account_keys();
    if nsig > keys.len() {
        return Err(BuildError::Message(
            "compiled message has no signers".into(),
        ));
    }
    let required_signers = keys[..nsig].to_vec();

    let message_bytes = versioned.serialize();
    if message_bytes.first().copied() != Some(V1_PREFIX) {
        return Err(BuildError::Message(
            "compiler did not produce a v1 message (expected 0x81 prefix)".into(),
        ));
    }

    let mut wire = message_bytes.clone();
    wire.extend(std::iter::repeat_n(
        0u8,
        nsig.saturating_mul(SIGNATURE_BYTES),
    ));
    if wire.len() > MAX_TRANSACTION_SIZE {
        return Err(BuildError::TooLarge { len: wire.len() });
    }

    Ok(CompiledTx {
        wire,
        message: message_bytes,
        required_signers,
        uses_durable_nonce: false,
    })
}

/// The System program moving `lamports` from `from` to `to`.
pub fn sol_transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(from, to, lamports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{blockhash, pk, token_transfer_checked, transfer};
    use crate::ids::{SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID};
    use crate::tx::decode::decode_v1_transaction;

    fn nonce(authority: Pubkey) -> NonceSpec {
        NonceSpec {
            account: pk(3),
            authority,
            hash: blockhash(9),
        }
    }

    #[test]
    fn nonce_transfer_is_v1_with_sigs_at_tail() {
        let from = pk(1);
        let compiled = compile_v1(
            from,
            vec![transfer(from, pk(2), 1_000)],
            Some(nonce(from)),
            None,
        )
        .unwrap();

        assert!(compiled.uses_durable_nonce);
        assert_eq!(compiled.wire[0], V1_PREFIX);
        assert_eq!(compiled.required_signers, vec![from]);
        let sig_bytes = compiled.required_signers.len() * SIGNATURE_BYTES;
        assert_eq!(compiled.wire.len(), compiled.message.len() + sig_bytes);
        assert!(compiled.wire[compiled.message.len()..]
            .iter()
            .all(|b| *b == 0));
        assert!(compiled.wire.len() < MAX_TRANSACTION_SIZE);
    }

    #[test]
    fn offline_without_nonce_is_rejected() {
        let err = compile_v1(pk(1), vec![transfer(pk(1), pk(2), 1)], None, None).unwrap_err();
        assert_eq!(err, BuildError::BlockhashOffline);
    }

    #[test]
    fn every_signer_an_instruction_needs_is_listed_after_the_fee_payer() {
        let (payer, from, owner) = (pk(1), pk(4), pk(5));
        let instructions = vec![
            transfer(from, pk(7), 42),
            token_transfer_checked(TOKEN_PROGRAM_ID, pk(8), pk(9), pk(10), owner, 1_000, 6),
        ];
        let compiled = compile_v1(payer, instructions, None, Some(blockhash(1))).unwrap();
        assert!(!compiled.uses_durable_nonce);
        assert_eq!(compiled.required_signers[0], payer);
        assert_eq!(compiled.required_signers.len(), 3);
        assert!(compiled.required_signers.contains(&from));
        assert!(compiled.required_signers.contains(&owner));
    }

    fn config_of(compiled: &CompiledTx) -> TransactionConfig {
        v1::deserialize(&compiled.message[1..]).unwrap().config
    }

    #[test]
    fn limits_go_in_the_header_and_never_default_to_zero() {
        let from = pk(1);
        let two = vec![transfer(from, pk(2), 1_000), transfer(from, pk(4), 2_000)];

        let blank = compile_v1(from, two.clone(), None, Some(blockhash(1))).unwrap();
        assert_eq!(
            config_of(&blank),
            TransactionConfig::empty()
                .with_compute_unit_limit(2 * DEFAULT_CU_PER_INSTRUCTION)
                .with_loaded_accounts_data_size_limit(DEFAULT_LOADED_ACCOUNTS_DATA_SIZE)
        );

        let set = compile_v1_budget(
            from,
            two,
            Some(nonce(from)),
            None,
            ComputeBudgetSpec {
                cu_limit: Some(300_000),
                cu_price_micro_lamports: Some(1_001),
                loaded_accounts_data_size: Some(4_096),
            },
        )
        .unwrap();
        assert_eq!(
            config_of(&set),
            TransactionConfig::empty()
                .with_compute_unit_limit(300_000)
                .with_loaded_accounts_data_size_limit(4_096)
                // 300,000 units at 1,001 micro-lamports is 300.3 lamports.
                .with_priority_fee(301)
        );
        // Nothing is added besides the nonce advance.
        let decoded = decode_v1_transaction(&set.wire).unwrap();
        let programs: Vec<Pubkey> = decoded
            .instructions
            .iter()
            .map(|ix| ix.program_id)
            .collect();
        assert_eq!(programs, vec![SYSTEM_PROGRAM_ID; 3]);
        assert!(decoded.uses_durable_nonce);
    }

    #[test]
    fn the_default_limit_stops_at_the_most_a_transaction_may_ask_for() {
        let from = pk(1);
        let many = (0..10).map(|n| transfer(from, pk(20 + n), 1)).collect();
        let compiled = compile_v1(from, many, None, Some(blockhash(1))).unwrap();
        assert_eq!(config_of(&compiled).compute_unit_limit, Some(MAX_CU_LIMIT));
    }
}
