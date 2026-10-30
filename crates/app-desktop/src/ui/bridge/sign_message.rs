//! A site asking a key to sign a message, or to sign in. Both show exactly the
//! bytes that are signed. Only a wallet with a key on this device can do either.

use super::*;
use crate::bridge::{message_account, message_bytes, sign_in_request};
use wallet_core::site_message::{message_text, SignIn};
use wallet_core::ShareableAccount;

/// The bytes a request signs, as text or else in hex.
#[component]
fn MessageBody(bytes: Vec<u8>) -> Element {
    match message_text(&bytes) {
        Some(text) => rsx! { pre { class: "site-message", "{text}" } },
        None => {
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            rsx! {
                p { class: "warn",
                    "This message is not text, so you cannot read what it says. Sign it only if you trust this site with it."
                }
                pre { class: "site-message", code { "{hex}" } }
            }
        }
    }
}

/// The vault is locked, and signing needs its keys.
#[component]
fn UnlockFirst() -> Element {
    let session = use_context::<Session>();
    rsx! {
        p { class: "warn", "The vault is locked. Unlock it to sign this." }
        div { class: "btn-row",
            button {
                onclick: move |_| {
                    session.when_unlocked("Unlock the vault to sign this request.", || {});
                },
                "Unlock"
            }
        }
    }
}

#[component]
pub(in crate::ui) fn MessageRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let account = message_account(&current.params);
    let bytes = message_bytes(&current.params).unwrap_or_default();
    let mut error = use_signal(|| None::<String>);
    let unlocked = session.is_unlocked();
    let reject_id = current.id.clone();
    let sign_id = current.id.clone();
    let to_sign = bytes.clone();

    rsx! {
        RequestHead { current: current.clone(), title: "Sign message".to_string(), on_close }
        div { class: "request-body",
            if let Some(account) = account {
                AddressRow { pubkey: account, no_copy: true }
            }
            MessageBody { bytes }
            p { class: "hint",
                "Signing costs nothing, but a site or a program can act on what you sign. Sign only what you understand."
            }
            if !unlocked {
                UnlockFirst {}
            }
            if let Some(message) = error() {
                p { class: "warn", "{message}" }
            }
        }
        div { class: "btn-row",
            button {
                onclick: move |_| {
                    session.bridge_answer(&reject_id, BridgeAnswer::Rejected);
                },
                "Reject"
            }
            button {
                class: "primary",
                disabled: !unlocked || account.is_none(),
                onclick: move |_| {
                    let Some(account) = account else {
                        return;
                    };
                    match session.sign_site_message(account, &to_sign) {
                        Ok(signature) => {
                            let answer = BridgeAnswer::SignedMessage {
                                signed: to_sign.clone(),
                                signature,
                            };
                            if !session.bridge_answer(&sign_id, answer) {
                                error.set(Some("The site stopped waiting for this request.".into()));
                            }
                        }
                        Err(message) => error.set(Some(message)),
                    }
                },
                "Sign"
            }
        }
    }
}

/// The accounts a sign-in can use: those with a key here, or the one it named.
fn sign_in_accounts(accounts: Vec<ShareableAccount>, sign_in: &SignIn) -> Vec<ShareableAccount> {
    accounts
        .into_iter()
        .filter(|account| !account.contract)
        .filter(|account| {
            sign_in
                .address
                .as_ref()
                .is_none_or(|asked| *asked == account.pubkey.to_string())
        })
        .collect()
}

#[component]
pub(in crate::ui) fn SignInRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let origin = current.origin.clone();
    let sign_in = sign_in_request(&current.params).unwrap_or_default();
    let accounts = sign_in_accounts(session.shareable_accounts(), &sign_in);
    let start_origin = origin.clone();
    let start = use_hook(move || session.site_choice(&start_origin));
    let first = start
        .choice
        .current
        .into_iter()
        .chain(start.choice.ticked.iter().copied())
        .chain(accounts.iter().map(|account| account.pubkey))
        .find(|key| accounts.iter().any(|account| account.pubkey == *key));
    let mut chosen = use_signal(|| first);
    let mut remember = use_signal(|| start.remembered);
    let mut error = use_signal(|| None::<String>);
    let unlocked = session.is_unlocked();
    let elsewhere = sign_in.uri_elsewhere(&origin);
    let message = chosen().map(|key| sign_in.message(&origin, &key.to_string()));
    let to_sign = message.clone();
    let reject_id = current.id.clone();
    let block_id = current.id.clone();
    let sign_id = current.id.clone();

    rsx! {
        RequestHead { current: current.clone(), title: "Sign in".to_string(), on_close }
        div { class: "request-body",
            if accounts.is_empty() {
                p { class: "warn",
                    "No wallet on this device can sign in here. It takes a wallet whose key is on this device."
                }
            }
            for account in accounts.clone() {
                label { class: "check", key: "{account.pubkey}",
                    input {
                        r#type: "radio",
                        name: "sign-in-account",
                        checked: chosen() == Some(account.pubkey),
                        onchange: move |_| chosen.set(Some(account.pubkey)),
                    }
                    AddressRow { pubkey: account.pubkey, no_copy: true }
                    if start.choice.current == Some(account.pubkey) {
                        span { class: "chip", "Current" }
                    }
                }
            }
            if let Some(text) = message {
                pre { class: "site-message", "{text}" }
            }
            if elsewhere {
                p { class: "warn",
                    "The URI in this message is not this site's address. Check it before you sign."
                }
            }
            p { class: "hint",
                "Signing in shows this site the account you choose, and costs nothing."
            }
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked: remember(),
                    onchange: move |evt| remember.set(evt.checked()),
                }
                span { "Remember this connection" }
            }
            if !unlocked {
                UnlockFirst {}
            }
            if let Some(message) = error() {
                p { class: "warn", "{message}" }
            }
        }
        div { class: "btn-row",
            button {
                class: "danger",
                title: "Reject this request and every request this site makes from now on.",
                onclick: move |_| {
                    session.bridge_block(&block_id);
                },
                "Block site"
            }
            button {
                onclick: move |_| {
                    session.bridge_answer(&reject_id, BridgeAnswer::Rejected);
                },
                "Reject"
            }
            button {
                class: "primary",
                disabled: !unlocked || to_sign.is_none(),
                onclick: move |_| {
                    let (Some(account), Some(text)) = (chosen(), to_sign.clone()) else {
                        return;
                    };
                    match session.sign_site_message(account, text.as_bytes()) {
                        Ok(signature) => {
                            let waiting = session.bridge_sign_in(
                                &sign_id,
                                account,
                                text.into_bytes(),
                                signature,
                                remember(),
                            );
                            if !waiting {
                                error.set(Some("The site stopped waiting for this request.".into()));
                            }
                        }
                        Err(message) => error.set(Some(message)),
                    }
                },
                "Sign in"
            }
        }
    }
}
