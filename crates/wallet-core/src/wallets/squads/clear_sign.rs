//! The Squads clear signing program's checks, read from their own bytes.
//!
//! A v4 vote signs only the address of a proposal. `verifyProposal`, put in
//! the same transaction as the vote, carries every instruction the proposal's
//! stored transaction holds, and the program aborts the transaction unless the
//! stored one is exactly that. So the instructions the vote says yes to are in
//! the bytes being signed, and a device with no network can show them.
//! `verifyProposalCpi` is the same check for a member that signs through a
//! program, run inside what that program runs. `verifyBufferHash` does the
//! same for what a program upgrade deploys: the hash of the buffer's build.
//!
//! These are read here byte by byte in the program's Borsh layout, not through
//! the IDL, so they open on any network and before any IDL arrives. Nothing is
//! taken that the program would not read the same way: no byte is left over,
//! a flag is 0 or 1, and no count is more than a stored message can hold.
//!
//! # What the deployed program takes
//!
//! The program deployed on devnet takes a check in three shapes, one vote in
//! each:
//!
//! - a member that signs the transaction with its own key:
//!   `[compute budget*, verifyBufferHash*, verifyProposal, proposalApprove]`,
//!   nothing else, at the top of the transaction;
//! - a programmatic signer: `[verifyBufferHash*, verifyProposalCpi,
//!   proposalApprove]` as the whole execution message of a top-level
//!   `Submit`, the member being the signer's address. The rest of the
//!   transaction is not restricted;
//! - a Vector account, or its system PDA: the same list as the whole of a
//!   top-level `Passthrough`, beside the `Advance` and no other instruction
//!   of the Vector program.
//!
//! It refuses an execution of the proposal, a second member's vote and a
//! leading System `AdvanceNonceAccount` in any of them. What this app
//! knows of that is here, and only the offer and the builder consult it,
//! never the reader:
//!
//! - [`ClearSigningAccepts::DEPLOYED`], what the deployed program takes beside
//!   a check. Edit it when a new program ships.
//! - [`checks_for`]: several members are assumed to need one check each, with
//!   the same content, and the reader holds a check to the member it names
//!   ([`ProposalCheck::names`]). This is assumed of a program that takes
//!   several votes, which none does yet. Another form, one check naming a
//!   list of members, changes [`ProposalCheck::from_data`], `names` and
//!   `checks_for`, nothing else.
//! - [`place_checks`]: where the checks go among the instructions.
//! - [`moves_into_authorization`]: the checks for a member that signs
//!   through a program go inside its `Submit` or `Passthrough`, with the
//!   vote, to run there by CPI.
//!
//! What a transaction holding a check means does not depend on any of that:
//! the check's content diff holds whenever it runs (see
//! `crate::review::checked`).

use super::*;
use crate::error::BuildError;
use crate::idl::builtin::BUILTIN_IDLS;
use crate::ids::SQUADS_CLEAR_SIGNING_PROGRAM_ID;
use crate::program_code::BPF_LOADER_UPGRADEABLE_ID;
use crate::tx::builder::{compile_message, MAX_CU_LIMIT};
use crate::tx::decode::TxBudget;
use solana_hash::Hash;

/// Anchor's tag for `verifyProposal`.
pub const VERIFY_PROPOSAL: [u8; 8] = [0x36, 0x8c, 0x39, 0x46, 0xf3, 0x9d, 0x1e, 0x7c];
/// Anchor's tag for `verifyProposalCpi`.
pub const VERIFY_PROPOSAL_CPI: [u8; 8] = [0xdc, 0x96, 0x9b, 0xbf, 0x04, 0xfb, 0x8c, 0xa8];
/// Anchor's tag for `verifyBufferHash`.
pub const VERIFY_BUFFER_HASH: [u8; 8] = [0x45, 0x61, 0x0c, 0x8e, 0x75, 0xad, 0x7c, 0x1f];

/// The instructions sysvar `verifyProposal` reads the transaction from.
const INSTRUCTIONS_SYSVAR: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// The loader-v3 `Upgrade`, and where it names the program and the buffer.
pub(crate) const LOADER_UPGRADE: u32 = 3;
pub(crate) const UPGRADE_PROGRAM: usize = 1;
pub(crate) const UPGRADE_BUFFER: usize = 2;

/// A stored message counts its accounts and instructions in one byte each,
/// and an instruction's data in two, so a check that carries more cannot match
/// any proposal.
const MAX_INSTRUCTIONS: usize = u8::MAX as usize;
const MAX_ACCOUNTS: usize = u8::MAX as usize;
const MAX_DATA: usize = u16::MAX as usize;

/// The vote a check expects beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoteAction {
    Approve,
    Reject,
    Cancel,
    /// `proposalCancelV2`.
    CancelV2,
}

impl VoteAction {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Approve,
            1 => Self::Reject,
            2 => Self::Cancel,
            3 => Self::CancelV2,
            _ => return None,
        })
    }

    fn byte(self) -> u8 {
        match self {
            Self::Approve => 0,
            Self::Reject => 1,
            Self::Cancel => 2,
            Self::CancelV2 => 3,
        }
    }

    /// The vote in words, lower case: "approve".
    pub fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Cancel | Self::CancelV2 => "cancel",
        }
    }
}

/// Which of the two instructions a check is, which says where its vote is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckForm {
    /// `verifyProposal`, one of the transaction's own instructions. It looks
    /// for the vote among them.
    TopLevel,
    /// `verifyProposalCpi`, run by a top-level `Submit` of a programmatic
    /// signer or a Vector `Passthrough`. It looks for the vote in the list
    /// that runs it, and the member, which signs only there, signs it too.
    Cpi,
}

impl CheckForm {
    fn tag(self) -> [u8; 8] {
        match self {
            Self::TopLevel => VERIFY_PROPOSAL,
            Self::Cpi => VERIFY_PROPOSAL_CPI,
        }
    }

    fn from_tag(tag: [u8; 8]) -> Option<Self> {
        match tag {
            VERIFY_PROPOSAL => Some(Self::TopLevel),
            VERIFY_PROPOSAL_CPI => Some(Self::Cpi),
            _ => None,
        }
    }
}

/// A `verifyProposal` or `verifyProposalCpi`: what proposal
/// `transaction_index` of `squad` must store for the vote beside it to land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalCheck {
    pub form: CheckForm,
    /// Always a v4 multisig: the program reads only v4 accounts.
    pub squad: Squad,
    pub transaction_index: u64,
    pub vault_index: u8,
    /// The vault the instructions run as, derived from the multisig and the
    /// vault index, which the program holds to the stored transaction's.
    pub vault: Pubkey,
    pub ephemeral_signers: u8,
    /// The member whose vote this check expects.
    pub member: Pubkey,
    pub action: VoteAction,
    /// Every instruction the stored transaction holds, in order, with its
    /// accounts resolved and flagged as Squads flags them.
    pub instructions: Vec<Instruction>,
}

impl ProposalCheck {
    /// The check of `member`'s approval of `stored`, a vault transaction of
    /// `squad` as its account holds it. The instructions are the ones the
    /// account's message expands to, a signer being an account before its
    /// signer count and writable as its header says, which is how the program
    /// expands it to compare. `form` is where it runs: see [`CheckForm`].
    pub fn approving(
        squad: Squad,
        stored: &VaultTransaction,
        member: Pubkey,
        form: CheckForm,
    ) -> Self {
        Self {
            form,
            squad,
            transaction_index: stored.index,
            vault_index: stored.vault_index,
            vault: stored.vault,
            ephemeral_signers: stored.ephemeral_signers,
            member,
            action: VoteAction::Approve,
            instructions: stored.instructions.clone(),
        }
    }

    /// `None` when `instruction` is not a well-formed `verifyProposal` or
    /// `verifyProposalCpi` of the clear signing program.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        if instruction.program_id != SQUADS_CLEAR_SIGNING_PROGRAM_ID {
            return None;
        }
        Self::from_data(&instruction.data)
    }

    /// The check in `data`, an instruction's data, tag included. `None`
    /// unless it reads to the last byte. Both forms take the same arguments.
    pub fn from_data(data: &[u8]) -> Option<Self> {
        let mut data = Reader(data);
        let form = CheckForm::from_tag(data.array::<8>()?)?;
        let multisig = data.pubkey()?;
        let transaction_index = u64::from_le_bytes(data.array()?);
        let vault_index = data.u8()?;
        let ephemeral_signers = data.u8()?;
        let member = data.pubkey()?;
        let action = VoteAction::from_byte(data.u8()?)?;
        let count = data.count(MAX_INSTRUCTIONS)?;
        let mut instructions = Vec::new();
        for _ in 0..count {
            let program_id = data.pubkey()?;
            let metas = data.count(MAX_ACCOUNTS)?;
            let mut accounts = Vec::new();
            for _ in 0..metas {
                accounts.push(AccountMeta {
                    pubkey: data.pubkey()?,
                    is_signer: data.bool()?,
                    is_writable: data.bool()?,
                });
            }
            let length = data.count(MAX_DATA)?;
            instructions.push(Instruction {
                program_id,
                accounts,
                data: data.take(length)?.to_vec(),
            });
        }
        if !data.0.is_empty() {
            return None;
        }
        let squad = Squad {
            program: SquadsProgram::V4,
            address: multisig,
        };
        Some(Self {
            form,
            vault: squad.vault(vault_index),
            squad,
            transaction_index,
            vault_index,
            ephemeral_signers,
            member,
            action,
            instructions,
        })
    }

    /// The instruction's data: the counterpart of [`Self::from_data`].
    pub fn to_data(&self) -> Vec<u8> {
        let mut data = self.form.tag().to_vec();
        data.extend_from_slice(self.squad.address.as_ref());
        data.extend_from_slice(&self.transaction_index.to_le_bytes());
        data.push(self.vault_index);
        data.push(self.ephemeral_signers);
        data.extend_from_slice(self.member.as_ref());
        data.push(self.action.byte());
        data.extend_from_slice(&(self.instructions.len() as u32).to_le_bytes());
        for instruction in &self.instructions {
            data.extend_from_slice(instruction.program_id.as_ref());
            data.extend_from_slice(&(instruction.accounts.len() as u32).to_le_bytes());
            for meta in &instruction.accounts {
                data.extend_from_slice(meta.pubkey.as_ref());
                data.push(u8::from(meta.is_signer));
                data.push(u8::from(meta.is_writable));
            }
            data.extend_from_slice(&(instruction.data.len() as u32).to_le_bytes());
            data.extend_from_slice(&instruction.data);
        }
        data
    }

    /// The `verifyProposal` or `verifyProposalCpi` instruction, on the
    /// proposal and transaction accounts derived for the index, the second
    /// with the member after the instructions sysvar, as a signer.
    /// `lookup_tables` are the tables the stored message reads, in its order.
    pub fn instruction(&self, lookup_tables: &[Pubkey]) -> Instruction {
        let mut accounts = vec![
            AccountMeta::new_readonly(self.proposal(), false),
            AccountMeta::new_readonly(self.transaction(), false),
            AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR, false),
        ];
        if self.form == CheckForm::Cpi {
            accounts.push(AccountMeta::new_readonly(self.member, true));
        }
        accounts.extend(
            lookup_tables
                .iter()
                .map(|table| AccountMeta::new_readonly(*table, false)),
        );
        Instruction {
            program_id: SQUADS_CLEAR_SIGNING_PROGRAM_ID,
            accounts,
            data: self.to_data(),
        }
    }

    /// Whether this check expects `member`'s vote. The one place the reader
    /// asks which member a check is for.
    pub fn names(&self, member: &Pubkey) -> bool {
        self.member == *member
    }

    /// The proposal this check is about.
    pub fn proposal(&self) -> Pubkey {
        self.squad.proposal(self.transaction_index)
    }

    /// The stored transaction this check compares.
    pub fn transaction(&self) -> Pubkey {
        self.squad.transaction(self.transaction_index)
    }
}

/// The program a loader-v3 `Upgrade` replaces and the buffer it deploys
/// from. `None` for any other instruction.
pub fn loader_upgrade(instruction: &Instruction) -> Option<(Pubkey, Pubkey)> {
    if instruction.program_id != BPF_LOADER_UPGRADEABLE_ID
        || instruction.data.get(..4) != Some(&LOADER_UPGRADE.to_le_bytes())
    {
        return None;
    }
    Some((
        instruction.accounts.get(UPGRADE_PROGRAM)?.pubkey,
        instruction.accounts.get(UPGRADE_BUFFER)?.pubkey,
    ))
}

/// The buffers the program upgrades among `instructions` deploy from, each
/// once, in the order they first come.
pub fn upgrade_buffers(instructions: &[Instruction]) -> Vec<Pubkey> {
    let mut buffers = Vec::new();
    for (_, buffer) in instructions.iter().filter_map(loader_upgrade) {
        if !buffers.contains(&buffer) {
            buffers.push(buffer);
        }
    }
    buffers
}

/// Whether the clear signing program can be counted on at its address on
/// `network`: the networks its IDL ships for, which are those it is deployed
/// on. A local cluster runs whatever was loaded into it, so there it is when
/// the catalog holds an IDL for it, imported or shipped.
pub fn clear_signing_on(network: IdlNetwork, catalog_has_idl: bool) -> bool {
    match network {
        IdlNetwork::Localnet => catalog_has_idl,
        public => BUILTIN_IDLS
            .iter()
            .any(|idl| idl.program_id == SQUADS_CLEAR_SIGNING_PROGRAM_ID && idl.runs_on(public)),
    }
}

/// `instructions`, a list [`place_checks`] made, paid by `fee_payer`, on
/// `lifetime` (a recent blockhash, or the nonce value when the list starts
/// with `AdvanceNonceAccount`), at the highest compute limit with the
/// default loaded accounts limit, to be measured and fitted, and with
/// `budget`'s priority fee and heap size. Refused when it does not fit in one
/// transaction.
pub fn compile_with_checks(
    fee_payer: Pubkey,
    budget: &TxBudget,
    instructions: &[Instruction],
    lifetime: Hash,
) -> Result<Vec<u8>, BuildError> {
    let config = budget.rebuilt_config(MAX_CU_LIMIT);
    compile_message(&fee_payer, instructions, lifetime, config).map(|compiled| compiled.wire)
}

/// What a transaction holds beside the checks it would carry, as far as the
/// clear signing program cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeFacts {
    /// Members' votes on the proposal.
    pub votes: usize,
    /// It executes the proposal too.
    pub execute: bool,
    /// It starts with System `AdvanceNonceAccount`.
    pub nonce: bool,
    /// A vote, or the execution, runs inside a programmatic signer's
    /// `Submit`, so its check runs there too, as `verifyProposalCpi`.
    pub in_submit: bool,
    /// A vote, or the execution, runs inside a Vector `Passthrough`, beside
    /// the transaction's `Advance`, so its check runs there too, as
    /// `verifyProposalCpi`.
    pub in_passthrough: bool,
}

/// What of a [`ShapeFacts`] the program does not take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeRefused {
    DurableNonce,
    Execute,
    SeveralVotes,
    InSubmit,
    InPassthrough,
}

/// What the deployed clear signing program takes beside a check, besides
/// one vote and, at the top of the transaction, compute budget. The one
/// place to edit when a new program ships. The offer and the builder take it
/// as a parameter, so tests run them with [`Self::EVERYTHING`]; the reader
/// never looks at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearSigningAccepts {
    /// Executing the same proposal in the same transaction.
    pub execute: bool,
    /// Several members' votes on the proposal, each with its check.
    pub several_votes: bool,
    /// A leading System `AdvanceNonceAccount`.
    pub durable_nonce: bool,
    /// The vote and its check inside a programmatic signer's top-level
    /// `Submit`, the check as `verifyProposalCpi`.
    pub in_submit: bool,
    /// The vote and its check inside a top-level Vector `Passthrough`, the
    /// check as `verifyProposalCpi`.
    pub in_passthrough: bool,
}

impl ClearSigningAccepts {
    /// The program at [`SQUADS_CLEAR_SIGNING_PROGRAM_ID`] on devnet, the
    /// build of squads-cs-poc 0922251: a vote inside a `Submit` or a
    /// `Passthrough`, by CPI, and nothing else of this list.
    pub const DEPLOYED: Self = Self {
        execute: false,
        several_votes: false,
        durable_nonce: false,
        in_submit: true,
        in_passthrough: true,
    };

    /// All of it, for tests of what is built once a program takes it.
    pub const EVERYTHING: Self = Self {
        execute: true,
        several_votes: true,
        durable_nonce: true,
        in_submit: true,
        in_passthrough: true,
    };

    /// The first thing in `facts` the program does not take, if any.
    pub fn refuses(self, facts: ShapeFacts) -> Option<ShapeRefused> {
        if facts.nonce && !self.durable_nonce {
            Some(ShapeRefused::DurableNonce)
        } else if facts.execute && !self.execute {
            Some(ShapeRefused::Execute)
        } else if facts.votes > 1 && !self.several_votes {
            Some(ShapeRefused::SeveralVotes)
        } else if facts.in_submit && !self.in_submit {
            Some(ShapeRefused::InSubmit)
        } else if facts.in_passthrough && !self.in_passthrough {
            Some(ShapeRefused::InPassthrough)
        } else {
            None
        }
    }
}

/// The checks for `members`' approvals of `stored`, a vault transaction of
/// `squad`, each member once, in the form its vote needs: [`CheckForm::Cpi`]
/// for a member that votes inside a `Submit` or a `Passthrough`.
///
/// Assumed of a program that takes several votes, and known only here:
/// several members each get a check of their own, with the same content. The
/// deployed program takes one vote, so one member, which is the same either
/// way.
pub fn checks_for(
    squad: Squad,
    stored: &VaultTransaction,
    members: &[(Pubkey, CheckForm)],
) -> Vec<ProposalCheck> {
    let mut checks: Vec<ProposalCheck> = Vec::new();
    for (member, form) in members {
        if !checks.iter().any(|check| check.names(member)) {
            checks.push(ProposalCheck::approving(squad, stored, *member, *form));
        }
    }
    checks
}

/// `rest`, the instructions on screen, with the checks added: after the
/// leading `nonce` advance, which stays first, the buffer hashes, then the
/// checks, then `rest` as it was. The deployed program wants the checks
/// before the vote. For a member that votes through a program, this is the
/// list before that program's rebuild, which moves the checks in with the
/// vote ([`moves_into_authorization`]) and keeps their order.
pub fn place_checks(
    nonce: Option<Instruction>,
    rest: &[Instruction],
    checks: &[ProposalCheck],
    buffers: &[BufferHashCheck],
) -> Vec<Instruction> {
    nonce
        .into_iter()
        .chain(buffers.iter().map(BufferHashCheck::instruction))
        .chain(checks.iter().map(|check| check.instruction(&[])))
        .chain(rest.iter().cloned())
        .collect()
}

/// Whether `instruction` moves into a programmatic signer's `Submit`, or a
/// Vector `Passthrough`, with the instructions it signs: a clear signing
/// instruction other than a top-level `verifyProposal`. The program runs
/// `verifyProposalCpi` only inside the list that holds the vote, and a
/// `verifyBufferHash` belongs in that list beside it. An authority signs only
/// what its `Submit` carries, so a check left beside it could be dropped
/// after it signed. A `verifyProposal` stays where it is: it checks a vote at
/// the top of the transaction, and runs only there.
pub fn moves_into_authorization(instruction: &Instruction) -> bool {
    instruction.program_id == SQUADS_CLEAR_SIGNING_PROGRAM_ID
        && !instruction.data.starts_with(&VERIFY_PROPOSAL)
}

/// A `verifyBufferHash`: the hash a loader-v3 buffer's build must have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferHashCheck {
    pub buffer: Pubkey,
    /// 64 lowercase hex characters, as `solana-verify get-buffer-hash` prints
    /// them.
    pub hash: String,
}

impl BufferHashCheck {
    /// `None` when `instruction` is not a well-formed `verifyBufferHash` of
    /// the clear signing program, or its hash is not one the program reads.
    pub fn parse(instruction: &Instruction) -> Option<Self> {
        if instruction.program_id != SQUADS_CLEAR_SIGNING_PROGRAM_ID {
            return None;
        }
        Self::from_data(&instruction.data)
    }

    /// The check in `data`, tag included.
    pub fn from_data(data: &[u8]) -> Option<Self> {
        let mut data = Reader(data);
        if data.array::<8>()? != VERIFY_BUFFER_HASH {
            return None;
        }
        let buffer = data.pubkey()?;
        let length = data.count(64)?;
        let hash = data.take(length)?;
        let lower_hex = |byte: &u8| matches!(byte, b'0'..=b'9' | b'a'..=b'f');
        if !data.0.is_empty() || length != 64 || !hash.iter().all(lower_hex) {
            return None;
        }
        Some(Self {
            buffer,
            hash: String::from_utf8(hash.to_vec()).ok()?,
        })
    }

    pub fn to_data(&self) -> Vec<u8> {
        let mut data = VERIFY_BUFFER_HASH.to_vec();
        data.extend_from_slice(self.buffer.as_ref());
        data.extend_from_slice(&(self.hash.len() as u32).to_le_bytes());
        data.extend_from_slice(self.hash.as_bytes());
        data
    }

    /// The check that `buffer` holds the build whose hash is `hash`, as
    /// [`crate::program_code::buffer_hash`] gives it.
    pub fn new(buffer: Pubkey, hash: &[u8; 32]) -> Self {
        Self {
            buffer,
            hash: hex::encode(hash),
        }
    }

    pub fn instruction(&self) -> Instruction {
        Instruction {
            program_id: SQUADS_CLEAR_SIGNING_PROGRAM_ID,
            accounts: vec![AccountMeta::new_readonly(self.buffer, false)],
            data: self.to_data(),
        }
    }
}

/// Borsh, as far as these checks need it.
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

    /// A `u32` count, no more than `max` and no more than the bytes left:
    /// every item it counts takes at least one.
    fn count(&mut self, max: usize) -> Option<usize> {
        let n = usize::try_from(u32::from_le_bytes(self.array()?)).ok()?;
        (n <= max && n <= self.0.len()).then_some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{pk, transfer};

    fn check(instructions: Vec<Instruction>) -> ProposalCheck {
        let squad = Squad::v4(&pk(50));
        ProposalCheck {
            form: CheckForm::TopLevel,
            squad,
            transaction_index: 7,
            vault_index: 1,
            vault: squad.vault(1),
            ephemeral_signers: 0,
            member: pk(9),
            action: VoteAction::Approve,
            instructions,
        }
    }

    fn sample() -> ProposalCheck {
        let squad = Squad::v4(&pk(50));
        check(vec![
            transfer(squad.vault(1), pk(2), 5),
            Instruction {
                program_id: pk(3),
                accounts: Vec::new(),
                data: Vec::new(),
            },
        ])
    }

    #[test]
    fn a_check_reads_back_what_it_was_written_from() {
        let check = sample();
        let instruction = check.instruction(&[pk(4)]);
        assert_eq!(ProposalCheck::parse(&instruction), Some(check.clone()));
        assert_eq!(instruction.accounts[0].pubkey, check.squad.proposal(7));
        assert_eq!(instruction.accounts[1].pubkey, check.squad.transaction(7));
        assert_eq!(instruction.accounts[3].pubkey, pk(4));
        // The layout the program reads: tag, multisig, index, vault index,
        // ephemeral signers, member, action, then the instructions.
        let data = check.to_data();
        assert_eq!(data[..8], VERIFY_PROPOSAL);
        assert_eq!(data[8..40], *check.squad.address.as_ref());
        assert_eq!(data[40..48], 7u64.to_le_bytes());
        assert_eq!(data[48..50], [1, 0]);
        assert_eq!(data[82], 0);
        assert_eq!(data[83..87], 2u32.to_le_bytes());

        // Another program's bytes are not a check.
        let elsewhere = Instruction {
            program_id: pk(1),
            ..instruction
        };
        assert_eq!(ProposalCheck::parse(&elsewhere), None);
    }

    fn by_cpi(check: ProposalCheck) -> ProposalCheck {
        ProposalCheck {
            form: CheckForm::Cpi,
            ..check
        }
    }

    #[test]
    fn a_check_by_cpi_has_its_own_tag_and_the_member_as_a_signer() {
        let check = by_cpi(sample());
        let instruction = check.instruction(&[pk(4)]);
        assert_eq!(ProposalCheck::parse(&instruction), Some(check.clone()));
        // Proposal, transaction, the instructions sysvar, the member signing,
        // then the lookup tables.
        assert_eq!(
            instruction.accounts,
            [
                AccountMeta::new_readonly(check.squad.proposal(7), false),
                AccountMeta::new_readonly(check.squad.transaction(7), false),
                AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR, false),
                AccountMeta::new_readonly(pk(9), true),
                AccountMeta::new_readonly(pk(4), false),
            ]
        );
        // The same arguments as `verifyProposal`, under another tag.
        let data = check.to_data();
        assert_eq!(data[..8], VERIFY_PROPOSAL_CPI);
        assert_eq!(data[8..], sample().to_data()[8..]);
        assert_eq!(
            ProposalCheck::from_data(&data).map(|check| check.form),
            Some(CheckForm::Cpi)
        );
        // Without lookup tables the member is last.
        let bare = check.instruction(&[]);
        assert_eq!(bare.accounts.len(), 4);
        assert!(bare.accounts[3].is_signer);
    }

    #[test]
    fn a_check_that_does_not_read_to_its_last_byte_is_none() {
        for data in [sample().to_data(), by_cpi(sample()).to_data()] {
            for cut in 0..data.len() {
                assert_eq!(ProposalCheck::from_data(&data[..cut]), None, "{cut}");
            }
            let mut longer = data.clone();
            longer.push(0);
            assert_eq!(ProposalCheck::from_data(&longer), None);
            // An action the program does not have.
            let mut action = data.clone();
            action[82] = 4;
            assert_eq!(ProposalCheck::from_data(&action), None);
            // A flag that is neither 0 nor 1: the first meta's signer flag.
            let mut flag = data.clone();
            let first_flag = 87 + 32 + 4 + 32;
            assert_eq!(flag[first_flag], 1);
            flag[first_flag] = 2;
            assert_eq!(ProposalCheck::from_data(&flag), None);
            // Another tag.
            let mut tag = data;
            tag[0] ^= 1;
            assert_eq!(ProposalCheck::from_data(&tag), None);
        }
    }

    #[test]
    fn a_hostile_count_is_refused_before_anything_is_held_for_it() {
        let head = &sample().to_data()[..83];
        for count in [u32::MAX, 256, 1 << 20] {
            let mut data = head.to_vec();
            data.extend_from_slice(&count.to_le_bytes());
            data.extend_from_slice(&[0; 64]);
            assert_eq!(ProposalCheck::from_data(&data), None, "{count}");
        }
        // One instruction claiming more accounts, or more data, than can be.
        for (accounts, length) in [(u32::MAX, 0), (256, 0), (0, u32::MAX), (0, 70_000)] {
            let mut data = head.to_vec();
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(&[3; 32]);
            data.extend_from_slice(&accounts.to_le_bytes());
            data.extend_from_slice(&length.to_le_bytes());
            data.extend_from_slice(&[0; 70_000]);
            assert_eq!(ProposalCheck::from_data(&data), None);
        }
        // The most a stored message holds still reads.
        let wide = check(vec![
            Instruction {
                program_id: pk(3),
                accounts: (0..255).map(|_| AccountMeta::new(pk(4), false)).collect(),
                data: vec![1; MAX_DATA],
            };
            2
        ]);
        assert_eq!(ProposalCheck::from_data(&wide.to_data()), Some(wide));
    }

    #[test]
    fn a_buffer_hash_is_taken_only_as_the_program_reads_it() {
        let hash = "7ff737ad44ad241dc6cb45dae1010ba61774c6d55b662876282dbc40a9efe573";
        let check = BufferHashCheck {
            buffer: pk(4),
            hash: hash.into(),
        };
        assert_eq!(BufferHashCheck::parse(&check.instruction()), Some(check));
        let written = |hash: &str| {
            BufferHashCheck::from_data(
                &BufferHashCheck {
                    buffer: pk(4),
                    hash: hash.into(),
                }
                .to_data(),
            )
        };
        assert_eq!(written(&hash.to_uppercase()), None);
        assert_eq!(written(&hash[..62]), None);
        assert_eq!(written(&format!("{hash}00")), None);
        assert_eq!(written(&hash.replace('7', "g")), None);
        let mut longer = BufferHashCheck {
            buffer: pk(4),
            hash: hash.into(),
        }
        .to_data();
        longer.push(0);
        assert_eq!(BufferHashCheck::from_data(&longer), None);
        // A proposal check is not a buffer check, nor the other way round.
        assert_eq!(BufferHashCheck::from_data(&sample().to_data()), None);
    }

    /// A vault transaction Squads v4 stored on mainnet (5K1Vtf…, from the
    /// clear signing program's own tests), as its account holds it.
    const MAINNET_STORED: &[&str] = &[
    "a8faa264510ea2cf0f6df3e617c986d860ab0644d7e20135dde7934382763fc8ed19a5036c73e5c5e0193792dcee8538",
    "81d71eba2ff8eb03b683be6de4b41b13aa889e3492ba4b625300000000000000ff00ff0000000001010a100000000960",
    "c60587b02fd4f442ce8d196ddd7594f88ce73bf75f95c70a0b780a640986aec3f29b28a5af5a80dbc45b0de0dc897ebe",
    "f5501b88554057b13172b349c0920741055e3e260b51884dbe004c5790187642119b243e5d1ec8b8e771fba778c1d381",
    "67879b57c36d213e85a3a0b6d21927ea8efb730810fab9354ace73b1b50f29510b6a2fc72b9f6385cb83e043bc291847",
    "1d3bca2b8c51ce226d45d52a360d96b33673c13292b6c626c5ac0aca43aacee4028dbd40559984e556ecec083d34eabf",
    "426cf38de021948bfc31901c3e7e428ecb5acfada3a11f96c98dcc1ac8e2b7e60350f01f1bc177093f8a1157f392714e",
    "712ff2bf7d88bb505a8c528690e616f09925d25a9142a3355dc5beeb9529e5245cade8dc2ae97e8513d843bd4aece75c",
    "c82acefe8cfdcefc595f603d82e8cd5c3dfc53731a586f32a63c78ecdad3b0524a6e18bc00d3200290e61782f2ba8ee8",
    "8fdf6f8ed3eb03c28b97a7a1405c13943af860f1c62f68823dbf3ef7a5a35453643c8745edbabe7e766fd3d731c1c6fa",
    "7af3bedbad3a3d65f36aabc97431b1bbe4c2d2f6e0e47ca60203452f5d6106ddf6e1ee758fde18425dbce46ccddab61a",
    "fc4d83b90d27febdf928d8a18bfc06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a90000",
    "000000000000000000000000000000000000000000000000000000000000010000000b10000000000001020c03040506",
    "0708090a0d0e0f10000000ac1b344a98c08488c0d8a7000000000000000000",
    ];

    /// Squads' stored message expanded as the clear signing program compares
    /// it, written out here from the account's Borsh layout: a signer is an
    /// account before `num_signers`, writable as the header says.
    fn expanded_as_the_program_does(data: &[u8]) -> (Pubkey, u64, u8, u8, Vec<Instruction>) {
        let mut data = Reader(data);
        data.array::<8>().unwrap();
        let multisig = data.pubkey().unwrap();
        data.pubkey().unwrap(); // creator
        let index = u64::from_le_bytes(data.array().unwrap());
        let _bump = data.u8().unwrap();
        let vault_index = data.u8().unwrap();
        let _vault_bump = data.u8().unwrap();
        let ephemeral = data.count(255).unwrap();
        data.take(ephemeral).unwrap();
        let (signers, writable_signers, writable_others) =
            (data.u8().unwrap(), data.u8().unwrap(), data.u8().unwrap());
        let keys: Vec<Pubkey> = (0..data.count(255).unwrap())
            .map(|_| data.pubkey().unwrap())
            .collect();
        let writable = |at: usize| {
            if at < usize::from(signers) {
                at < usize::from(writable_signers)
            } else {
                at - usize::from(signers) < usize::from(writable_others)
            }
        };
        let instructions = (0..data.count(255).unwrap())
            .map(|_| {
                let program_id = keys[usize::from(data.u8().unwrap())];
                let indexes = data.count(255).unwrap();
                let accounts = data
                    .take(indexes)
                    .unwrap()
                    .iter()
                    .map(|&at| {
                        let at = usize::from(at);
                        AccountMeta {
                            pubkey: keys[at],
                            is_signer: at < usize::from(signers),
                            is_writable: writable(at),
                        }
                    })
                    .collect();
                let length = data.count(MAX_DATA).unwrap();
                Instruction {
                    program_id,
                    accounts,
                    data: data.take(length).unwrap().to_vec(),
                }
            })
            .collect();
        (multisig, index, vault_index, ephemeral as u8, instructions)
    }

    #[test]
    fn what_squads_stored_on_mainnet_is_what_a_check_carries_and_reads_back() {
        let data = hex::decode(MAINNET_STORED.concat()).unwrap();
        let (multisig, index, vault_index, ephemeral, expanded) =
            expanded_as_the_program_does(&data);
        let squad = Squad {
            program: SquadsProgram::V4,
            address: multisig,
        };
        assert_eq!(
            squad.transaction(index),
            Pubkey::from_str_const("5K1VtfSzzJ42X74eXyoMaGLxsxKBk36ygcejirfRDmn1")
        );
        // The app's own reading of the account gives the same instructions,
        // flags and all, so a check built from it is one the program accepts.
        let Some(StoredAccount::Vault(stored)) = squad.parse_transaction(&data) else {
            panic!("a vault transaction");
        };
        assert!(!expanded.is_empty());
        assert_eq!(stored.instructions, expanded);
        assert_eq!(
            (stored.index, stored.vault_index, stored.ephemeral_signers),
            (index, vault_index, ephemeral)
        );

        let check = ProposalCheck {
            form: CheckForm::TopLevel,
            squad,
            transaction_index: index,
            vault_index,
            vault: squad.vault(vault_index),
            ephemeral_signers: ephemeral,
            member: pk(9),
            action: VoteAction::Approve,
            instructions: stored.instructions.clone(),
        };
        // What the screen builds from the account is that check.
        assert_eq!(
            ProposalCheck::approving(squad, &stored, pk(9), CheckForm::TopLevel),
            check
        );
        assert_eq!(
            ProposalCheck::parse(&check.instruction(&[])),
            Some(check.clone())
        );
        let cpi = ProposalCheck::approving(squad, &stored, pk(9), CheckForm::Cpi);
        assert_eq!(cpi, by_cpi(check));
        assert_eq!(ProposalCheck::parse(&cpi.instruction(&[])), Some(cpi));
    }

    fn mainnet_check() -> (Squad, ProposalCheck) {
        let data = hex::decode(MAINNET_STORED.concat()).unwrap();
        let (multisig, ..) = expanded_as_the_program_does(&data);
        let squad = Squad {
            program: SquadsProgram::V4,
            address: multisig,
        };
        let Some(StoredAccount::Vault(stored)) = squad.parse_transaction(&data) else {
            panic!("a vault transaction");
        };
        (
            squad,
            ProposalCheck::approving(squad, &stored, pk(9), CheckForm::TopLevel),
        )
    }

    fn explained(wire: &[u8]) -> crate::ExplainedTx {
        let tx = crate::decode_v1_transaction(wire).unwrap();
        crate::explain_transaction(&crate::NoIdls, &tx)
    }

    /// `check`'s member's approval, its vote's bytes `vote_data`, with the
    /// checks placed before it, compiled as the rebuild compiles it.
    fn lone(
        fee_payer: Pubkey,
        budget: &TxBudget,
        check: &ProposalCheck,
        buffers: &[BufferHashCheck],
        vote_data: &[u8],
        blockhash: Hash,
    ) -> Result<Vec<u8>, BuildError> {
        let mut vote = check.squad.approve(check.transaction_index, &check.member);
        vote.data = vote_data.to_vec();
        let instructions = place_checks(None, &[vote], std::slice::from_ref(check), buffers);
        compile_with_checks(fee_payer, budget, &instructions, blockhash)
    }

    #[test]
    fn what_the_program_takes_is_refused_in_one_order() {
        let all = ShapeFacts {
            votes: 2,
            execute: true,
            nonce: true,
            in_submit: true,
            in_passthrough: true,
        };
        assert_eq!(ClearSigningAccepts::EVERYTHING.refuses(all), None);
        assert_eq!(
            ClearSigningAccepts::DEPLOYED.refuses(all),
            Some(ShapeRefused::DurableNonce)
        );
        let lone = ShapeFacts {
            votes: 1,
            ..ShapeFacts::default()
        };
        // A lone vote, at the top, in a `Submit`, or in a `Passthrough`.
        for taken in [
            lone,
            ShapeFacts {
                in_submit: true,
                ..lone
            },
            ShapeFacts {
                in_passthrough: true,
                ..lone
            },
        ] {
            assert_eq!(ClearSigningAccepts::DEPLOYED.refuses(taken), None);
        }
        for carried in [
            ShapeFacts::default(),
            ShapeFacts {
                in_submit: true,
                ..ShapeFacts::default()
            },
            ShapeFacts {
                in_passthrough: true,
                ..ShapeFacts::default()
            },
        ] {
            for (facts, refused) in [
                (
                    ShapeFacts {
                        votes: 1,
                        execute: true,
                        ..carried
                    },
                    ShapeRefused::Execute,
                ),
                (
                    ShapeFacts {
                        votes: 2,
                        ..carried
                    },
                    ShapeRefused::SeveralVotes,
                ),
            ] {
                assert_eq!(ClearSigningAccepts::DEPLOYED.refuses(facts), Some(refused));
                assert_eq!(ClearSigningAccepts::EVERYTHING.refuses(facts), None);
            }
        }
        // A program that took neither carrier says which.
        let top_only = ClearSigningAccepts {
            in_submit: false,
            in_passthrough: false,
            ..ClearSigningAccepts::DEPLOYED
        };
        assert_eq!(
            top_only.refuses(ShapeFacts {
                in_submit: true,
                ..lone
            }),
            Some(ShapeRefused::InSubmit)
        );
        assert_eq!(
            top_only.refuses(ShapeFacts {
                in_passthrough: true,
                ..lone
            }),
            Some(ShapeRefused::InPassthrough)
        );
    }

    #[test]
    fn each_member_gets_one_check_with_the_same_content() {
        let squad = Squad::v4(&pk(50));
        let stored =
            VaultTransaction::for_test(&squad, 1, 0, &[transfer(squad.vault(0), pk(2), 5)]);
        let top = CheckForm::TopLevel;
        let checks = checks_for(
            squad,
            &stored,
            &[(pk(1), top), (pk(2), CheckForm::Cpi), (pk(1), top)],
        );
        assert_eq!(checks.len(), 2);
        assert!(checks[0].names(&pk(1)) && checks[1].names(&pk(2)));
        assert_eq!(
            (checks[0].form, checks[1].form),
            (CheckForm::TopLevel, CheckForm::Cpi)
        );
        assert_eq!(checks[0].instructions, checks[1].instructions);
        // The checks go after a nonce advance and the buffer hashes.
        let advance = transfer(pk(1), pk(2), 1);
        let buffer = BufferHashCheck::new(pk(70), &[1; 32]);
        let placed = place_checks(
            Some(advance.clone()),
            &[squad.approve(1, &pk(1))],
            &checks,
            std::slice::from_ref(&buffer),
        );
        assert_eq!(placed[0], advance);
        assert_eq!(placed[1], buffer.instruction());
        assert!(ProposalCheck::parse(&placed[2]).is_some_and(|check| check.names(&pk(1))));
        assert!(ProposalCheck::parse(&placed[3]).is_some_and(|check| check.names(&pk(2))));
        assert_eq!(placed[4], squad.approve(1, &pk(1)));
        // The buffer hash and the check by CPI go in with the vote they
        // belong to; a top-level check stays at the top, and so does the vote
        // until its own signer's rebuild takes it.
        assert!(moves_into_authorization(&placed[1]));
        assert!(!moves_into_authorization(&placed[2]));
        assert!(moves_into_authorization(&placed[3]));
        assert!(!moves_into_authorization(&placed[4]));
    }

    #[test]
    fn a_built_approval_carries_the_check_and_reads_as_checked() {
        let (squad, check) = mainnet_check();
        let memo = squad.approve(check.transaction_index, &check.member).data;
        let budget = TxBudget {
            priority_fee: Some(5_000),
            ..TxBudget::default()
        };
        let wire = lone(
            pk(8),
            &budget,
            &check,
            &[],
            &memo,
            Hash::new_from_array([4; 32]),
        )
        .unwrap();
        let tx = crate::decode_v1_transaction(&wire).unwrap();
        assert_eq!(tx.fee_payer(), Some(pk(8)));
        assert_eq!(
            tx.signers
                .iter()
                .map(|slot| slot.pubkey)
                .collect::<Vec<_>>(),
            [pk(8), pk(9)]
        );
        // Probed at the most compute there is, with the priority fee it had.
        assert_eq!(tx.budget.compute_unit_limit, Some(MAX_CU_LIMIT));
        assert_eq!(tx.budget.priority_fee, Some(5_000));
        let native = tx.native_instructions();
        assert_eq!(native.len(), 2);
        assert_eq!(ProposalCheck::parse(&native[0]), Some(check.clone()));
        assert_eq!(native[1].data, memo);

        let pinned = crate::checked_in(&explained(&wire));
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].proposal, squad.proposal(check.transaction_index));
        assert_eq!(pinned[0].check, crate::IxPlace::top(0));
        assert_eq!(
            pinned[0].votes,
            [crate::PinnedVote {
                member: pk(9),
                at: crate::IxPlace::top(1),
            }]
        );
        assert!(pinned[0].upgrades.is_empty());
        assert!(crate::blind_in(&explained(&wire)).is_empty());
    }

    #[test]
    fn an_upgrade_is_built_with_its_buffer_hash_first() {
        let squad = Squad::v4(&pk(50));
        let vault = squad.vault(0);
        let upgrade = Instruction {
            program_id: BPF_LOADER_UPGRADEABLE_ID,
            accounts: vec![
                AccountMeta::new(pk(60), false),
                AccountMeta::new(pk(71), false),
                AccountMeta::new(pk(70), false),
                AccountMeta::new(pk(61), false),
                AccountMeta::new_readonly(pk(62), false),
                AccountMeta::new_readonly(pk(63), false),
                AccountMeta::new_readonly(vault, true),
            ],
            data: LOADER_UPGRADE.to_le_bytes().to_vec(),
        };
        // The same buffer twice is hashed once.
        let instructions = vec![upgrade.clone(), transfer(vault, pk(2), 5), upgrade];
        assert_eq!(upgrade_buffers(&instructions), [pk(70)]);
        assert_eq!(loader_upgrade(&instructions[0]), Some((pk(71), pk(70))));
        assert_eq!(loader_upgrade(&instructions[1]), None);
        let check = ProposalCheck {
            form: CheckForm::TopLevel,
            member: pk(1),
            ..check(instructions)
        };
        let buffer = BufferHashCheck::new(pk(70), &[0xab; 32]);
        assert_eq!(buffer.hash, "ab".repeat(32));
        let memo = squad.approve(7, &pk(1)).data;
        let wire = lone(
            pk(8),
            &TxBudget::default(),
            &check,
            std::slice::from_ref(&buffer),
            &memo,
            Hash::new_from_array([4; 32]),
        )
        .unwrap();
        let tx = crate::decode_v1_transaction(&wire).unwrap();
        let native = tx.native_instructions();
        assert_eq!(BufferHashCheck::parse(&native[0]), Some(buffer.clone()));
        assert_eq!(ProposalCheck::parse(&native[1]), Some(check));
        let pinned = crate::checked_in(&explained(&wire));
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].upgrades.len(), 2);
        assert!(pinned[0]
            .upgrades
            .iter()
            .all(|upgrade| upgrade.hash.as_deref() == Some(buffer.hash.as_str())));
    }

    #[test]
    fn a_proposal_too_large_to_carry_is_refused() {
        let squad = Squad::v4(&pk(50));
        let big = check(vec![Instruction {
            program_id: pk(3),
            accounts: Vec::new(),
            data: vec![1; 5_000],
        }]);
        let memo = squad.approve(7, &pk(9)).data;
        let built = lone(
            pk(8),
            &TxBudget::default(),
            &big,
            &[],
            &memo,
            Hash::default(),
        );
        assert!(
            matches!(built, Err(BuildError::TooLarge { .. })),
            "{built:?}"
        );
    }

    #[test]
    fn the_program_is_counted_on_where_its_idl_ships_and_on_a_local_cluster_with_one() {
        assert!(clear_signing_on(IdlNetwork::Devnet, false));
        assert!(!clear_signing_on(IdlNetwork::Mainnet, true));
        assert!(!clear_signing_on(IdlNetwork::Testnet, false));
        assert!(!clear_signing_on(IdlNetwork::Localnet, false));
        assert!(clear_signing_on(IdlNetwork::Localnet, true));
    }
}
