//! What one rebuilt transaction does to a new proposal, or what a combined
//! approval does to an open one: who votes, and who executes.

use super::*;

/// Members of `chosen` that may cast a vote this transaction still needs.
///
/// A member with [`PERMISSION_VOTE`], not already in `approved`, that is not
/// offline unless `offline_ok`. Programmatic and Vector members may share one
/// transaction: the programmatic authorities sign first, then the Vector keys.
pub fn usable_voters<'a>(
    chosen: &'a [ChosenMember],
    approved: &[Pubkey],
    offline_ok: bool,
) -> Vec<&'a ChosenMember> {
    chosen
        .iter()
        .filter(|member| {
            (member.kind != MemberKind::Offline || offline_ok)
                && member.permissions & PERMISSION_VOTE != 0
                && !approved.contains(&member.address)
        })
        .collect()
}

/// Who executes among `voters`, or another usable member of `chosen` with
/// [`PERMISSION_EXECUTE`].
pub fn executor_for<'a>(
    voters: &[&'a ChosenMember],
    chosen: &'a [ChosenMember],
    offline_ok: bool,
) -> Option<&'a ChosenMember> {
    let usable = |member: &&ChosenMember| member.kind != MemberKind::Offline || offline_ok;
    let may_execute = |member: &&ChosenMember| member.permissions & PERMISSION_EXECUTE != 0;
    voters
        .iter()
        .copied()
        .find(may_execute)
        .or_else(|| chosen.iter().filter(usable).find(may_execute))
}

/// What one transaction does to a proposal: who votes, and who executes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalShape {
    /// Members that approve, in the order of their instructions.
    pub voters: Vec<Pubkey>,
    /// The member that executes. `None` leaves the proposal for later.
    pub executor: Option<Pubkey>,
}

/// Every shape the members can carry out, the one that does the most first.
///
/// Offline members are left out: a site rebuild refuses a durable nonce.
pub fn proposal_shapes(
    state: &SquadState,
    approved: &[Pubkey],
    chosen: &[ChosenMember],
) -> Vec<ProposalShape> {
    proposal_shapes_with(state, approved, chosen, false)
}

/// Like [`proposal_shapes`], with offline members usable when `offline_ok`.
pub(crate) fn proposal_shapes_with(
    state: &SquadState,
    approved: &[Pubkey],
    chosen: &[ChosenMember],
    offline_ok: bool,
) -> Vec<ProposalShape> {
    let missing = usize::from(state.threshold).saturating_sub(approved.len());
    let voters: Vec<&ChosenMember> = usable_voters(chosen, approved, offline_ok)
        .into_iter()
        .take(missing)
        .collect();

    let mut shapes = Vec::new();
    for count in (0..=voters.len()).rev() {
        let slice = &voters[..count];
        let reaches = approved.len() + count >= usize::from(state.threshold);
        if reaches && state.time_lock == 0 {
            if let Some(executor) = executor_for(slice, chosen, offline_ok) {
                shapes.push(ProposalShape {
                    voters: slice.iter().map(|member| member.address).collect(),
                    executor: Some(executor.address),
                });
            }
        }
        shapes.push(ProposalShape {
            voters: slice.iter().map(|member| member.address).collect(),
            executor: None,
        });
    }
    shapes.dedup();
    shapes
}

/// An order of `voters`' approvals that keeps the instructions of members
/// without a key together, by what they sign through (see
/// [`MemberKind::advances`]). Every instruction a programmatic member signs
/// sits next to the others, so one `Submit` carries them without running
/// anything out of order. When `vector_too`, so does every instruction a
/// Vector member signs, so one `Passthrough` carries them and the `Submit`
/// does not split them.
///
/// The instructions are create and propose by `creator` when it is set, the
/// approvals, then execute by `executor` when it is set. `None` when no order
/// of the approvals does it: for programmatic members, a programmatic creator
/// and executor with another member's vote that has to fall between them.
pub fn keyless_together(
    creator: Option<Pubkey>,
    voters: &[Pubkey],
    executor: Option<Pubkey>,
    advances: impl Fn(&Pubkey) -> Option<bool>,
    vector_too: bool,
) -> Option<Vec<Pubkey>> {
    let of = |kind: Option<bool>| -> Vec<Pubkey> {
        voters
            .iter()
            .copied()
            .filter(|voter| advances(voter) == kind)
            .collect()
    };
    let (programmatic, vector, others) = (of(Some(false)), of(Some(true)), of(None));
    let rest: Vec<Pubkey> = voters
        .iter()
        .copied()
        .filter(|voter| advances(voter) != Some(false))
        .collect();
    let blocks = |order: [&Vec<Pubkey>; 3]| -> Vec<Pubkey> {
        order.into_iter().flatten().copied().collect()
    };
    // The row's order when it already works, so nothing moves needlessly.
    // Then the programmatic votes first or last, then every kind in a block.
    let candidates = [
        voters.to_vec(),
        blocks([&programmatic, &rest, &Vec::new()]),
        blocks([&rest, &programmatic, &Vec::new()]),
        blocks([&programmatic, &vector, &others]),
        blocks([&programmatic, &others, &vector]),
        blocks([&vector, &programmatic, &others]),
        blocks([&vector, &others, &programmatic]),
        blocks([&others, &programmatic, &vector]),
        blocks([&others, &vector, &programmatic]),
    ];
    candidates.into_iter().find(|order| {
        let signers: Vec<Pubkey> = creator
            .into_iter()
            .chain(creator)
            .chain(order.iter().copied())
            .chain(executor)
            .collect();
        let together = |kind: bool| {
            let is = |address: &Pubkey| advances(address) == Some(kind);
            let start = signers.iter().position(is);
            let end = signers.iter().rposition(is);
            match (start, end) {
                (Some(start), Some(end)) => signers[start..=end].iter().all(is),
                _ => true,
            }
        };
        together(false) && (!vector_too || together(true))
    })
}
