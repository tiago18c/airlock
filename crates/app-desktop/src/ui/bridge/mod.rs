//! Requests the browser extension forwarded, answered in a modal.
//!
//! A new request opens the modal. "Not now" folds it into a toast that
//! cannot be dismissed, so the rest of the app stays usable while a site
//! waits and the way back to it is always in sight.
//!
//! A connect request is Approve or Reject. A sign request is Reject, Review
//! or Sign. A site may send several transactions in one request: each is
//! prepared and explained on its own card, and the site gets them back
//! together once every one is signed, never some of them.
//!
//! - A transaction that asks a programmatic signer or a Squads vault to sign
//!   is rebuilt first (`wallet_core::adapt`). What is shown, reviewed and
//!   signed is the rebuilt transaction.
//! - A fee payer the site named, whose key is not here and which no
//!   instruction uses, can be replaced by a hot wallet of this device. That
//!   is offered only after the replaced transaction simulated successfully.
//! - Sign is offered when this device can add every missing signature
//!   (`wallet_core::quick_sign`). It is the primary button only when every
//!   instruction was put in words by an IDL that ships with the app or that the user
//!   imported, and the node expects it to succeed. Otherwise it is amber and says
//!   why. What hands over control, or says yes to what the transaction does not hold,
//!   gets no Sign here at all: the only way forward is Review, on the Sign screen.
//!   `wallet_flows::offer::modal_offer` decides.

use crate::bridge::BridgePrompt;
use app_core::bridge::{BridgeAnswer, SendPlan, SiteCall};
use app_core::ports::Clock;
use app_ui_kit::actions::idl_check::fetch_missing_idls;
use app_ui_kit::actions::sentence;
use app_ui_kit::actions::sign::{offer_paid_here, prepare_request, PaidHere, ONTO_NONCE_NOTE};
use app_ui_kit::state::forms::{use_blockhash_clock_state, BlockhashExpiry, SimState, TxSource};
use app_ui_kit::ui::clock::BlockhashCountdown;
use app_ui_kit::ui::tx_view::{
    use_explained_for_site, FeeLine, InstructionList, ReadingNote, RiskNotice, SimulatedDeltas,
};
use app_ui_kit::{Address, AddressRow, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::sync::Arc;
use std::time::Duration;
use wallet_core::adapt::{Adapted, AdaptedStep};
use wallet_core::{missing_catalog_programs, quick_sign_in, sha256_bytes};
use wallet_flows::adapt::held_back_note;

mod connect;
mod prepared;
mod sign_message;
mod sign_request;

pub(in crate::ui) use connect::*;
pub(in crate::ui) use sign_message::*;
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
    use_context_provider(crate::ui::connections::ConnectionsModal::closed);
    use_context_provider(|| RequestModal {
        open: Signal::new(false),
    })
}

/// How long a button that approves a request stays off once it could be pressed.
const ARM_DELAY: Duration = Duration::from_millis(800);

/// Whether an approving button that could be pressed (`ready`) may be pressed yet.
/// A site chooses when its request appears, so a click meant for something else,
/// perhaps on its own page, could land on the button. It has to have been on
/// screen and ready for a moment first, and starts again each time it is not.
fn use_armed(ready: bool) -> bool {
    let session = use_context::<Session>();
    use_armed_on(session.clock(), ready)
}

fn use_armed_on(clock: Arc<dyn Clock>, ready: bool) -> bool {
    let mut armed = use_signal(|| false);
    // Counts changes of `ready`, so an older wait cannot arm a newer one.
    let mut turn = use_signal(|| 0u64);
    use_effect(use_reactive!(|(ready)| {
        let mine = *turn.peek() + 1;
        turn.set(mine);
        armed.set(false);
        if !ready {
            return;
        }
        let waited = clock.sleep(ARM_DELAY);
        spawn(async move {
            waited.await;
            if *turn.peek() == mine {
                armed.set(true);
            }
        });
    }));
    ready && armed()
}

#[cfg(test)]
mod arming_tests {
    use super::*;
    use app_core::testing::ManualClock;
    use dioxus::dioxus_core::{NoOpMutations, VirtualDom};
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct Held {
        clock: Arc<ManualClock>,
        ready: Rc<Cell<Option<Signal<bool>>>>,
        armed: Rc<Cell<bool>>,
    }

    fn app(held: Held) -> Element {
        let ready = use_signal(|| false);
        held.ready.set(Some(ready));
        let armed = use_armed_on(held.clock.clone(), ready());
        held.armed.set(armed);
        rsx! {}
    }

    async fn settle(dom: &mut VirtualDom) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(20), dom.wait_for_work()).await;
            dom.render_immediate(&mut NoOpMutations);
        }
    }

    #[tokio::test]
    async fn a_button_arms_only_after_it_has_been_ready_for_a_moment() {
        let clock = Arc::new(ManualClock::default());
        let held = Held {
            clock: clock.clone(),
            ready: Rc::new(Cell::new(None)),
            armed: Rc::new(Cell::new(false)),
        };
        let mut dom = VirtualDom::new_with_props(app, held.clone());
        dom.rebuild_in_place();
        settle(&mut dom).await;
        let armed = || held.armed.get();
        assert!(!armed());
        let mut ready = held.ready.get().unwrap();

        // Not ready: time passing arms nothing.
        clock.advance(ARM_DELAY * 2);
        settle(&mut dom).await;
        assert!(!armed());

        dom.in_runtime(|| ready.set(true));
        settle(&mut dom).await;
        assert!(!armed());
        clock.advance(ARM_DELAY - Duration::from_millis(1));
        settle(&mut dom).await;
        assert!(!armed());
        clock.advance(Duration::from_millis(1));
        settle(&mut dom).await;
        assert!(armed());

        // Off at once when it no longer could be pressed, and it starts again.
        dom.in_runtime(|| ready.set(false));
        settle(&mut dom).await;
        assert!(!armed());
        dom.in_runtime(|| ready.set(true));
        settle(&mut dom).await;
        assert!(!armed());
        clock.advance(ARM_DELAY);
        settle(&mut dom).await;
        assert!(armed());
    }

    #[tokio::test]
    async fn a_wait_from_before_does_not_arm_a_later_one() {
        let clock = Arc::new(ManualClock::default());
        let held = Held {
            clock: clock.clone(),
            ready: Rc::new(Cell::new(None)),
            armed: Rc::new(Cell::new(false)),
        };
        let mut dom = VirtualDom::new_with_props(app, held.clone());
        dom.rebuild_in_place();
        settle(&mut dom).await;
        let mut ready = held.ready.get().unwrap();

        dom.in_runtime(|| ready.set(true));
        settle(&mut dom).await;
        clock.advance(ARM_DELAY / 2);
        settle(&mut dom).await;
        dom.in_runtime(|| ready.set(false));
        settle(&mut dom).await;
        dom.in_runtime(|| ready.set(true));
        settle(&mut dom).await;

        // The first wait is up now. The second, which started later, is not.
        clock.advance(ARM_DELAY / 2);
        settle(&mut dom).await;
        assert!(!held.armed.get());
        clock.advance(ARM_DELAY / 2);
        settle(&mut dom).await;
        assert!(held.armed.get());
    }
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
                    if matches!(current.request.call, SiteCall::Connect) {
                        ConnectRequest {
                            key: "{current.id}",
                            current: current.clone(),
                            on_close: move |_| open.set(false),
                        }
                    } else if matches!(current.request.call, SiteCall::SignMessage { .. }) {
                        MessageRequest {
                            key: "{current.id}",
                            current: current.clone(),
                            on_close: move |_| open.set(false),
                        }
                    } else if matches!(current.request.call, SiteCall::SignIn(_)) {
                        SignInRequest {
                            key: "{current.id}",
                            current: current.clone(),
                            on_close: move |_| open.set(false),
                        }
                    } else if current.transactions > 1 {
                        SignBatch {
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
    let what = match (&current.request.call, current.transactions, reviewing) {
        (SiteCall::Connect, _, _) => "wants to connect".to_string(),
        (SiteCall::SignMessage { .. }, _, _) => "asks you to sign a message".to_string(),
        (SiteCall::SignIn(_), _, _) => "asks you to sign in".to_string(),
        (SiteCall::Send { .. }, count, _) if count > 1 => {
            format!("asks you to sign and send {count} transactions")
        }
        (SiteCall::Send { .. }, _, _) => "asks you to sign and send a transaction".to_string(),
        (SiteCall::SignTransactions(_), count, true) if count > 1 => {
            format!("asks to sign {count} transactions · open on Sign")
        }
        (SiteCall::SignTransactions(_), count, false) if count > 1 => {
            format!("asks to sign {count} transactions")
        }
        (SiteCall::SignTransactions(_), _, true) => {
            "asks for a signature · open on Sign".to_string()
        }
        (SiteCall::SignTransactions(_), _, false) => "asks for a signature".to_string(),
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

/// A site's network against the app's. A send cannot go on another network,
/// so it waits for the user to change it. A signature goes back to the site,
/// which may know what it is doing, so that is only said.
#[component]
fn ChainNotice(current: BridgePrompt) -> Element {
    let session = use_context::<Session>();
    let cluster = session.cluster();
    let Some(chain) = current
        .chains
        .iter()
        .find(|chain| cluster.differs_from_chain(chain))
    else {
        return rsx! {};
    };
    let name = cluster.name().to_string();
    rsx! {
        p { class: "warn",
            if current.sends {
                "This site is on {chain}, and Airlock is on {name}. Change the network to send this."
            } else {
                "This site says it is on {chain}, and Airlock is on {name}. What you see here is read from {name}."
            }
        }
    }
}

/// Whether a send has to wait for the user to change the network.
fn send_waits_for_network(session: Session, current: &BridgePrompt) -> bool {
    let cluster = session.cluster();
    current.sends
        && current
            .chains
            .iter()
            .any(|chain| cluster.differs_from_chain(chain))
}

/// How a request's Sign reads. `device` is the hardware wallet a single
/// transaction is signed on.
fn sign_label(
    current: &BridgePrompt,
    count: Option<usize>,
    device: Option<wallet_core::HardwareVendor>,
) -> String {
    match (current.sends, count, device) {
        (true, Some(count), _) => format!("Sign and send all {count}"),
        (true, None, Some(vendor)) => format!("Sign on {} and send", vendor.label()),
        (true, None, None) => "Sign and send".to_string(),
        (false, Some(count), _) => format!("Sign all {count}"),
        (false, None, Some(vendor)) => format!("Sign on {}", vendor.label()),
        (false, None, None) => "Sign".to_string(),
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
