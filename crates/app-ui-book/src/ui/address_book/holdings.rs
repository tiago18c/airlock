//! The token accounts found under an owner.

use super::*;

pub(in crate::ui) const HOLDING_PAGE: usize = 20;

/// How many tints a token's mark is drawn on: the `ab-tint-*` classes.
const TOKEN_TINTS: usize = 5;

/// The letters of a token's mark: the first two letters or digits of its
/// name, in capitals; its first two characters when it has none.
pub(in crate::ui) fn token_initials(name: &str) -> String {
    let letters: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect();
    let letters = if letters.is_empty() {
        name.trim().chars().take(2).collect()
    } else {
        letters
    };
    letters.to_uppercase()
}

/// Which tint a token's mark takes: picked from its mint, so a token keeps
/// its tint wherever it shows and two tokens of one name may differ.
pub(in crate::ui) fn token_tint(mint: &Pubkey) -> usize {
    let hash = mint.to_bytes().iter().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(*byte))
    });
    hash as usize % TOKEN_TINTS
}

/// A named token's mark: a small round avatar of its initials on a tint of
/// its mint. A token nothing names has the plain "?" mark instead.
#[component]
pub(in crate::ui) fn TokenAvatar(
    mint: Pubkey,
    name: String,
    #[props(into, default)] class: String,
    #[props(into, default)] title: String,
) -> Element {
    let initials = token_initials(&name);
    let tint = token_tint(&mint);
    let title = if title.is_empty() { name } else { title };
    rsx! {
        span {
            class: "{class} ab-token-avatar ab-tint-{tint}",
            title: "{title}",
            aria_hidden: "true",
            "{initials}"
        }
    }
}

/// How many token accounts an owner has, at the end of its kind line: a
/// button that opens them under the row. Nothing when it has none. `alone`
/// when it is all the line holds, so nothing separates it from what is
/// before it. It reads "N token accounts" on a desktop and a phone, where
/// it shares the address's line and is kept from wrapping.
#[component]
pub(in crate::ui) fn HoldingsToggle(
    owner: Pubkey,
    open: Signal<bool>,
    #[props(default)] alone: bool,
) -> Element {
    let session = use_context::<Session>();
    let summary = session.holding_summary(&owner);
    if summary.total == 0 {
        return rsx! {};
    }
    let count_of = |one: &str, many: &str| {
        let noun = if summary.total == 1 { one } else { many };
        if summary.red > 0 {
            format!("{} {noun}, {} red", summary.total, summary.red)
        } else {
            format!("{} {noun}", summary.total)
        }
    };
    let count = count_of("token account", "token accounts");

    // The separator does not break from the toggle: when the line wraps, the
    // two go to the next line together.
    rsx! {
        if !alone {
            " "
        }
        span { class: "ab-holdings-more",
            if !alone {
                span { class: "ab-kind-sep", aria_hidden: "true", "·\u{a0}" }
            }
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
    if crate::ui::use_phone_layout() {
        return rsx! {
            PhoneHoldingRow { owner, holding, name, symbol, amount }
        };
    }
    let white_label = if flag == Some(TokenFlag::White) {
        "Clear white flag".to_string()
    } else {
        format!("Flag white: {WHITE_FLAG_MEANS}")
    };
    let red_label = if flag == Some(TokenFlag::Red) {
        "Clear red flag".to_string()
    } else {
        format!("Flag red: {RED_FLAG_MEANS}")
    };

    rsx! {
        li { class: "holding-row",
            div { class: "ab-holding-token",
                // Every row leads with a mark, so the names line up: the
                // token's initials on its tint, or a plain "?" for a token
                // nothing names.
                if let Some(name) = name {
                    TokenAvatar { mint, name: name.clone(), class: "ab-token-icon" }
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
            // Each button says what its flag does, as the phone's sheet does.
            div { class: if flag.is_some() { "flag-actions visible" } else { "flag-actions" },
                if flag != Some(TokenFlag::Red) {
                    button {
                        class: "ghost icon-btn sm flag white",
                        title: "{white_label}",
                        aria_label: "{white_label}",
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
                        title: "{red_label}",
                        aria_label: "{red_label}",
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

/// What a white flag means, in words. It changes nothing but where the
/// token account is listed: after the red ones, before the rest.
const WHITE_FLAG_MEANS: &str =
    "A mark of your own. It changes nothing but the order: listed after red ones, before the rest.";

/// What a red flag means, in words: what the app does with such a token
/// account (see Portfolio's total, Send's token list and Manage tokens).
const RED_FLAG_MEANS: &str =
    "Left out of Portfolio's total, listed last when sending, and offered to burn and close in Manage tokens.";

/// One token account on a phone, in two lines that cannot run into each
/// other: the token's mark and name (its short mint, captioned "Mint", when
/// nothing names it), with its flag if it has one, and the amount at the
/// right; under them the account's short address, captioned "Account", with
/// the copy icon right after it. One "…" at the row's end opens a sheet to
/// set or clear a flag, each choice with what it means, in place of two bare
/// flag buttons.
#[component]
fn PhoneHoldingRow(
    owner: Pubkey,
    holding: TokenHolding,
    name: Option<String>,
    symbol: Option<String>,
    amount: String,
) -> Element {
    let session = use_context::<Session>();
    let mut sheet_open = use_signal(|| false);
    let token = holding.pubkey;
    let mint = holding.mint;
    let flag = holding.flag;
    let mint_text = mint.to_string();
    let sheet_title = name.clone().unwrap_or_else(|| "Unknown token".to_string());
    let set_flag = move |next: Option<TokenFlag>| {
        session.edit_holdings(|holdings| {
            holdings.set_flag(&owner, &token, next);
        });
    };

    rsx! {
        li { class: "holding-row",
            div { class: "ab-holding-token",
                if let Some(name) = name {
                    TokenAvatar { mint, name: name.clone(), class: "ab-token-icon" }
                    span { class: "ab-token-name", title: "{mint_text}", "{name}" }
                } else {
                    span { class: "ab-token-icon ab-token-mark", title: "Unknown token", "?" }
                    span { class: "sr-only", "Unknown token" }
                    span { class: "ab-caption", "Mint" }
                    AddressRow {
                        pubkey: mint,
                        no_copy: true,
                        no_label: true,
                        no_icon: true,
                        no_card: true,
                        class: "ab-holding-mint",
                    }
                }
                match flag {
                    Some(TokenFlag::Red) => rsx! {
                        span { class: "flag red ab-flag-mark",
                            role: "img",
                            title: "Flagged red: {RED_FLAG_MEANS}",
                            aria_label: "Flagged red",
                            Icon { glyph: Glyph::Flag }
                        }
                    },
                    Some(TokenFlag::White) => rsx! {
                        span { class: "flag white ab-flag-mark",
                            role: "img",
                            title: "Flagged white: {WHITE_FLAG_MEANS}",
                            aria_label: "Flagged white",
                            Icon { glyph: Glyph::Flag }
                        }
                    },
                    None => rsx! {},
                }
            }
            span { class: "ab-holding-amount",
                "{amount}"
                if let Some(symbol) = symbol {
                    span { class: "ab-holding-symbol", " {symbol}" }
                }
            }
            div { class: "ab-holding-account",
                span { class: "ab-caption", "Account" }
                CopyAddress { pubkey: token, class: "ab-copy-addr" }
            }
            button { class: "ghost icon-btn ab-holding-more",
                title: "Token account actions",
                aria_label: "Token account actions",
                aria_haspopup: "dialog",
                onclick: move |_| sheet_open.set(true),
                Icon { glyph: Glyph::More }
            }
        }
        ActionSheet { open: sheet_open, title: sheet_title.clone(), label: "Flags of {sheet_title}",
            if flag == Some(TokenFlag::White) {
                button { class: "more-item ab-flag-item",
                    onclick: move |_| set_flag(None),
                    span { class: "flag white", Icon { glyph: Glyph::Flag } }
                    span { class: "ab-flag-text",
                        span { "Clear white flag" }
                        span { class: "ab-flag-means", "{WHITE_FLAG_MEANS}" }
                    }
                }
            } else {
                button { class: "more-item ab-flag-item",
                    onclick: move |_| set_flag(Some(TokenFlag::White)),
                    span { class: "flag white", Icon { glyph: Glyph::Flag } }
                    span { class: "ab-flag-text",
                        span { "Flag white" }
                        span { class: "ab-flag-means", "{WHITE_FLAG_MEANS}" }
                    }
                }
            }
            if flag == Some(TokenFlag::Red) {
                button { class: "more-item ab-flag-item",
                    onclick: move |_| set_flag(None),
                    span { class: "flag red", Icon { glyph: Glyph::Flag } }
                    span { class: "ab-flag-text",
                        span { "Clear red flag" }
                        span { class: "ab-flag-means", "{RED_FLAG_MEANS}" }
                    }
                }
            } else {
                button { class: "more-item ab-flag-item",
                    onclick: move |_| set_flag(Some(TokenFlag::Red)),
                    span { class: "flag red", Icon { glyph: Glyph::Flag } }
                    span { class: "ab-flag-text",
                        span { "Flag red" }
                        span { class: "ab-flag-means", "{RED_FLAG_MEANS}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_the_first_two_letters_in_capitals() {
        assert_eq!(token_initials("JitoSOL"), "JI");
        assert_eq!(token_initials("usdc"), "US");
        assert_eq!(token_initials("$WIF"), "WI");
        assert_eq!(token_initials("X"), "X");
        assert_eq!(token_initials("★★★"), "★★");
    }

    #[test]
    fn a_mint_always_takes_the_same_tint() {
        let mint = Pubkey::new_unique();
        assert_eq!(token_tint(&mint), token_tint(&mint));
        assert!(token_tint(&mint) < TOKEN_TINTS);
    }
}
