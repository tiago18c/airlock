//! Proposing a change to the multisig itself from this device: adding or
//! removing a member, changing the threshold, setting the rent collector.
//!
//! A change is stored as the multisig's next transaction, opened for votes,
//! approved by the acting member and the wallets set to approve together
//! with it, and executed in the same transaction when those votes already
//! reach the threshold and no time lock holds it. These are the shapes a
//! payment from the vault takes when the multisig sends (see
//! [`proposal_shapes`]), with a change stored in place of a vault
//! transaction, and the instructions of members without a key kept
//! together the same way ([`keyless_together`]), so their authorizations
//! wrap them as they wrap any other.
//!
//! Before any of it is built, the change is vetted on the multisig as read
//! ([`vet_change`]): one the program would refuse to keep, one that changes
//! nothing, and any change at all to a controlled multisig (whose authority
//! changes it directly, and which both programs refuse to take by proposal)
//! is never proposed. Who proposes is checked too ([`change_proposal`]):
//! the acting member needs the permission to propose.
//!
//! The transaction number is the multisig's next one as read when the
//! transaction is built. Another proposal that lands first takes it, and
//! this one then fails on chain, as a payment proposed from the vault does.

use super::*;
use crate::wallets::squads::{ChangeOutcome, ConfigAction, Said, SquadsMember, SquadsProgram};

/// Why a change is not proposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CannotPropose {
    /// Its config (v4) or settings authority changes it directly: both
    /// programs refuse a change to it by proposal.
    Controlled,
    /// Nothing was asked for yet.
    NoActions,
    /// The multisig after it would be the multisig now.
    ChangesNothing,
    /// An action this program does not have, such as a rent collector on
    /// the smart account program.
    NotOnThisProgram,
    /// The program would refuse to execute it.
    Refused(ChangeRefusal),
    /// No wallet of this device acts as a member.
    NoMember,
    /// The acting member is not a member, as the chain says.
    NotAMember(Pubkey),
    /// The acting member may not propose.
    MayNotPropose(Pubkey),
    /// The acting member is a programmatic signer without a nonce account,
    /// which its authorization needs.
    NoNonce(Pubkey),
}

impl CannotPropose {
    /// One short sentence, each address named apart.
    pub fn said(&self) -> Said {
        match self {
            Self::Controlled => {
                Said::new("Its authority changes this multisig directly, without proposals.")
            }
            Self::NoActions => Said::new("Nothing to propose yet."),
            Self::ChangesNothing => Said::new("This changes nothing."),
            Self::NotOnThisProgram => Said::new("This program has no such setting."),
            Self::Refused(refusal) => refusal_said(refusal),
            Self::NoMember => Said::new("No wallet of this device acts as a member."),
            Self::NotAMember(key) => Said::default().k(*key).t(" is not a member."),
            Self::MayNotPropose(key) => Said::default().k(*key).t(" may not propose."),
            Self::NoNonce(key) => Said::default().k(*key).t(" has no nonce account yet."),
        }
    }
}

/// What the program refuses, in one short sentence.
fn refusal_said(refusal: &ChangeRefusal) -> Said {
    match refusal {
        ChangeRefusal::NotAMember(key) => Said::default().k(*key).t(" is not a member."),
        ChangeRefusal::AlreadyAMember(key) => Said::default().k(*key).t(" is a member already."),
        ChangeRefusal::UnknownPermission(key) => Said::new("")
            .k(*key)
            .t(" is given a permission that does not exist."),
        other => Said::new(other.to_string()),
    }
}

/// What the form of a change asks for. Each field left as it is asks for
/// nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeDraft {
    pub add: Option<SquadsMember>,
    pub remove: Option<Pubkey>,
    pub threshold: Option<u16>,
    /// `Some(None)` clears it.
    pub rent_collector: Option<Option<Pubkey>>,
}

impl ChangeDraft {
    /// The actions of one change, in the order they run: the member added,
    /// the one removed, then the threshold, so a member added and the
    /// threshold raised to count it go in one change (two would make the
    /// second stale once the first executes). A threshold or rent collector
    /// the same as `state` has is left out.
    pub fn actions(&self, state: &SquadState) -> Vec<ConfigAction> {
        let mut actions = Vec::new();
        if let Some(member) = self.add {
            actions.push(ConfigAction::AddMember(member));
        }
        if let Some(key) = self.remove {
            actions.push(ConfigAction::RemoveMember(key));
        }
        if let Some(threshold) = self.threshold.filter(|new| *new != state.threshold) {
            actions.push(ConfigAction::ChangeThreshold(threshold));
        }
        if let Some(collector) = self
            .rent_collector
            .filter(|new| *new != state.rent_collector)
        {
            actions.push(ConfigAction::SetRentCollector(collector));
        }
        actions
    }
}

/// Whether `actions` can be proposed to `state`, a multisig of `program`,
/// at time `now`: the multisig after it, or why not.
pub fn vet_change(
    program: SquadsProgram,
    state: &SquadState,
    actions: &[ConfigAction],
    now: i64,
) -> Result<ChangeOutcome, CannotPropose> {
    if state.config_authority.is_some() {
        return Err(CannotPropose::Controlled);
    }
    if actions.is_empty() {
        return Err(CannotPropose::NoActions);
    }
    let elsewhere = actions.iter().any(|action| match action {
        ConfigAction::SetRentCollector(_) => program != SquadsProgram::V4,
        ConfigAction::SetArchivalAuthority(_) => program != SquadsProgram::SmartAccount,
        ConfigAction::Policy { .. } => true,
        _ => false,
    });
    if elsewhere {
        return Err(CannotPropose::NotOnThisProgram);
    }
    let outcome = change_outcome(program, state, actions, now).map_err(CannotPropose::Refused)?;
    let mut members = state.members.clone();
    members.sort_by_key(|member| member.key);
    let same = members == outcome.members
        && state.threshold == outcome.threshold
        && state.time_lock == outcome.time_lock
        && state.rent_collector == outcome.rent_collector;
    if same {
        return Err(CannotPropose::ChangesNothing);
    }
    Ok(outcome)
}

/// A change to propose, and how far one transaction takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeProposal {
    /// The transaction number it is stored under.
    pub index: u64,
    /// The acting member, which stores it and opens it for votes.
    pub creator: Pubkey,
    /// Members that approve it, in the order of their instructions.
    pub voters: Vec<Pubkey>,
    /// The member that executes it. `None` leaves it for later.
    pub executor: Option<Pubkey>,
    /// A wallet that signs on another device signs it, so it goes on a
    /// durable nonce when one is at hand.
    pub offline: bool,
}

impl ChangeProposal {
    /// The members that sign: the creator, the voters, then the executor.
    pub fn signers(&self) -> Vec<Pubkey> {
        let mut signers = vec![self.creator];
        for key in self.voters.iter().chain(&self.executor) {
            if !signers.contains(key) {
                signers.push(*key);
            }
        }
        signers
    }

    /// Store `actions`, open them for votes, approve, execute. `payer` pays
    /// the rent of the new accounts and what executing costs. `None` when an
    /// action cannot be stored on `squad`'s program.
    pub fn instructions(
        &self,
        squad: &Squad,
        actions: &[ConfigAction],
        payer: &Pubkey,
    ) -> Option<Vec<Instruction>> {
        let mut instructions = vec![
            squad.create_change(self.index, actions, &self.creator, payer)?,
            squad.create_proposal(self.index, &self.creator, payer),
        ];
        instructions.extend(
            self.voters
                .iter()
                .map(|voter| squad.approve(self.index, voter)),
        );
        if let Some(executor) = &self.executor {
            let change = ConfigTransaction {
                program: squad.program,
                creator: self.creator,
                index: self.index,
                actions: actions.to_vec(),
            };
            instructions.push(squad.execute_change(&change, executor, payer)?);
        }
        Some(instructions)
    }
}

/// How `chosen` (the acting member first, then the wallets that approve
/// together with it, as the chain knows them) proposes a change to `state`.
/// `approval_nonce` is the durable nonce the multisig's row names for
/// approving together, which lets an offline wallet vote beside the acting
/// member.
///
/// The acting member stores and opens it, so it must be a member that may
/// propose, and a programmatic signer needs its nonce account. The others
/// vote as far as they can: an offline wallet only on the row's durable
/// nonce, never beside a member without a key (whose authorization cannot
/// go on a durable nonce), and a programmatic signer only with a nonce
/// account of its own. The shape that does the most and keeps the keyless
/// members' instructions together is taken.
pub fn change_proposal(
    state: &SquadState,
    chosen: &[ChosenMember],
    approval_nonce: Option<Pubkey>,
) -> Result<ChangeProposal, CannotPropose> {
    if state.config_authority.is_some() {
        return Err(CannotPropose::Controlled);
    }
    let acting = chosen.first().ok_or(CannotPropose::NoMember)?;
    if acting.permissions == 0 {
        return Err(CannotPropose::NotAMember(acting.address));
    }
    if acting.permissions & PERMISSION_INITIATE == 0 {
        return Err(CannotPropose::MayNotPropose(acting.address));
    }
    if acting.kind == (MemberKind::Programmatic { has_nonce: false }) {
        return Err(CannotPropose::NoNonce(acting.address));
    }
    let acting_offline = acting.kind == MemberKind::Offline;
    let acting_keyless = acting.kind.advances().is_some();
    let mut others: Vec<ChosenMember> = chosen[1..]
        .iter()
        .filter(|member| match member.kind {
            MemberKind::HotKey => true,
            MemberKind::Offline => approval_nonce.is_some() && !acting_keyless,
            MemberKind::Programmatic { has_nonce } => has_nonce && !acting_offline,
            MemberKind::Vector => !acting_offline,
        })
        .cloned()
        .collect();
    if others.iter().any(|member| member.kind.advances().is_some()) {
        others.retain(|member| member.kind != MemberKind::Offline);
    }
    let usable: Vec<ChosenMember> = std::iter::once(acting.clone()).chain(others).collect();
    let advances = |address: &Pubkey| {
        usable
            .iter()
            .find(|member| member.address == *address)
            .and_then(|member| member.kind.advances())
    };
    let creator = acting.address;
    let shape = proposal_shapes_with(state, &[], &usable, true)
        .into_iter()
        .find_map(|shape| {
            keyless_together(Some(creator), &shape.voters, shape.executor, advances, true)
                .or_else(|| {
                    keyless_together(
                        Some(creator),
                        &shape.voters,
                        shape.executor,
                        advances,
                        false,
                    )
                })
                .map(|voters| ProposalShape {
                    voters,
                    executor: shape.executor,
                })
        })
        // Storing and opening alone always keeps together.
        .unwrap_or(ProposalShape {
            voters: Vec::new(),
            executor: None,
        });
    let offline = usable.iter().any(|member| {
        member.kind == MemberKind::Offline
            && (member.address == creator
                || shape.voters.contains(&member.address)
                || shape.executor == Some(member.address))
    });
    Ok(ChangeProposal {
        index: state.transaction_index.saturating_add(1),
        creator,
        voters: shape.voters,
        executor: shape.executor,
        offline,
    })
}
