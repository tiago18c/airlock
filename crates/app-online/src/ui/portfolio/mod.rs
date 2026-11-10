//! What the wallets of this device, and the addresses watched with them, hold
//! and are worth. Amounts are this app's own reads of the chain; prices come
//! from the Tokens API through the `prices` sync job.

use crate::actions::stake_screen::{load_key, refresh as refresh_stakes};
use app_ui_kit::actions::unix_now;
use app_ui_kit::state::forms::{SendAsset, SendOpen};
use app_ui_kit::ui::{CopyButton, PaneMenu};
use app_ui_kit::{Address, AddressRow, Glyph, Icon, Session, Tab};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::portfolio::{
    build, format_amount, format_usd, format_value, prices_apply_to, Group, HeldStake, Line, Owner,
    Portfolio,
};
use wallet_core::{AddressKind, WalletId};

mod watch;

use watch::WatchModal;

/// Digits after the point in the amount column.
const AMOUNT_DIGITS: usize = 4;

#[component]
pub fn PortfolioPanel() -> Element {
    let session = use_context::<Session>();
    let stake_form = session.forms.stake;
    let mut only = use_signal(|| None::<Pubkey>);
    let mut watch_open = use_signal(|| false);
    let mut refreshing = use_signal(|| false);
    let mut refresh_error = use_signal(|| None::<String>);

    // Staked SOL comes from the Stake screen's read of the wallets' stake
    // accounts. Read it when it is missing or out of date, as Stake does.
    use_effect(move || {
        if session.is_unlocked() && (stake_form.loaded_for)() != Some(load_key(session)) {
            refresh_stakes(session);
        }
    });

    let owners = portfolio_owners(session);
    let book = (session.book)();
    let holdings = (session.holdings_view)();
    let market = &holdings.market;
    let stakes: Vec<HeldStake> = (stake_form.snapshot)()
        .map(|snapshot| {
            owners
                .iter()
                .filter(|owner| !owner.watched)
                .flat_map(|owner| {
                    snapshot
                        .of(&owner.address)
                        .into_iter()
                        .map(|listed| HeldStake {
                            owner: owner.address,
                            account: listed.account.pubkey,
                            lamports: listed.account.lamports,
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    let selected = only().filter(|pk| owners.iter().any(|owner| owner.address == *pk));
    let portfolio = build(&owners, &book, &holdings, &stakes, market, selected);

    let cluster = session.cluster();
    let mainnet = prices_apply_to(&cluster);
    let has_key = (session.settings)().tokens_api_key.is_some();
    let priced = mainnet && has_key && !portfolio.no_prices;
    let ago = market.quoted_at.map(|at| ago(unix_now() - at));
    let locked = !session.is_unlocked();
    let can_send = owners.iter().any(|owner| !owner.watched);

    let refresh = move |_| {
        refresh_error.set(None);
        refreshing.set(true);
        spawn(async move {
            let _ = session.refresh_from_chain().await;
            if let Err(err) = session.refresh_prices().await {
                refresh_error.set(Some(err));
            }
            refreshing.set(false);
        });
    };

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Portfolio" }
                    }
                    div { class: "btn-row pf-controls",
                        if can_send {
                            button { class: "primary",
                                onclick: move |_| session.open_send(SendOpen {
                                    from: selected.and_then(|owner| wallet_at(session, owner)),
                                    ..SendOpen::default()
                                }),
                                "Send"
                            }
                        }
                        select {
                            aria_label: "Whose holdings",
                            onchange: move |e| only.set(e.value().parse().ok()),
                            option { value: "", selected: selected.is_none(), "All wallets" }
                            for owner in owners.iter() {
                                option {
                                    key: "{owner.address}",
                                    value: "{owner.address}",
                                    selected: selected == Some(owner.address),
                                    if owner.watched { "{owner.label} (watched)" } else { "{owner.label}" }
                                }
                            }
                        }
                        button {
                            disabled: refreshing(),
                            onclick: refresh,
                            if refreshing() { "Refreshing…" } else { "Refresh" }
                        }
                        PaneMenu { label: "More portfolio actions",
                            if can_send {
                                button { class: "pane-menu-item",
                                    role: "menuitem",
                                    title: "Burn and close token accounts of the wallet shown, or the one in use, to get their rent back",
                                    onclick: move |_| {
                                        let wallet = selected
                                            .and_then(|owner| wallet_at(session, owner))
                                            .or_else(|| session.selected_wallet());
                                        if let Some(wallet) = wallet {
                                            session.open_cleanup(wallet);
                                        }
                                    },
                                    Icon { glyph: Glyph::Trash }
                                    "Manage tokens"
                                }
                            }
                            button { class: "pane-menu-item",
                                role: "menuitem",
                                title: "Pick the address book accounts counted here beside this device's wallets",
                                onclick: move |_| watch_open.set(true),
                                Icon { glyph: Glyph::Book }
                                "Watched addresses"
                            }
                        }
                    }
                }

                if !mainnet {
                    p { class: "note",
                        "Prices are for mainnet. This app is on {cluster.name()}, so amounts are shown without values."
                    }
                } else if !has_key {
                    div { class: "note pf-note",
                        span { "Add a Tokens API key in Settings to see values." }
                        button { class: "ghost sm",
                            onclick: move |_| session.goto(Tab::Settings),
                            "Open Settings"
                        }
                    }
                }
                if locked {
                    p { class: "hint",
                        "The vault is locked: only watched addresses are counted. Unlock it to count this device's wallets."
                    }
                }
                if let Some(err) = refresh_error().or_else(|| market.error.clone()) {
                    p { class: "error", "{err}" }
                }

                if owners.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::Pie }
                        p { "No wallets or watched addresses yet. Add a wallet, or watch an address from the address book." }
                    }
                } else {
                    Summary { portfolio: portfolio.clone(), priced, ago }
                    for group in portfolio.groups.iter().cloned() {
                        GroupBlock { key: "{group.key}", group, total: portfolio.total_usd, priced }
                    }
                    Section {
                        title: "Not priced",
                        hint: "The Tokens API has no market for these, so they are not in the total.",
                        lines: portfolio.unpriced.clone(),
                        priced,
                    }
                    Section {
                        title: "Collectibles",
                        hint: "One-of-a-kind tokens. They are not valued.",
                        lines: portfolio.collectibles.clone(),
                        priced,
                    }
                    Section {
                        title: "Left out",
                        hint: "Flagged red in the address book, or a token the Tokens API thinks passes itself off as another. Not in the total.",
                        lines: portfolio.left_out.clone(),
                        priced,
                    }
                }
            }
        }
        WatchModal { open: watch_open }
    }
}

/// The wallets of this device with an address, then every watched account
/// in the address book that is not one of them.
fn portfolio_owners(session: Session) -> Vec<Owner> {
    let mut owners: Vec<Owner> = session
        .wallet_summaries()
        .into_iter()
        .filter_map(|wallet| {
            Some(Owner {
                address: wallet.address?,
                label: wallet.label,
                watched: false,
            })
        })
        .collect();
    let book = (session.book)();
    for entry in book.iter() {
        if entry.watched
            && entry.kind == AddressKind::Account
            && !owners.iter().any(|owner| owner.address == entry.pubkey)
        {
            owners.push(Owner {
                address: entry.pubkey,
                label: if entry.label.is_empty() {
                    wallet_core::short_pubkey(&entry.pubkey)
                } else {
                    entry.label.clone()
                },
                watched: true,
            });
        }
    }
    owners
}

/// The wallet of this device at `address`. `None` for a watched address,
/// which this device cannot send from.
fn wallet_at(session: Session, address: Pubkey) -> Option<WalletId> {
    session
        .wallet_summaries()
        .into_iter()
        .find(|wallet| wallet.address == Some(address))
        .map(|wallet| wallet.id)
}

fn ago(secs: i64) -> String {
    match secs {
        ..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// The total, its change today, and what it is made of.
#[component]
fn Summary(portfolio: Portfolio, priced: bool, ago: Option<String>) -> Element {
    let allocation = allocation_parts(&portfolio);
    let change = portfolio.change_24h_usd;
    let pct = change.map(|change| {
        let before = portfolio.total_usd - change;
        if before.abs() > f64::EPSILON {
            change / before * 100.0
        } else {
            0.0
        }
    });
    rsx! {
        div { class: "card pf-summary",
            if priced {
                div { class: "pf-total-row",
                    span { class: "pf-total", "{format_usd(portfolio.total_usd)}" }
                    if let (Some(change), Some(pct)) = (change, pct) {
                        span {
                            class: change_class(pct),
                            "{signed_pct(pct)} ({signed_usd(change)}) today"
                        }
                    }
                }
                if let Some(ago) = ago {
                    p { class: "hint", "Prices from the Tokens API · {ago}" }
                }
                if !allocation.is_empty() {
                    div { class: "pf-alloc", role: "img", aria_label: "Allocation",
                        for (title, share, color) in allocation.iter().cloned() {
                            span {
                                key: "{title}",
                                class: "pf-alloc-part {color}",
                                style: "flex-grow: {share}",
                                title: "{title} {percent(share)}",
                            }
                        }
                    }
                    div { class: "chip-row pf-legend",
                        for (title, share, color) in allocation.iter().cloned() {
                            span { key: "{title}", class: "pf-legend-item",
                                span { class: "pf-swatch {color}" }
                                "{title} {percent(share)}"
                            }
                        }
                    }
                }
            } else {
                p { class: "muted", "Amounts only. Values show once prices are known." }
            }
        }
    }
}

/// Colours for the allocation bar. A colour follows its group, not its rank:
/// SOL takes the first, the rest take the next ones in the order of their
/// keys. Past five groups, the smallest fold into a grey "Other".
const ALLOCATION_SLOTS: usize = 6;

fn allocation_parts(portfolio: &Portfolio) -> Vec<(String, f64, String)> {
    if portfolio.total_usd <= 0.0 {
        return Vec::new();
    }
    let groups: Vec<&Group> = portfolio.groups.iter().filter(|g| g.usd > 0.0).collect();
    let (shown, rest) = if groups.len() > ALLOCATION_SLOTS {
        groups.split_at(ALLOCATION_SLOTS - 1)
    } else {
        (groups.as_slice(), &[][..])
    };
    let mut keys: Vec<&str> = shown.iter().map(|g| g.key.as_str()).collect();
    keys.sort_by_key(|key| (*key != "sol", *key));
    let mut parts: Vec<(String, f64, String)> = shown
        .iter()
        .map(|group| {
            let slot = keys.iter().position(|key| *key == group.key).unwrap_or(0);
            (
                group.title.clone(),
                group.usd / portfolio.total_usd,
                format!("pf-c{slot}"),
            )
        })
        .collect();
    let other: f64 = rest.iter().map(|g| g.usd).sum();
    if other > 0.0 {
        parts.push((
            "Other".into(),
            other / portfolio.total_usd,
            "pf-cother".into(),
        ));
    }
    parts
}

/// What the numbers of a row are, at the top of a group's rows, on their
/// columns. Hidden where the columns narrow, on a phone.
#[component]
fn ColumnHeads() -> Element {
    rsx! {
        div { class: "pf-cols",
            div { class: "pf-cols-main",
                span {}
                span {}
                span { class: "pf-col-num", "Amount" }
                span { class: "pf-col-num", "Price" }
                span { class: "pf-col-num", "24h" }
                span { class: "pf-col-num", "Value" }
            }
            span { class: "pf-send-slot" }
        }
    }
}

/// Related assets under one heading: SOL with staked SOL and liquid staking
/// tokens, or the variants of one asset.
#[component]
fn GroupBlock(group: Group, total: f64, priced: bool) -> Element {
    let share = if total > 0.0 {
        group.usd / total * 100.0
    } else {
        0.0
    };
    let several = group.lines.len() > 1;
    rsx! {
        div { class: "card pf-group",
            if several {
                div { class: "pf-group-head",
                    span { class: "pf-group-title", "{group.title}" }
                    if priced {
                        span { class: "muted", "{percent(share / 100.0)}" }
                        span { class: "pf-value", "{format_usd(group.usd)}" }
                    }
                }
            }
            div { class: "pf-lines",
                if priced {
                    ColumnHeads {}
                }
                for line in group.lines.iter().cloned() {
                    LineRow { key: "{line.symbol}-{line.raw}", line, priced }
                }
            }
        }
    }
}

/// Lines that are not in the total, folded away. Where prices are shown,
/// they keep the columns of the priced rows, with no value in them.
#[component]
fn Section(title: &'static str, hint: &'static str, lines: Vec<Line>, priced: bool) -> Element {
    let mut open = use_signal(|| false);
    if lines.is_empty() {
        return rsx! {};
    }
    let count = lines.len();
    rsx! {
        div { class: "card pf-group",
            button { class: "pf-fold",
                r#type: "button",
                aria_expanded: "{open()}",
                onclick: move |_| open.set(!open()),
                span { class: "pf-group-title", "{title} ({count})" }
                span { class: "pf-chevron", Icon { glyph: Glyph::ChevronRight } }
            }
            if open() {
                p { class: "hint", "{hint}" }
                div { class: "pf-lines",
                    for line in lines {
                        LineRow { key: "{line.symbol}-{line.raw}", line, priced, valued: false }
                    }
                }
            }
        }
    }
}

/// One asset: what it is, how much, its price and value. A click shows who
/// holds it. `priced` lays the row out on the price columns; `valued` fills
/// them, and is off for lines that are not in the total.
#[component]
fn LineRow(line: Line, priced: bool, #[props(default = true)] valued: bool) -> Element {
    let mut open = use_signal(|| false);
    let amount = format_amount(line.raw, line.decimals, AMOUNT_DIGITS);
    let session = use_context::<Session>();
    let mint = match line.asset {
        wallet_core::portfolio::Asset::Token(mint) => Some(mint),
        _ => None,
    };
    // A token the Tokens API gave no symbol or name for: its mint is no
    // name, so it is an unknown token, with the mint under it.
    let unknown = mint
        .is_some_and(|mint| line.name.is_none() && line.symbol == wallet_core::short_pubkey(&mint));
    // A token with no name shows its mint where the name would be.
    let mint_below = mint.filter(|_| line.name.is_none());
    let (monogram, tint) = if unknown {
        ("?".to_string(), "pf-unknown".to_string())
    } else {
        (
            line.symbol
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_uppercase(),
            tint_class(&line.asset.price_mint()),
        )
    };
    let title = if unknown {
        "Unknown token".to_string()
    } else {
        line.symbol.clone()
    };
    let mark = verified_mark(&line);
    // Staked SOL is moved on Stake, not sent from here.
    let sendable = match line.asset {
        wallet_core::portfolio::Asset::Sol => Some(SendAsset::Sol),
        wallet_core::portfolio::Asset::Token(mint) => Some(SendAsset::Token(mint)),
        wallet_core::portfolio::Asset::StakedSol => None,
    };
    let senders: Vec<(Pubkey, WalletId)> = line
        .shares
        .iter()
        .filter_map(|share| Some((share.owner, wallet_at(session, share.owner)?)))
        .collect();
    let only_sender = (senders.len() == 1).then(|| senders[0].1);
    let symbol = line.symbol.clone();
    let now = unix_now();
    let warnings: Vec<(String, Option<String>)> = line
        .check
        .as_ref()
        .map(|check| {
            check
                .warnings
                .iter()
                .map(|code| {
                    (
                        warning_text(code),
                        warning_tip(code, &symbol, check.checked_at, now),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let several = line.shares.len() > 1;
    let account_kind = match line.asset {
        wallet_core::portfolio::Asset::StakedSol => "Stake account",
        _ => "Token account",
    };
    rsx! {
        div { class: "pf-line",
            div { class: "pf-line-head",
                // The whole row opens the holders; the symbol is the control
                // a keyboard reaches. Not one button, so the mint's copy
                // button is not inside another.
                div { class: "pf-line-main",
                    onclick: move |_| open.set(!open()),
                    span { class: "pf-mono {tint}", aria_hidden: "true", "{monogram}" }
                    span { class: "pf-asset",
                        span { class: "pf-title",
                            button { class: "pf-line-toggle",
                                r#type: "button",
                                title: "{title}",
                                aria_expanded: "{open()}",
                                span { class: "pf-symbol", "{title}" }
                            }
                            if let Some(tip) = mark {
                                span { class: "pf-verified",
                                    role: "img",
                                    title: tip,
                                    aria_label: tip,
                                    Icon { glyph: Glyph::Check }
                                }
                            }
                            for (warning, tip) in warnings {
                                span { key: "{warning}", class: "chip warn", title: tip, "{warning}" }
                            }
                        }
                        if let Some(mint) = mint_below {
                            span { class: "muted pf-sub",
                                code { class: "pf-mint", title: "{mint}", "{wallet_core::short_pubkey(&mint)}" }
                                CopyButton { text: mint.to_string(), label: "Copy mint address" }
                            }
                        } else if let Some(name) = line.name.clone() {
                            span { class: "muted pf-name", "{name}" }
                        }
                    }
                    span { class: "pf-amount", "{amount}" }
                    if priced && valued {
                        span { class: "pf-price muted",
                            if let Some(price) = line.price_usd { "{format_usd(price)}" } else { "–" }
                        }
                        span { class: "pf-change",
                            if let Some(pct) = line.change_24h_pct {
                                span { class: change_class(pct), "{signed_pct(pct)}" }
                            }
                        }
                        span { class: "pf-value",
                            if let Some(usd) = line.usd { "{format_value(usd)}" } else { "–" }
                        }
                    } else if priced {
                        span { class: "pf-price muted", "–" }
                        span { class: "pf-change" }
                        span { class: "pf-value muted", "–" }
                    }
                }
                if let (Some(asset), false) = (sendable, senders.is_empty()) {
                    button { class: "ghost icon-btn sm",
                        r#type: "button",
                        title: "Send {symbol}",
                        aria_label: "Send {symbol}",
                        onclick: move |_| session.open_send(SendOpen {
                            from: only_sender,
                            asset: Some(asset),
                            to: None,
                        }),
                        Icon { glyph: Glyph::Send }
                    }
                } else {
                    span { class: "pf-send-slot" }
                }
            }
            if open() {
                div { class: "pf-shares",
                    if let Some(mint) = mint {
                        div { class: "chip-row",
                            span { class: "muted", "mint" }
                            Address { pubkey: mint }
                        }
                    }
                    for share in line.shares.iter() {
                        AddressRow { key: "{share.owner}", pubkey: share.owner,
                            card_accounts: share.accounts.iter().map(|account| (account_kind, *account)).collect::<Vec<_>>(),
                            span { class: "pf-amount", "{format_amount(u128::from(share.raw), line.decimals, AMOUNT_DIGITS)}" }
                            if priced && several {
                                if let Some(usd) = share.usd {
                                    span { class: "muted", "{format_value(usd)}" }
                                }
                            }
                            if let (Some(asset), Some(id), true) = (
                                sendable,
                                senders.iter().find(|(owner, _)| *owner == share.owner).map(|(_, id)| *id),
                                several,
                            ) {
                                button { class: "ghost icon-btn sm",
                                    r#type: "button",
                                    title: "Send from this wallet",
                                    aria_label: "Send from this wallet",
                                    onclick: move |_| session.open_send(SendOpen {
                                        from: Some(id),
                                        asset: Some(asset),
                                        to: None,
                                    }),
                                    Icon { glyph: Glyph::Send }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A share of the total, "<1%" when it rounds to nothing.
fn percent(share: f64) -> String {
    let pct = (share * 100.0).round();
    if pct < 1.0 {
        "<1%".to_string()
    } else {
        format!("{pct}%")
    }
}

/// A change shown with two decimals reads 0.00%: it has no direction.
fn flat_pct(pct: f64) -> bool {
    (pct * 100.0).round() == 0.0
}

/// The class of a change: green up, red down, muted when it is 0.00%.
fn change_class(pct: f64) -> &'static str {
    if flat_pct(pct) {
        "delta pf-flat"
    } else if pct > 0.0 {
        "delta up"
    } else {
        "delta down"
    }
}

/// A change with its sign, "0.00%" with none when it rounds to nothing.
fn signed_pct(pct: f64) -> String {
    if flat_pct(pct) {
        "0.00%".to_string()
    } else if pct > 0.0 {
        format!("+{pct:.2}%")
    } else {
        format!("−{:.2}%", -pct)
    }
}

/// Dollars with their sign, "$0.00" with none when they round to nothing.
fn signed_usd(usd: f64) -> String {
    if (usd * 100.0).round() == 0.0 {
        format_usd(0.0)
    } else if usd > 0.0 {
        format!("+{}", format_usd(usd))
    } else {
        format!("−{}", format_usd(-usd))
    }
}

/// Avatar tints, `pf-t0` to `pf-t7` in the stylesheet.
const TINTS: u32 = 8;

/// The avatar tint of an asset, picked from its mint, so that two symbols
/// with the same first letters, such as USDC and USDT, mostly differ.
fn tint_class(mint: &Pubkey) -> String {
    // FNV-1a over the mint's bytes: stable across runs and builds.
    let hash = mint.to_bytes().iter().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    });
    format!("pf-t{}", hash % TINTS)
}

/// The tooltip of the check after a line's symbol, if it has one: a token
/// the Tokens API verified, or SOL itself, which needs no verifying.
fn verified_mark(line: &Line) -> Option<&'static str> {
    match line.asset {
        wallet_core::portfolio::Asset::Sol | wallet_core::portfolio::Asset::StakedSol => {
            Some("SOL is the network's own asset, not a token")
        }
        wallet_core::portfolio::Asset::Token(_) => line
            .check
            .as_ref()
            .is_some_and(|check| check.verified)
            .then_some("Verified by the Tokens API"),
    }
}

/// A warning's tooltip, where its words alone leave out what it means.
fn warning_tip(code: &str, symbol: &str, checked_at: i64, now: i64) -> Option<String> {
    match code {
        "stale_data" => {
            let mut tip = format!(
                "The Tokens API says its market data for {symbol} has not been updated lately, so its price and 24h change may be out of date."
            );
            if checked_at > 0 {
                tip.push_str(&format!(
                    " It said so when it checked the token, {}.",
                    ago(now.saturating_sub(checked_at))
                ));
            }
            Some(tip)
        }
        _ => None,
    }
}

/// The Tokens API's warning codes, in words.
fn warning_text(code: &str) -> String {
    match code {
        "possible_impersonation" => "may impersonate another token".into(),
        "low_liquidity" => "low liquidity".into(),
        "no_market_data" => "no market data".into(),
        "symbol_collision" => "shares its symbol".into(),
        "new_token" => "new token".into(),
        // The prices were fetched just now; it is this token's market that
        // has not moved them lately.
        "stale_data" => "outdated price".into(),
        "high_bot_volume" => "mostly bot trading".into(),
        "concentrated_holders" => "few holders".into(),
        "unverified" => "unverified".into(),
        "weak_market_score" => "weak market".into(),
        "advisory_caution" => "advisory".into(),
        "suspicious_characters" => "odd characters in its name".into(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_helpers_read_well() {
        assert_eq!(ago(30), "just now");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.256), "26%");
        assert_eq!(ago(150), "2 min ago");
        assert_eq!(signed_pct(2.345), "+2.35%");
        assert_eq!(signed_pct(-1.0), "−1.00%");
        assert_eq!(signed_usd(-5.0), "−$5.00");
        assert_eq!(
            warning_text("possible_impersonation"),
            "may impersonate another token"
        );
        assert_eq!(warning_text("brand_new_code"), "brand new code");
        assert_eq!(warning_text("stale_data"), "outdated price");
    }

    #[test]
    fn sol_and_verified_tokens_carry_a_check() {
        let line = |asset, verified: Option<bool>| Line {
            asset,
            symbol: "X".into(),
            name: None,
            raw: 1,
            decimals: 0,
            price_usd: None,
            change_24h_pct: None,
            usd: None,
            check: verified.map(|verified| wallet_core::portfolio::TokenCheck {
                verified,
                ..Default::default()
            }),
            shares: Vec::new(),
        };
        use wallet_core::portfolio::Asset;
        let mint: Pubkey = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            .parse()
            .unwrap();
        assert!(verified_mark(&line(Asset::Sol, None))
            .unwrap()
            .contains("network's own asset"));
        assert_eq!(
            verified_mark(&line(Asset::Token(mint), Some(true))),
            Some("Verified by the Tokens API")
        );
        assert_eq!(verified_mark(&line(Asset::Token(mint), Some(false))), None);
        assert_eq!(verified_mark(&line(Asset::Token(mint), None)), None);
    }

    #[test]
    fn a_change_that_rounds_to_zero_has_no_sign_or_colour() {
        assert_eq!(signed_pct(-0.004), "0.00%");
        assert_eq!(signed_pct(0.004), "0.00%");
        assert_eq!(change_class(-0.004), "delta pf-flat");
        assert_eq!(change_class(0.0), "delta pf-flat");
        assert_eq!(signed_pct(0.006), "+0.01%");
        assert_eq!(change_class(0.006), "delta up");
        assert_eq!(change_class(-0.006), "delta down");
        assert_eq!(signed_usd(-0.004), "$0.00");
        assert_eq!(signed_usd(0.5), "+$0.50");
    }

    #[test]
    fn usdc_and_usdt_get_different_tints() {
        let usdc: Pubkey = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            .parse()
            .unwrap();
        let usdt: Pubkey = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
            .parse()
            .unwrap();
        assert_ne!(tint_class(&usdc), tint_class(&usdt));
        assert!(tint_class(&usdc).starts_with("pf-t"));
    }

    #[test]
    fn stale_data_says_what_is_stale_and_when() {
        let tip = warning_tip("stale_data", "USDC", 1_000, 1_000 + 7_200).unwrap();
        assert!(tip.contains("market data for USDC"), "{tip}");
        assert!(tip.ends_with("2 h ago."), "{tip}");
        assert!(!warning_tip("stale_data", "USDC", 0, 5)
            .unwrap()
            .contains("ago"));
        assert_eq!(warning_tip("unverified", "USDC", 1, 2), None);
    }
}
