//! Instructions that hand over control of something, however well they read.
//!
//! Reading a transaction says what it does. It does not say that what it does
//! cannot be taken back: a site drains a wallet as readily by taking the authority
//! of a token account as by moving its tokens, and that reads as plainly as a
//! transfer. So these are told apart by program and instruction, from the bytes the
//! program will read. No IDL comes into it: the IDLs that ship with the app are
//! pinned by checksum across devices, and one from the chain says what its author
//! wants said.
//!
//! Saying yes to what the transaction does not hold is on the list too: see
//! [`super::blind`].

use crate::idl::catalog::IdlOrigin;
use crate::ids::{STAKE_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::review::blind::{blind_in, blind_in_carried, CHECKED_APPROVAL};
use crate::review::checked::checked_in;
use crate::review::explain::{ExplainedIx, ExplainedTx};
use solana_pubkey::Pubkey;

const UPGRADEABLE_LOADER_ID: Pubkey =
    Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
const VOTE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Vote111111111111111111111111111111111111111");

const TOKEN_APPROVAL: &str =
    "Lets another address spend tokens from an account of yours, until you revoke it.";
const TOKEN_AUTHORITY: &str = "Hands over the authority of a token account or mint.";
const SYSTEM_ASSIGN: &str =
    "Gives an account to a program, which from then on controls it and all it holds.";
const NONCE_AUTHORITY: &str = "Hands over who can use a durable nonce.";
const STAKE_AUTHORITY: &str = "Hands over the authority of a stake account.";
const STAKE_LOCKUP: &str = "Changes the lockup of a stake account, and who can change it.";
const VOTE_AUTHORITY: &str =
    "Hands over who can vote with a vote account, or who can withdraw from it.";
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
    if *program == VOTE_PROGRAM_ID {
        // Authorize, and its checked and seeded forms.
        return match tag()? {
            1 | 7 | 10 | 11 => Some(VOTE_AUTHORITY),
            _ => None,
        };
    }
    if *program == UPGRADEABLE_LOADER_ID {
        // Upgrade, set authority, close, set authority checked, and the
        // move to loader v4.
        return match tag()? {
            3 | 4 | 5 | 7 | 8 => Some(PROGRAM_CONTROL),
            _ => None,
        };
    }
    None
}

/// What `explained` would do that hands over control, each thing once, at any
/// depth the app could open. A call a program makes by itself shows only when the
/// transaction runs.
///
/// A vote the clear signing program checks is not blind, but it is still a
/// vote: it is told as one, and what the proposal holds is told as for any
/// other instruction.
pub fn risks_in(explained: &ExplainedTx) -> Vec<&'static str> {
    let mut found = Vec::new();
    for instruction in &explained.instructions {
        collect(instruction, &mut found);
    }
    if !checked_in(explained).is_empty() {
        found.push(CHECKED_APPROVAL);
    }
    told_once(&mut found, blind_in(explained).iter().map(|one| one.what));
    found
}

/// [`risks_in`] for instructions carried for another transaction, such as what
/// a proposal stores. A check among them pins nothing: see
/// [`super::blind::blind_in_carried`].
pub fn risks_in_carried(instructions: &[ExplainedIx]) -> Vec<&'static str> {
    let mut found = Vec::new();
    for instruction in instructions {
        collect(instruction, &mut found);
    }
    told_once(
        &mut found,
        blind_in_carried(instructions).iter().map(|one| one.what),
    );
    found
}

/// What of [`risks_in`] a screen tells as a warning. What counts, for the
/// request modal's Sign, is [`risks_in`]: this only leaves out what the
/// instruction rows already say. A checked vote is never a strip of its own:
/// what the proposal holds is opened under its check. A program upgrade, or
/// a change of who can upgrade it, is not one either while the loader's
/// shipped IDL says it in a sentence; read raw, or worded by another file,
/// it is told like any other.
pub fn risks_told(explained: &ExplainedTx) -> Vec<&'static str> {
    told(risks_in(explained), &explained.instructions)
}

/// [`risks_told`] for instructions carried for another transaction.
pub fn risks_told_carried(instructions: &[ExplainedIx]) -> Vec<&'static str> {
    told(risks_in_carried(instructions), instructions)
}

fn told(mut risks: Vec<&'static str>, instructions: &[ExplainedIx]) -> Vec<&'static str> {
    let said = program_control_is_said(instructions);
    risks.retain(|risk| *risk != CHECKED_APPROVAL && (*risk != PROGRAM_CONTROL || !said));
    risks
}

/// Whether every instruction that changes a program, at any depth, is worded
/// by the IDL that ships with the app.
fn program_control_is_said(instructions: &[ExplainedIx]) -> bool {
    instructions.iter().all(|instruction| {
        let said = risk_of(&instruction.program_id, &instruction.data) != Some(PROGRAM_CONTROL)
            || instruction
                .idl
                .as_ref()
                .is_some_and(|idl| idl.origin == IdlOrigin::Builtin);
        said && instruction
            .inner
            .as_ref()
            .is_none_or(|inner| program_control_is_said(&inner.instructions))
    })
}

fn told_once(found: &mut Vec<&'static str>, more: impl Iterator<Item = &'static str>) {
    for what in more {
        if !found.contains(&what) {
            found.push(what);
        }
    }
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
        for tag in [3, 4, 5, 7, 8] {
            assert_eq!(of(UPGRADEABLE_LOADER_ID, tag), [PROGRAM_CONTROL]);
        }
        assert!(of(UPGRADEABLE_LOADER_ID, 1).is_empty());
        // Authorize, authorize checked, and both with a seed.
        for tag in [1, 7, 10, 11] {
            assert_eq!(of(VOTE_PROGRAM_ID, tag), [VOTE_AUTHORITY]);
        }
        // Vote, withdraw, a new commission and a tower sync.
        for harmless in [2, 3, 5, 14] {
            assert!(of(VOTE_PROGRAM_ID, harmless).is_empty());
        }
    }

    /// The loader's shipped IDL says an upgrade in a sentence, so it gets no
    /// strip of its own. Read raw, or worded by a file that did not ship,
    /// nothing trustworthy says it, and the strip is told.
    #[test]
    fn a_program_change_is_told_unless_the_shipped_idl_words_it() {
        use crate::review::explain::IdlExplanation;
        let worded_by = |origin: Option<IdlOrigin>| {
            let mut explained = one(UPGRADEABLE_LOADER_ID, tagged(3));
            explained.instructions[0].idl = origin.map(|origin| IdlExplanation {
                idl_sha256: [0; 32],
                origin,
                intent: "Upgrade".into(),
                interpolated_intent: None,
                spans: None,
                fields: Vec::new(),
                accounts: Vec::new(),
                accounts_needed: Vec::new(),
                unnamed_accounts: 0,
                undescribed_bytes: Vec::new(),
            });
            explained
        };
        let shipped = worded_by(Some(IdlOrigin::Builtin));
        assert_eq!(risks_in(&shipped), [PROGRAM_CONTROL]);
        assert!(risks_told(&shipped).is_empty());
        assert!(risks_told_carried(&shipped.instructions).is_empty());
        for other in [
            None,
            Some(IdlOrigin::File),
            Some(IdlOrigin::ProgramMetadata),
            Some(IdlOrigin::Anchor),
        ] {
            let explained = worded_by(other);
            assert_eq!(risks_told(&explained), [PROGRAM_CONTROL], "{other:?}");
            assert_eq!(
                risks_told_carried(&explained.instructions),
                [PROGRAM_CONTROL],
                "{other:?}"
            );
        }
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

    #[test]
    fn an_upgrade_a_checked_vote_pins_is_told_and_the_vote_is_not_blind() {
        use crate::fixtures::{compile, pk, transfer};
        use crate::idl::catalog::{IdlStore, MemoryBodies};
        use crate::review::explain::{explain_instructions, explain_transaction};
        use crate::tx::decode::decode_v1_transaction;
        use crate::wallets::squads::{CheckForm, ProposalCheck, Squad, VoteAction};
        use solana_instruction::{AccountMeta, Instruction};
        use wallet_vault::store::MemoryStore;

        let squad = Squad::v4(&pk(50));
        let vault = squad.vault(0);
        let upgrade = Instruction {
            program_id: UPGRADEABLE_LOADER_ID,
            accounts: [pk(2), pk(3), pk(4), pk(5), pk(6), pk(7)]
                .into_iter()
                .map(|key| AccountMeta::new(key, false))
                .chain([AccountMeta::new_readonly(vault, true)])
                .collect(),
            data: tagged(3),
        };
        let check = ProposalCheck {
            form: CheckForm::TopLevel,
            squad,
            transaction_index: 4,
            vault_index: 0,
            vault,
            ephemeral_signers: 0,
            member: pk(1),
            action: VoteAction::Approve,
            instructions: vec![transfer(vault, pk(8), 5), upgrade],
        };
        let pair = vec![check.instruction(&[]), squad.approve(4, &pk(1))];
        // With the IDLs that ship, the loader's among them.
        let catalog = crate::fixtures::shipped_catalog(crate::IdlNetwork::Devnet);
        let tx = decode_v1_transaction(&compile(pk(1), pair.clone()).wire).unwrap();
        assert_eq!(
            risks_in(&explain_transaction(&catalog, &tx)),
            [PROGRAM_CONTROL, CHECKED_APPROVAL]
        );
        // Carried for another transaction, the same pair pins nothing.
        let carried = explain_instructions(&catalog, &pair);
        assert!(risks_in_carried(&carried).contains(&PROGRAM_CONTROL));
        assert!(!risks_in_carried(&carried).contains(&CHECKED_APPROVAL));
        assert_eq!(risks_in_carried(&carried).len(), 2);

        // Counted, but told by the rows alone: no strip says either. The
        // carried vote is still blind, and that is told.
        assert!(risks_told(&explain_transaction(&catalog, &tx)).is_empty());
        assert_eq!(risks_told_carried(&carried).len(), 1);
        assert!(!risks_told_carried(&carried).contains(&PROGRAM_CONTROL));

        // Without the loader's IDL the upgrade reads raw, no row says it, and
        // the strip does.
        let bare = IdlStore::open(MemoryStore::new(), MemoryBodies::new());
        assert_eq!(
            risks_told(&explain_transaction(&bare, &tx)),
            [PROGRAM_CONTROL]
        );
        assert!(risks_told_carried(&explain_instructions(&bare, &pair)).contains(&PROGRAM_CONTROL));
    }

    #[test]
    fn every_strip_is_told_for_an_instruction_read_raw() {
        let of = |program, tag| one(program, tagged(tag));
        // A program change too: only the shipped IDL's sentence stands in
        // for its strip.
        for tag in [3, 4, 5, 7, 8] {
            let explained = of(UPGRADEABLE_LOADER_ID, tag);
            assert_eq!(risks_in(&explained), [PROGRAM_CONTROL]);
            assert_eq!(risks_told(&explained), [PROGRAM_CONTROL]);
        }
        assert_eq!(risks_told(&of(VOTE_PROGRAM_ID, 1)), [VOTE_AUTHORITY]);
        for (program, data, risk) in [
            (TOKEN_PROGRAM_ID, vec![4, 1], TOKEN_APPROVAL),
            (TOKEN_PROGRAM_ID, vec![6, 1], TOKEN_AUTHORITY),
            (SYSTEM_PROGRAM_ID, tagged(1), SYSTEM_ASSIGN),
            (SYSTEM_PROGRAM_ID, tagged(7), NONCE_AUTHORITY),
            (STAKE_PROGRAM_ID, tagged(1), STAKE_AUTHORITY),
            (STAKE_PROGRAM_ID, tagged(6), STAKE_LOCKUP),
        ] {
            assert_eq!(risks_told(&one(program, data)), [risk]);
        }
    }
}
