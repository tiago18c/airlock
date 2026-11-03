//! What a transaction says yes to without holding it: what is known of it here,
//! and the user's word that they have checked it when nothing here can say.

use super::*;
use crate::ui::tx_view::RiskNotice;
use std::collections::HashMap;
use wallet_core::squads::VaultTransaction;
use wallet_core::{
    display_note, explain_instructions, stored_in, BlindApproval, ExplainedTx, StoredTarget,
};
use wallet_flows::offer::{blind_offer, BlindFacts, BlindOffer};

/// Ask a node what is stored behind `approvals`, for each transaction put on the
/// screen. Answers whether that is still under way.
pub(super) fn use_stored_behind(approvals: &[BlindApproval], online: bool) -> bool {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let reading = use_signal(|| false);
    let targets: Vec<StoredTarget> = approvals.iter().filter_map(|one| one.target).collect();
    let waiting = online && !targets.is_empty() && (form.stored_behind)().is_none();
    use_effect(use_reactive!(|(targets, online)| {
        if online && !targets.is_empty() && !reading() && form.stored_behind.read().is_none() {
            read_stored_behind(session, targets.clone(), reading);
        }
    }));
    waiting
}

/// What `approvals` ask of the screen.
pub(super) fn offer_on_screen(
    session: Session,
    tx: &DecodedTx,
    approvals: &[BlindApproval],
    reading: bool,
) -> BlindOffer {
    let form = session.forms.sign;
    let behind = (form.stored_behind)().unwrap_or_default();
    blind_offer(BlindFacts {
        approvals: approvals.len(),
        read: approvals
            .iter()
            .filter(|one| {
                one.target
                    .is_some_and(|target| stored_in(&target, &behind).is_some())
            })
            .count(),
        reading,
        checked: (form.blind_checked)() == Some(sha256_bytes(&tx.message)),
    })
}

/// What is known of what `approvals` say yes to, and the box that says the user
/// has checked it where they trust.
#[component]
pub(super) fn BlindApprovals(
    tx: Rc<DecodedTx>,
    approvals: Vec<BlindApproval>,
    offer: BlindOffer,
    reading: bool,
    online: bool,
    inspect: bool,
) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let digest = sha256_bytes(&tx.message);
    let mut checked = form.blind_checked;
    let is_checked = checked() == Some(digest);
    let from_node = (form.stored_behind)().unwrap_or_default();
    let from_note = display_note(&tx, (form.context)().as_ref()).accounts;
    // Approving and running one proposal is one thing to show.
    let mut targets: Vec<StoredTarget> = Vec::new();
    for target in approvals.iter().filter_map(|one| one.target) {
        if !targets.iter().any(|seen| seen.proposal == target.proposal) {
            targets.push(target);
        }
    }
    let unreadable = approvals.iter().any(|one| one.target.is_none());
    rsx! {
        div { class: "card",
            h3 { "Stored on chain" }
            for target in targets {
                StoredBehind {
                    key: "{target.proposal}",
                    target,
                    from_node: from_node.clone(),
                    from_note: from_note.clone(),
                    reading,
                    online,
                    inspect,
                }
            }
            if unreadable {
                p { class: "hint", "This app cannot read what is stored for this one." }
            }
            if offer.ask {
                label { class: "check",
                    input {
                        r#type: "checkbox",
                        checked: is_checked,
                        onchange: move |_| checked.set((!is_checked).then_some(digest)),
                    }
                    "I have checked what this does somewhere I trust."
                }
            }
        }
    }
}

/// What is known of one proposal: what the node says, or else what the other
/// device says, and the two are not worth the same.
#[component]
fn StoredBehind(
    target: StoredTarget,
    from_node: HashMap<Pubkey, Vec<u8>>,
    from_note: HashMap<Pubkey, Vec<u8>>,
    reading: bool,
    online: bool,
    inspect: bool,
) -> Element {
    if let Some(stored) = stored_in(&target, &from_node) {
        return rsx! {
            StoredInstructions {
                stored,
                said: "The node says it holds this.",
                inspect,
            }
        };
    }
    if let Some(stored) = stored_in(&target, &from_note) {
        return rsx! {
            StoredInstructions {
                stored,
                said: "The other device says it holds this. This device cannot check that.",
                inspect,
            }
        };
    }
    let said = if reading {
        "Asking the node what it holds…"
    } else if online {
        "The node did not say what it holds."
    } else {
        "Nothing here says what it holds. A device that is online can send that in the context note."
    };
    rsx! {
        p { class: "hint", "{said}" }
    }
}

#[component]
fn StoredInstructions(stored: VaultTransaction, said: &'static str, inspect: bool) -> Element {
    let session = use_context::<Session>();
    let explained = use_memo(use_reactive!(|stored| {
        ExplainedTx {
            programs: Vec::new(),
            instructions: explain_instructions(&*session.idls.read(), &stored.instructions),
        }
    }))();
    rsx! {
        p { class: "hint", "{said}" }
        RiskNotice { explained: explained.clone() }
        InstructionList { instructions: explained.instructions, inspect }
    }
}
