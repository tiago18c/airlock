//! Who voted how on one proposal, and who has yet to vote.
//!
//! Each member is named as everywhere else, through [`Address`]: a wallet of
//! this device by its label, an address book entry by its name, else its
//! address. The acting member is marked "you", the other wallets of this
//! device "on this device". A line shows only when someone is on it.

use super::*;
use wallet_core::proposals::{vote_of, Standing, Vote, VoteCounts, Voter};
use wallet_core::squads::ProposalState;

/// The chips of a closed row that say where the vote stands: rejections and
/// cancel votes when there are any, and how the acting member voted.
/// Approvals have their own chip.
pub(super) fn vote_chips(
    proposal: &ProposalState,
    counts: &VoteCounts,
    acting: Option<Pubkey>,
) -> Vec<(&'static str, String, Option<String>)> {
    let mut chips = Vec::new();
    if proposal.status == ProposalStatus::Active && counts.rejected.count > 0 {
        chips.push((
            "chip warn",
            format!(
                "{} of {} rejected",
                counts.rejected.count, counts.rejected.needed
            ),
            Some(cutoff_words(counts)),
        ));
    }
    if proposal.status == ProposalStatus::Approved && counts.cancelled.count > 0 {
        chips.push((
            "chip warn",
            format!(
                "{} of {} voted to cancel",
                counts.cancelled.count, counts.cancelled.needed
            ),
            Some(format!(
                "It is cancelled once {} members vote to cancel it.",
                counts.cancelled.needed
            )),
        ));
    }
    match acting.and_then(|member| vote_of(proposal, &member)) {
        Some(Vote::Approved) => chips.push(("chip ok", "you approved".into(), None)),
        Some(Vote::Rejected) => chips.push(("chip warn", "you rejected".into(), None)),
        Some(Vote::Cancelled) => chips.push(("chip warn", "you voted to cancel".into(), None)),
        None => {}
    }
    chips
}

/// How many rejections reject a proposal, and why that many.
fn cutoff_words(counts: &VoteCounts) -> String {
    let needed = counts.rejected.needed;
    let rejections = if needed == 1 {
        "One rejection rejects it".to_string()
    } else {
        format!("It is rejected once {needed} members reject it")
    };
    format!(
        "{rejections}: the {} approvals it needs could then no longer be reached.",
        counts.approved.needed
    )
}

#[component]
pub(super) fn ProposalVotes(
    standing: Standing,
    status: ProposalStatus,
    acting: Option<Pubkey>,
) -> Element {
    let session = use_context::<Session>();
    let device: Vec<Pubkey> = (session.wallets)()
        .summaries
        .iter()
        .filter_map(|wallet| wallet.address)
        .collect();
    let gone = standing
        .approved
        .iter()
        .chain(&standing.rejected)
        .chain(&standing.cancelled)
        .any(|voter| !voter.member);
    let waiting: Vec<Voter> = standing
        .waiting
        .iter()
        .map(|key| Voter {
            key: *key,
            member: true,
        })
        .collect();
    let lines = [
        ("Approved by", standing.approved.clone()),
        ("Rejected by", standing.rejected.clone()),
        ("Voted to cancel", standing.cancelled.clone()),
        ("Yet to vote", waiting),
    ];
    let open_with_rejections = status == ProposalStatus::Active && !standing.rejected.is_empty();

    rsx! {
        div { class: "proposal-votes",
            for (name, voters) in lines.into_iter().filter(|(_, voters)| !voters.is_empty()) {
                p { key: "{name}", class: "chip-row",
                    span { class: "muted", "{name}" }
                    for voter in voters {
                        span { key: "{voter.key}", class: "proposal-voter",
                            Address { pubkey: voter.key, warn: !voter.member }
                            if acting == Some(voter.key) {
                                span { class: "chip", "you" }
                            } else if device.contains(&voter.key) {
                                span { class: "chip", "on this device" }
                            }
                            if !voter.member {
                                span { class: "chip warn", "no longer a member" }
                            }
                        }
                    }
                }
            }
            if open_with_rejections {
                p { class: "hint", "{cutoff_words(&standing.counts)}" }
            }
            if gone {
                p { class: "hint", "A member marked here left the multisig after voting. Its vote stays on the proposal, though a former member's vote to cancel no longer counts." }
            }
        }
    }
}
