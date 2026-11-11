//! Votes the Squads clear signing program pins, read from the bytes alone.
//!
//! A v4 vote signs only the address of a proposal. A `verifyProposal` (or
//! `verifyProposalCpi`) carries every instruction the proposal stores, and
//! compares them with the stored transaction whenever it runs: if it runs in
//! a transaction that lands, the proposal holds exactly what it carries.
//! Nothing else about the transaction changes that. So a vote lands only if
//! the proposal holds what a check carries when two things hold:
//!
//! - the check runs when the vote runs: it is one of the transaction's own
//!   instructions, or carried in one that runs what it carries now (a
//!   programmatic signer's `Submit`, the executor's `Execute` in it, a Vector
//!   `Passthrough`). A list that is only stored, a Squads vault transaction,
//!   a Realms proposal's or the one inside a check, runs some other time: a
//!   check or a vote in it pins nothing and is pinned by nothing;
//! - the check cannot be taken out of what the vote's member signs. A key
//!   that signs the transaction signs all of it, and so does a Vector
//!   wallet's key, whose `Advance` signs every instruction of the transaction
//!   (`crate::wallets::vector::advance_message`). A programmatic signer's
//!   authority signs only what its `Submit` carries: a check beside the
//!   `Submit` could be dropped by whoever sends it after the authority signed,
//!   so for a vote inside a `Submit` only a check inside the same `Submit`
//!   counts.
//!
//! Those two are the floor. On top of them, a check pins only a vote it can
//! ever run beside, which its form says ([`CheckForm`]):
//!
//! - a `verifyProposal` looks among the transaction's own instructions, so
//!   it pins a vote there, and only when it is one of them itself;
//! - a `verifyProposalCpi` looks in the list that runs it, and runs only
//!   when that list is what a top-level `Submit` (through its `Execute`) or a
//!   top-level `Passthrough` runs, and its member is an address that carrier
//!   signs as. It pins a vote in that same list. A `Passthrough` that is not
//!   the transaction's only Vector instruction beside `Advance`s runs none.
//!
//! Whether the deployed program takes the transaction otherwise does not
//! come into it: one it refuses does not land, and then no vote in it counts.
//! Nor does anything else the transaction holds: a nonce advance, another
//! vote, an execution or a payment are rows of their own, read like any
//! instruction. What the deployed program takes is the builder's concern
//! (`crate::wallets::squads::ClearSigningAccepts`), not this reader's.
//!
//! Every vote is judged on its own, so one transaction can hold votes that
//! are pinned and votes that stay blind ([`super::blind`]).

use super::blind::{squads_call, SquadsCall};
use crate::edit::{Carrier, CarrierKind};
use crate::ids::{COMPUTE_BUDGET_PROGRAM_ID, SQUADS_CLEAR_SIGNING_PROGRAM_ID, SYSTEM_PROGRAM_ID};
use crate::review::explain::{ExplainedIx, ExplainedTx, InnerKind};
use crate::tx::decode::DecodedTx;
use crate::wallets::programmatic::programmatic_signer_address;
use crate::wallets::squads::{
    checks_for, loader_upgrade, place_checks, BufferHashCheck, CheckForm, ClearSigningAccepts,
    ProposalCheck, ShapeFacts, ShapeRefused, Squad, SquadsProgram, VaultTransaction, VoteAction,
    VERIFY_BUFFER_HASH, VERIFY_PROPOSAL, VERIFY_PROPOSAL_CPI,
};
use crate::wallets::vector::{ADVANCE_DISCRIMINATOR, PROGRAM_ID as VECTOR_PROGRAM_ID};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// Where an instruction is: its place among the transaction's own
/// instructions, then among those each carrier around it carries, from 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IxPlace(pub Vec<usize>);

impl IxPlace {
    /// One of the transaction's own instructions.
    pub fn top(at: usize) -> Self {
        Self(vec![at])
    }

    pub(super) fn at(outer: Option<&IxPlace>, at: usize) -> Self {
        match outer {
            Some(outer) => {
                let mut path = outer.0.clone();
                path.push(at);
                Self(path)
            }
            None => Self::top(at),
        }
    }

    /// Whether this is carried, at any depth, by the instruction at `outer`.
    fn inside(&self, outer: &IxPlace) -> bool {
        self.0.len() > outer.0.len() && self.0.starts_with(&outer.0)
    }

    /// The place in words, numbered from 1 within each list as the screen
    /// numbers instructions: "instruction 2", or "instruction 1 inside
    /// instruction 1 inside instruction 3".
    pub fn label(&self) -> String {
        self.0
            .iter()
            .rev()
            .map(|at| format!("instruction {}", at + 1))
            .collect::<Vec<_>>()
            .join(" inside ")
    }
}

/// What signs for an instruction's signer: `None`, the whole transaction, or
/// only what the `Submit` at that place carries.
type Coverage = Option<IxPlace>;

fn covers(coverage: &Coverage, place: &IxPlace) -> bool {
    coverage.as_ref().is_none_or(|submit| place.inside(submit))
}

/// A program upgrade among the instructions a checked approval pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedUpgrade {
    pub program: Pubkey,
    /// The buffer it deploys from. The check pins its address, not what it
    /// holds.
    pub buffer: Pubkey,
    /// The hash a `verifyBufferHash` pins the buffer's build to, as 64
    /// lowercase hex characters, when one runs that no pinned vote's member
    /// can have taken out. `None` when none does.
    pub hash: Option<String>,
}

/// A vote a check pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedVote {
    pub member: Pubkey,
    pub at: IxPlace,
}

/// A v4 proposal whose content checks in the transaction pin, and what of
/// the transaction they pin: votes, and its execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedApproval {
    pub squad: Squad,
    pub proposal: Pubkey,
    pub transaction_index: u64,
    /// The vault the instructions run as.
    pub vault: Pubkey,
    /// The first check that pins something here. Its opened instructions
    /// are what the proposal holds: every check of the proposal carries the
    /// same, or none of them pins anything.
    pub check: IxPlace,
    /// The votes pinned, in order. A vote on the proposal that is not among
    /// them stays blind.
    pub votes: Vec<PinnedVote>,
    /// Where the transaction executes the proposal, when a check pins that.
    pub execute: Option<IxPlace>,
    pub upgrades: Vec<CheckedUpgrade>,
}

impl CheckedApproval {
    /// Whether `member`'s vote is pinned.
    pub fn pins_vote_of(&self, member: &Pubkey) -> bool {
        self.votes.iter().any(|vote| vote.member == *member)
    }
}

/// What the scan finds among the instructions that run.
#[derive(Default)]
struct Scan {
    /// Each well-formed check on the accounts derived for its number.
    checks: Vec<SeenCheck>,
    /// Each well-formed buffer hash check on the buffer it names.
    buffers: Vec<(IxPlace, BufferHashCheck)>,
    votes: Vec<SeenVote>,
    executes: Vec<SeenExecute>,
}

struct SeenCheck {
    at: IxPlace,
    check: ProposalCheck,
    /// The instruction whose list it is in: `None` for the transaction's
    /// own.
    list: Option<IxPlace>,
    /// Its form can run where it is: see [`checked_in`].
    runs: bool,
}

impl SeenCheck {
    /// Whether this check looks for an instruction at `at` in `list`, by
    /// `coverage`: it runs, the instruction is in its own list, and it
    /// cannot be dropped from what the instruction's signer signs.
    fn reaches(&self, list: &Option<IxPlace>, coverage: &Coverage) -> bool {
        self.runs && self.list == *list && covers(coverage, &self.at)
    }
}

struct SeenVote {
    at: IxPlace,
    list: Option<IxPlace>,
    multisig: Pubkey,
    member: Pubkey,
    proposal: Pubkey,
    coverage: Coverage,
}

struct SeenExecute {
    at: IxPlace,
    list: Option<IxPlace>,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    coverage: Coverage,
}

/// The carrier whose list is being read.
#[derive(Clone, Default)]
struct RunBy {
    /// The addresses it signs as: its authorities' for a `Submit` and the
    /// `Execute` in it, its accounts or their system PDAs for a
    /// `Passthrough`.
    signs_as: Vec<Pubkey>,
    /// A list `verifyProposalCpi` runs in: what a top-level `Submit` runs
    /// through its `Execute`, or a top-level `Passthrough` that is the
    /// transaction's only Vector instruction beside `Advance`s.
    driver: bool,
    /// For a `Submit`'s own list, which holds its `Execute`: whether the
    /// `Submit` is one of the transaction's own instructions.
    submit_at_top: Option<bool>,
}

/// `list`, carried at `outer` (the transaction's own when `None`) by
/// `run_by`, which runs. `submits` are the `Submit`s around it, innermost
/// last, each with the addresses its authorities sign as.
/// `one_passthrough`: the transaction holds no Vector instruction but
/// `Advance`s and at most one other.
fn scan(
    list: &[ExplainedIx],
    outer: Option<&IxPlace>,
    run_by: Option<&RunBy>,
    submits: &[(IxPlace, Vec<Pubkey>)],
    one_passthrough: bool,
    found: &mut Scan,
) {
    // A signer that is one of a `Submit`'s authorities signs only what that
    // `Submit` carries. Any other signer signs the transaction.
    let coverage = |signer: &Pubkey| -> Coverage {
        submits
            .iter()
            .rev()
            .find(|(_, addresses)| addresses.contains(signer))
            .map(|(place, _)| place.clone())
    };
    for (at, instruction) in list.iter().enumerate() {
        let place = IxPlace::at(outer, at);
        let account = |at: usize| instruction.accounts.get(at).copied();
        if instruction.program_id == SQUADS_CLEAR_SIGNING_PROGRAM_ID {
            if let Some(check) = ProposalCheck::from_data(&instruction.data) {
                // The program refuses accounts other than the derived ones.
                if account(0) == Some(check.proposal()) && account(1) == Some(check.transaction()) {
                    let runs = match check.form {
                        CheckForm::TopLevel => outer.is_none(),
                        // The member account is the one in the arguments,
                        // and the carrier signs as it.
                        CheckForm::Cpi => {
                            account(3) == Some(check.member)
                                && run_by.is_some_and(|run_by| {
                                    run_by.driver && run_by.signs_as.contains(&check.member)
                                })
                        }
                    };
                    found.checks.push(SeenCheck {
                        at: place.clone(),
                        list: outer.cloned(),
                        check,
                        runs,
                    });
                }
            } else if let Some(check) = BufferHashCheck::from_data(&instruction.data) {
                if account(0) == Some(check.buffer) {
                    found.buffers.push((place.clone(), check));
                }
            }
        }
        if SquadsProgram::from_program_id(&instruction.program_id) == Some(SquadsProgram::V4) {
            match squads_call(SquadsProgram::V4, &instruction.data) {
                Some(SquadsCall::Approves { proposal }) => {
                    if let (Some(multisig), Some(member), Some(proposal)) =
                        (account(0), account(1), account(proposal))
                    {
                        found.votes.push(SeenVote {
                            at: place.clone(),
                            list: outer.cloned(),
                            multisig,
                            member,
                            proposal,
                            coverage: coverage(&member),
                        });
                    }
                }
                // `vaultTransactionExecute`: multisig, proposal, transaction,
                // then the member that executes.
                Some(SquadsCall::Runs {
                    proposal,
                    transaction: Some(transaction),
                }) => {
                    if let (Some(multisig), Some(proposal), Some(transaction), Some(member)) = (
                        account(0),
                        account(proposal),
                        account(transaction),
                        account(3),
                    ) {
                        found.executes.push(SeenExecute {
                            at: place.clone(),
                            list: outer.cloned(),
                            multisig,
                            proposal,
                            transaction,
                            coverage: coverage(&member),
                        });
                    }
                }
                _ => {}
            }
        }
        let Some(inner) = &instruction.inner else {
            continue;
        };
        match &inner.kind {
            InnerKind::Authorization { authorities, .. } => {
                let addresses: Vec<Pubkey> = authorities.iter().map(|key| key.address).collect();
                let mut around = submits.to_vec();
                around.push((place.clone(), addresses.clone()));
                let run_by = RunBy {
                    signs_as: addresses,
                    driver: false,
                    submit_at_top: Some(outer.is_none()),
                };
                scan(
                    &inner.instructions,
                    Some(&place),
                    Some(&run_by),
                    &around,
                    one_passthrough,
                    found,
                );
            }
            InnerKind::Execution { .. } => {
                // Run by the `Submit` whose list holds it, signing as its
                // authorities.
                let run_by = match run_by {
                    Some(submit) if submit.submit_at_top.is_some() => RunBy {
                        signs_as: submit.signs_as.clone(),
                        driver: submit.submit_at_top == Some(true),
                        submit_at_top: None,
                    },
                    _ => RunBy::default(),
                };
                scan(
                    &inner.instructions,
                    Some(&place),
                    Some(&run_by),
                    submits,
                    one_passthrough,
                    found,
                );
            }
            InnerKind::Passthrough { signers } => {
                let run_by = RunBy {
                    signs_as: signers
                        .iter()
                        .map(|runs| runs.system.unwrap_or(runs.account))
                        .collect(),
                    driver: outer.is_none() && one_passthrough,
                    submit_at_top: None,
                };
                scan(
                    &inner.instructions,
                    Some(&place),
                    Some(&run_by),
                    submits,
                    one_passthrough,
                    found,
                );
            }
            // Stored to run some other time, or not known to run now.
            InnerKind::SquadsTransaction { .. }
            | InnerKind::SquadsSync { .. }
            | InnerKind::ProposalTransaction { .. }
            | InnerKind::SquadsCheck { .. } => {}
        }
    }
}

/// The proposals in `explained` that checks pin, each with what of the
/// transaction they pin, in the order their first check comes. Read from
/// the bytes alone, so it holds with no IDL and no network.
///
/// A vote of `member` on proposal P of multisig M is pinned when the vote
/// runs and a check runs, where the member's signature covers it, that:
///
/// - reads to its last byte, on Squads v4, for M, and expects `member` to
///   approve;
/// - names, as its proposal and stored transaction, the accounts derived
///   from M and the check's number, and P is that proposal;
/// - carries what every other check of P in the transaction carries;
/// - sits in the vote's own list, and runs there: a `verifyProposal` among
///   the transaction's own instructions, or a `verifyProposalCpi` in what a
///   top-level `Submit` runs through its `Execute`, or in a top-level
///   `Passthrough` that is the transaction's only Vector instruction beside
///   `Advance`s, its member account `member` and an address that carrier
///   signs as.
///
/// An execution of P (`vaultTransactionExecute` on the derived proposal and
/// transaction) is pinned the same way by any approving check of P in its
/// list that its member's signature covers.
pub fn checked_in(explained: &ExplainedTx) -> Vec<CheckedApproval> {
    let mut found = Scan::default();
    let vector_calls = explained
        .instructions
        .iter()
        .filter(|ix| {
            ix.program_id == VECTOR_PROGRAM_ID && ix.data.first() != Some(&ADVANCE_DISCRIMINATOR)
        })
        .count();
    scan(
        &explained.instructions,
        None,
        None,
        &[],
        vector_calls <= 1,
        &mut found,
    );
    let mut proposals: Vec<(Pubkey, u64)> = Vec::new();
    for SeenCheck { check, .. } in &found.checks {
        let key = (check.squad.address, check.transaction_index);
        if !proposals.contains(&key) {
            proposals.push(key);
        }
    }
    proposals
        .into_iter()
        .filter_map(|key| approval_of(&found, key))
        .collect()
}

/// The places of every vote and execution [`checked_in`] pins.
pub(super) fn pinned_places(explained: &ExplainedTx) -> Vec<IxPlace> {
    checked_in(explained)
        .into_iter()
        .flat_map(|approval| {
            approval
                .votes
                .into_iter()
                .map(|vote| vote.at)
                .chain(approval.execute)
        })
        .collect()
}

fn approval_of(found: &Scan, (multisig, index): (Pubkey, u64)) -> Option<CheckedApproval> {
    let of_it: Vec<&SeenCheck> = found
        .checks
        .iter()
        .filter(|seen| {
            seen.check.squad.address == multisig && seen.check.transaction_index == index
        })
        .collect();
    let first = &of_it.first()?.check;
    // Every one of them has to hold, so if they carry different things the
    // transaction cannot land, and none is taken for what the proposal holds.
    let same = |check: &ProposalCheck| {
        check.vault_index == first.vault_index
            && check.ephemeral_signers == first.ephemeral_signers
            && check.instructions == first.instructions
    };
    if !of_it.iter().all(|seen| same(&seen.check)) {
        return None;
    }
    let (proposal, transaction) = (first.proposal(), first.transaction());
    let approving: Vec<&SeenCheck> = of_it
        .into_iter()
        .filter(|seen| seen.check.action == VoteAction::Approve)
        .collect();
    let mut shown: Option<IxPlace> = None;
    let mut coverages: Vec<Coverage> = Vec::new();
    let mut votes = Vec::new();
    for vote in found
        .votes
        .iter()
        .filter(|vote| vote.multisig == multisig && vote.proposal == proposal)
    {
        let pinning = approving.iter().find(|seen| {
            seen.check.names(&vote.member) && seen.reaches(&vote.list, &vote.coverage)
        });
        if let Some(SeenCheck { at, .. }) = pinning {
            if shown.as_ref().is_none_or(|shown| at < shown) {
                shown = Some(at.clone());
            }
            coverages.push(vote.coverage.clone());
            votes.push(PinnedVote {
                member: vote.member,
                at: vote.at.clone(),
            });
        }
    }
    let mut execute = None;
    for run in found.executes.iter().filter(|run| {
        run.multisig == multisig && run.proposal == proposal && run.transaction == transaction
    }) {
        if let Some(SeenCheck { at, .. }) = approving
            .iter()
            .find(|seen| seen.reaches(&run.list, &run.coverage))
        {
            if shown.as_ref().is_none_or(|shown| at < shown) {
                shown = Some(at.clone());
            }
            coverages.push(run.coverage.clone());
            execute = Some(run.at.clone());
            // A second execution of the same proposal would fail.
            break;
        }
    }
    let check = shown?;
    let upgrades = first
        .instructions
        .iter()
        .filter_map(loader_upgrade)
        .map(|(program, buffer)| CheckedUpgrade {
            program,
            buffer,
            hash: buffer_hash(found, &buffer, &coverages),
        })
        .collect();
    Some(CheckedApproval {
        squad: first.squad,
        proposal,
        transaction_index: index,
        vault: first.vault,
        check,
        votes,
        execute,
        upgrades,
    })
}

/// The hash the buffer hash checks that run pin `buffer`'s build to, when
/// one of them is covered by every signature in `coverages`. `None` when
/// none is, or when they name more than one hash: all of them have to hold,
/// so the transaction would fail.
fn buffer_hash(found: &Scan, buffer: &Pubkey, coverages: &[Coverage]) -> Option<String> {
    let of_it: Vec<&(IxPlace, BufferHashCheck)> = found
        .buffers
        .iter()
        .filter(|(_, check)| check.buffer == *buffer)
        .collect();
    let (_, first) = of_it.first()?;
    if !of_it.iter().all(|(_, check)| check.hash == first.hash) {
        return None;
    }
    of_it
        .iter()
        .any(|(at, _)| coverages.iter().all(|coverage| covers(coverage, at)))
        .then(|| first.hash.clone())
}

/// A v4 vote: who votes, on which proposal of which multisig.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct V4Vote {
    pub squad: Squad,
    pub proposal: Pubkey,
    pub member: Pubkey,
    /// It runs inside a programmatic signer's `Submit` or a Vector
    /// `Passthrough` that signs as its member, so its check runs there too,
    /// as `verifyProposalCpi`.
    pub carried: bool,
}

impl V4Vote {
    /// The form of the check that pins this vote.
    pub fn check_form(&self) -> CheckForm {
        if self.carried {
            CheckForm::Cpi
        } else {
            CheckForm::TopLevel
        }
    }
}

/// Votes on one v4 proposal that checks could pin, and what else the
/// transaction does, as far as the clear signing program cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenApproval {
    pub squad: Squad,
    pub proposal: Pubkey,
    /// Every vote on it, in order, each member once.
    pub votes: Vec<V4Vote>,
    /// It executes the proposal too.
    pub execute: bool,
    /// It starts with System `AdvanceNonceAccount`.
    pub nonce: bool,
    /// A vote, or the execution, runs inside a programmatic signer's
    /// `Submit`.
    pub in_submit: bool,
    /// A vote, or the execution, runs inside a Vector `Passthrough`, beside
    /// the transaction's `Advance`.
    pub in_passthrough: bool,
}

impl OpenApproval {
    pub fn facts(&self) -> ShapeFacts {
        ShapeFacts {
            votes: self.votes.len(),
            execute: self.execute,
            nonce: self.nonce,
            in_submit: self.in_submit,
            in_passthrough: self.in_passthrough,
        }
    }

    /// Something a keyless wallet's rebuild made, which has to be made
    /// again around the checks.
    pub fn wrapped(&self) -> bool {
        self.in_submit || self.in_passthrough
    }
}

/// What a transaction is to adding checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalShape {
    /// Not votes on one v4 proposal and its execution, or something else
    /// beside them, or a vote no check could reach: one inside a `Submit`
    /// or a `Passthrough` that does not sign as its member, or one beside a
    /// Vector `Advance` but outside a `Passthrough`.
    Other,
    /// Checks pin every vote and the execution already.
    Checked,
    /// Votes checks could pin, some of them or all not pinned yet.
    Open(OpenApproval),
}

/// The carriers around an instruction, as far as adding checks cares.
#[derive(Clone, Default)]
struct Around {
    in_submit: bool,
    in_passthrough: bool,
    /// What the innermost carrier signs as: a `Submit`'s authorities'
    /// addresses, a `Passthrough`'s accounts or their system PDAs.
    signs_as: Vec<Pubkey>,
}

impl Around {
    fn submit(&self, signs_as: Vec<Pubkey>) -> Self {
        Self {
            in_submit: true,
            signs_as,
            ..self.clone()
        }
    }

    fn passthrough(&self, signs_as: Vec<Pubkey>) -> Self {
        Self {
            in_passthrough: true,
            signs_as,
            ..self.clone()
        }
    }

    fn carried(&self) -> bool {
        self.in_submit || self.in_passthrough
    }
}

/// One instruction that runs, with the carriers it runs in opened.
struct Item {
    program: Pubkey,
    accounts: Vec<Pubkey>,
    data: Vec<u8>,
    around: Around,
}

struct Flat<N> {
    items: Vec<Item>,
    /// The leading `AdvanceNonceAccount`.
    nonce: Option<N>,
    /// A Vector `Advance` at the top.
    advance: bool,
}

impl<N> Default for Flat<N> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            nonce: None,
            advance: false,
        }
    }
}

fn is_nonce_advance(program: &Pubkey, data: &[u8]) -> bool {
    *program == SYSTEM_PROGRAM_ID && data.get(..4) == Some(&4u32.to_le_bytes())
}

fn is_check(program: &Pubkey, data: &[u8]) -> bool {
    *program == SQUADS_CLEAR_SIGNING_PROGRAM_ID
        && (data.starts_with(&VERIFY_PROPOSAL)
            || data.starts_with(&VERIFY_PROPOSAL_CPI)
            || data.starts_with(&VERIFY_BUFFER_HASH))
}

/// Whether the instruction at `at` of a list is left out of the plain
/// list, noting what it says in `flat`.
fn left_out<N>(top: bool, at: usize, program: &Pubkey, data: &[u8], flat: &mut Flat<N>) -> bool {
    if top && at == 0 && is_nonce_advance(program, data) {
        return true;
    }
    if top && *program == VECTOR_PROGRAM_ID && data.first() == Some(&ADVANCE_DISCRIMINATOR) {
        flat.advance = true;
        return true;
    }
    is_check(program, data) || *program == COMPUTE_BUDGET_PROGRAM_ID
}

fn flatten_explained(list: &[ExplainedIx], top: bool, around: &Around, flat: &mut Flat<()>) {
    for (at, instruction) in list.iter().enumerate() {
        if top && at == 0 && is_nonce_advance(&instruction.program_id, &instruction.data) {
            flat.nonce = Some(());
        }
        if left_out(top, at, &instruction.program_id, &instruction.data, flat) {
            continue;
        }
        match instruction.inner.as_ref().map(|inner| (&inner.kind, inner)) {
            Some((InnerKind::Authorization { authorities, .. }, inner)) => {
                let signs_as = authorities.iter().map(|key| key.address).collect();
                flatten_explained(&inner.instructions, false, &around.submit(signs_as), flat)
            }
            // Inside a `Submit`: signed for as its authorities.
            Some((InnerKind::Execution { .. }, inner)) => flatten_explained(
                &inner.instructions,
                false,
                &around.submit(around.signs_as.clone()),
                flat,
            ),
            Some((InnerKind::Passthrough { signers }, inner)) => {
                let signs_as = signers
                    .iter()
                    .map(|runs| runs.system.unwrap_or(runs.account))
                    .collect();
                flatten_explained(
                    &inner.instructions,
                    false,
                    &around.passthrough(signs_as),
                    flat,
                )
            }
            _ => flat.items.push(Item {
                program: instruction.program_id,
                accounts: instruction.accounts.clone(),
                data: instruction.data.clone(),
                around: around.clone(),
            }),
        }
    }
}

/// The plain list of `list`, carriers of a keyless wallet opened, into
/// `flat`, and the instructions themselves into `plain`.
fn flatten_decoded(
    list: Vec<Instruction>,
    top: bool,
    around: &Around,
    flat: &mut Flat<Instruction>,
    plain: &mut Vec<Instruction>,
) {
    for (at, instruction) in list.into_iter().enumerate() {
        if top && at == 0 && is_nonce_advance(&instruction.program_id, &instruction.data) {
            flat.nonce = Some(instruction);
            continue;
        }
        if left_out(top, at, &instruction.program_id, &instruction.data, flat) {
            continue;
        }
        match Carrier::open(&instruction) {
            Some((
                Carrier {
                    kind: CarrierKind::Programmatic { authorities, .. },
                    ..
                },
                inner,
            )) => {
                let signs_as = authorities
                    .iter()
                    .map(programmatic_signer_address)
                    .collect();
                flatten_decoded(inner, false, &around.submit(signs_as), flat, plain)
            }
            Some((
                Carrier {
                    kind: CarrierKind::Passthrough { signers },
                    ..
                },
                inner,
            )) => {
                let signs_as = signers.iter().map(|signer| signer.address()).collect();
                flatten_decoded(inner, false, &around.passthrough(signs_as), flat, plain)
            }
            _ => {
                flat.items.push(Item {
                    program: instruction.program_id,
                    accounts: instruction
                        .accounts
                        .iter()
                        .map(|meta| meta.pubkey)
                        .collect(),
                    data: instruction.data.clone(),
                    around: around.clone(),
                });
                plain.push(instruction);
            }
        }
    }
}

/// The votes and execution of one v4 proposal `items` are, if that is all
/// they are, and a check could reach each vote.
fn open_of(items: &[Item], nonce: bool, advance: bool) -> Option<OpenApproval> {
    let mut votes: Vec<V4Vote> = Vec::new();
    let mut execute: Option<(Pubkey, Pubkey)> = None;
    let (mut in_submit, mut in_passthrough) = (false, false);
    for item in items {
        if SquadsProgram::from_program_id(&item.program) != Some(SquadsProgram::V4) {
            return None;
        }
        let account = |at: usize| item.accounts.get(at).copied();
        match squads_call(SquadsProgram::V4, &item.data)? {
            SquadsCall::Approves { proposal } => {
                let member = account(1)?;
                // A check by CPI has to be signed by the member, so the
                // carrier the vote runs in has to sign as it.
                if item.around.carried() && !item.around.signs_as.contains(&member) {
                    return None;
                }
                votes.push(V4Vote {
                    squad: Squad {
                        program: SquadsProgram::V4,
                        address: account(0)?,
                    },
                    member,
                    proposal: account(proposal)?,
                    carried: item.around.carried(),
                })
            }
            SquadsCall::Runs {
                proposal,
                transaction: Some(_),
            } if execute.is_none() => execute = Some((account(0)?, account(proposal)?)),
            _ => return None,
        }
        in_submit |= item.around.in_submit;
        in_passthrough |= item.around.in_passthrough;
    }
    let first = *votes.first()?;
    for (at, vote) in votes.iter().enumerate() {
        let again = votes[..at].iter().any(|seen| seen.member == vote.member);
        if vote.squad != first.squad || vote.proposal != first.proposal || again {
            return None;
        }
    }
    if execute.is_some_and(|run| run != (first.squad.address, first.proposal)) {
        return None;
    }
    // A vote beside a Vector `Advance` that no `Passthrough` carries: no
    // check runs beside an `Advance` at the top, and none by CPI reaches it.
    if advance && !in_passthrough {
        return None;
    }
    // A keyless wallet's rebuild takes no durable nonce.
    if nonce && (in_submit || in_passthrough) {
        return None;
    }
    Some(OpenApproval {
        squad: first.squad,
        proposal: first.proposal,
        votes,
        execute: execute.is_some(),
        nonce,
        in_submit,
        in_passthrough,
    })
}

/// What `explained` is to adding checks: see [`ApprovalShape`]. The checks
/// already in it are left out of the reading, so votes some of them leave
/// blind are offered again.
pub fn approval_shape(explained: &ExplainedTx) -> ApprovalShape {
    let mut flat = Flat::default();
    flatten_explained(&explained.instructions, true, &Around::default(), &mut flat);
    let Some(open) = open_of(&flat.items, flat.nonce.is_some(), flat.advance) else {
        return ApprovalShape::Other;
    };
    let pinned = checked_in(explained)
        .into_iter()
        .find(|approval| approval.proposal == open.proposal);
    let all = pinned.is_some_and(|approval| {
        open.votes
            .iter()
            .all(|vote| approval.pins_vote_of(&vote.member))
            && (!open.execute || approval.execute.is_some())
    });
    if all {
        ApprovalShape::Checked
    } else {
        ApprovalShape::Open(open)
    }
}

/// A transaction of votes on one v4 proposal, read for adding checks: what
/// it is, and its instructions with a keyless wallet's carriers opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlainApproval {
    pub shape: OpenApproval,
    /// The leading `AdvanceNonceAccount`, when it lives on a durable nonce.
    pub nonce: Option<Instruction>,
    /// Every other instruction as a site or this app first wrote it, before
    /// a keyless wallet's rebuild: what a `Submit` or `Passthrough` carries
    /// is in its place, a Vector `Advance`, compute budget and the checks
    /// already there are left out.
    pub instructions: Vec<Instruction>,
}

/// `tx` read for adding checks, when it is votes on one v4 proposal and its
/// execution: see [`approval_shape`], which reads the same from what the
/// screen shows.
pub fn plain_approval(tx: &DecodedTx) -> Option<PlainApproval> {
    let mut flat = Flat::default();
    let mut instructions = Vec::new();
    flatten_decoded(
        tx.native_instructions(),
        true,
        &Around::default(),
        &mut flat,
        &mut instructions,
    );
    let shape = open_of(&flat.items, flat.nonce.is_some(), flat.advance)?;
    Some(PlainApproval {
        shape,
        nonce: flat.nonce,
        instructions,
    })
}

/// `plain`'s instructions with the checks that pin each of its votes and its
/// execution: one check per voting member ([`checks_for`]) carrying
/// `stored`, the vault transaction the proposal stores, and `buffers`, the
/// hash of what each upgrade in it deploys. A member that votes inside a
/// `Submit` or a `Passthrough` gets a `verifyProposalCpi`, which that
/// wallet's rebuild moves in beside the vote; any other a `verifyProposal`.
/// Refused when `accepts` says the program does not take the shape.
pub fn checked_instructions(
    plain: &PlainApproval,
    stored: &VaultTransaction,
    buffers: &[BufferHashCheck],
    accepts: ClearSigningAccepts,
) -> Result<Vec<Instruction>, ShapeRefused> {
    if let Some(refused) = accepts.refuses(plain.shape.facts()) {
        return Err(refused);
    }
    let members: Vec<(Pubkey, CheckForm)> = plain
        .shape
        .votes
        .iter()
        .map(|vote| (vote.member, vote.check_form()))
        .collect();
    let checks = checks_for(plain.shape.squad, stored, &members);
    Ok(place_checks(
        plain.nonce.clone(),
        &plain.instructions,
        &checks,
        buffers,
    ))
}

#[cfg(test)]
#[path = "checked_tests.rs"]
mod tests;
