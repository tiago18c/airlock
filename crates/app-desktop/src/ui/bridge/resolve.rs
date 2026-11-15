//! What the request modal says of a `resolve` (`docs/intents.md`) beside
//! the transaction itself: what the site asked to have done with it, which
//! of its accounts Airlock replaced and with what, and for a message signed
//! as a memo, the message. Everything about what the transaction does is
//! still read from the transaction, as for any request.

use super::*;
use app_core::bridge::ResolveCall;
use app_ui_kit::actions::resolve::{delivery_words, derived_again};
use app_ui_kit::state::forms::ResolvedRequest;
use wallet_core::intent::{Delivery, Execution, Timing};

/// The request's title in the modal.
pub(in crate::ui) fn resolve_title(call: &ResolveCall, count: usize) -> String {
    match (call.message.is_some(), call.delivery, count) {
        (true, _, _) => "Sign a message as a memo".to_string(),
        (false, Delivery::Send, 1) => "Sign and send a transaction".to_string(),
        (false, Delivery::Send, count) => format!("Sign and send {count} transactions"),
        (false, _, 1) => "Sign a transaction".to_string(),
        (false, _, count) => format!("Sign {count} transactions"),
    }
}

/// Whether Airlock sends what this request asks to have signed, now or
/// once the site has added its signatures.
pub(in crate::ui) fn airlock_sends(current: &BridgePrompt) -> bool {
    current.sends
        || current
            .request
            .resolve()
            .is_some_and(|call| call.delivery == Delivery::Cosign)
}

/// What a resolution asks, said once above its transactions.
#[component]
pub(in crate::ui) fn ResolveNotice(
    call: ResolveCall,
    resolved: Option<ResolvedRequest>,
    /// Whether the prepared transaction is on a durable nonce, once known:
    /// for a key on another device, how long that device has.
    #[props(default)]
    on_nonce: Option<bool>,
) -> Element {
    let memo = call.message.clone();
    let key_elsewhere =
        call.profile.timing == Timing::Async && call.profile.execution == Execution::TopLevel;
    let asked = if key_elsewhere {
        "Review approves it here. The account's key signs it on its own device, and Airlock sends it once that signature is back on Review and Sign."
    } else if memo.is_some() {
        "This signs a message by sending it as a memo: one transaction whose only instruction is the Memo program's, with the message as its data and this account signing it. Airlock sends it once you sign, and the site gets the transaction's signature as proof that this account said it. Sending it costs the fee."
    } else {
        delivery_words(call.delivery)
    };
    let later = call.profile.timing == Timing::Async;
    // A key on another device signs the transaction itself, lifetime and
    // all. A multisig's other members sign transactions of their own.
    let lifetime = on_nonce
        .filter(|_| later && call.profile.execution == Execution::TopLevel)
        .map(app_ui_kit::actions::resolve::lifetime_words);
    rsx! {
        div { class: "callout",
            p { "{asked}" }
            if let Some(memo) = memo {
                MessageBody { bytes: memo.into_bytes() }
            }
            if later {
                p {
                    "This account's signature comes from elsewhere, so this may finish later. The site is told when it has run."
                }
            }
            if let Some(lifetime) = lifetime {
                p { "{lifetime}" }
            }
            if let Some(resolved) = resolved {
                ReplacedLines { resolved }
            }
        }
    }
}

/// Which of the site's accounts Airlock replaced, and with what.
#[component]
fn ReplacedLines(resolved: ResolvedRequest) -> Element {
    let derived = derived_again(&resolved);
    let app_signs = resolved.app_signs.clone();
    rsx! {
        for (old, new) in resolved.new_keys.iter().copied() {
            p { key: "new-{old}",
                Address { pubkey: old }
                " is a new account the site made a key for. "
                Address { pubkey: new }
                ", a key made on this device, takes its place and signs for it, as the site allowed. The account is created at the new address."
            }
        }
        for (old, new) in resolved.carried.iter().copied() {
            p { key: "carried-{old}",
                Address { pubkey: old }
                " is a new account the site made a key for. The multisig's program signs for it instead, at "
                Address { pubkey: new }
                ", an address it derives for this proposal, as the site allowed. No key exists for that address. The account is created there when the proposal runs."
            }
        }
        if let Some((old, new)) = resolved.fee_payer {
            p {
                Address { pubkey: new }
                " pays the fee in place of "
                Address { pubkey: old }
                ", as the site allowed."
            }
        }
        for (old, new) in derived {
            p { key: "derived-{old}",
                Address { pubkey: old }
                " is derived from an address that was replaced, so it is "
                Address { pubkey: new }
                " now."
            }
        }
        if !app_signs.is_empty() {
            p {
                "The site signs for "
                for (i, key) in app_signs.into_iter().enumerate() {
                    span { key: "{key}",
                        if i > 0 { ", " }
                        Address { pubkey: key }
                    }
                }
                " after you."
            }
        }
    }
}
