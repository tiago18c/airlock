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
    /// Leave out the category icon, where every entry of the list is of
    /// one kind and the icon would tell none apart. An address that needs
    /// care keeps its icon, so colour is not the only sign of it.
    #[props(default)]
    no_icon: bool,
    /// Accounts the card lists under the address's own facts, each with
    /// what it is: the token accounts a balance sits in.
    #[props(default)]
    card_accounts: Vec<(&'static str, Pubkey)>,
    /// Leave out the label, where the list shows the name in a column of
    /// its own.
    #[props(default)]
    no_label: bool,
    /// The short address can lose two more characters at each end where
    /// room is short: they are marked `addr-short-more`, for the caller's
    /// styles to hide. It reads the same to a screen reader either way.
    #[props(default)]
    shrinks: bool,
) -> Element {
    let described = use_described(pubkey);
    let care = needs_care(&described);
    let collides = !described.collides_with.is_empty();
    let label = described.full_label().filter(|_| !no_label);
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
                if !no_icon || care {
                    CategoryIcon { category: described.category }
                }
                if let Some(label) = label {
                    span { class: "addr-label", "{label}" }
                }
                if full {
                    code { class: "addr-short mono", "{pubkey}" }
                } else if let (true, Some([head, more_head, more_tail, tail])) = (shrinks, short_parts(&pubkey)) {
                    code { class: "addr-short",
                        "{head}"
                        span { class: "addr-short-more", "{more_head}" }
                        "…"
                        span { class: "addr-short-more", "{more_tail}" }
                        "{tail}"
                    }
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

/// `short_pubkey`'s form in four parts: what a tighter form keeps at each
/// end, and the two characters beside the ellipsis that it leaves out. None
/// for an address too short to have been shortened.
fn short_parts(pubkey: &Pubkey) -> Option<[String; 4]> {
    let short = short_pubkey(pubkey);
    let (head, tail) = short.split_once('…')?;
    // Base58 is ASCII, so these are character boundaries.
    let (keep_head, more_head) = head.split_at(head.len().checked_sub(2)?);
    let (more_tail, keep_tail) = tail.split_at(2.min(tail.len()));
    Some([
        keep_head.to_string(),
        more_head.to_string(),
        more_tail.to_string(),
        keep_tail.to_string(),
    ])
}
