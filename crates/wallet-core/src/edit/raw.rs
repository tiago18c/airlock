//! An instruction written out by hand: a program, its accounts and its bytes.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

/// One account of a [`RawDraft`], with the flags its meta carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawAccountDraft {
    pub pubkey: Option<Pubkey>,
    pub signer: bool,
    pub writable: bool,
}

/// An instruction no stored IDL reads, or one the user asked to write by
/// hand. The data is kept as the hex that was typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawDraft {
    pub program_id: Option<Pubkey>,
    pub accounts: Vec<RawAccountDraft>,
    pub data: String,
}

impl RawDraft {
    pub fn from_instruction(instruction: &Instruction) -> Self {
        Self {
            program_id: Some(instruction.program_id),
            accounts: instruction
                .accounts
                .iter()
                .map(|meta| RawAccountDraft {
                    pubkey: Some(meta.pubkey),
                    signer: meta.is_signer,
                    writable: meta.is_writable,
                })
                .collect(),
            data: hex::encode(&instruction.data),
        }
    }

    pub fn build(&self) -> Result<Instruction, String> {
        let program_id = self.program_id.ok_or("Choose a program.")?;
        let accounts = self
            .accounts
            .iter()
            .enumerate()
            .map(|(i, account)| {
                let pubkey = account
                    .pubkey
                    .ok_or_else(|| format!("Pick account {}.", i + 1))?;
                Ok(AccountMeta {
                    pubkey,
                    is_signer: account.signer,
                    is_writable: account.writable,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let text: String = self.data.split_whitespace().collect();
        let data = hex::decode(text.trim_start_matches("0x"))
            .map_err(|_| "The data is not hex.".to_string())?;
        Ok(Instruction {
            program_id,
            accounts,
            data,
        })
    }
}
