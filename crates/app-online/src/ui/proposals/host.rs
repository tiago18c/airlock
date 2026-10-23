//! Mounts the Proposals modal when Wallets opens the session slot.

use app_ui_kit::Session;
use dioxus::prelude::*;

use super::ProposalsModal;

/// Renders [`ProposalsModal`] when [`Session::proposals`] is set.
#[component]
pub fn ProposalsHost() -> Element {
    let session = use_context::<Session>();
    match (session.proposals)() {
        Some(open) => rsx! {
            ProposalsModal {
                wallet: open.wallet,
                label: open.label,
                squad: open.squad,
                on_close: move |_| session.close_proposals(),
            }
        },
        None => rsx! {},
    }
}
