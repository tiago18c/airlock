//! Wallets moved to another device by their public keys: exporting one or
//! several as QR codes or text, and adding what another device exported.

use super::*;
use crate::ui::transfer::receive::QrScanner;
use crate::ui::transfer::send::PayloadShare;
use wallet_core::protocol::{
    decode_payload, encode_wallet_keys, parse_payload_text, QrObject, WalletKey,
};
use wallet_core::shared_keys::{arriving, exportable, Arrival, KeyToAdd};

/// The keys of `keys` as one or more wallet keys objects, each a QR
/// carousel and text. More than one when they do not fit in one transfer.
#[component]
pub(in crate::ui) fn KeyExportModal(keys: Vec<WalletKey>, on_close: EventHandler<()>) -> Element {
    let parts = use_memo(use_reactive!(|(keys)| {
        encode_wallet_keys(&keys).map_err(|err| err.to_string())
    }));
    let mut at = use_signal(|| 0usize);
    let (all, failed) = match parts() {
        Ok(all) => (all, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let count = all.len();
    let shown = at().min(count.saturating_sub(1));
    let part = all.get(shown).cloned();
    let many = keys.len() > 1;

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { if many { "Export {keys.len()} public keys" } else { "Export public key" } }
                    p { class: "lede",
                        "No secret leaves this device. On the online device, open Wallets, Add wallet, From another device, and scan this or paste the text. A Vector wallet is initialized from there."
                    }
                    if count > 1 {
                        div { class: "segmented", role: "group", aria_label: "Part",
                            for part in 0..count {
                                button {
                                    key: "{part}",
                                    class: if part == shown { "tab on" } else { "tab" },
                                    onclick: move |_| at.set(part),
                                    "Part {part + 1} of {count}"
                                }
                            }
                        }
                        p { class: "hint", "The keys do not fit in one transfer. Scan every part." }
                    }
                    if let Some(err) = failed {
                        p { class: "error", "{err}" }
                    }
                    if let Some(part) = part {
                        PayloadShare { objects: vec![part],
                            button { class: "ghost", onclick: move |_| on_close.call(()), "Close" }
                        }
                    } else {
                        div { class: "btn-row",
                            button { class: "ghost", onclick: move |_| on_close.call(()), "Close" }
                        }
                    }
                }
            }
        }
    }
}

/// A row's button that exports its wallet's public key, when another device
/// can add it by that key.
#[component]
pub(in crate::ui) fn ExportKeyButton(id: WalletId) -> Element {
    let session = use_context::<Session>();
    let mut open = use_signal(|| false);
    let found = exportable(&(session.wallets)().summaries)
        .into_iter()
        .find(|one| one.id == id);
    let Some(found) = found else {
        return rsx! {};
    };
    rsx! {
        button { class: "sm",
            title: "Show this wallet's public key, for another device to add it as {found.becomes}",
            onclick: move |_| open.set(true),
            "Export key"
        }
        if open() {
            KeyExportModal { keys: vec![found.key.clone()], on_close: move |_| open.set(false) }
        }
    }
}

/// Pick wallets of this device and export their public keys together.
#[component]
pub fn ExportKeysModal(on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let options = exportable(&wallets);
    let mut picked = use_signal(|| options.iter().map(|one| one.id).collect::<Vec<_>>());
    let mut showing = use_signal(|| false);
    let chosen: Vec<WalletKey> = options
        .iter()
        .filter(|one| picked().contains(&one.id))
        .map(|one| one.key.clone())
        .collect();

    if showing() && !chosen.is_empty() {
        return rsx! {
            KeyExportModal { keys: chosen, on_close: move |_| on_close.call(()) }
        };
    }
    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Export public keys" }
                    p { class: "lede",
                        "Another device adds each key as an offline wallet, or as a Vector wallet whose key is kept here. Programmatic signers and multisigs are set up there from these."
                    }
                    if options.is_empty() {
                        p { class: "note", "No wallet here can be added elsewhere by its key." }
                    }
                    ul { class: "derive-list",
                        for one in options.clone() {
                            li { class: "derive-row", key: "{one.id}",
                                input {
                                    r#type: "checkbox",
                                    "aria-label": "{one.key.label}",
                                    checked: picked().contains(&one.id),
                                    onchange: move |_| {
                                        let mut next = picked();
                                        match next.iter().position(|id| *id == one.id) {
                                            Some(at) => {
                                                next.remove(at);
                                            }
                                            None => next.push(one.id),
                                        }
                                        picked.set(next);
                                    },
                                }
                                div {
                                    p { "{one.key.label}" }
                                    p { class: "muted", "{one.becomes}" }
                                }
                            }
                        }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            disabled: chosen.is_empty(),
                            onclick: move |_| showing.set(true),
                            "Show {chosen.len()} key(s)"
                        }
                        button { class: "ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    }
                }
            }
        }
    }
}

/// One key read from another device, as the form holds it.
#[derive(Clone, PartialEq)]
struct ImportRow {
    key: WalletKey,
    label: String,
    checked: bool,
}

/// Add-wallet form: wallets another device exported by their public keys.
/// Takes one object or several, pasted or scanned, and lists every key.
#[component]
pub fn KeyImportForm(on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let payer_wallets = payers(&wallets, crate::state::use_network().is_none());
    let mut rows = use_signal(Vec::<ImportRow>::new);
    let mut pasted = use_signal(String::new);
    let mut scanning = use_signal(|| false);
    let mut fee_payer = use_signal(|| None::<WalletId>);
    let mut error = use_signal(|| None::<String>);

    let keys: Vec<WalletKey> = rows().iter().map(|row| row.key.clone()).collect();
    let seen = arriving(&wallets, &keys);

    // Keys already listed are not listed again. A key new here starts ticked.
    let mut take = move |objects: Vec<QrObject>| {
        let mut arrived = Vec::new();
        for object in objects {
            let QrObject::WalletKeys(keys) = object else {
                error.set(Some(
                    "That is not an export of wallet keys. On the other device, use Export key on a wallet, or Export keys on Wallets."
                        .into(),
                ));
                return;
            };
            arrived.extend(keys);
        }
        error.set(None);
        let here = (session.wallets)().summaries.clone();
        let mut next = rows();
        for key in arrived {
            if next.iter().any(|row| row.key == key) {
                continue;
            }
            let fresh = arriving(&here, std::slice::from_ref(&key))
                .first()
                .is_some_and(|one| one.arrival == Arrival::New);
            next.push(ImportRow {
                label: key.label.clone(),
                key,
                checked: fresh,
            });
        }
        rows.set(next);
    };

    let save = move |_| {
        error.set(None);
        let listed = rows();
        let read = arriving(
            &(session.wallets)().summaries,
            &listed.iter().map(|row| row.key.clone()).collect::<Vec<_>>(),
        );
        let picks: Vec<KeyToAdd> = listed
            .iter()
            .zip(&read)
            .filter(|(row, one)| row.checked && one.arrival == Arrival::New)
            .map(|(row, _)| KeyToAdd {
                key: row.key.clone(),
                label: row.label.clone(),
            })
            .collect();
        if picks.is_empty() {
            error.set(Some("Tick at least one new wallet.".into()));
            return;
        }
        match session.add_wallet_keys(picks, fee_payer()) {
            Ok(_) => {
                on_done.call(());
                crate::actions::network::start_refresh(session);
            }
            Err(err) => error.set(Some(err.to_string())),
        }
    };

    rsx! {
        p { class: "note",
            "Wallets whose keys stay on another device, as that device exported them: a key becomes an offline wallet, and a Vector account a Vector wallet whose key is kept there. Initialize a Vector wallet from its row here once it is added."
        }
        div { class: "segmented", role: "group", aria_label: "How to load it",
            button {
                class: if scanning() { "tab" } else { "tab on" },
                onclick: move |_| scanning.set(false),
                "Paste"
            }
            button {
                class: if scanning() { "tab on" } else { "tab" },
                onclick: move |_| scanning.set(true),
                "Scan a QR code"
            }
        }
        if scanning() {
            QrScanner {
                on_payload: move |bytes: Vec<u8>| match decode_payload(&bytes) {
                    Ok(objects) => take(objects),
                    Err(err) => error.set(Some(err.to_string())),
                },
            }
        } else {
            label { "Exported keys (JSON)"
                textarea {
                    value: "{pasted}",
                    placeholder: "Paste here…",
                    oninput: move |evt| pasted.set(evt.value()),
                }
            }
            div { class: "btn-row",
                button {
                    onclick: move |_| match parse_payload_text(&pasted()) {
                        Ok(objects) => {
                            take(objects);
                            pasted.set(String::new());
                        }
                        Err(err) => error.set(Some(err.to_string())),
                    },
                    "Read"
                }
            }
        }
        if !seen.is_empty() {
            ul { class: "derive-list",
                for (at, one) in seen.iter().cloned().enumerate() {
                    li { class: "derive-row", key: "{at}",
                        input {
                            r#type: "checkbox",
                            "aria-label": "Add {one.key.label}",
                            disabled: one.arrival != Arrival::New,
                            checked: one.arrival == Arrival::New && rows().get(at).is_some_and(|row| row.checked),
                            onchange: move |_| {
                                if let Some(row) = rows.write().get_mut(at) {
                                    row.checked = !row.checked;
                                }
                            },
                        }
                        div { class: "stack",
                            input {
                                r#type: "text",
                                "aria-label": "Label",
                                value: "{rows().get(at).map(|row| row.label.clone()).unwrap_or_default()}",
                                oninput: move |evt| {
                                    if let Some(row) = rows.write().get_mut(at) {
                                        row.label = evt.value();
                                    }
                                },
                            }
                            p { class: "muted", "{one.becomes}" }
                            if let Some(address) = one.address {
                                AddressRow { pubkey: address }
                            }
                            match one.arrival.clone() {
                                Arrival::New => rsx! {},
                                Arrival::AlreadyHere { label } => rsx! {
                                    p { class: "muted", "Already here, as {label}." }
                                },
                                Arrival::Refused(why) => rsx! {
                                    p { class: "warn", "{why}" }
                                },
                            }
                        }
                    }
                }
            }
            WalletPicker {
                label: "Pays fees",
                options: payer_wallets.clone(),
                selected: fee_payer(),
                none_label: "Choose a hot wallet",
                on_change: move |id| fee_payer.set(id),
            }
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            button { class: "primary",
                disabled: seen.is_empty(),
                onclick: save,
                "Add selected"
            }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}
