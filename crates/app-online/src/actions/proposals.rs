//! Reading one page of a multisig's proposals, and acting on one of them.

use app_ui_kit::actions::sentence;
use app_ui_kit::{Session, TxSource};
use solana_pubkey::Pubkey;
use wallet_core::proposals::{ProposalAction, ProposalEntry, ProposalRequest};
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
pub async fn open_request(
    session: Session,
    squad: &Squad,
    entry: &ProposalEntry,
    request: &ProposalRequest,
    payer: Pubkey,
    label: &str,
) -> Result<(), String> {
    let instructions = request.instructions(squad, entry).ok_or_else(|| {
        "The stored transaction could not be read, so there is nothing to build.".to_string()
    })?;
    let rpc = session.rpc().map_err(sentence)?;
    let durable = match request {
        ProposalRequest::Combined(combined) => combined
            .durable_nonce
            .map(|nonce| (nonce, combined.signers())),
        ProposalRequest::Single { .. } => None,
    };
    // A durable nonce has no height at which it expires.
    let (wire, last_valid_height) = match durable {
        Some((nonce, members)) => {
            let signers: Vec<Pubkey> = std::iter::once(payer).chain(members).collect();
            let wire = wallet_flows::compile::compile_on_nonce(
                rpc.as_ref(),
                payer,
                instructions,
                nonce,
                &signers,
            )
            .await
            .map_err(sentence)?;
            (wire, None)
        }
        None => {
            let (wire, height) =
                wallet_flows::compile::compile_fresh(rpc.as_ref(), payer, instructions)
                    .await
                    .map_err(sentence)?;
            (wire, Some(height))
        }
    };
    session.open_in_sign_with_lifetime(
        wire,
        TxSource::Wallets,
        request_note(request, entry.index, label),
        last_valid_height,
    );
    Ok(())
}

/// What `request` does to proposal `index` of the multisig `label`, for the
/// note above the transaction.
pub fn request_note(request: &ProposalRequest, index: u64, label: &str) -> String {
    match request {
        ProposalRequest::Single { action, .. } => {
            let what = match action {
                ProposalAction::Approve => "approves",
                ProposalAction::Execute => "executes",
                ProposalAction::ApproveAndExecute => "approves and executes",
            };
            format!("This {what} proposal {index} of {label}.")
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
    use wallet_core::proposals::CombinedApproval;

    fn pk(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn the_note_says_what_one_member_does_as_it_always_did() {
        let single = |action| ProposalRequest::Single {
            member: pk(1),
            action,
        };
        assert_eq!(
            request_note(&single(ProposalAction::Approve), 4, "Treasury"),
            "This approves proposal 4 of Treasury."
        );
        assert_eq!(
            request_note(&single(ProposalAction::Execute), 4, "Treasury"),
            "This executes proposal 4 of Treasury."
        );
        assert_eq!(
            request_note(&single(ProposalAction::ApproveAndExecute), 4, "Treasury"),
            "This approves and executes proposal 4 of Treasury."
        );
    }

    #[test]
    fn the_note_of_a_combined_approval_counts_the_members_and_names_the_durable_nonce() {
        let mut combined = CombinedApproval {
            voters: vec![pk(1), pk(2)],
            executor: pk(1),
            durable_nonce: None,
        };
        assert_eq!(
            request_note(&ProposalRequest::Combined(combined.clone()), 4, "Treasury"),
            "This approves proposal 4 of Treasury as 2 members and executes it."
        );
        combined.durable_nonce = Some(pk(7));
        assert_eq!(
            request_note(&ProposalRequest::Combined(combined), 4, "Treasury"),
            format!(
                "This approves proposal 4 of Treasury as 2 members and executes it. It uses the durable nonce {}, so the offline wallets can sign it at any time.",
                pk(7)
            )
        );
    }
}
