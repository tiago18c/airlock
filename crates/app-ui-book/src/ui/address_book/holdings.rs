//! The token accounts found under an owner.

use super::*;

pub(in crate::ui) const HOLDING_PAGE: usize = 20;

#[component]
pub(in crate::ui) fn OwnerHoldings(owner: Pubkey) -> Element {
    let session = use_context::<Session>();
    let mut open = use_signal(|| false);
    let mut page = use_signal(|| 0usize);
    let summary = session.holding_summary(&owner);
    if summary.total == 0 {
        return rsx! {};
    }
    let pages = summary.total.div_ceil(HOLDING_PAGE).max(1);
    let page_i = page().min(pages - 1);
    let start = page_i * HOLDING_PAGE;
    let shown = session.holding_page(&owner, start, HOLDING_PAGE);
    let from = start + 1;
    let to = start + shown.len();
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

    rsx! {
        div { class: "ab-holdings",
            button {
                class: if open() { "ghost sm ab-holdings-toggle open" } else { "ghost sm ab-holdings-toggle" },
                aria_expanded: "{open()}",
                onclick: move |_| open.set(!open()),
                Icon { glyph: Glyph::ChevronRight }
                "{count}"
            }
            if open() {
                ul { class: "holding-list",
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

/// One token account on one line: the token, the account's address, and the
/// amount, with its flags at the end.
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
    let mint_text = mint.to_string();

    rsx! {
        li { class: "holding-row",
            div { class: "ab-holding-token",
                if let Some(name) = name {
                    span { class: "cat cat-token", Icon { glyph: Glyph::Coin } }
                    span { class: "ab-token-name", title: "{mint_text}", "{name}" }
                } else {
                    AddressRow { pubkey: mint, no_copy: true, no_label: true, class: "ab-holding-mint" }
                }
            }
            AddressRow { pubkey: token, no_label: true, class: "ab-holding-account" }
            span { class: "ab-holding-amount", "{amount}" }
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
