//! A list of wallets with a checkbox each.

use super::*;

/// One checkbox per wallet, showing its label and address. `selected` keeps
/// the order the wallets were ticked in.
#[component]
pub(in crate::ui) fn WalletChecklist(
    label: String,
    options: Vec<WalletSummary>,
    selected: Vec<WalletId>,
    on_change: EventHandler<Vec<WalletId>>,
) -> Element {
    rsx! {
        p { class: "muted", "{label}" }
        ul { class: "derive-list",
            for wallet in options {
                li { class: "derive-row", key: "{wallet.id}",
                    input {
                        r#type: "checkbox",
                        "aria-label": "{wallet.label}",
                        checked: selected.contains(&wallet.id),
                        onchange: {
                            let selected = selected.clone();
                            move |_| {
                                let mut next = selected.clone();
                                match next.iter().position(|id| *id == wallet.id) {
                                    Some(at) => {
                                        next.remove(at);
                                    }
                                    None => next.push(wallet.id),
                                }
                                on_change.call(next);
                            }
                        },
                    }
                    match wallet.address {
                        Some(pubkey) => rsx! { AddressRow { pubkey } },
                        None => rsx! { p { "{wallet.label}" } },
                    }
                }
            }
        }
    }
}
