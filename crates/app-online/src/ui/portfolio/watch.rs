//! Which address book accounts count in the portfolio, next to the wallets.

use app_ui_kit::{AddressRow, Session, Tab};
use dioxus::prelude::*;
use wallet_core::AddressKind;

/// A switch per account in the address book. A watched account is counted
/// in the portfolio like a wallet, without a key.
#[component]
pub(super) fn WatchModal(open: Signal<bool>) -> Element {
    let session = use_context::<Session>();
    let mut query = use_signal(String::new);
    if !open() {
        return rsx! {};
    }
    let wallets: Vec<_> = session
        .wallet_summaries()
        .into_iter()
        .filter_map(|wallet| wallet.address)
        .collect();
    let book = (session.book)();
    let needle = query().trim().to_lowercase();
    let mut rows: Vec<(solana_pubkey::Pubkey, String, bool)> = book
        .iter()
        .filter(|entry| entry.kind == AddressKind::Account && !wallets.contains(&entry.pubkey))
        .filter(|entry| {
            needle.is_empty()
                || entry.label.to_lowercase().contains(&needle)
                || entry.pubkey.to_string().to_lowercase().contains(&needle)
        })
        .map(|entry| (entry.pubkey, entry.label.clone(), entry.watched))
        .collect();
    // Watched first, then by name.
    rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    let watched = rows.iter().filter(|row| row.2).count();
    let mut close = move || {
        query.set(String::new());
        open.set(false);
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal wide",
                role: "dialog",
                aria_modal: "true",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Watched addresses" }
                    p { class: "lede",
                        "Count an address from the address book in the portfolio, next to the wallets of this device. Nothing here can sign for it."
                    }
                    input {
                        r#type: "search",
                        placeholder: "Filter by name or address",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                    if rows.is_empty() {
                        p { class: "hint",
                            if needle.is_empty() {
                                "The address book has no accounts besides the wallets. Add one there to watch it."
                            } else {
                                "No account matches."
                            }
                        }
                    } else {
                        p { class: "hint", "{watched} watched." }
                        ul { class: "book-list pf-watch-list",
                            for (pubkey, _label, on) in rows {
                                li { class: "book-row", key: "{pubkey}",
                                    AddressRow { pubkey,
                                        label { class: "check",
                                            input {
                                                r#type: "checkbox",
                                                checked: on,
                                                onchange: move |e| {
                                                    let watch = e.checked();
                                                    session.edit_workspace(|ws| {
                                                        if let Some(entry) = ws.book.get_mut(&pubkey) {
                                                            entry.watched = watch;
                                                        }
                                                    });
                                                    if watch {
                                                        // Price what it holds now, not in five minutes.
                                                        spawn(async move {
                                                            let _ = session.refresh_prices().await;
                                                        });
                                                    }
                                                },
                                            }
                                            " Count in portfolio"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| {
                                close();
                                session.goto(Tab::AddressBook);
                            },
                            "Open Address Book"
                        }
                        button { class: "primary",
                            onclick: move |_| close(),
                            "Done"
                        }
                    }
                }
            }
        }
    }
}
