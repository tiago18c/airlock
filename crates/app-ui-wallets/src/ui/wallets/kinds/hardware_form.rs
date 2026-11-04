//! Add-wallet form: keys read from a hardware wallet.

use super::*;
use crate::actions::device::{keystone_export, read_keys, show_address, DeviceKey, DeviceKeys};
use crate::ui::transfer::receive::QrScanner;
use wallet_core::{HardwareVendor, PATH_SCHEMES};

/// How many keys a press of Read reads.
const READ_BATCH: u32 = 10;

/// Add-wallet form: a hardware wallet. The device is asked for the keys of a
/// path scheme, the user ticks the ones to add, and can check each on the
/// device. Nothing secret is stored: the device keeps the keys.
#[component]
pub fn HardwareForm(on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let offered = session.hardware_offered();
    let online = crate::state::use_network().is_some();
    let mut vendor = use_signal(|| None::<HardwareVendor>);
    let mut scheme = use_signal(|| 0usize);
    let mut read = use_signal(|| None::<DeviceKeys>);
    let mut ticked = use_signal(Vec::<u32>::new);
    let mut balances = use_signal(Vec::<(Pubkey, u64)>::new);
    let mut label = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut reading = use_signal(|| false);

    let Some(&first) = offered.first() else {
        return rsx! {
            p { class: "note",
                "Hardware wallets are offered when Pre-release hardware wallets is on in Settings. Ledger reads v1 transactions from its Solana app 1.18, and Trezor from its next firmware, both only pre-release builds so far. Airlock's support for Keystone has not been checked on a device yet."
            }
        };
    };
    let chosen = vendor()
        .filter(|vendor| offered.contains(vendor))
        .unwrap_or(first);
    let here = session.known_wallet_pubkeys();

    let mut read_more = move |start: u32| {
        error.set(None);
        reading.set(true);
        let picked = PATH_SCHEMES[*scheme.peek()];
        spawn(async move {
            match read_keys(session, chosen, picked, start, READ_BATCH).await {
                Ok(found) => {
                    let mut all = read.peek().clone().filter(|_| start > 0);
                    match all.as_mut() {
                        Some(all) => {
                            all.keys.extend(found.keys.clone());
                            all.seen = found.seen.clone();
                        }
                        None => all = Some(found.clone()),
                    }
                    if start == 0 {
                        // The first key not already a wallet here.
                        let here = session.known_wallet_pubkeys();
                        let first_new = found
                            .keys
                            .iter()
                            .find(|key| !here.contains(&key.pubkey))
                            .map(|key| key.index);
                        ticked.set(first_new.into_iter().collect());
                    }
                    read.set(all);
                    if online {
                        if let Ok(rpc) = session.rpc() {
                            let keys: Vec<Pubkey> =
                                found.keys.iter().map(|key| key.pubkey).collect();
                            if let Ok(found) = rpc.get_lamports(&keys).await {
                                balances.write().extend(
                                    keys.iter()
                                        .zip(found)
                                        .filter_map(|(key, lamports)| Some((*key, lamports?))),
                                );
                            }
                        }
                    }
                }
                Err(why) if why == crate::actions::device::CANCELLED => {}
                Err(why) => error.set(Some(why)),
            }
            reading.set(false);
        });
    };

    let save = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let Some(found) = read() else {
            return;
        };
        let picked: Vec<&DeviceKey> = found
            .keys
            .iter()
            .filter(|key| ticked().contains(&key.index))
            .collect();
        if picked.is_empty() {
            error.set(Some("Tick at least one account.".into()));
            return;
        }
        let several = picked.len() > 1;
        let records = picked
            .into_iter()
            .map(|key| {
                let label = if several {
                    format!("{name} {}", key.index + 1)
                } else {
                    name.clone()
                };
                let mut record =
                    WalletRecord::hardware(label, found.vendor, key.pubkey, key.path.clone());
                record.kind.set_device_seen(found.seen.clone());
                record.kind.set_hardware_device(found.device.clone());
                record
            })
            .collect();
        match session.add_hardware_wallets(records) {
            Ok(()) => {
                on_done.call(());
                crate::actions::network::start_refresh(session);
            }
            Err(err) => error.set(Some(err.to_string())),
        }
    };

    let keys = read().map(|found| found.keys).unwrap_or_default();
    let next = keys.last().map_or(0, |key| key.index + 1);
    let numbered = PATH_SCHEMES[scheme()].template.contains('*');

    rsx! {
        p { class: "note",
            "A hardware wallet keeps its key on the device, and signs there once you confirm. This device stores only the address and where the key is on the device."
        }
        if offered.len() > 1 {
            div { class: "segmented",
                for option in offered.iter().copied() {
                    button {
                        key: "{option.label()}",
                        class: if option == chosen { "tab on" } else { "tab" },
                        onclick: move |_| {
                            vendor.set(Some(option));
                            read.set(None);
                        },
                        "{option.label()}"
                    }
                }
            }
        }
        if chosen == HardwareVendor::Keystone {
            if keys.is_empty() {
                p { class: "hint",
                    "On your Keystone, firmware {chosen.minimum()} or later, open Connect Software Wallet, choose Solflare and pick the accounts' path there. Then scan the codes it shows."
                }
                QrScanner {
                    on_payload: move |_| {},
                    on_ur: move |(ur_type, message): (String, Vec<u8>)| {
                        match keystone_export(&ur_type, &message) {
                            Ok(found) => {
                                error.set(None);
                                let here = session.known_wallet_pubkeys();
                                let first_new = found
                                    .keys
                                    .iter()
                                    .find(|key| !here.contains(&key.pubkey))
                                    .map(|key| key.index);
                                ticked.set(first_new.into_iter().collect());
                                read.set(Some(found));
                            }
                            Err(why) => error.set(Some(why)),
                        }
                    },
                }
            }
        } else {
        p { class: "hint",
            if chosen == HardwareVendor::Ledger {
                "Connect your Ledger, unlock it and open the Solana app, version {chosen.minimum()} or later."
            } else {
                "Connect your {chosen.label()} and unlock it. It needs firmware {chosen.minimum()} or later. Close Trezor Suite first: it holds the device while it runs."
            }
        }
        label { "Accounts"
            select {
                onchange: move |evt| {
                    if let Ok(at) = evt.value().parse::<usize>() {
                        scheme.set(at.min(PATH_SCHEMES.len() - 1));
                        read.set(None);
                        ticked.set(Vec::new());
                    }
                },
                for (at, option) in PATH_SCHEMES.iter().enumerate() {
                    option {
                        value: "{at}",
                        selected: scheme() == at,
                        "{option.template} ({option.used_by})"
                    }
                }
            }
        }
        }
        if keys.is_empty() && chosen != HardwareVendor::Keystone {
            div { class: "btn-row",
                button {
                    disabled: reading(),
                    onclick: move |_| read_more(0),
                    if reading() { "Reading…" } else { "Read accounts" }
                }
            }
        } else if !keys.is_empty() {
            ul { class: "derive-list",
                for key in keys {
                    DeviceKeyRow {
                        key: "{key.path}",
                        row: key.clone(),
                        vendor: chosen,
                        here: here.contains(&key.pubkey),
                        checked: ticked().contains(&key.index),
                        lamports: balances().iter().find(|(held, _)| *held == key.pubkey).map(|(_, lamports)| *lamports),
                        on_toggle: move |index| {
                            let mut list = ticked.write();
                            match list.iter().position(|at| *at == index) {
                                Some(at) => {
                                    list.remove(at);
                                }
                                None => list.push(index),
                            }
                        },
                    }
                }
            }
            if numbered && chosen != HardwareVendor::Keystone {
                div { class: "btn-row",
                    button {
                        disabled: reading(),
                        onclick: move |_| read_more(next),
                        if reading() { "Reading…" } else { "Read {READ_BATCH} more" }
                    }
                }
            }
            label { "Label"
                input {
                    r#type: "text",
                    placeholder: "Ledger",
                    value: "{label}",
                    oninput: move |evt| label.set(evt.value()),
                }
            }
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            if read().is_some() {
                button { class: "primary", onclick: save, "Add selected" }
            }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}

/// One key read from the device, to tick and to check on it.
#[component]
fn DeviceKeyRow(
    row: DeviceKey,
    vendor: HardwareVendor,
    /// Already a wallet of this vault: listed, and cannot be ticked.
    here: bool,
    checked: bool,
    lamports: Option<u64>,
    on_toggle: EventHandler<u32>,
) -> Element {
    let session = use_context::<Session>();
    let DeviceKey {
        index,
        path,
        pubkey,
    } = row;
    let check_path = path.clone();
    rsx! {
        li { class: "derive-row",
            input {
                r#type: "checkbox",
                checked: checked && !here,
                disabled: here,
                onchange: move |_| on_toggle.call(index),
            }
            div {
                code { class: "mono", "{path}" }
                div { class: "addr-full",
                    code { class: "mono", "{pubkey}" }
                    CopyButton { text: pubkey.to_string() }
                }
                if here {
                    span { class: "chip", "already a wallet here" }
                }
                if let Some(lamports) = lamports {
                    span { class: "muted", " {crate::ui::format_sol(lamports)}" }
                }
            }
            if vendor != HardwareVendor::Keystone {
                button { class: "sm ghost",
                    title: "Show this address on the device, to check it is the one on this screen",
                    onclick: move |_| show_address(session, vendor, check_path.clone(), pubkey, None),
                    "Check on device"
                }
            }
        }
    }
}
