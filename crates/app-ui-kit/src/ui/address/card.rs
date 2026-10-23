//! What the app knows about an address, over the address.

use super::inline::{Beside, CardAt};
use super::*;

/// The whole address, what it is, and every fact the directory has, then
/// any `accounts` the place it is shown adds, such as the token accounts a
/// balance sits in.
#[component]
pub fn AddressCard(
    described: Described,
    at: CardAt,
    #[props(default)] accounts: Vec<(&'static str, Pubkey)>,
) -> Element {
    let session = use_context::<Session>();
    let pubkey = described.pubkey;
    let full = pubkey.to_string();
    let (class, style) = match at.beside.and_then(beside_place) {
        // Beside a dropdown list, level with the entry, so the entries
        // under it stay in reach. Near the bottom of the window it rises
        // from the entry's lower edge instead.
        Some((left, width, on_left)) => {
            let class = if on_left {
                "addr-card beside left"
            } else {
                "addr-card beside"
            };
            let vertical = if at.top + 280.0 > at.viewport && at.top > 280.0 {
                format!("bottom: calc(100vh - {}px);", at.bottom)
            } else {
                format!("top: {}px;", at.top)
            };
            (
                class,
                format!("left: {left}px; width: {width}px; {vertical}"),
            )
        }
        // Below the address, or above it when the address is low on the screen.
        None => {
            let flip = at.bottom + 280.0 > at.viewport && at.top > 280.0;
            let style = format!(
                "left: max(8px, min({left}px, calc(100vw - {CARD_REM}rem - 1rem))); \
                 top: {below}px; --card-above: calc(100vh - {top}px + 6px);",
                left = at.left.max(8.0),
                below = at.bottom + 6.0,
                top = at.top,
            );
            let class = if flip { "addr-card above" } else { "addr-card" };
            (class, style)
        }
    };
    let title = described.full_label();
    let unknown = described.category == Category::Unknown;
    let facts: Vec<(&'static str, String)> = described
        .facts
        .iter()
        .filter_map(|fact| fact_line(&session, described.category, fact))
        .collect();
    let others: Vec<String> = described.collides_with.iter().map(short_pubkey).collect();

    rsx! {
        div {
            class: "{class}",
            style: "{style}",
            role: "dialog",
            // Clicks inside the card do not toggle the address under it.
            onclick: move |e| e.stop_propagation(),
            div { class: "addr-card-head",
                CategoryIcon { category: described.category }
                div {
                    if let Some(title) = title {
                        p { class: "addr-card-title", "{title}" }
                    }
                    p { class: "muted", "{described.category.name()}" }
                }
            }
            div { class: "addr-card-full",
                code { class: "mono", "{full}" }
                CopyButton { text: full.clone() }
            }
            if !facts.is_empty() || !accounts.is_empty() {
                dl { class: "addr-facts",
                    for (i, (term, value)) in facts.into_iter().enumerate() {
                        div { key: "{i}",
                            dt { "{term}" }
                            dd { "{value}" }
                        }
                    }
                    for (term, account) in accounts {
                        div { key: "{account}",
                            dt { "{term}" }
                            dd { class: "addr-card-account",
                                code { class: "mono", "{account}" }
                                CopyButton { text: account.to_string() }
                            }
                        }
                    }
                }
            }
            if !others.is_empty() {
                p { class: "warn",
                    Icon { glyph: Glyph::Alert }
                    " The same name is used for {others.join(\", \")}. Rename one in the address book."
                }
            }
            if unknown {
                div { class: "btn-row",
                    button { class: "sm",
                        onclick: move |_| add_to_book(session, pubkey),
                        "Add to the address book"
                    }
                }
            }
        }
    }
}

/// The card's width, in rem. `.addr-card` in main.css matches it.
const CARD_REM: f64 = 26.25;
/// The space between a dropdown list and a card beside it, in pixels.
const BESIDE_GAP: f64 = 6.0;

/// The narrowest a card beside a dropdown list gets, in rem, before it
/// opens below the entry instead.
const BESIDE_MIN_REM: f64 = 18.0;

/// Where a card beside a dropdown list starts, how wide it is, and whether
/// it is on the list's left: on the right when it fits there, else on the
/// side with more room, narrower than usual if that side is short. `None`
/// when neither side has room (a narrow window): the card then opens as
/// usual.
fn beside_place(beside: Beside) -> Option<(f64, f64, bool)> {
    let margin = 8.0;
    let full = CARD_REM * beside.rem;
    let room_right = beside.viewport - beside.right - BESIDE_GAP - margin;
    let room_left = beside.left - BESIDE_GAP - margin;
    let on_left = room_right < full && room_left > room_right;
    let room = if on_left { room_left } else { room_right };
    if room < BESIDE_MIN_REM * beside.rem {
        return None;
    }
    let width = room.min(full);
    let left = if on_left {
        beside.left - BESIDE_GAP - width
    } else {
        beside.right + BESIDE_GAP
    };
    Some((left, width, on_left))
}

fn fact_line(session: &Session, category: Category, fact: &Fact) -> Option<(&'static str, String)> {
    Some(match fact {
        Fact::WalletKind(kind) => ("Kind", kind.clone()),
        Fact::Balance(lamports) => ("Balance", format_sol(*lamports)),
        Fact::TokenAmount {
            amount,
            decimals,
            mint,
        } => (
            "Holds",
            format!(
                "{} {}",
                format_token_amount(*amount, *decimals),
                name_or_short(session, mint)
            ),
        ),
        Fact::OwnedBy(owner) => (
            if category == Category::Nonce {
                "Authority"
            } else {
                "Owner"
            },
            name_or_short(session, owner),
        ),
        Fact::MemberOf(multisig) => ("Member of", name_or_short(session, multisig)),
        Fact::PartOf { wallet, role } => (
            "Part of",
            format!("{} ({role})", name_or_short(session, wallet)),
        ),
        Fact::PaysFeesVia(payer) => ("Fees paid by", name_or_short(session, payer)),
        Fact::Flag(TokenFlag::White) => ("Flag", "white".into()),
        Fact::Flag(TokenFlag::Red) => ("Flag", "red".into()),
        Fact::Origin(origin) => (
            "Source",
            match origin {
                Origin::Imported => "added by hand or imported",
                Origin::Derived => "derived on this device",
                Origin::Discovered => "read from the chain",
            }
            .into(),
        ),
    })
}

/// Open the address book's add form with this address filled in.
fn add_to_book(session: Session, pubkey: Pubkey) {
    let form = session.forms.contact;
    form.clear_draft();
    let mut field = form.pubkey;
    field.set(pubkey.to_string());
    let mut open = form.modal_open;
    open.set(true);
    let mut tab = session.tab;
    tab.set(Tab::AddressBook);
}
