//! Top bar: the wallet the app is acting as.

use super::*;

/// Top bar: the wallet the app is acting as.
#[component]
pub fn WalletSwitcher() -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    if wallets.is_empty() {
        return rsx! {};
    }
    let selected = session.selected_wallet();
    let address = wallets
        .iter()
        .find(|wallet| Some(wallet.id) == selected)
        .and_then(|wallet| wallet.address);
    let choices = wallets.clone();
    rsx! {
        label { class: "wallet-switch", title: "The wallet the app is acting as",
            span { class: "sr-only", "Current wallet" }
            select {
                onchange: move |evt| {
                    if let Some(id) = find_id(&choices, &evt.value()) {
                        let _ = session.select_wallet(id);
                    }
                },
                for wallet in wallets {
                    option {
                        key: "{wallet.id}",
                        value: "{wallet.id}",
                        selected: selected == Some(wallet.id),
                        "{wallet.label}"
                    }
                }
            }
        }
        if let Some(address) = address {
            CopyButton { text: address.to_string(), label: "Copy wallet address", class: "sm" }
        }
    }
}
