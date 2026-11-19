//! Reading one page of a multisig's proposals, and acting on one of them.

use app_ui_kit::actions::sentence;
use app_ui_kit::format_sol;
use app_ui_kit::{Session, SignLoad, TxSource};
use solana_pubkey::Pubkey;
use wallet_core::proposals::{
    available_cancel, available_rejection, rent_returned, ProposalAction, ProposalEntry,
    ProposalRequest,
};
use wallet_core::squads::{Squad, SquadState};

/// Read the multisig, and one page of its transactions starting at `from`,
/// or at the newest when `from` is `None`.
pub async fn load_page(
    session: Session,
    squad: &Squad,
    from: Option<u64>,
) -> Result<(SquadState, Vec<ProposalEntry>), String> {
    let rpc = session.rpc().map_err(sentence)?;
    wallet_flows::squads::load_page(rpc.as_ref(), squad, from)
        .await
        .map_err(sentence)
}

/// Compile what `request` asks for, on a fresh blockhash or on the combined
/// approval's durable nonce, and open it on the Sign screen. `payer` pays
/// the fee, and `label` is the multisig's name for the note.
///
/// A member that signs on another device, through QR codes, takes longer
/// than a blockhash lives. Its transaction goes on that key's own durable
/// nonce when its wallet has one, and otherwise on `offline_nonce`, the
/// multisig row's durable nonce, which the caller passes when the member
/// acting alone is such a key.
#[allow(clippy::too_many_arguments)]
pub async fn open_request(
    session: Session,
    squad: &Squad,
    state: &SquadState,
    entry: &ProposalEntry,
    request: &ProposalRequest,
    payer: Pubkey,
    label: &str,
    offline_nonce: Option<Pubkey>,
) -> Result<(), String> {
    let rpc = session.rpc().map_err(sentence)?;
    let (wire, last_valid_height) = wallet_flows::squads::compile_request(
        rpc.as_ref(),
        squad,
        entry,
        request,
        payer,
        |tx| session.offline_nonce_for(tx),
        offline_nonce,
    )
    .await
    .map_err(sentence)?;
    session.open_in_sign(
        SignLoad::new(wire, TxSource::Wallets)
            .with_note(request_note(request, state, entry, label))
            .expiring_at(last_valid_height),
    );
    Ok(())
}

/// What `request` does to `entry` of the multisig `label`, whose state is
/// `state`, for the note above the transaction. A rejection or a vote to
/// cancel says what it changes, as the row's hint did before it was pressed.
pub fn request_note(
    request: &ProposalRequest,
    state: &SquadState,
    entry: &ProposalEntry,
    label: &str,
) -> String {
    let index = entry.index;
    match request {
        ProposalRequest::Single {
            member,
            action: ProposalAction::Reject,
        } => {
            let mut note = format!("This rejects proposal {index} of {label}.");
            if let Some(rejection) = available_rejection(state, entry, member) {
                if rejection.replaces_approval {
                    note.push_str(" It takes back the member's approval.");
                }
                if rejection.last() {
                    note.push_str(
                        " It is the last rejection needed: the proposal is rejected for good and cannot be reopened.",
                    );
                }
            }
            note
        }
        ProposalRequest::Single {
            member,
            action: ProposalAction::Cancel,
        } => {
            let mut note = format!("This votes to cancel proposal {index} of {label}.");
            if available_cancel(state, entry, member).is_some_and(|cancel| cancel.last()) {
                note.push_str(
                    " It is the last vote to cancel needed: the proposal is cancelled for good and can never be executed.",
                );
            }
            note
        }
        ProposalRequest::Single {
            action: ProposalAction::Activate,
            ..
        } => format!("This opens proposal {index} of {label} for votes. It approves nothing."),
        ProposalRequest::Single { action, .. } => {
            let what = match action {
                ProposalAction::Approve => "approves",
                ProposalAction::Execute => "executes",
                ProposalAction::ApproveAndExecute => "approves and executes",
                ProposalAction::Activate => "opens",
                ProposalAction::Reject => "rejects",
                ProposalAction::Cancel => "votes to cancel",
            };
            format!("This {what} proposal {index} of {label}.")
        }
        ProposalRequest::Close(closings) => {
            let what = match closings.as_slice() {
                [one] => format!("proposal {} of {label}", one.index),
                many => format!(
                    "{} proposals of {label} ({})",
                    many.len(),
                    many.iter()
                        .map(|closing| format!("#{}", closing.index))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            let to = rent_returned(closings)
                .into_iter()
                .map(|(collector, lamports)| format!("{} to {collector}", format_sol(lamports)))
                .collect::<Vec<_>>()
                .join(" and ");
            format!(
                "This closes the accounts of {what} and returns their rent, {to}. What they proposed and who voted leave the chain."
            )
        }
        ProposalRequest::Combined(combined) => {
            let mut note = format!(
                "This approves proposal {index} of {label} as {} members and executes it.",
                combined.voters.len()
            );
            if let Some(nonce) = combined.durable_nonce {
                note.push_str(&format!(
                    " It uses the durable nonce {nonce}, so the offline wallets can sign it at any time."
                ));
            }
            note
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::proposals::{CombinedApproval, Stored};
    use wallet_core::squads::{ProposalState, ProposalStatus, SquadsMember, PERMISSION_ALL};

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    /// Three members, 2 of 3: two rejections reject a proposal.
    fn team() -> SquadState {
        SquadState {
            threshold: 2,
            time_lock: 0,
            transaction_index: 4,
            stale_transaction_index: 0,
            members: (1..=3)
                .map(|n| SquadsMember {
                    key: pk(n),
                    permissions: PERMISSION_ALL,
                })
                .collect(),
            config_authority: None,
            rent_collector: None,
        }
    }

    /// Proposal 4, open, with these votes. What it stores does not matter.
    fn open(approved: &[Pubkey], rejected: &[Pubkey]) -> ProposalEntry {
        ProposalEntry {
            index: 4,
            proposal: Some(ProposalState {
                index: 4,
                status: ProposalStatus::Active,
                since: 0,
                approved: approved.to_vec(),
                rejected: rejected.to_vec(),
                cancelled: Vec::new(),
            }),
            stored: Stored::Other,
            stale: false,
            accounts: Default::default(),
        }
    }

    fn note(request: &ProposalRequest, entry: &ProposalEntry) -> String {
        request_note(request, &team(), entry, "Treasury")
    }

    #[test]
    fn the_note_says_what_one_member_does_as_it_always_did() {
        let single = |action| ProposalRequest::Single {
            member: pk(1),
            action,
        };
        let entry = open(&[], &[]);
        assert_eq!(
            note(&single(ProposalAction::Approve), &entry),
            "This approves proposal 4 of Treasury."
        );
        assert_eq!(
            note(&single(ProposalAction::Execute), &entry),
            "This executes proposal 4 of Treasury."
        );
        assert_eq!(
            note(&single(ProposalAction::ApproveAndExecute), &entry),
            "This approves and executes proposal 4 of Treasury."
        );
    }

    #[test]
    fn the_note_of_a_rejection_says_what_it_takes_back_and_whether_it_is_final() {
        let reject = ProposalRequest::Single {
            member: pk(1),
            action: ProposalAction::Reject,
        };
        assert_eq!(
            note(&reject, &open(&[], &[])),
            "This rejects proposal 4 of Treasury."
        );
        assert_eq!(
            note(&reject, &open(&[pk(1)], &[])),
            "This rejects proposal 4 of Treasury. It takes back the member's approval."
        );
        assert_eq!(
            note(&reject, &open(&[], &[pk(2)])),
            "This rejects proposal 4 of Treasury. It is the last rejection needed: the proposal is rejected for good and cannot be reopened."
        );
        assert_eq!(
            note(&reject, &open(&[pk(1)], &[pk(2)])),
            "This rejects proposal 4 of Treasury. It takes back the member's approval. It is the last rejection needed: the proposal is rejected for good and cannot be reopened."
        );
    }

    #[test]
    fn the_note_of_an_activation_says_it_approves_nothing() {
        let mut draft = open(&[], &[]);
        draft.proposal.as_mut().unwrap().status = ProposalStatus::Draft;
        let activate = ProposalRequest::Single {
            member: pk(1),
            action: ProposalAction::Activate,
        };
        assert_eq!(
            note(&activate, &draft),
            "This opens proposal 4 of Treasury for votes. It approves nothing."
        );
    }

    #[test]
    fn the_note_of_a_cancel_vote_says_whether_it_is_final() {
        let approved = |cancelled: &[Pubkey]| {
            let mut entry = open(&[pk(2), pk(3)], &[]);
            let proposal = entry.proposal.as_mut().unwrap();
            proposal.status = ProposalStatus::Approved;
            proposal.cancelled = cancelled.to_vec();
            entry
        };
        let cancel = ProposalRequest::Single {
            member: pk(1),
            action: ProposalAction::Cancel,
        };
        assert_eq!(
            note(&cancel, &approved(&[])),
            "This votes to cancel proposal 4 of Treasury."
        );
        assert_eq!(
            note(&cancel, &approved(&[pk(2)])),
            "This votes to cancel proposal 4 of Treasury. It is the last vote to cancel needed: the proposal is cancelled for good and can never be executed."
        );
    }

    #[test]
    fn the_note_of_a_close_says_what_it_closes_and_where_the_rent_goes() {
        use wallet_core::proposals::Closing;
        use wallet_core::squads::CloseKind;
        let closing = |index, returns: Vec<(Pubkey, u64)>| Closing {
            index,
            kind: CloseKind::Vault,
            proposal_collector: pk(8),
            transaction_collector: pk(8),
            returns,
        };
        let entry = open(&[], &[]);
        assert_eq!(
            note(
                &ProposalRequest::Close(vec![closing(4, vec![(pk(8), 3_000_000)])]),
                &entry
            ),
            format!(
                "This closes the accounts of proposal 4 of Treasury and returns their rent, 0.003 SOL to {}. What they proposed and who voted leave the chain.",
                pk(8)
            )
        );
        assert_eq!(
            note(
                &ProposalRequest::Close(vec![
                    closing(4, vec![(pk(8), 1_000_000)]),
                    closing(2, vec![(pk(8), 1_000_000), (pk(9), 500_000_000)]),
                ]),
                &entry
            ),
            format!(
                "This closes the accounts of 2 proposals of Treasury (#4, #2) and returns their rent, 0.002 SOL to {} and 0.5 SOL to {}. What they proposed and who voted leave the chain.",
                pk(8),
                pk(9)
            )
        );
    }

    #[test]
    fn the_note_of_a_combined_approval_counts_the_members_and_names_the_durable_nonce() {
        let mut combined = CombinedApproval {
            voters: vec![pk(1), pk(2)],
            executor: pk(1),
            durable_nonce: None,
        };
        let entry = open(&[], &[]);
        assert_eq!(
            note(&ProposalRequest::Combined(combined.clone()), &entry),
            "This approves proposal 4 of Treasury as 2 members and executes it."
        );
        combined.durable_nonce = Some(pk(7));
        assert_eq!(
            note(&ProposalRequest::Combined(combined), &entry),
            format!(
                "This approves proposal 4 of Treasury as 2 members and executes it. It uses the durable nonce {}, so the offline wallets can sign it at any time.",
                pk(7)
            )
        );
    }
}
