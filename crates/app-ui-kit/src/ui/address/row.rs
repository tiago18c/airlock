//! An address as a line of a list or a picker.

use super::inline::use_card;
use super::*;

/// Icon, label, short address, then whatever the list adds, then a copy
/// button. Hovering the address opens its card, unless `no_card`.
#[component]
pub fn AddressRow(
    pubkey: Pubkey,
    /// Shown after the address: chips, an amount, buttons.
    children: Element,
    /// Leave out the copy button, where the row itself is a button.
    #[props(default)]
    no_copy: bool,
    /// The whole address instead of its short form: for Inspect.
    #[props(default)]
    full: bool,
    /// Extra classes for the row.
    #[props(default)]
    class: String,
    /// No card on hover, where the list already says what every entry is.
    #[props(default)]
    no_card: bool,
    /// Accounts the card lists under the address's own facts, each with
    /// what it is: the token accounts a balance sits in.
    #[props(default)]
    card_accounts: Vec<(&'static str, Pubkey)>,
) -> Element {
    let described = use_described(pubkey);
    let care = needs_care(&described);
    let collides = !described.collides_with.is_empty();
    let label = described.full_label();
    let card = use_card();
    let row_class = format!(
        "addr-row{}{}",
        if care { " warn" } else { "" },
        if class.is_empty() {
            String::new()
        } else {
            format!(" {class}")
        }
    );

    rsx! {
        div { class: "{row_class}",
            span {
                class: "addr-row-main",
                onmounted: move |e| card.mounted(e),
                onmouseenter: move |_| {
                    if !no_card {
                        card.enter();
                    }
                },
                onmouseleave: move |_| card.leave(),
                CategoryIcon { category: described.category }
                if let Some(label) = label {
                    span { class: "addr-label", "{label}" }
                }
                if full {
                    code { class: "addr-short mono", "{pubkey}" }
                } else {
                    code { class: "addr-short", "{short_pubkey(&pubkey)}" }
                }
                if collides {
                    span { class: "addr-alert", title: "Another address has this name",
                        Icon { glyph: Glyph::Alert }
                    }
                }
                BalanceDelta { pubkey }
                if let Some(at) = card.at() {
                    AddressCard { described: described.clone(), at, accounts: card_accounts.clone() }
                }
            }
            span { class: "addr-row-rest", {children} }
            if !no_copy {
                CopyButton { text: pubkey.to_string() }
            }
        }
    }
}
