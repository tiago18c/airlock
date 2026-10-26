//! Requests the browser extension forwarded, answered in a modal.
//!
//! A new request opens the modal. "Not now" folds it into a toast that
//! cannot be dismissed, so the rest of the app stays usable while a site
//! waits and the way back to it is always in sight.
//!
//! A connect request is Approve or Reject. A sign request is Reject, Review
//! or Sign:
//!
//! - A transaction that asks a programmatic signer or a Squads vault to sign
//!   is rebuilt first (`wallet_core::adapt`). What is shown, reviewed and
//!   signed is the rebuilt transaction.
//! - A fee payer the site named, whose key is not here and which no
//!   instruction uses, can be replaced by a hot wallet of this device. That
//!   is offered only after the replaced transaction simulated successfully.
//! - Sign is offered only when every instruction was put in words and this
//!   device can add every missing signature (`wallet_core::quick_sign`).
//!   Otherwise the only way forward is Review, on the Sign screen.

use crate::bridge::BridgePrompt;
use app_core::bridge::BridgeAnswer;
use app_ui_kit::actions::idl_check::fetch_missing_idls;
use app_ui_kit::actions::sentence;
use app_ui_kit::actions::sign::{offer_paid_here, prepare_request, PaidHere};
use app_ui_kit::state::forms::{use_blockhash_clock_state, BlockhashExpiry, TxSource};
use app_ui_kit::ui::clock::BlockhashCountdown;
use app_ui_kit::ui::tx_view::{FeeLine, InstructionList};
use app_ui_kit::{Address, AddressRow, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::adapt::{Adapted, AdaptedStep};
use wallet_core::{missing_catalog_programs, quick_sign_in, sha256_bytes};
use wallet_flows::adapt::held_back_note;

mod connect;
mod prepared;
mod sign_request;

pub(in crate::ui) use connect::*;
pub(in crate::ui) use sign_request::*;

/// Whether the request modal is open. The shell provides it, so a click on an
/// OS toast about a waiting request can open the modal.
#[derive(Clone, Copy)]
pub struct RequestModal {
    open: Signal<bool>,
}

impl RequestModal {
    pub fn show(mut self) {
        self.open.set(true);
    }
}

/// Provide the [`RequestModal`] [`BridgeRequests`] reads. Call from the root.
pub fn use_request_modal() -> RequestModal {
    use_context_provider(|| RequestModal {
        open: Signal::new(false),
    })
}

#[component]
pub fn BridgeRequests() -> Element {
    let session = use_context::<Session>();
    // The core's queue is the only one; an answer leaves it at once.
    let prompt = use_memo(move || BridgePrompt::head_of(&session.bridge.read()));
    let window = dioxus::desktop::use_window();
    let mut announced = use_signal(|| None::<String>);
    let RequestModal { mut open } = use_context::<RequestModal>();
    use_effect(move || {
        let Some(id) = prompt().map(|item| item.id) else {
            return;
        };
        if *announced.peek() == Some(id.clone()) {
            return;
        }
        let code = crate::bridge::request_code(&id);
        tracing::debug!(
            target: "airlock::bridge",
            code = %code,
            "request shown"
        );
        announced.set(Some(id));
        open.set(true);
        window.request_user_attention(Some(
            dioxus::desktop::tao::window::UserAttentionType::Critical,
        ));
    });

    let Some(current) = prompt() else {
        return rsx! {};
    };

    rsx! {
        if open() {
            div { class: "modal-backdrop",
                onclick: move |_| open.set(false),
                div { class: "modal wide request", role: "dialog", aria_modal: "true",
                    onclick: move |e| e.stop_propagation(),
                    if current.method == "connect" {
                        ConnectRequest {
                            key: "{current.id}",
                            current: current.clone(),
                            on_close: move |_| open.set(false),
                        }
                    } else {
                        SignRequest {
                            key: "{current.id}",
                            current: current.clone(),
                            on_close: move |_| open.set(false),
                        }
                    }
                }
            }
        }
    }
}

/// The request that was set aside, as a toast. It stays until the request
/// is answered. The shell puts it in the toast stack.
#[component]
pub fn RequestToast() -> Element {
    let session = use_context::<Session>();
    let prompt = use_memo(move || BridgePrompt::head_of(&session.bridge.read()));
    let RequestModal { mut open } = use_context::<RequestModal>();
    match prompt() {
        Some(current) if !open() => rsx! {
            RequestCard {
                current,
                on_open: move |_| open.set(true),
            }
        },
        _ => rsx! {},
    }
}

/// A request that was set aside, with the way back to it.
#[component]
fn RequestCard(current: BridgePrompt, on_open: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let code = crate::bridge::request_code(&current.id);
    let reviewing = matches!(
        (session.forms.sign.source)(),
        Some(TxSource::Browser { id, .. }) if id == current.id
    );
    let what = if current.method == "connect" {
        "wants to connect"
    } else if reviewing {
        "asks for a signature · open on Sign"
    } else {
        "asks for a signature"
    };
    let id = current.id.clone();
    rsx! {
        div { class: "toast request-toast", role: "status",
            div { class: "toast-head",
                span { class: "toast-label", "Browser request" }
                span { class: "chip kind",
                    title: "The extension shows the same code for this request.",
                    code { "{code}" }
                }
            }
            p { class: "bridge-origin", "{current.origin}" }
            div { class: "toast-meta",
                span { class: "muted", "{what}" }
                if current.waiting > 0 {
                    span { class: "chip", "{current.waiting} more waiting" }
                }
            }
            div { class: "btn-row toast-actions",
                button { class: "primary sm",
                    onclick: move |_| on_open.call(()),
                    "Open"
                }
                button { class: "sm",
                    onclick: move |_| {
                        session.bridge_answer(&id, BridgeAnswer::Rejected);
                    },
                    "Reject"
                }
            }
        }
    }
}

#[component]
fn RequestHead(current: BridgePrompt, title: String, on_close: EventHandler<()>) -> Element {
    let code = crate::bridge::request_code(&current.id);
    rsx! {
        div { class: "request-head",
            div { class: "chip-row",
                span { class: "eyebrow", "Browser request" }
                span { class: "chip kind",
                    title: "The extension shows the same code for this request.",
                    code { "{code}" }
                }
                if current.waiting > 0 {
                    span { class: "chip", "{current.waiting} more waiting" }
                }
            }
            button { class: "ghost sm",
                title: "Keep the request waiting and get back to the app.",
                onclick: move |_| on_close.call(()),
                "Not now"
            }
        }
        h3 { "{title}" }
        p { class: "bridge-origin", "{current.origin}" }
    }
}
