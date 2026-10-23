//! The executor nonce account that orders a signer's actions.

use super::*;

/// An executor nonce account as last read from the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutorNonce {
    pub account: Pubkey,
    /// Must sign for the nonce to advance. Usually a programmatic signer address.
    pub authority: Pubkey,
    pub value: Hash,
}

impl ExecutorNonce {
    /// Read the data of an account owned by the nonce program.
    pub fn parse(account: Pubkey, data: &[u8]) -> Option<Self> {
        if data.len() != EXECUTOR_NONCE_LEN || data.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self {
            account,
            authority: Pubkey::new_from_array(data[32..64].try_into().ok()?),
            value: Hash::new_from_array(data[..32].try_into().ok()?),
        })
    }
}

/// Instructions that create and initialize an executor nonce account at
/// `base` + `seed`, so no key besides `payer` and `base` has to sign.
pub fn create_executor_nonce(
    payer: &Pubkey,
    base: &Pubkey,
    seed: &str,
    authority: &Pubkey,
    rent_lamports: u64,
) -> Result<(Pubkey, [Instruction; 2]), ProgrammaticError> {
    let account = Pubkey::create_with_seed(base, seed, &EXECUTOR_NONCE_PROGRAM_ID)
        .map_err(|err| ProgrammaticError::Message(err.to_string()))?;
    let create = solana_system_interface::instruction::create_account_with_seed(
        payer,
        &account,
        base,
        seed,
        rent_lamports,
        EXECUTOR_NONCE_LEN as u64,
        &EXECUTOR_NONCE_PROGRAM_ID,
    );
    let initialize = Instruction {
        program_id: EXECUTOR_NONCE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(account, false),
            AccountMeta::new_readonly(*authority, false),
            AccountMeta::new_readonly(SLOT_HASHES_SYSVAR_ID, false),
        ],
        data: vec![NONCE_INITIALIZE_TAG],
    };
    Ok((account, [create, initialize]))
}
