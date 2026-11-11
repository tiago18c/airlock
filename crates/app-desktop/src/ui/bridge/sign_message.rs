//! A site asking a key to sign a message, or to sign in. Both show exactly the
//! bytes that are signed. Only a wallet with a key on this device can do either.

use super::*;
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
    let SiteCall::SignMessage { account, message } = &current.request.call else {
        return rsx! {};
    };
    let (account, bytes) = (*account, message.clone());
    let session = use_context::<Session>();
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    // A Ledger signs the text inside an offchain message naming the
    // account, and that whole message goes back to the site.
    let held_on = hardware_vendor_of(session, account);
    let device = held_on.filter(|vendor| vendor.wraps_messages());
    // A Keystone signs the bytes as they are, as a hot key does.
    let keystone = held_on.filter(|vendor| !vendor.wraps_messages());
    let wrapped = device.map(|vendor| {
        wallet_core::site_message::device_message(&bytes, &current.origin, &account, vendor.label())
            .map_err(|err| err.to_string())
    });
    // No vault key signs for a device, so the vault may stay locked.
    let unlocked = session.is_unlocked() || held_on.is_some();
    let armed = use_armed(unlocked && !matches!(wrapped, Some(Err(_))));
    let reject_id = current.id.clone();
    let sign_id = current.id.clone();
    let to_sign = bytes.clone();
    // What the site sent, as it is, when it is already an offchain message.
    let shown = wallet_core::site_message::read_offchain_v1(&bytes)
        .filter(|_| device.is_some())
        .map(|(_, text)| text.to_vec())
        .unwrap_or(bytes.clone());

    rsx! {
        RequestHead { current: current.clone(), title: "Sign message".to_string(), on_close }
        div { class: "request-body",
            AddressRow { pubkey: account, no_copy: true }
            MessageBody { bytes: shown }
            p { class: "hint",
                "Signing costs nothing, but a site or a program can act on what you sign. Sign only what you understand."
            }
            if let Some(vendor) = device {
                p { class: "hint",
                    "Your {vendor.label()} signs this text inside an offchain message that names this account, and the site gets that message back with the signature."
                }
            }
            if let Some(Err(why)) = wrapped.clone() {
                p { class: "warn", "{why}" }
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
                disabled: !armed || busy(),
                onclick: move |_| {
                    error.set(None);
                    if let Some(Ok(ocms)) = wrapped.clone() {
                        busy.set(true);
                        let sign_id = sign_id.clone();
                        spawn(async move {
                            match app_ui_kit::actions::device::sign_offchain(session, account, &ocms).await {
                                Ok(signature) => {
                                    let answer = BridgeAnswer::SignedMessage {
                                        signed: ocms,
                                        signature,
                                    };
                                    if !session.bridge_answer(&sign_id, answer) {
                                        error.set(Some("The site stopped waiting for this request.".into()));
                                    }
                                }
                                Err(message) if message == app_ui_kit::actions::device::CANCELLED => {}
                                Err(message) => error.set(Some(message)),
                            }
                            busy.set(false);
                        });
                        return;
                    }
                    if keystone.is_some() {
                        busy.set(true);
                        let (sign_id, to_sign) = (sign_id.clone(), to_sign.clone());
                        spawn(async move {
                            match app_ui_kit::actions::device::sign_message_as_is(session, account, &to_sign).await {
                                Ok(signature) => {
                                    let answer = BridgeAnswer::SignedMessage {
                                        signed: to_sign,
                                        signature,
                                    };
                                    if !session.bridge_answer(&sign_id, answer) {
                                        error.set(Some("The site stopped waiting for this request.".into()));
                                    }
                                }
                                Err(message) if message == app_ui_kit::actions::device::CANCELLED => {}
                                Err(message) => error.set(Some(message)),
                            }
                            busy.set(false);
                        });
                        return;
                    }
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
                match held_on {
                    Some(vendor) => rsx! { "Sign on {vendor.label()}" },
                    None => rsx! { "Sign" },
                }
            }
        }
    }
}

/// The hardware wallet of this vault that holds `account`, if one does.
fn hardware_vendor_of(session: Session, account: Pubkey) -> Option<wallet_core::HardwareVendor> {
    session
        .local_device_keys()
        .into_iter()
        .find(|(key, _)| *key == account)
        .map(|(_, vendor)| vendor)
}

/// The accounts a sign-in can use: those with a key here, or the one it
/// named. A Ledger or a Trezor cannot sign in: it signs a header with the
/// text, which the site would not read as a sign-in.
fn sign_in_accounts(accounts: Vec<ShareableAccount>, sign_in: &SignIn) -> Vec<ShareableAccount> {
    accounts
        .into_iter()
        .filter(|account| !account.contract)
        .filter(|account| {
            !account
                .on_device
                .is_some_and(|vendor| vendor.wraps_messages())
        })
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
    let SiteCall::SignIn(sign_in) = &current.request.call else {
        return rsx! {};
    };
    let sign_in = (**sign_in).clone();
    let session = use_context::<Session>();
    let origin = current.origin.clone();
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
    let mut busy = use_signal(|| false);
    // A Keystone signs in as a hot key does, on its own screen.
    let on_keystone = chosen()
        .and_then(|key| hardware_vendor_of(session, key))
        .is_some();
    let unlocked = session.is_unlocked() || on_keystone;
    let elsewhere = sign_in.uri_elsewhere(&origin);
    let message = chosen().map(|key| sign_in.message(&origin, &key.to_string()));
    let to_sign = message.clone();
    let armed = use_armed(unlocked && to_sign.is_some());
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
                        span { class: "chip selected", "current" }
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
                class: "danger btn-lead",
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
                disabled: !armed || busy(),
                onclick: move |_| {
                    let (Some(account), Some(text)) = (chosen(), to_sign.clone()) else {
                        return;
                    };
                    if hardware_vendor_of(session, account).is_some() {
                        busy.set(true);
                        let sign_id = sign_id.clone();
                        let remember = remember();
                        spawn(async move {
                            match app_ui_kit::actions::device::sign_message_as_is(session, account, text.as_bytes()).await {
                                Ok(signature) => {
                                    let waiting = session.bridge_sign_in(&sign_id, account, text.into_bytes(), signature, remember);
                                    if !waiting {
                                        error.set(Some("The site stopped waiting for this request.".into()));
                                    }
                                }
                                Err(message) if message == app_ui_kit::actions::device::CANCELLED => {}
                                Err(message) => error.set(Some(message)),
                            }
                            busy.set(false);
                        });
                        return;
                    }
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
