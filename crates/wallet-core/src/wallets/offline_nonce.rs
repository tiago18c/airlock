//! The durable nonce an offline key signs its own transactions on.
//!
//! A transaction on a recent blockhash is accepted for about a minute, which
//! the trip to an offline device and back can outlast. One that advances a
//! system nonce account first lives on that account's value instead, until
//! it lands. The account is made per offline key, with the key as its
//! authority, so only that key can advance it.
//!
//! Deprecated once the programmatic signer or Vector is on mainnet: either
//! one lets an offline key act without a lifetime to race.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_system_interface::instruction::create_nonce_account_with_seed;

use crate::ids::SYSTEM_PROGRAM_ID;

/// Size of a system nonce account.
pub const DURABLE_NONCE_LEN: usize = 80;

/// The seed the nonce account of `key` is derived with, from the wallet
/// that pays for it.
pub fn durable_nonce_seed(key: &Pubkey) -> String {
    let text = key.to_string();
    format!("airlock-dn:{}", &text[..text.len().min(16)])
}

/// Instructions that create and initialize the durable nonce account of
/// `authority` at `payer` + its seed, so no key besides `payer` has to sign.
pub fn create_durable_nonce(
    payer: &Pubkey,
    authority: &Pubkey,
    rent_lamports: u64,
) -> Result<(Pubkey, Vec<Instruction>), String> {
    let seed = durable_nonce_seed(authority);
    let account = Pubkey::create_with_seed(payer, &seed, &SYSTEM_PROGRAM_ID)
        .map_err(|err| err.to_string())?;
    let instructions =
        create_nonce_account_with_seed(payer, &account, payer, &seed, authority, rent_lamports);
    Ok((account, instructions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::pk;

    #[test]
    fn the_account_is_derived_from_the_payer_and_the_key_is_its_authority() {
        let (account, instructions) = create_durable_nonce(&pk(1), &pk(2), 1_500_000).unwrap();
        let (again, _) = create_durable_nonce(&pk(1), &pk(2), 1_500_000).unwrap();
        let (other, _) = create_durable_nonce(&pk(1), &pk(3), 1_500_000).unwrap();
        assert_eq!(account, again);
        assert_ne!(account, other);
        assert_eq!(instructions.len(), 2);
        let initialize = &instructions[1];
        assert_eq!(initialize.accounts[0].pubkey, account);
        // InitializeNonceAccount(authority): tag 6, then the key.
        assert_eq!(initialize.data[..4], 6u32.to_le_bytes());
        assert_eq!(initialize.data[4..36], pk(2).to_bytes());
        assert!(durable_nonce_seed(&pk(2)).len() <= 32);
    }
}
