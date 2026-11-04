//! The device dialog: what a hardware wallet is asked, and what the user
//! does on it. Mounted once, by the shell, above the other dialogs, so the
//! Sign screen, the request modal and Add wallet all show their jobs here.

use crate::actions::device::{cancel, keystone_scanned};
use crate::state::device::{DeviceAsk, DeviceRun, DeviceStage};
use crate::state::session::Session;
use crate::ui::transfer::receive::QrScanner;
use crate::ui::transfer::ur::UrCarousel;
use dioxus::prelude::*;
use wallet_core::HardwareVendor;

#[component]
pub fn DeviceDialog() -> Element {
    let session = use_context::<Session>();
    let Some(run) = (session.device)() else {
        return rsx! {};
    };
    let title = run.title();
    let vendor = run.vendor.label();
    let ended = run.ended();
    let close_label = if ended {
        "Close"
    } else if run.stage == DeviceStage::Confirm {
        "Close and stop waiting"
    } else {
        "Cancel"
    };

    rsx! {
        div { class: "modal-backdrop device-backdrop",
            div { class: if run.vendor == HardwareVendor::Keystone { "modal wide device-dialog" } else { "modal device-dialog" },
                role: "dialog",
                aria_label: "{title}",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    if let Some(model) = run.model.as_ref() {
                        p { class: "muted", "{vendor} {model}" }
                    }
                    if run.vendor == HardwareVendor::Keystone {
                        KeystoneSteps { run: run.clone() }
                    } else {
                        DeviceSteps { run: run.clone() }
                    }
                    div { class: "btn-row",
                        button {
                            class: if ended { "primary" } else { "ghost" },
                            onclick: move |_| cancel(session),
                            "{close_label}"
                        }
                    }
                }
            }
        }
    }
}

/// The steps of the job, each one marked as it passes.
#[component]
fn DeviceSteps(run: DeviceRun) -> Element {
    let session = use_context::<Session>();
    let vendor = run.vendor.label();
    let device_vendor = run.vendor;
    // A page sees a device only once the user chose it, from a click.
    let choose =
        matches!(run.stage, DeviceStage::Find(_)) && crate::actions::device::needs_request(session);
    let order = |stage: &DeviceStage| match stage {
        DeviceStage::Find(_) => 0,
        DeviceStage::Account => 1,
        DeviceStage::Confirm => 2,
        DeviceStage::Done => 3,
        DeviceStage::Failed(_) => 4,
    };
    let at = order(&run.stage);
    let failed = matches!(run.stage, DeviceStage::Failed(_));
    let mark = |step: usize| {
        if failed {
            "device-step"
        } else if step < at {
            "device-step done"
        } else if step == at {
            "device-step now"
        } else {
            "device-step later"
        }
    };
    let find_text = match &run.stage {
        DeviceStage::Find(text) => text.clone(),
        _ => format!("Your {vendor} is connected, with the Solana app open."),
    };
    let account_text = match &run.ask {
        DeviceAsk::Read { scheme } => format!("Read the accounts at {scheme}."),
        DeviceAsk::Show { path, .. } | DeviceAsk::Sign { path, .. } => {
            format!("Check that the key at {path} is this wallet's.")
        }
    };
    let shows_account = !matches!(run.ask, DeviceAsk::Show { .. });

    rsx! {
        ol { class: "device-steps",
            li { class: mark(0), role: if at == 0 { "status" } else { "" },
                "{find_text}"
                if choose {
                    div { class: "btn-row",
                        button { class: "primary sm",
                            onclick: move |_| crate::actions::device::request_device(session, device_vendor),
                            "Choose your {vendor}"
                        }
                    }
                    span { class: "hint",
                        " The browser lists the devices it sees. Choose yours once, and this page can open it from then on."
                    }
                }
            }
            if shows_account {
                li { class: mark(1), "{account_text}" }
            }
            if !matches!(run.ask, DeviceAsk::Read { .. }) {
                li { class: mark(2),
                    ConfirmStep { ask: run.ask.clone(), vendor }
                }
            }
        }
        match &run.stage {
            DeviceStage::Failed(why) => rsx! {
                p { class: "error", role: "alert", "{why}" }
            },
            DeviceStage::Done => match &run.ask {
                DeviceAsk::Show { .. } => rsx! {
                    p { class: "hint", role: "status",
                        "Your {vendor} holds this address. If it showed another one, do not use this wallet: remove it and add it again from the device."
                    }
                },
                _ => rsx! {},
            },
            DeviceStage::Confirm => rsx! {
                p { class: "hint",
                    "To stop, reject it on your {vendor}. Closing this does not stop the {vendor} from asking."
                }
            },
            _ => rsx! {},
        }
    }
}

/// What to confirm on the device, and what it will show.
#[component]
fn ConfirmStep(ask: DeviceAsk, vendor: &'static str) -> Element {
    match ask {
        DeviceAsk::Show { account, .. } => rsx! {
            span { "Check that your {vendor} shows this address, then approve it there." }
            pre { class: "mono device-address", "{account}" }
        },
        DeviceAsk::Sign { message: true, .. } => rsx! {
            span { "Read the message on your {vendor}, then sign it there." }
        },
        DeviceAsk::Sign {
            authorization: true,
            hash,
            ..
        } => rsx! {
            span {
                "Your {vendor} signs this authorization blind: what it lets the programmatic signer do is inside it, and this screen is where you read that. Blind signing has to be on in the Solana app."
            }
            if let Some(hash) = hash {
                span { class: "hint",
                    " It shows a Message Hash, which must be "
                    code { class: "mono", "{hash}" }
                    ". The fee payer it names pays nothing: the authorization is never sent on its own."
                }
            }
        },
        DeviceAsk::Sign { hash, .. } => rsx! {
            span { "Check what your {vendor} shows against this screen, then sign it there." }
            if let Some(hash) = hash {
                span { class: "hint",
                    " If it shows a Message Hash, it must be "
                    code { class: "mono", "{hash}" }
                    ". The fee payer it names must be the one on this screen."
                }
            }
        },
        DeviceAsk::Read { .. } => rsx! {},
    }
}

/// A Keystone's job: it reads the request from codes on this screen, and
/// answers with codes on its own, which this screen scans.
#[component]
fn KeystoneSteps(run: DeviceRun) -> Element {
    let session = use_context::<Session>();
    let mut scanning = use_signal(|| false);
    if let DeviceAsk::Show { account, .. } = &run.ask {
        return rsx! {
            p { "On your Keystone, open this account's Receive screen, and check that it shows this address." }
            pre { class: "mono device-address", "{account}" }
        };
    }
    let what = match &run.ask {
        DeviceAsk::Sign { message: true, .. } => "the message",
        DeviceAsk::Sign {
            authorization: true,
            ..
        } => "the authorization",
        _ => "the transaction",
    };
    let Some(trip) = run.keystone.clone() else {
        return match &run.stage {
            DeviceStage::Failed(why) => rsx! { p { class: "error", role: "alert", "{why}" } },
            _ => rsx! { p { class: "muted", role: "status", "Preparing the request…" } },
        };
    };
    rsx! {
        ol { class: "device-steps",
            li { class: if scanning() { "device-step done" } else { "device-step now" },
                "On your Keystone, scan these codes, read {what} there, and sign it."
            }
            li { class: if scanning() { "device-step now" } else { "device-step later" },
                "Then scan the signature your Keystone shows."
            }
        }
        if scanning() {
            QrScanner {
                on_payload: move |_| {},
                on_ur: move |(ur_type, message): (String, Vec<u8>)| keystone_scanned(session, &ur_type, &message),
            }
        } else {
            UrCarousel {
                ur_type: hw_protocol::keystone::SOL_SIGN_REQUEST.to_string(),
                message: trip.request.clone(),
            }
        }
        if let Some(problem) = trip.problem.clone() {
            p { class: "warn", role: "alert", "{problem}" }
        }
        if let DeviceStage::Failed(why) = &run.stage {
            p { class: "error", role: "alert", "{why}" }
        }
        div { class: "btn-row",
            button {
                class: if scanning() { "" } else { "primary" },
                onclick: move |_| scanning.set(!scanning()),
                if scanning() { "Show the request again" } else { "Scan the signature" }
            }
        }
    }
}
