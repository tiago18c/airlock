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
    let count = if summary.red > 0 {
        format!("{} token accounts, {} red", summary.total, summary.red)
    } else {
        format!("{} token accounts", summary.total)
    };

    rsx! {
        div { class: "holdings",
            div { class: "book-head",
                p { class: "muted", "{count}" }
                button { class: "sm",
                    onclick: move |_| open.set(!open()),
                    if open() { "Hide tokens" } else { "Show tokens" }
                }
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

#[component]
pub(in crate::ui) fn HoldingRow(owner: Pubkey, holding: TokenHolding) -> Element {
    let session = use_context::<Session>();
    let token = holding.pubkey;
    let flag = holding.flag;
    let amount = format_token_amount(holding.amount, holding.decimals);

    rsx! {
        li { class: "holding-row",
            AddressRow { pubkey: token,
                span { class: "muted", "{amount}" }
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
