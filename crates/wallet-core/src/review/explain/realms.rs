//! What a Realms `InsertTransaction` adds to a proposal: the instructions the
//! governance runs, signing as itself or its treasury, once the proposal
//! passes. Read from the instruction's own bytes in spl-governance's layout,
//! so it does not wait on the IDL pack.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

/// The SPL Governance program Realms runs, on mainnet and devnet.
const REALMS_GOVERNANCE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("GovER5Lthms3bLBqWub97yVrMmEogzX7xNjdXpPPCVZw");

const INSERT_TRANSACTION: u8 = 9;

/// A transaction added to a proposal, read back from the instruction that
/// adds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProposalTransaction {
    pub governance: Pubkey,
    pub proposal: Pubkey,
    pub option_index: u8,
    pub transaction_index: u16,
    pub instructions: Vec<Instruction>,
}

impl ProposalTransaction {
    /// `None` when `instruction` is not a well-formed `InsertTransaction`.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        if instruction.program_id != REALMS_GOVERNANCE_PROGRAM_ID {
            return None;
        }
        let mut data = Reader(&instruction.data);
        if data.u8()? != INSERT_TRANSACTION {
            return None;
        }
        let option_index = data.u8()?;
        let transaction_index = u16::from_le_bytes(data.array()?);
        // The legacy hold-up time, which the program ignores.
        data.array::<4>()?;
        let count = data.count()?;
        let mut instructions = Vec::new();
        for _ in 0..count {
            let program_id = data.pubkey()?;
            let accounts = (0..data.count()?)
                .map(|_| {
                    Some(AccountMeta {
                        pubkey: data.pubkey()?,
                        is_signer: data.bool()?,
                        is_writable: data.bool()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let length = data.count()?;
            instructions.push(Instruction {
                program_id,
                accounts,
                data: data.take(length)?.to_vec(),
            });
        }
        if !data.0.is_empty() {
            return None;
        }
        Some(Self {
            governance: instruction.accounts.first()?.pubkey,
            proposal: instruction.accounts.get(1)?.pubkey,
            option_index,
            transaction_index,
            instructions,
        })
    }
}

/// Borsh, as far as these instructions need it.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn pubkey(&mut self) -> Option<Pubkey> {
        Some(Pubkey::new_from_array(self.array()?))
    }

    /// A `u32` length prefix, which can be no more than the bytes left: every
    /// item it counts takes at least one.
    fn count(&mut self) -> Option<usize> {
        let n = u32::from_le_bytes(self.array()?) as usize;
        (n <= self.0.len()).then_some(n)
    }
}
