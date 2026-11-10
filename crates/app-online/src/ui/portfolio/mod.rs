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
use std::collections::BTreeMap;
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
    let monograms = monograms_of(&portfolio);
    let colors = group_colors(&portfolio);
    let names: Names = owners
        .iter()
        .map(|owner| (owner.address, owner.label.clone()))
        .collect();

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
                    Summary { portfolio: portfolio.clone(), priced, ago, colors: colors.clone() }
                    Holdings {
                        groups: portfolio.groups.clone(),
                        total: portfolio.total_usd,
                        priced,
                        colors,
                        monograms: monograms.clone(),
                        names: names.clone(),
                    }
                    Section {
                        title: "Not priced",
                        hint: "The Tokens API has no market for these, so they are not in the total.",
                        lines: portfolio.unpriced.clone(),
                        priced,
                        says_no_market: true,
                        monograms: monograms.clone(),
                        names: names.clone(),
                    }
                    Section {
                        title: "Collectibles",
                        hint: "One-of-a-kind tokens. They are not valued.",
                        lines: portfolio.collectibles.clone(),
                        priced,
                        monograms: monograms.clone(),
                        names: names.clone(),
                    }
                    Section {
                        title: "Left out",
                        hint: "Flagged red in the address book, or a token the Tokens API thinks passes itself off as another. Not in the total.",
                        lines: portfolio.left_out.clone(),
                        priced,
                        monograms: monograms.clone(),
                        names,
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

/// The total, its change in 24 hours, and what it is made of.
#[component]
fn Summary(portfolio: Portfolio, priced: bool, ago: Option<String>, colors: Colors) -> Element {
    let allocation = allocation_parts(&portfolio, &colors);
    let excludes = excludes_note(portfolio.unpriced.len(), portfolio.left_out.len());
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
                            "{signed_pct(pct)} ({signed_usd(change)}) in 24h"
                        }
                    }
                }
                if ago.is_some() || excludes.is_some() {
                    div { class: "pf-source",
                        if let Some(ago) = ago {
                            p { class: "hint", "Prices from the Tokens API · {ago}" }
                        }
                        if let Some(excludes) = excludes {
                            span { class: "pf-excludes", "{excludes}" }
                        }
                    }
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
                                span { class: "pf-swatch {color}", aria_hidden: "true" }
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

/// Colours for the allocation bar, and the dot before each group's name.
/// A colour follows its group, not its rank: SOL takes the first, the rest
/// take the next ones in the order of their keys. Past six groups, the
/// smallest fold into a grey "Other".
const ALLOCATION_SLOTS: usize = 6;

/// The colour class of each group worth something, by its key.
type Colors = BTreeMap<String, String>;

fn group_colors(portfolio: &Portfolio) -> Colors {
    let groups: Vec<&Group> = portfolio.groups.iter().filter(|g| g.usd > 0.0).collect();
    let named = if groups.len() > ALLOCATION_SLOTS {
        ALLOCATION_SLOTS - 1
    } else {
        groups.len()
    };
    let mut keys: Vec<&str> = groups[..named].iter().map(|g| g.key.as_str()).collect();
    keys.sort_by_key(|key| (*key != "sol", *key));
    groups
        .iter()
        .enumerate()
        .map(|(at, group)| {
            let class = match keys.iter().position(|key| *key == group.key) {
                Some(slot) if at < named => format!("pf-c{slot}"),
                _ => "pf-cother".to_string(),
            };
            (group.key.clone(), class)
        })
        .collect()
}

fn allocation_parts(portfolio: &Portfolio, colors: &Colors) -> Vec<(String, f64, String)> {
    if portfolio.total_usd <= 0.0 {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut other = 0.0;
    for group in portfolio.groups.iter().filter(|g| g.usd > 0.0) {
        match colors.get(&group.key) {
            Some(class) if class != "pf-cother" => parts.push((
                group.title.clone(),
                group.usd / portfolio.total_usd,
                class.clone(),
            )),
            _ => other += group.usd,
        }
    }
    if other > 0.0 {
        parts.push((
            "Other".into(),
            other / portfolio.total_usd,
            "pf-cother".into(),
        ));
    }
    parts
}

/// What the columns of a row are, once, at the top of the table of priced
/// groups. Hidden where the columns narrow, on a phone.
#[component]
fn ColumnHeads() -> Element {
    rsx! {
        div { class: "pf-cols",
            div { class: "pf-cols-main",
                span {}
                span {}
                span { class: "pf-col", "In" }
                span { class: "pf-col-num", "Amount" }
                span { class: "pf-col-num", "Price" }
                span { class: "pf-col-num", "24h" }
                span { class: "pf-col-num", "Value" }
            }
            span { class: "pf-send-slot" }
        }
    }
}

/// The priced groups, as one table: the column heads once, then each group
/// as a section of it.
#[component]
fn Holdings(
    groups: Vec<Group>,
    total: f64,
    priced: bool,
    colors: Colors,
    monograms: Monograms,
    names: Names,
) -> Element {
    if groups.is_empty() {
        return rsx! {};
    }
    // A group of one holding needs no heading of its own, unless others
    // share the table with it.
    let headed = groups.len() > 1;
    rsx! {
        div { class: "card pf-table",
            if priced {
                ColumnHeads {}
            }
            for group in groups {
                GroupSection {
                    key: "{group.key}",
                    color: colors.get(&group.key).cloned().filter(|_| priced),
                    headed: headed || group.lines.len() > 1,
                    group,
                    total,
                    priced,
                    monograms: monograms.clone(),
                    names: names.clone(),
                }
            }
        }
    }
}

/// Related assets under one heading: SOL with staked SOL and liquid staking
/// tokens, or the variants of one asset. The dot before the name has the
/// group's colour in the allocation bar.
#[component]
fn GroupSection(
    group: Group,
    total: f64,
    priced: bool,
    headed: bool,
    color: Option<String>,
    monograms: Monograms,
    names: Names,
) -> Element {
    let share = if total > 0.0 { group.usd / total } else { 0.0 };
    rsx! {
        div { class: "pf-section",
            if headed {
                div { class: "pf-group-head",
                    if let Some(color) = color {
                        span { class: "pf-swatch {color}", aria_hidden: "true" }
                    }
                    span { class: "pf-group-title", "{group.title}" }
                    if priced {
                        span { class: "muted", "{percent(share)}" }
                        span { class: "pf-value", "{format_usd(group.usd)}" }
                    }
                }
            }
            div { class: "pf-lines",
                for line in group.lines.iter().cloned() {
                    LineRow {
                        key: "{line.symbol}-{line.raw}",
                        monogram: monogram_for(&monograms, &line),
                        line,
                        priced,
                        names: names.clone(),
                    }
                }
            }
        }
    }
}

/// Lines that are not in the total, folded away. Where prices are shown,
/// they keep the columns of the priced rows, with no value in them.
#[component]
fn Section(
    title: &'static str,
    hint: &'static str,
    lines: Vec<Line>,
    priced: bool,
    /// The hint says the Tokens API has no market for any of these, so no
    /// row says it again, but in its tooltip.
    #[props(default)]
    says_no_market: bool,
    monograms: Monograms,
    names: Names,
) -> Element {
    let mut open = use_signal(|| false);
    if lines.is_empty() {
        return rsx! {};
    }
    let count = lines.len();
    let tip_only: Vec<String> = if says_no_market {
        vec![NO_MARKET.to_string()]
    } else {
        Vec::new()
    };
    // What every row would say goes once on the hint line.
    let common = common_warnings(&lines, &tip_only);
    let skip: Vec<String> = common.iter().chain(&tip_only).cloned().collect();
    let now = unix_now();
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
                div { class: "pf-hint-row",
                    p { class: "hint", "{hint}" }
                    for code in common {
                        span {
                            key: "{code}",
                            class: "chip warn",
                            title: warning_tip(&code, "these tokens", 0, now),
                            "{warning_text(&code)}"
                        }
                    }
                }
                div { class: "pf-lines",
                    for line in lines {
                        LineRow {
                            key: "{line.symbol}-{line.raw}",
                            monogram: monogram_for(&monograms, &line),
                            line,
                            priced,
                            valued: false,
                            names: names.clone(),
                            skip: skip.clone(),
                            tip_only: tip_only.clone(),
                        }
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
fn LineRow(
    line: Line,
    monogram: String,
    priced: bool,
    #[props(default = true)] valued: bool,
    names: Names,
    /// Warning codes not shown on the row: its group says them.
    #[props(default)]
    skip: Vec<String>,
    /// Warning codes shown only in the row's tooltip.
    #[props(default)]
    tip_only: Vec<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let amount = format_amount(line.raw, line.decimals, AMOUNT_DIGITS);
    let session = use_context::<Session>();
    let mint = match line.asset {
        wallet_core::portfolio::Asset::Token(mint) => Some(mint),
        _ => None,
    };
    // A token the Tokens API gave no symbol or name for goes by its short
    // mint, with its copy button after it, and "Unknown token" under it.
    let unknown = is_unknown(&line);
    // A token with a symbol but no name shows its mint where the name would be.
    let mint_below = mint.filter(|_| line.name.is_none() && !unknown);
    let (monogram, tint) = if unknown {
        ("?".to_string(), "pf-unknown".to_string())
    } else {
        (monogram, tint_class(&line.asset.price_mint()))
    };
    // The tooltip: all of the mint for an unknown token.
    let mut title = match (unknown, mint) {
        (true, Some(mint)) => mint.to_string(),
        _ => line.symbol.clone(),
    };
    for code in warning_codes(&line).filter(|code| tip_only.contains(code)) {
        title.push_str(&format!("\nTokens API: {}", warning_text(code)));
    }
    let holders: Vec<String> = line
        .shares
        .iter()
        .map(|share| {
            names
                .get(&share.owner)
                .cloned()
                .unwrap_or_else(|| wallet_core::short_pubkey(&share.owner))
        })
        .collect();
    let holders_short = holders_text(&holders);
    let holders_all = holders.join(", ");
    let title_class = if unknown {
        "pf-symbol pf-mint"
    } else {
        "pf-symbol"
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
                .filter(|code| !skip.contains(code) && !tip_only.contains(code))
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
        div { class: if open() { "pf-line open" } else { "pf-line" },
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
                                span { class: "{title_class}", "{line.symbol}" }
                            }
                            if let (true, Some(mint)) = (unknown, mint) {
                                CopyButton { text: mint.to_string(), label: "Copy mint address" }
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
                        } else if unknown {
                            span { class: "muted pf-name", "Unknown token" }
                        } else if let Some(name) = line.name.clone() {
                            span { class: "muted pf-name", "{name}" }
                        }
                    }
                    // Who holds it, and the mark that the row opens to list them.
                    span { class: "muted pf-holders", title: "{holders_all}",
                        span { class: "pf-holders-text", "{holders_short}" }
                        span { class: "pf-open-mark", aria_hidden: "true", Icon { glyph: Glyph::ChevronRight } }
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
                    button { class: "ghost icon-btn sm pf-send",
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
                // One line per wallet, on the table's columns: the wallet
                // where the asset and its holders are, its amount under
                // Amount, its value under Value, its send button under the
                // asset's. The mint comes last, as a caption.
                div { class: "pf-shares",
                    for share in line.shares.iter() {
                        div { key: "{share.owner}", class: "pf-share",
                            div { class: "pf-share-main",
                                span { class: "pf-share-gap" }
                                AddressRow { pubkey: share.owner, no_copy: true, class: "pf-share-who",
                                    card_accounts: share.accounts.iter().map(|account| (account_kind, *account)).collect::<Vec<_>>(),
                                    CopyButton { text: share.owner.to_string(), label: "Copy address" }
                                }
                                span { class: "pf-amount", "{format_amount(u128::from(share.raw), line.decimals, AMOUNT_DIGITS)}" }
                                if priced {
                                    span { class: "pf-share-gap pf-price" }
                                    span { class: "pf-share-gap pf-change" }
                                    span { class: "pf-value",
                                        if let (Some(usd), true) = (share.usd, several) { "{format_value(usd)}" }
                                    }
                                }
                            }
                            if let (Some(asset), Some(id), true) = (
                                sendable,
                                senders.iter().find(|(owner, _)| *owner == share.owner).map(|(_, id)| *id),
                                several,
                            ) {
                                button { class: "ghost icon-btn sm pf-send",
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
                            } else {
                                span { class: "pf-send-slot" }
                            }
                        }
                    }
                    if let Some(mint) = mint {
                        div { class: "pf-share pf-share-mint",
                            div { class: "pf-share-main",
                                span { class: "pf-share-gap" }
                                span { class: "pf-share-who",
                                    span { class: "pf-mint-label", "Mint" }
                                    Address { pubkey: mint }
                                }
                            }
                            span { class: "pf-send-slot" }
                        }
                    }
                }
            }
        }
    }
}

/// The Tokens API's code for a token it has no market for.
const NO_MARKET: &str = "no_market_data";

/// The warning codes the Tokens API gave a line.
fn warning_codes(line: &Line) -> impl Iterator<Item = &String> {
    line.check.iter().flat_map(|check| check.warnings.iter())
}

/// The warnings every one of several lines carries, but those in `except`,
/// in the order of the first line's.
fn common_warnings(lines: &[Line], except: &[String]) -> Vec<String> {
    let Some((first, rest)) = lines.split_first() else {
        return Vec::new();
    };
    if rest.is_empty() {
        return Vec::new();
    }
    warning_codes(first)
        .filter(|code| !except.contains(code))
        .filter(|code| {
            rest.iter()
                .all(|line| warning_codes(line).any(|c| c == *code))
        })
        .cloned()
        .collect()
}

/// Who holds an asset, short: the first two names, then how many more.
fn holders_text(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a}, {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} +{}", rest.len()),
    }
}

/// The names of the wallets and watched addresses, by address.
type Names = BTreeMap<Pubkey, String>;

/// A token the Tokens API gave no symbol or name for: its symbol is its
/// short mint, which is no name.
fn is_unknown(line: &Line) -> bool {
    match line.asset {
        wallet_core::portfolio::Asset::Token(mint) => {
            line.name.is_none() && line.symbol == wallet_core::short_pubkey(&mint)
        }
        _ => false,
    }
}

/// The note beside where the prices come from, when the total leaves
/// holdings out: the tokens with no price, and those left out.
fn excludes_note(unpriced: usize, left_out: usize) -> Option<String> {
    let tokens = |n: usize| if n == 1 { "token" } else { "tokens" };
    match (unpriced, left_out) {
        (0, 0) => None,
        (n, 0) => Some(format!("Excludes {n} {} with no price", tokens(n))),
        (0, n) => Some(format!("Excludes {n} {} left out", tokens(n))),
        (n, m) => Some(format!(
            "Excludes {n} {} with no price and {m} left out",
            tokens(n)
        )),
    }
}

/// The avatar letters of each line listed, by asset.
type Monograms = BTreeMap<wallet_core::portfolio::Asset, String>;

/// The avatar letters of a line: its own, or the first two of its symbol.
fn monogram_for(monograms: &Monograms, line: &Line) -> String {
    monograms
        .get(&line.asset)
        .cloned()
        .unwrap_or_else(|| first_two(&letters(&line.symbol)))
}

/// The letters and digits of a symbol, upper case.
fn letters(symbol: &str) -> Vec<char> {
    symbol
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn first_two(letters: &[char]) -> String {
    letters.iter().take(2).collect()
}

/// The avatar letters of every line the screen lists, folded or not, so
/// that no two of them read the same where their symbols allow it.
fn monograms_of(portfolio: &Portfolio) -> Monograms {
    let lines: Vec<&Line> = portfolio
        .groups
        .iter()
        .flat_map(|group| group.lines.iter())
        .chain(&portfolio.unpriced)
        .chain(&portfolio.collectibles)
        .chain(&portfolio.left_out)
        .filter(|line| !is_unknown(line))
        .collect();
    let symbols: Vec<String> = lines.iter().map(|line| line.symbol.clone()).collect();
    lines
        .iter()
        .zip(monograms(&symbols))
        .map(|(line, monogram)| (line.asset, monogram))
        .collect()
}

/// Two letters for each symbol. The first two where no other symbol starts
/// with the same two; otherwise the first letter and the first later one
/// that sets the symbol apart from those it shares its first two with: one
/// none of them has at that place (USDC and USDT: "UC" and "UT"). A symbol
/// nothing sets apart keeps its first two.
fn monograms(symbols: &[String]) -> Vec<String> {
    let letters: Vec<Vec<char>> = symbols.iter().map(|symbol| letters(symbol)).collect();
    let firsts: Vec<String> = letters.iter().map(|l| first_two(l)).collect();
    let shared = |at: usize| firsts.iter().filter(|f| **f == firsts[at]).count() > 1;
    // Pairs already given: the unique ones, then each one picked.
    let mut taken: Vec<String> = (0..firsts.len())
        .filter(|at| !shared(*at))
        .map(|at| firsts[at].clone())
        .collect();
    let mut out = firsts.clone();
    for at in (0..letters.len()).filter(|at| shared(*at)) {
        let own = &letters[at];
        let Some(first) = own.first() else { continue };
        let rivals: Vec<&Vec<char>> = (0..letters.len())
            .filter(|other| *other != at && firsts[*other] == firsts[at])
            .map(|other| &letters[other])
            .collect();
        let pick = own.iter().enumerate().skip(1).find_map(|(i, c)| {
            let pair: String = [*first, *c].iter().collect();
            let apart = rivals.iter().all(|rival| rival.get(i) != Some(c));
            (apart && !taken.contains(&pair)).then_some(pair)
        });
        if let Some(pair) = pick {
            taken.push(pair.clone());
            out[at] = pair;
        }
    }
    out
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
    fn avatar_letters_differ_where_symbols_allow() {
        let of = |symbols: &[&str]| {
            monograms(&symbols.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(of(&["USDC", "USDT", "SOL"]), ["UC", "UT", "SO"]);
        assert_eq!(of(&["JitoSOL", "JUP"]), ["JI", "JU"]);
        assert_eq!(of(&["JUP", "JUPSOL"]), ["JU", "JS"]);
        // Two of one symbol cannot be told apart.
        assert_eq!(of(&["USDC", "USDC"]), ["US", "US"]);
        assert_eq!(of(&["$WIF", "w"]), ["WI", "W"]);
    }

    #[test]
    fn the_note_counts_what_the_total_leaves_out() {
        assert_eq!(excludes_note(0, 0), None);
        assert_eq!(
            excludes_note(4, 0).as_deref(),
            Some("Excludes 4 tokens with no price")
        );
        assert_eq!(
            excludes_note(1, 0).as_deref(),
            Some("Excludes 1 token with no price")
        );
        assert_eq!(
            excludes_note(0, 1).as_deref(),
            Some("Excludes 1 token left out")
        );
        assert_eq!(
            excludes_note(2, 3).as_deref(),
            Some("Excludes 2 tokens with no price and 3 left out")
        );
    }

    #[test]
    fn holders_read_as_two_names_then_a_count() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(holders_text(&names(&[])), "");
        assert_eq!(holders_text(&names(&["Main"])), "Main");
        assert_eq!(
            holders_text(&names(&["Main", "Cold vault"])),
            "Main, Cold vault"
        );
        assert_eq!(
            holders_text(&names(&["Main", "Cold vault", "Ops"])),
            "Main, Cold vault +1"
        );
    }

    #[test]
    fn a_warning_every_row_carries_goes_to_the_group() {
        let line = |codes: &[&str]| Line {
            asset: wallet_core::portfolio::Asset::Sol,
            symbol: "X".into(),
            name: None,
            raw: 1,
            decimals: 0,
            price_usd: None,
            change_24h_pct: None,
            usd: None,
            check: Some(wallet_core::portfolio::TokenCheck {
                warnings: codes.iter().map(|c| c.to_string()).collect(),
                ..Default::default()
            }),
            shares: Vec::new(),
        };
        let lines = [
            line(&["no_market_data", "unverified"]),
            line(&["unverified", "no_market_data"]),
            line(&["unverified", "new_token"]),
        ];
        assert_eq!(common_warnings(&lines, &[]), ["unverified"]);
        let two = [
            line(&["no_market_data", "unverified"]),
            line(&["unverified", "no_market_data"]),
        ];
        assert_eq!(common_warnings(&two, &[]), ["no_market_data", "unverified"]);
        assert_eq!(
            common_warnings(&two, &[NO_MARKET.to_string()]),
            ["unverified"]
        );
        // One row has no others to share with.
        assert!(common_warnings(&lines[..1], &[]).is_empty());
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
