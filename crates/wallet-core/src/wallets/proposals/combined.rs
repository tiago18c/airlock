//! Several members of one device finishing a proposal in one transaction.
//!
//! Which wallets approve together is a setting of the multisig's wallet
//! (`WalletKind::Squads::approves_with`). This module reads that setting,
//! decides whether those wallets can cast every vote a proposal still needs
//! and execute it, and builds the instructions. It ranks nobody: who votes,
//! and in which order, is what the setting holds.

use super::*;
use crate::ids::WalletId;
use crate::wallets::record::{Protocol, WalletKind, WalletRecord};

/// How a member's signature is obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A key in this vault. It signs the transaction.
    HotKey,
    /// A plain key on another device. It signs the transaction, so it can
    /// vote only on the multisig's durable nonce.
    Offline,
    /// A programmatic signer. Its authority signs the authorization.
    Programmatic {
        /// It has an executor nonce account, which can order the authorization.
        has_nonce: bool,
    },
    /// A Vector wallet. Its key signs the transaction's `Advance`, which
    /// carries its own nonce.
    Vector,
}

impl MemberKind {
    /// Which authorization it signs through, for a member without a key of
    /// its own: `Some(false)` a `Submit`, `Some(true)` an `Advance`.
    pub(crate) fn advances(self) -> Option<bool> {
        match self {
            Self::Programmatic { .. } => Some(false),
            Self::Vector => Some(true),
            Self::HotKey | Self::Offline => None,
        }
    }
}

/// A wallet of this device that votes for a multisig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChosenMember {
    pub wallet: WalletId,
    pub address: Pubkey,
    /// As the chain has them. Zero when the address is not a member there.
    pub permissions: u8,
    pub kind: MemberKind,
}

/// The acting member of `multisig` first, then its `approves_with`, in the
/// order they were set.
///
/// A hardware wallet or a multisig in the list is skipped. When the acting
/// member is one of those, or none is set, nobody is chosen.
pub fn chosen_members(
    wallets: &[WalletRecord],
    multisig: WalletId,
    state: &SquadState,
) -> Vec<ChosenMember> {
    let Some(WalletKind::Squads {
        member_wallet,
        approves_with,
        ..
    }) = wallets
        .iter()
        .find(|wallet| wallet.id == multisig)
        .map(|wallet| &wallet.kind)
    else {
        return Vec::new();
    };
    let chosen = |id: WalletId| {
        let wallet = wallets.iter().find(|wallet| wallet.id == id)?;
        let kind = match &wallet.kind {
            WalletKind::Keypair { .. } | WalletKind::Seed { .. } => MemberKind::HotKey,
            WalletKind::Offline { .. } => MemberKind::Offline,
            WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer { nonce, .. },
                ..
            } => MemberKind::Programmatic {
                has_nonce: nonce.is_some(),
            },
            WalletKind::Programmatic {
                protocol: Protocol::Vector { .. },
                ..
            } => MemberKind::Vector,
            WalletKind::Hardware { .. } | WalletKind::Squads { .. } => return None,
        };
        let address = wallet.address()?;
        // Membership is what the chain says, not what the wallet remembers.
        let permissions = state
            .members
            .iter()
            .find(|member| member.key == address)
            .map(|member| member.permissions)
            .unwrap_or(0);
        Some(ChosenMember {
            wallet: id,
            address,
            permissions,
            kind,
        })
    };
    let Some(acting) = member_wallet.and_then(chosen) else {
        return Vec::new();
    };
    let mut members = vec![acting];
    for id in approves_with {
        let Some(member) = chosen(*id) else {
            continue;
        };
        if !members.iter().any(|known| known.address == member.address) {
            members.push(member);
        }
    }
    members
}

/// Several members of this device finishing a proposal in one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombinedApproval {
    /// The members that approve, in the order of their instructions.
    pub voters: Vec<Pubkey>,
    /// The member that executes.
    pub executor: Pubkey,
    /// The durable nonce the transaction advances. Set only when an offline
    /// member signs it.
    pub durable_nonce: Option<Pubkey>,
}

impl CombinedApproval {
    /// The members that sign for it: the voters, then the executor when it
    /// does not vote.
    pub fn signers(&self) -> Vec<Pubkey> {
        let mut signers = self.voters.clone();
        if !signers.contains(&self.executor) {
            signers.push(self.executor);
        }
        signers
    }
}

/// Whether `chosen` can cast every approval `entry` still needs and execute
/// it, in one transaction. `approval_nonce` is the multisig row's durable
/// nonce, if it names one.
///
/// This is the first [`proposal_shapes`] shape that executes, when it needs
/// at least two votes. `None` when one approval is missing:
/// [`available_action`] already offers that as approve and execute.
pub fn combined_approval(
    state: &SquadState,
    entry: &ProposalEntry,
    chosen: &[ChosenMember],
    approval_nonce: Option<Pubkey>,
) -> Option<CombinedApproval> {
    let proposal = entry.proposal.as_ref()?;
    let votable = proposal.status == ProposalStatus::Active
        && !entry.stale
        && matches!(entry.stored, Stored::Vault(_));
    if !votable || state.time_lock != 0 {
        return None;
    }
    let missing = usize::from(state.threshold).saturating_sub(proposal.approved.len());
    if missing < 2 {
        return None;
    }
    let shape = proposal_shapes_with(state, &proposal.approved, chosen, approval_nonce.is_some())
        .into_iter()
        .next()?;
    let executor = shape.executor?;
    if shape.voters.len() < missing {
        return None;
    }
    let involved: Vec<&ChosenMember> = chosen
        .iter()
        .filter(|member| shape.voters.contains(&member.address) || member.address == executor)
        .collect();
    let offline = involved
        .iter()
        .any(|member| member.kind == MemberKind::Offline);
    let programmatic = involved
        .iter()
        .any(|member| matches!(member.kind, MemberKind::Programmatic { .. }));
    let keyless = involved
        .iter()
        .any(|member| member.kind.advances().is_some());
    // The rebuild for a member without a key refuses a durable nonce, and an
    // offline key cannot sign in time without one.
    if offline && keyless {
        return None;
    }
    let ordered = involved
        .iter()
        .any(|member| member.kind == MemberKind::Programmatic { has_nonce: true });
    if programmatic && !ordered {
        return None;
    }
    // One `Submit` carries every programmatic member's instructions, so
    // they sit together. Without a create step some order always does. The
    // Vector members' sit together too when they can, for one `Passthrough`.
    let advances = |address: &Pubkey| {
        chosen
            .iter()
            .find(|member| member.address == *address)
            .and_then(|member| member.kind.advances())
    };
    let voters = keyless_together(None, &shape.voters, Some(executor), advances, true)
        .or_else(|| keyless_together(None, &shape.voters, Some(executor), advances, false))?;
    Some(CombinedApproval {
        voters,
        executor,
        durable_nonce: approval_nonce.filter(|_| offline),
    })
}

/// `approve` for each voter, then `execute`. `None` when the stored
/// transaction could not be read.
pub fn combined_instructions(
    squad: &Squad,
    entry: &ProposalEntry,
    combined: &CombinedApproval,
) -> Option<Vec<Instruction>> {
    let Stored::Vault(stored) = &entry.stored else {
        return None;
    };
    let mut instructions: Vec<Instruction> = combined
        .voters
        .iter()
        .map(|voter| squad.approve(entry.index, voter))
        .collect();
    instructions.push(squad.execute_stored(stored, &combined.executor));
    Some(instructions)
}

/// What the Proposals screen asks for on one proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalRequest {
    Single {
        member: Pubkey,
        action: ProposalAction,
    },
    Combined(CombinedApproval),
}

impl ProposalRequest {
    /// The instructions of the request on `entry`. `None` when it needs the
    /// stored transaction and it could not be read.
    pub fn instructions(&self, squad: &Squad, entry: &ProposalEntry) -> Option<Vec<Instruction>> {
        match self {
            Self::Single { member, action } => action_instructions(squad, entry, member, *action),
            Self::Combined(combined) => combined_instructions(squad, entry, combined),
        }
    }
}
