//! Squads multisigs: addresses and the instructions of a proposal's life.
//!
//! Two programs are covered. They share a design and differ in names, seeds
//! and a few accounts:
//!
//! - **v4** (`SQDS4ep…`): a *multisig* account, created from a throwaway key.
//! - **Smart account** (`SMRTzfY…`): a *settings* account, numbered by a
//!   counter in the program's own config.
//!
//! In both, the account that holds the funds (the *vault*) is a separate
//! address derived from the multisig. Spending from it takes four steps:
//! store the transaction, open a proposal for it, approve until the threshold
//! is met, execute.
//!
//! Every instruction here is built, and every account read, from the
//! program's own public IDL, converted to Codama (`idls/squads-multisig.json`
//! and `idls/squads-smart-account.json`, see `builtin`), through the dynamic
//! client. Three things are not in those IDLs and are written out here:
//!
//! - the seeds of the derived addresses, which an Anchor 0.29 IDL does not
//!   carry;
//! - the layout of the stored message, which the IDL passes as plain `bytes`;
//! - the accounts an execution lists after its own, which are the accounts
//!   of the stored message and so differ for every transaction.

use std::sync::OnceLock;

use codama_dynamic_client::{ProgramClient, Value};
use serde::{Deserialize, Serialize};
use solana_instruction::{AccountMeta, Instruction};
use solana_message::legacy::Message as LegacyMessage;
use solana_pubkey::Pubkey;

use crate::idl::builtin::builtin_idl;
use crate::idl::network::IdlNetwork;
use crate::ids::SYSTEM_PROGRAM_ID;

pub use crate::ids::{SQUADS_SMART_ACCOUNT_PROGRAM_ID, SQUADS_V4_PROGRAM_ID};

mod instructions;
mod message;
mod program;
mod state;
mod values;

#[cfg(test)]
mod tests;

pub use message::*;
pub use program::*;
pub use state::*;
use values::*;

/// One multisig: the program it lives in and its config account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Squad {
    pub program: SquadsProgram,
    /// The multisig account (v4) or the settings account (smart account).
    pub address: Pubkey,
}

impl Squad {
    /// A v4 multisig is addressed by the key that signed its creation.
    pub fn v4(create_key: &Pubkey) -> Self {
        let program = SquadsProgram::V4;
        Self {
            program,
            address: program.address(&[program.prefix(), b"multisig", create_key.as_ref()]),
        }
    }

    /// A smart account is addressed by its number. See
    /// [`SquadsProgram::smart_account_count`].
    pub fn smart_account(number: u128) -> Self {
        let program = SquadsProgram::SmartAccount;
        Self {
            program,
            address: program.address(&[program.prefix(), b"settings", &number.to_le_bytes()]),
        }
    }

    /// Where the funds are. This is the address to send to and to spend from.
    pub fn vault(&self, index: u8) -> Pubkey {
        let seed: &[u8] = match self.program {
            SquadsProgram::V4 => b"vault",
            SquadsProgram::SmartAccount => b"smart_account",
        };
        self.program
            .address(&[self.program.prefix(), self.address.as_ref(), seed, &[index]])
    }

    pub fn transaction(&self, index: u64) -> Pubkey {
        self.program.address(&[
            self.program.prefix(),
            self.address.as_ref(),
            b"transaction",
            &index.to_le_bytes(),
        ])
    }

    pub fn proposal(&self, index: u64) -> Pubkey {
        self.program.address(&[
            self.program.prefix(),
            self.address.as_ref(),
            b"transaction",
            &index.to_le_bytes(),
            b"proposal",
        ])
    }

    /// What the IDL calls the multisig in the instructions about a proposal.
    fn owner_account(&self) -> &'static str {
        match self.program {
            SquadsProgram::V4 => "multisig",
            SquadsProgram::SmartAccount => "consensusAccount",
        }
    }
}
