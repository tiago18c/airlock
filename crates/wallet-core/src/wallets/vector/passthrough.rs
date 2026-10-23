//! A `Passthrough` instruction, read back into the instructions it carries.
//!
//! ```text
//! accounts   the instructions sysvar, one Vector account per signer, then
//!            for each instruction: its program, its accounts
//! data       1
//!            signers u8
//!            for each signer what it signs as: 0 the account, 1 its system PDA
//!            count u8
//!            for each instruction: accounts u8, length u16 LE, data
//! ```
//!
//! Nothing in it depends on a signer's scheme, so it reads the same with or
//! without knowing the accounts. A hash-based key's next key is in the
//! `Advance`, not here.

use super::instructions::{check_inner, MAX_PASSTHROUGH_SIGNERS};
use super::{VectorError, PROGRAM_ID};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

/// The first byte of a `Passthrough`'s data.
pub const PASSTHROUGH_DISCRIMINATOR: u8 = 1;

/// One account a `Passthrough` runs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassthroughSigner {
    /// The Vector account, which the `Advance` lists.
    pub account: Pubkey,
    /// It signs as its system PDA, for what only a System account can do.
    pub system: bool,
}

impl PassthroughSigner {
    /// The address that signs the instructions: the account, or its system
    /// PDA.
    pub fn address(&self) -> Pubkey {
        if self.system {
            system_pda(&self.account)
        } else {
            self.account
        }
    }
}

/// The system PDA of the Vector account `account`: owned by the System
/// program, and signed for by a `Passthrough` that asks for it.
pub fn system_pda(account: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[account.as_ref()], &PROGRAM_ID).0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passthrough {
    /// The accounts the instructions run as, in the order they are listed.
    pub signers: Vec<PassthroughSigner>,
    /// What runs, in order, each signed for by the signers it names.
    pub instructions: Vec<Instruction>,
}

/// Read `instruction` as a `Passthrough`. The caller has checked that it is
/// an instruction of the Vector program.
pub fn parse_passthrough(instruction: &Instruction) -> Result<Passthrough, VectorError> {
    let malformed = VectorError::MalformedPassthrough;
    let [PASSTHROUGH_DISCRIMINATOR, data @ ..] = instruction.data.as_slice() else {
        return Err(VectorError::NotPassthrough);
    };
    let [_sysvar, accounts @ ..] = instruction.accounts.as_slice() else {
        return Err(malformed("it lists no accounts"));
    };
    let mut data = Bytes(data);
    let count = usize::from(data.take(1)?[0]);
    if count == 0 {
        return Err(malformed("it names no account to run as"));
    }
    let flags = data.take(count)?;
    let (listed, mut accounts) = accounts
        .split_at_checked(count)
        .ok_or(malformed("it lists fewer accounts than it runs as"))?;
    let signers = listed
        .iter()
        .zip(flags)
        .map(|(meta, flag)| match flag {
            0 | 1 => Ok(PassthroughSigner {
                account: meta.pubkey,
                system: *flag == 1,
            }),
            _ => Err(malformed(
                "a signer is neither the account nor its system PDA",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let signing: Vec<Pubkey> = signers.iter().map(PassthroughSigner::address).collect();

    let mut instructions = Vec::new();
    for _ in 0..data.take(1)?[0] {
        let count = usize::from(data.take(1)?[0]);
        let len = data.take(2)?;
        let len = usize::from(u16::from_le_bytes([len[0], len[1]]));
        let body = data.take(len)?;
        let ([program, metas @ ..], rest) = accounts.split_at_checked(1 + count).ok_or(
            malformed("it lists fewer accounts than its instructions use"),
        )?
        else {
            return Err(malformed("an instruction has no program"));
        };
        accounts = rest;
        instructions.push(Instruction {
            program_id: program.pubkey,
            accounts: metas
                .iter()
                .map(|meta| AccountMeta {
                    pubkey: meta.pubkey,
                    // The program signs as each signer wherever it appears.
                    is_signer: meta.is_signer || signing.contains(&meta.pubkey),
                    is_writable: meta.is_writable,
                })
                .collect(),
            data: body.to_vec(),
        });
    }
    // The program refuses bytes or accounts nothing uses, so a reading that
    // leaves some over is not what would run.
    if !data.0.is_empty() {
        return Err(malformed("bytes follow its last instruction"));
    }
    if !accounts.is_empty() {
        return Err(malformed("it lists accounts no instruction uses"));
    }
    Ok(Passthrough {
        signers,
        instructions,
    })
}

/// `passthrough`, carrying `instructions` in place of the ones it carries:
/// the same accounts to run as, each signing as it did. Nothing in it
/// depends on a signer's scheme or key, so it is written from the
/// instruction alone.
pub fn rewrite_passthrough(
    passthrough: &Instruction,
    instructions: &[Instruction],
) -> Result<Instruction, VectorError> {
    let parsed = parse_passthrough(passthrough)?;
    let count = parsed.signers.len();
    if count > MAX_PASSTHROUGH_SIGNERS {
        return Err(VectorError::TooManyPassthroughSigners(count));
    }
    check_inner(count, instructions)?;
    let signing: Vec<Pubkey> = parsed
        .signers
        .iter()
        .map(PassthroughSigner::address)
        .collect();
    // The instructions sysvar and the accounts it runs as, then the flags
    // that say what each signs as, stay as they were.
    let mut accounts = passthrough.accounts[..1 + count].to_vec();
    let mut data = passthrough.data[..2 + count].to_vec();
    for instruction in instructions {
        accounts.push(AccountMeta::new_readonly(instruction.program_id, false));
        accounts.extend(instruction.accounts.iter().map(|meta| AccountMeta {
            // The program gives the signers their signature during CPI.
            is_signer: meta.is_signer && !signing.contains(&meta.pubkey),
            ..meta.clone()
        }));
    }
    data.push(instructions.len() as u8);
    for instruction in instructions {
        data.push(instruction.accounts.len() as u8);
        data.extend_from_slice(&(instruction.data.len() as u16).to_le_bytes());
        data.extend_from_slice(&instruction.data);
    }
    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data,
    })
}

struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], VectorError> {
        let (taken, rest) = self
            .0
            .split_at_checked(len)
            .ok_or(VectorError::MalformedPassthrough("it ends early"))?;
        self.0 = rest;
        Ok(taken)
    }
}
