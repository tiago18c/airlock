//! The token accounts found under an owner.

use super::*;

pub(in crate::ui) const HOLDING_PAGE: usize = 20;

/// How many token accounts an owner has, at the end of its kind line: a
/// button that opens them under the row. Nothing when it has none.
#[component]
pub(in crate::ui) fn HoldingsToggle(owner: Pubkey, open: Signal<bool>) -> Element {
    let session = use_context::<Session>();
    let summary = session.holding_summary(&owner);
    if summary.total == 0 {
        return rsx! {};
    }
    let noun = if summary.total == 1 {
        "token account"
    } else {
        "token accounts"
    };
    let count = if summary.red > 0 {
        format!("{} {noun}, {} red", summary.total, summary.red)
    } else {
        format!("{} {noun}", summary.total)
    };

    // The separator does not break from the toggle: when the line wraps, the
    // two go to the next line together.
    rsx! {
        " "
        span { class: "ab-holdings-more",
            span { class: "ab-kind-sep", aria_hidden: "true", "·\u{a0}" }
            button {
                class: if open() { "ab-holdings-toggle open" } else { "ab-holdings-toggle" },
                r#type: "button",
                aria_expanded: "{open()}",
                onclick: move |_| open.set(!open()),
                "{count}"
                Icon { glyph: Glyph::ChevronRight }
            }
        }
    }
}

/// An owner's token accounts, under its row while `open`.
#[component]
pub(in crate::ui) fn OwnerHoldings(owner: Pubkey, open: Signal<bool>) -> Element {
    let session = use_context::<Session>();
    let mut page = use_signal(|| 0usize);
    let summary = session.holding_summary(&owner);
    if summary.total == 0 || !open() {
        return rsx! {};
    }
    let pages = summary.total.div_ceil(HOLDING_PAGE).max(1);
    let page_i = page().min(pages - 1);
    let start = page_i * HOLDING_PAGE;
    let shown = session.holding_page(&owner, start, HOLDING_PAGE);
    let from = start + 1;
    let to = start + shown.len();

    rsx! {
        div { class: "ab-holdings",
            ul { class: "holding-list",
                // What each column holds, in the columns of the rows.
                // Hidden where the rows wrap.
                li { class: "ab-holdings-head", aria_hidden: "true",
                    span { class: "ab-holdings-head-token", "Token" }
                    span { class: "ab-holdings-head-account", "Account" }
                    span { class: "ab-holdings-head-amount", "Amount" }
                }
                for holding in shown {
                    HoldingRow { key: "{holding.pubkey}", owner, holding }
                }
            }
            if pages > 1 {
                div { class: "pager",
                    button {
                        disabled: page_i == 0,
                        onclick: move |_| page.set(page_i.saturating_sub(1)),
                        "Previous"
                    }
                    p { class: "muted", "{from}–{to} of {summary.total}" }
                    button {
                        disabled: page_i + 1 >= pages,
                        onclick: move |_| page.set(page_i + 1),
                        "Next"
                    }
                }
            }
        }
    }
}

/// What to call a token: its name in the book, else its symbol or name as
/// the market gives them. `None` when nothing names it.
fn token_name(session: &Session, mint: &Pubkey) -> Option<String> {
    session.label_of(mint).or_else(|| {
        let holdings = (session.holdings_view)();
        let quote = holdings.market.quote(mint)?;
        quote
            .symbol
            .clone()
            .or_else(|| quote.name.clone())
            .filter(|name| !name.trim().is_empty())
    })
}

/// A token's symbol, to write after an amount: the market's, else the name
/// the book gives its mint. `None` when neither has one.
fn token_symbol(session: &Session, mint: &Pubkey) -> Option<String> {
    let holdings = (session.holdings_view)();
    holdings
        .market
        .quote(mint)
        .and_then(|quote| quote.symbol.clone())
        .or_else(|| (session.book)().get(mint).map(|entry| entry.label.clone()))
        .map(|symbol| symbol.trim().to_string())
        .filter(|symbol| !symbol.is_empty())
}

/// One token account on one line: the token, the account's address, and the
/// amount with the token's symbol, in the columns of the owner's row, with
/// its flags at the end.
#[component]
pub(in crate::ui) fn HoldingRow(owner: Pubkey, holding: TokenHolding) -> Element {
    let session = use_context::<Session>();
    let token = holding.pubkey;
    let mint = holding.mint;
    let flag = holding.flag;
    let amount = wallet_core::portfolio::format_amount(
        u128::from(holding.amount),
        holding.decimals,
        usize::from(holding.decimals),
    );
    let name = token_name(&session, &mint);
    let symbol = token_symbol(&session, &mint);
    let mint_text = mint.to_string();

    rsx! {
        li { class: "holding-row",
            div { class: "ab-holding-token",
                // Every row leads with a mark, so the names line up: the
                // token's coin, or a plain "?" for a token nothing names.
                if let Some(name) = name {
                    span { class: "cat cat-token ab-token-icon", Icon { glyph: Glyph::Coin } }
                    span { class: "ab-token-name", title: "{mint_text}", "{name}" }
                } else {
                    span { class: "ab-token-icon ab-token-mark", aria_hidden: "true", "?" }
                    AddressRow { pubkey: mint, no_copy: true, no_label: true, class: "ab-holding-mint",
                        span { class: "ab-token-unknown", "Unknown token" }
                    }
                }
            }
            AddressRow { pubkey: token, no_label: true, class: "ab-holding-account" }
            span { class: "ab-holding-amount",
                "{amount}"
                if let Some(symbol) = symbol {
                    span { class: "ab-holding-symbol", " {symbol}" }
                }
            }
            div { class: if flag.is_some() { "flag-actions visible" } else { "flag-actions" },
                if flag != Some(TokenFlag::Red) {
                    button {
                        class: "ghost icon-btn sm flag white",
                        title: if flag == Some(TokenFlag::White) {
                            "Clear white flag"
                        } else {
                            "White flag"
                        },
                        onclick: move |_| {
                            let next = if flag == Some(TokenFlag::White) {
                                None
                            } else {
                                Some(TokenFlag::White)
                            };
                            session.edit_holdings(|holdings| {
                                holdings.set_flag(&owner, &token, next);
                            });
                        },
                        Icon { glyph: Glyph::Flag }
                    }
                }
                if flag != Some(TokenFlag::White) {
                    button {
                        class: "ghost icon-btn sm flag red",
                        title: if flag == Some(TokenFlag::Red) {
                            "Clear red flag"
                        } else {
                            "Red flag"
                        },
                        onclick: move |_| {
                            let next = if flag == Some(TokenFlag::Red) {
                                None
                            } else {
                                Some(TokenFlag::Red)
                            };
                            session.edit_holdings(|holdings| {
                                holdings.set_flag(&owner, &token, next);
                            });
                        },
                        Icon { glyph: Glyph::Flag }
                    }
                }
            }
        }
    }
}
