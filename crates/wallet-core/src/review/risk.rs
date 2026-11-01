//! Instructions that hand over control of something, however well they read.
//!
//! Reading a transaction says what it does. It does not say that what it does
//! cannot be taken back: a site drains a wallet as readily by taking the authority
//! of a token account as by moving its tokens, and that reads as plainly as a
//! transfer. So these are told apart by program and instruction, from the bytes the
//! program will read. No IDL comes into it: the IDLs that ship with the app are
//! pinned by checksum across devices, and one from the chain says what its author
//! wants said.

use crate::ids::{STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::review::explain::{ExplainedIx, ExplainedTx};
use solana_pubkey::Pubkey;

const UPGRADEABLE_LOADER_ID: Pubkey =
    Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");

const TOKEN_APPROVAL: &str =
    "Lets another address spend tokens from an account of yours, until you revoke it.";
const TOKEN_AUTHORITY: &str = "Hands over the authority of a token account or mint.";
const SYSTEM_ASSIGN: &str =
    "Gives an account to a program, which from then on controls it and all it holds.";
const NONCE_AUTHORITY: &str = "Hands over who can use a durable nonce.";
const STAKE_AUTHORITY: &str = "Hands over the authority of a stake account.";
const STAKE_LOCKUP: &str = "Changes the lockup of a stake account, and who can change it.";
const PROGRAM_CONTROL: &str = "Changes a program, or who can upgrade it.";

/// What is risky about one instruction, if anything.
fn risk_of(program: &Pubkey, data: &[u8]) -> Option<&'static str> {
    // The system, stake and loader programs start with a little-endian u32. The
    // token programs start with one byte.
    let tag = || Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?));
    if *program == TOKEN_PROGRAM_ID || *program == TOKEN_2022_PROGRAM_ID {
        return match data.first()? {
            4 | 13 => Some(TOKEN_APPROVAL),
            6 => Some(TOKEN_AUTHORITY),
            _ => None,
        };
    }
    if *program == SYSTEM_PROGRAM_ID {
        return match tag()? {
            1 | 10 => Some(SYSTEM_ASSIGN),
            7 => Some(NONCE_AUTHORITY),
            _ => None,
        };
    }
    if *program == STAKE_PROGRAM_ID {
        return match tag()? {
            1 | 8 | 10 | 11 => Some(STAKE_AUTHORITY),
            6 | 12 => Some(STAKE_LOCKUP),
            _ => None,
        };
    }
    if *program == UPGRADEABLE_LOADER_ID {
        return match tag()? {
            3 | 4 | 5 | 7 => Some(PROGRAM_CONTROL),
            _ => None,
        };
    }
    None
}

/// What `explained` would do that hands over control, each thing once, at any
/// depth the app could open. A call a program makes by itself shows only when the
/// transaction runs.
pub fn risks_in(explained: &ExplainedTx) -> Vec<&'static str> {
    let mut found = Vec::new();
    for instruction in &explained.instructions {
        collect(instruction, &mut found);
    }
    found
}

fn collect(instruction: &ExplainedIx, found: &mut Vec<&'static str>) {
    if let Some(risk) = risk_of(&instruction.program_id, &instruction.data) {
        if !found.contains(&risk) {
            found.push(risk);
        }
    }
    if let Some(inner) = &instruction.inner {
        for instruction in &inner.instructions {
            collect(instruction, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(program: Pubkey, data: Vec<u8>) -> ExplainedTx {
        ExplainedTx {
            programs: Vec::new(),
            instructions: vec![ExplainedIx {
                program_id: program,
                accounts: Vec::new(),
                data,
                idl: None,
                inner: None,
            }],
        }
    }

    fn tagged(tag: u32) -> Vec<u8> {
        tag.to_le_bytes().to_vec()
    }

    #[test]
    fn handing_over_a_token_account_is_told_on_either_token_program() {
        for program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
            assert_eq!(risks_in(&one(program, vec![4, 1])), [TOKEN_APPROVAL]);
            assert_eq!(risks_in(&one(program, vec![13, 1])), [TOKEN_APPROVAL]);
            assert_eq!(risks_in(&one(program, vec![6, 1])), [TOKEN_AUTHORITY]);
            // Transfer, transfer checked, close, and revoke.
            for harmless in [3, 12, 9, 5] {
                assert!(risks_in(&one(program, vec![harmless])).is_empty());
            }
        }
    }

    #[test]
    fn the_native_programs_are_told_by_their_instruction() {
        let of = |program, tag| risks_in(&one(program, tagged(tag)));
        assert_eq!(of(SYSTEM_PROGRAM_ID, 1), [SYSTEM_ASSIGN]);
        assert_eq!(of(SYSTEM_PROGRAM_ID, 10), [SYSTEM_ASSIGN]);
        assert_eq!(of(SYSTEM_PROGRAM_ID, 7), [NONCE_AUTHORITY]);
        // Create, transfer and advance a nonce.
        for harmless in [0, 2, 4] {
            assert!(of(SYSTEM_PROGRAM_ID, harmless).is_empty());
        }
        for tag in [1, 8, 10, 11] {
            assert_eq!(of(STAKE_PROGRAM_ID, tag), [STAKE_AUTHORITY]);
        }
        for tag in [6, 12] {
            assert_eq!(of(STAKE_PROGRAM_ID, tag), [STAKE_LOCKUP]);
        }
        // Delegate, withdraw and deactivate.
        for harmless in [2, 4, 5] {
            assert!(of(STAKE_PROGRAM_ID, harmless).is_empty());
        }
        for tag in [3, 4, 5, 7] {
            assert_eq!(of(UPGRADEABLE_LOADER_ID, tag), [PROGRAM_CONTROL]);
        }
        assert!(of(UPGRADEABLE_LOADER_ID, 1).is_empty());
    }

    #[test]
    fn a_tag_too_short_to_read_is_not_a_risk_and_does_not_panic() {
        assert!(risks_in(&one(SYSTEM_PROGRAM_ID, vec![1])).is_empty());
        assert!(risks_in(&one(TOKEN_PROGRAM_ID, Vec::new())).is_empty());
    }

    #[test]
    fn another_program_using_the_same_numbers_is_not_these() {
        let other = Pubkey::new_from_array([9; 32]);
        assert!(risks_in(&one(other, vec![6, 1])).is_empty());
        assert!(risks_in(&one(other, tagged(1))).is_empty());
    }

    #[test]
    fn what_an_instruction_carries_is_read_too_and_each_thing_is_said_once() {
        let approval = |inner| ExplainedIx {
            program_id: TOKEN_PROGRAM_ID,
            accounts: Vec::new(),
            data: vec![4],
            idl: None,
            inner,
        };
        let carrying = ExplainedIx {
            program_id: Pubkey::new_from_array([9; 32]),
            accounts: Vec::new(),
            data: Vec::new(),
            idl: None,
            inner: Some(crate::review::explain::InnerMessage {
                kind: crate::review::explain::InnerKind::Authorization {
                    authorities: Vec::new(),
                    forwarded: Vec::new(),
                },
                instructions: vec![approval(None)],
            }),
        };
        let explained = ExplainedTx {
            programs: Vec::new(),
            instructions: vec![carrying, approval(None)],
        };
        assert_eq!(risks_in(&explained), [TOKEN_APPROVAL]);
    }
}
