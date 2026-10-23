//! Sending SOL or a token from one of this device's wallets: from which
//! wallet, what, to whom and how much, then Sign. Wallets and Portfolio open
//! it with what they already know.

use app_ui_kit::actions::wallets::{send, send_destination};
use app_ui_kit::state::forms::{SendAsset, SendOpen};
use app_ui_kit::{format_sol, format_token_amount, AddressPicker, Session, WalletPicker};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use wallet_core::{parse_amount, short_pubkey, TokenFlag, WalletId};
use wallet_flows::token_transfer::{Destination, Resolved};

/// Mounts the Send modal while a screen has it open.
#[component]
pub fn SendHost() -> Element {
    let session = use_context::<Session>();
    match (session.forms.send.open)() {
        Some(open) => rsx! { SendModal { open } },
        None => rsx! {},
    }
}

/// One asset the sender can pick.
#[derive(Clone, PartialEq)]
struct Holding {
    asset: SendAsset,
    symbol: String,
    decimals: u8,
    /// All the sender holds of it.
    total: u64,
    /// The most one of its accounts holds: what one transfer can move.
    largest: u64,
    flagged: bool,
}

/// Where a token sent to the recipient lands, as last read.
#[derive(Clone, PartialEq)]
enum Landing {
    Unknown,
    Reading,
    Read(Resolved),
    Failed(String),
}

#[component]
fn SendModal(open: SendOpen) -> Element {
    let session = use_context::<Session>();
    let senders: Vec<_> = session
        .wallet_summaries()
        .into_iter()
        .filter(|wallet| wallet.address.is_some())
        .collect();
    let initial = open.clone();
    let initial_senders = senders.clone();
    let mut from = use_signal(move || {
        initial.from.or_else(|| {
            // A token picked first goes out of the first wallet that holds it.
            let holder =
                initial_senders
                    .iter()
                    .find(|wallet| match (initial.asset, wallet.address) {
                        (Some(asset @ SendAsset::Token(_)), Some(owner)) => {
                            holdings_of(session, owner)
                                .iter()
                                .any(|held| held.asset == asset)
                        }
                        _ => false,
                    });
            holder
                .or_else(|| (initial_senders.len() == 1).then(|| &initial_senders[0]))
                .map(|wallet| wallet.id)
        })
    });
    let mut asset = use_signal(move || open.asset.unwrap_or(SendAsset::Sol));
    let mut to = use_signal(move || open.to);
    let mut amount = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let reset = use_signal(|| 0u64);
    let mut landing = use_signal(|| Landing::Unknown);
    // Answers to an older recipient or asset are dropped.
    let mut asked = use_signal(|| 0u64);

    use_effect(move || {
        let (asset, to) = (asset(), to());
        let generation = *asked.peek() + 1;
        asked.set(generation);
        let (SendAsset::Token(mint), Some(to)) = (asset, to) else {
            landing.set(Landing::Unknown);
            return;
        };
        landing.set(Landing::Reading);
        spawn(async move {
            let read = send_destination(session, mint, to).await;
            if *asked.peek() == generation {
                landing.set(match read {
                    Ok(resolved) => Landing::Read(resolved),
                    Err(err) => Landing::Failed(err),
                });
            }
        });
    });

    let sender = from().and_then(|id| senders.iter().find(|wallet| wallet.id == id).cloned());
    let owner = sender.as_ref().and_then(|wallet| wallet.address);
    let mut holdings = owner
        .map(|owner| holdings_of(session, owner))
        .unwrap_or_default();
    // A token asked for that the sender does not hold still shows, as none.
    if let SendAsset::Token(mint) = asset() {
        if !holdings.iter().any(|holding| holding.asset == asset()) {
            holdings.push(Holding {
                asset: asset(),
                symbol: symbol_of(session, &mint),
                decimals: 0,
                total: 0,
                largest: 0,
                flagged: false,
            });
        }
    }
    let chosen = holdings
        .iter()
        .find(|holding| holding.asset == asset())
        .cloned();
    let symbol = chosen
        .as_ref()
        .map(|h| h.symbol.clone())
        .unwrap_or_default();

    let close = move || session.close_send();
    let mut review = move || {
        error.set(None);
        let Some(id) = from() else {
            error.set(Some("Choose the wallet it is sent from.".into()));
            return;
        };
        let Some(recipient) = to() else {
            error.set(Some("Choose who receives it.".into()));
            return;
        };
        let what = asset();
        let decimals = match what {
            SendAsset::Sol => 9,
            SendAsset::Token(mint) => {
                match owner
                    .map(|owner| holdings_of(session, owner))
                    .and_then(|list| {
                        list.into_iter()
                            .find(|holding| holding.asset == SendAsset::Token(mint))
                    }) {
                    Some(holding) => holding.decimals,
                    None => {
                        error.set(Some("This wallet holds none of this token.".into()));
                        return;
                    }
                }
            }
        };
        let Some(raw) = parse_amount(&amount(), decimals) else {
            error.set(Some(format!(
                "Enter an amount such as 1.5, with at most {decimals} decimals."
            )));
            return;
        };
        busy.set(true);
        spawn(async move {
            match send(session, id, recipient, what, raw).await {
                Ok(()) => session.close_send(),
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    };

    let title = match &sender {
        Some(wallet) if !symbol.is_empty() => format!("Send {symbol} from {}", wallet.label),
        Some(wallet) => format!("Send from {}", wallet.label),
        None => "Send".to_string(),
    };
    let sol_balance = owner.and_then(|owner| (session.book)().get(&owner).and_then(|e| e.lamports));
    let asset_value = |asset: SendAsset| match asset {
        SendAsset::Sol => "sol".to_string(),
        SendAsset::Token(mint) => mint.to_string(),
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal send",
                role: "dialog",
                aria_modal: "true",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    WalletPicker {
                        label: "From".to_string(),
                        options: senders.clone(),
                        selected: from(),
                        none_label: "Choose a wallet".to_string(),
                        on_change: move |id: Option<WalletId>| {
                            from.set(id);
                            error.set(None);
                        },
                    }
                    label { "Asset"
                        select {
                            aria_label: "Asset",
                            onchange: move |evt| {
                                let value = evt.value();
                                asset.set(if value == "sol" {
                                    SendAsset::Sol
                                } else {
                                    match value.parse() {
                                        Ok(mint) => SendAsset::Token(mint),
                                        Err(_) => SendAsset::Sol,
                                    }
                                });
                                amount.set(String::new());
                                error.set(None);
                            },
                            option {
                                value: "sol",
                                selected: asset() == SendAsset::Sol,
                                match sol_balance {
                                    Some(lamports) => format!("SOL · {}", format_sol(lamports)),
                                    None => "SOL".to_string(),
                                }
                            }
                            for holding in holdings.iter().cloned() {
                                option {
                                    key: "{asset_value(holding.asset)}",
                                    value: "{asset_value(holding.asset)}",
                                    selected: asset() == holding.asset,
                                    "{option_text(&holding)}"
                                }
                            }
                        }
                    }
                    AddressPicker {
                        label: "To".to_string(),
                        selected: to(),
                        reset,
                        on_change: move |pk| to.set(pk),
                    }
                    {landing_line(landing(), &symbol)}
                    label { "Amount"
                        div { class: "btn-row",
                            input {
                                r#type: "text",
                                inputmode: "decimal",
                                placeholder: "0.0",
                                value: "{amount}",
                                oninput: move |evt| amount.set(evt.value()),
                                onkeydown: move |evt: KeyboardEvent| {
                                    if evt.key() == Key::Enter && !busy() {
                                        evt.prevent_default();
                                        review();
                                    }
                                },
                            }
                            if let Some(holding) = chosen.clone().filter(|h| h.largest > 0) {
                                button { class: "ghost sm",
                                    r#type: "button",
                                    onclick: move |_| amount.set(format_token_amount(holding.largest, holding.decimals)),
                                    "Max"
                                }
                            }
                        }
                    }
                    if let Some(holding) = chosen.clone().filter(|h| h.total > h.largest) {
                        p { class: "hint",
                            "Holds {format_token_amount(holding.total, holding.decimals)} {holding.symbol} across several accounts. One transfer moves at most {format_token_amount(holding.largest, holding.decimals)}."
                        }
                    }
                    if let Some(err) = error() {
                        p { class: "error", role: "alert", "{err}" }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            disabled: busy(),
                            onclick: move |_| review(),
                            if busy() { "Preparing…" } else { "Review in Sign" }
                        }
                        button { class: "ghost",
                            onclick: move |_| close(),
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}

fn option_text(holding: &Holding) -> String {
    let flag = if holding.flagged { " (flagged)" } else { "" };
    if holding.total == 0 {
        format!("{}{flag} · none held", holding.symbol)
    } else {
        format!(
            "{}{flag} · {}",
            holding.symbol,
            format_token_amount(holding.total, holding.decimals)
        )
    }
}

fn landing_line(landing: Landing, symbol: &str) -> Element {
    match landing {
        Landing::Unknown => rsx! {},
        Landing::Reading => {
            rsx! { p { class: "hint", "Reading the recipient's {symbol} account…" } }
        }
        Landing::Failed(err) => rsx! { p { class: "error", "{err}" } },
        Landing::Read(resolved) => match resolved.destination {
            Destination::Existing { .. } => rsx! {
                p { class: "hint", "The recipient has a {symbol} account." }
            },
            Destination::TokenAccount { .. } => rsx! {
                p { class: "hint", "This is a {symbol} token account: the tokens go straight into it." }
            },
            Destination::Create { rent, .. } => rsx! {
                p { class: "note",
                    "The recipient has no {symbol} account yet. One is created first, for about {format_sol(rent)} of rent."
                }
            },
        },
    }
}

/// The tokens `owner` holds, one entry per mint, red-flagged ones last.
fn holdings_of(session: Session, owner: Pubkey) -> Vec<Holding> {
    let holdings = (session.holdings_view)();
    let mut by_mint: BTreeMap<Pubkey, Holding> = BTreeMap::new();
    for held in holdings.page(&owner, 0, usize::MAX) {
        if held.amount == 0 {
            continue;
        }
        let entry = by_mint.entry(held.mint).or_insert_with(|| Holding {
            asset: SendAsset::Token(held.mint),
            symbol: symbol_of(session, &held.mint),
            decimals: held.decimals,
            total: 0,
            largest: 0,
            flagged: false,
        });
        entry.total = entry.total.saturating_add(held.amount);
        entry.largest = entry.largest.max(held.amount);
        entry.flagged |= held.flag == Some(TokenFlag::Red);
    }
    let mut list: Vec<Holding> = by_mint.into_values().collect();
    list.sort_by(|a, b| {
        a.flagged
            .cmp(&b.flagged)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    list
}

/// A mint's symbol from the Tokens API, else its label in the book, else
/// its short address.
fn symbol_of(session: Session, mint: &Pubkey) -> String {
    let holdings = (session.holdings_view)();
    holdings
        .market
        .quote(mint)
        .and_then(|quote| quote.symbol.clone())
        .or_else(|| {
            (session.book)()
                .get(mint)
                .map(|entry| entry.label.clone())
                .filter(|label| !label.is_empty())
        })
        .unwrap_or_else(|| short_pubkey(mint))
}
