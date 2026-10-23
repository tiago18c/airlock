//! Which wallets of this device approve a multisig's proposals together
//! with its acting member, and the durable nonce the offline ones need.

use super::*;
use crate::actions::wallets::{together_candidates, together_choice, TogetherNote};
use wallet_core::AddressKind;

#[component]
pub(in crate::ui) fn ApprovesTogether(wallet: WalletSummary) -> Element {
    let session = use_context::<Session>();
    let Some(WalletKind::Squads {
        member_wallet,
        members,
        threshold,
        approves_with,
        approval_nonce,
        ..
    }) = wallet.contract.clone()
    else {
        return rsx! {};
    };
    let id = wallet.id;
    let wallets = (session.wallets)().summaries.clone();
    let options = together_candidates(&wallets, member_wallet, &members);
    if member_wallet.is_none() || options.is_empty() {
        return rsx! {};
    }
    let choice = together_choice(&wallets, member_wallet, &approves_with, approval_nonce);
    let nonce_accounts: Vec<_> = (session.book)()
        .iter()
        .filter(|entry| entry.kind == AddressKind::NonceAccount)
        .cloned()
        .collect();
    // One that was picked and is no longer in the book stays listed.
    let mut nonce_options: Vec<Pubkey> = nonce_accounts.iter().map(|entry| entry.pubkey).collect();
    if let Some(kept) = approval_nonce.filter(|kept| !nonce_options.contains(kept)) {
        nonce_options.push(kept);
    }

    rsx! {
        div { class: "stack",
            WalletChecklist {
                label: "Approves together with",
                options,
                selected: approves_with.clone(),
                on_change: move |chosen: Vec<WalletId>| {
                    session.when_unlocked(CHANGE_WALLET, move || {
                        let _ = session.update_wallet(id, |kind| kind.set_approves_with(chosen));
                    });
                },
            }
            if choice.members > 1 {
                p { class: "hint",
                    "{choice.members} members approve together."
                    if threshold > 0 {
                        " This multisig needs {threshold}."
                    }
                    " A proposal that still needs more than one of their votes can be approved by all of them and executed in one transaction."
                }
            }
            if choice.wants_nonce {
                AddressSelect {
                    label: "Durable nonce for approving together".to_string(),
                    options: nonce_options.clone(),
                    selected: approval_nonce,
                    none_label: "No durable nonce".to_string(),
                    on_change: move |picked| {
                        session.when_unlocked(CHANGE_WALLET, move || {
                            let _ = session.update_wallet(id, |kind| kind.set_approval_nonce(picked));
                        });
                    },
                }
                p { class: "hint",
                    "An offline wallet signs the transaction itself, so a combined approval it votes in uses a durable nonce. Its authority has to be the fee payer or one of the wallets that sign."
                    if nonce_accounts.is_empty() {
                        " There is no nonce account in the address book. Create one in Build first."
                    }
                }
            }
            for note in choice.notes.iter().copied() {
                p { class: "note", key: "{note:?}",
                    match note {
                        TogetherNote::NoExecutorNonce => "None of the programmatic signers chosen has a nonce account. One of them needs one before they can approve together.",
                        TogetherNote::OfflineWithoutNonce => "No durable nonce is picked, so the offline wallets are left out of combined approvals until one is.",
                        TogetherNote::OfflineBesideProgrammatic => "An offline wallet and a programmatic signer cannot vote in the same transaction, so a proposal that needs both gets no combined approval.",
                    }
                }
            }
        }
    }
}
