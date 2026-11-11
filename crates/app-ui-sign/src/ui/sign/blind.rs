//! What a transaction says yes to without holding it: what is known of it here,
//! and the user's word that they have checked it when nothing here can say.
//! And a vote the chain checks against what the transaction carries, which
//! needs neither.

use super::*;
use std::collections::HashMap;
use wallet_core::squads::{
    clear_signing_on, upgrade_buffers, ClearSigningAccepts, VaultTransaction,
};
use wallet_core::{
    approval_shape, display_note, stored_in, ApprovalShape, BlindApproval, CheckedApproval,
    CheckedUpgrade, ExplainedTx, SignerKey, StoredTarget,
};
use wallet_flows::offer::{
    blind_offer, check_offer, BlindFacts, BlindOffer, CheckBlock, CheckFacts, CheckOffer,
};

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

/// What the screen offers about adding what the proposal an approval votes
/// on holds, and, when it offers that, the proposal's number and what the
/// node said it stores, which is what gets carried.
pub(super) fn checks_on_screen(
    session: Session,
    tx: &DecodedTx,
    explained: &ExplainedTx,
    approvals: &[BlindApproval],
    status: &SigningStatus,
    online: bool,
    needs_rebuild: bool,
) -> (CheckOffer, Option<VaultTransaction>) {
    let form = session.forms.sign;
    let shape = approval_shape(explained);
    let ApprovalShape::Open(open) = &shape else {
        return (CheckOffer::Nothing, None);
    };
    let behind = (form.stored_behind)().unwrap_or_default();
    let shown = approvals
        .iter()
        .filter_map(|one| one.target)
        .filter(|target| target.proposal == open.proposal)
        .find_map(|target| stored_in(&target, &behind));
    let off_device =
        |state: SignerState| matches!(state, SignerState::Missing | SignerState::Device);
    // A member that signs itself, or an authority that signs for a member
    // voting through it, whose signature comes from off this device.
    let signs_elsewhere = status.lines.iter().any(|line| {
        let votes = open
            .votes
            .iter()
            .any(|vote| line.key == SignerKey::Key(vote.member));
        let authority = open.wrapped() && matches!(line.role, SignerRole::Authority { .. });
        (votes || authority) && off_device(line.state)
    });
    let program = wallet_core::squads::SQUADS_CLEAR_SIGNING_PROGRAM_ID;
    let facts = CheckFacts {
        online,
        stored_read: shown.is_some(),
        signs_elsewhere,
        program_on_cluster: clear_signing_on(
            session.idl_network(),
            session.idl_body(&program).is_some(),
        ),
        needs_rebuild,
        shareable: tx.format == TxFormat::V1,
        signatures_held: tx.signers.iter().filter(|slot| slot.filled).count(),
        shape,
    };
    (check_offer(facts, ClearSigningAccepts::DEPLOYED), shown)
}

/// The offer to rebuild an approval so it carries what its proposal holds,
/// for a device that signs it and cannot ask a node. Taking it is a choice:
/// left alone, the approval stays as it is, with the warning and the
/// checkbox it has always had, and Sign is not held back.
#[component]
pub(super) fn AddChecksOffer(
    digest: [u8; 32],
    shown: VaultTransaction,
    /// Signatures already made over the transaction.
    voids: usize,
) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let busy = (form.rebuilding)();
    let refused = (form.checks_refused)()
        .filter(|(message, _)| *message == digest)
        .map(|(_, why)| why);
    let upgrades = !upgrade_buffers(&shown.instructions).is_empty();
    let index = shown.index;
    rsx! {
        div { class: "callout",
            p {
                "The device that signs this approval cannot see what proposal #{index} runs: that is stored on chain, not in the transaction."
            }
            p {
                if upgrades {
                    "This screen can add instructions that carry what the proposal holds, and the hash of the buffer each program upgrade in it deploys. The chain checks them before the vote counts, so the device that signs reads what it approves from the transaction alone."
                } else {
                    "This screen can add instructions that carry what the proposal holds. The chain checks them before the vote counts, so the device that signs reads what it approves from the transaction alone."
                }
            }
            p { class: "muted",
                "Adding them changes the transaction and costs somewhat more compute."
                if voids == 1 {
                    " The signature already made over it has to be made again."
                } else if voids > 1 {
                    " The {voids} signatures already made over it have to be made again."
                }
            }
            if let Some(why) = refused {
                p { class: "error", role: "alert", "{why}" }
            }
            div { class: "btn-row",
                // A banner's action, and not the only way forward: Sign stays
                // as it is without it.
                button {
                    disabled: busy,
                    onclick: move |_| add_checks_loaded(session, shown.clone()),
                    if busy { "Adding…" } else { "Add what the proposal holds" }
                }
            }
        }
    }
}

/// Why what the proposal holds cannot be added to this approval, which would
/// otherwise take it.
fn cannot_add(block: CheckBlock) -> &'static str {
    match block {
        CheckBlock::DurableNonce => {
            "This screen cannot add what the proposal holds to this approval, because it lives on a durable nonce: the chain takes that only beside a vote alone. An approval on a recent blockhash could carry it, if the other device signs within about 30 seconds."
        }
        CheckBlock::Execute => {
            "This screen cannot add what the proposal holds to this approval, because it executes the proposal too: the chain takes that only beside a vote alone. An approval on its own could carry it."
        }
        CheckBlock::SeveralVotes => {
            "This screen cannot add what the proposal holds to these approvals, because several members vote in this transaction: the chain takes that only beside a vote alone. Each member's approval on its own could carry it."
        }
        CheckBlock::Authorization => {
            "This screen cannot add what the proposal holds to this approval, because the member votes through a wallet without a key, and the clear signing program on this cluster does not take that. An approval the member signs with its own key could carry it."
        }
    }
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
    /// Why what the proposal holds cannot be added here, when only that
    /// stands in the way.
    #[props(default)]
    cannot: Option<CheckBlock>,
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
            if let Some(block) = cannot {
                p { class: "hint", "{cannot_add(block)}" }
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
    rsx! {
        p { class: "hint", "{said}" }
        ReadInstructions { instructions: stored.instructions, inspect, risks: true }
    }
}

/// What a vote the chain checks approves: the instructions its check carries,
/// shown in the list above, and the build each upgrade among them deploys.
/// Nothing here is asked of a node or taken from a context note: it is all in
/// the transaction, so it reads the same with no network.
#[component]
pub(super) fn CheckedApprovals(checked: Vec<CheckedApproval>) -> Element {
    rsx! {
        div { class: "card",
            h3 { "What the proposal holds" }
            for approval in checked {
                CheckedOne { key: "{approval.proposal}", approval }
            }
        }
    }
}

/// One proposal: who approves it, whether it is executed too, and where
/// the instructions its checks carry are shown. A vote on it no check pins
/// is not here: it is under "Stored on chain".
#[component]
fn CheckedOne(approval: CheckedApproval) -> Element {
    let place = approval.check.label();
    let index = approval.transaction_index;
    let members: Vec<Pubkey> = approval.votes.iter().map(|vote| vote.member).collect();
    let executes = approval.execute.is_some();
    // "The vote", "Each vote", "The execution", "The vote and the execution".
    let lands = match (members.len(), executes) {
        (0, _) => "The execution lands",
        (1, false) => "The vote lands",
        (1, true) => "The vote and the execution land",
        (_, false) => "Each vote lands",
        (_, true) => "Each vote and the execution land",
    };
    let refuses = match (members.len(), executes) {
        (0, _) => "The chain refuses it otherwise.",
        (1, false) => "The chain refuses the vote otherwise.",
        _ => "The chain refuses them otherwise.",
    };
    let last = members.len().saturating_sub(1);
    rsx! {
        p {
            if members.is_empty() {
                "This executes proposal #{index} of multisig "
            } else {
                "This approves proposal #{index} of multisig "
            }
            Address { pubkey: approval.squad.address }
            if members.len() > 1 {
                " as {members.len()} members: "
                for (i, member) in members.iter().enumerate() {
                    span { key: "{member}",
                        if i > 0 {
                            if i == last { " and " } else { ", " }
                        }
                        Address { pubkey: *member }
                    }
                }
            }
            if executes && members.len() == 1 {
                " and executes it"
            }
            "."
            if executes && members.len() > 1 {
                " It also executes the proposal."
            }
            " {lands} only if the proposal holds exactly the instructions shown under {place}, which run as vault "
            Address { pubkey: approval.vault }
            ". {refuses}"
        }
        if executes {
            p { class: "hint",
                "That is checked in this transaction, which runs them at once."
            }
        } else {
            p { class: "hint",
                "That is checked when the vote is cast. What the proposal runs is what is shown, but accounts it uses can still change before it runs."
            }
        }
        for upgrade in approval.upgrades {
            CheckedBuild { key: "{upgrade.program}{upgrade.buffer}", upgrade }
        }
    }
}

/// The build a program upgrade in the proposal deploys, when the transaction
/// pins it, and a warning when it does not.
#[component]
fn CheckedBuild(upgrade: CheckedUpgrade) -> Element {
    let Some(hash) = upgrade.hash else {
        return rsx! {
            p { class: "warn",
                "It upgrades program "
                Address { pubkey: upgrade.program }
                " from buffer "
                Address { pubkey: upgrade.buffer }
                ". This transaction says which buffer the program is upgraded from, but not what the buffer holds."
            }
        };
    };
    rsx! {
        p {
            "It upgrades program "
            Address { pubkey: upgrade.program }
            " from buffer "
            Address { pubkey: upgrade.buffer }
            ". The vote lands only if the buffer holds the build with this hash:"
        }
        dl { class: "addr-facts",
            div {
                dt { "Buffer hash" }
                dd { class: "addr-card-account",
                    code { class: "mono", "{hash}" }
                    CopyButton { text: hash.clone(), label: "Copy the buffer hash" }
                }
            }
        }
        p { class: "hint",
            code { "solana-verify get-buffer-hash {upgrade.buffer}" }
            " prints the same hash. Compare it with the hash of a build you trust."
        }
    }
}
