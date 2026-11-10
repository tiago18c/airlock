//! Burning and closing a wallet's token accounts, to get their rent back.
//! Wallets and Portfolio open it on a wallet, and another can be picked in
//! it. It lists every token account
//! the chain has for it, ticks the empty ones and those of flagged tokens,
//! and runs what is ticked one step at a time through Sign.

use app_ui_kit::actions::cleanup;
use app_ui_kit::{format_sol, format_token_amount, AddressRow, Session, WalletPicker};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use wallet_core::portfolio::TokenCheck;
use wallet_core::spl::is_native_mint;
use wallet_core::token_plan::{Blocked, Choice, Held, Outcome, TokenPlan};
use wallet_core::{short_pubkey, TokenFlag, WalletId};
use wallet_flows::cleanup::default_choice;

/// What a burn asks to be typed before it runs.
const BURN_WORD: &str = "burn";

/// Mounts the Manage tokens modal while a screen has it open.
#[component]
pub fn CleanupHost() -> Element {
    let session = use_context::<Session>();
    match (session.forms.cleanup.open)() {
        Some(wallet) => rsx! { CleanupModal { key: "{wallet}", wallet } },
        None => rsx! {},
    }
}

#[derive(Clone, PartialEq)]
enum Accounts {
    Reading,
    Read(Vec<Held>),
    Failed(String),
}

#[component]
fn CleanupModal(wallet: WalletId) -> Element {
    let session = use_context::<Session>();
    let mut accounts = use_signal(|| Accounts::Reading);
    let mut chosen = use_signal(BTreeMap::<Pubkey, Choice>::new);
    let mut typed = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let notice = session.forms.cleanup.notice;
    // A cleanup under way for this wallet keeps its choices and its count.
    let run = session
        .forms
        .cleanup
        .run
        .read()
        .clone()
        .filter(|run| run.wallet == wallet);
    let done = run.as_ref().map_or(0, |run| run.done);

    let summary = session
        .wallet_summaries()
        .into_iter()
        .find(|summary| summary.id == wallet);
    let owner = summary.and_then(|summary| summary.address);

    let mut reload = move || {
        accounts.set(Accounts::Reading);
        let kept = session
            .forms
            .cleanup
            .run
            .peek()
            .clone()
            .filter(|run| run.wallet == wallet)
            .map(|run| run.chosen);
        spawn(async move {
            match cleanup::read(session, wallet).await {
                Ok(held) => {
                    let flagged = owner
                        .map(|owner| flagged(session, owner))
                        .unwrap_or_default();
                    let start = match &kept {
                        Some(kept) => held
                            .iter()
                            .filter_map(|held| {
                                kept.get(&held.address)
                                    .map(|choice| (held.address, choice.clone()))
                            })
                            .collect(),
                        None => held
                            .iter()
                            .map(|held| {
                                let choice = default_choice(held, flagged.contains(&held.address));
                                (held.address, choice)
                            })
                            .collect(),
                    };
                    chosen.set(start);
                    accounts.set(Accounts::Read(held));
                }
                Err(err) => accounts.set(Accounts::Failed(err)),
            }
        });
    };
    use_hook(reload);

    let plan: Result<TokenPlan, String> = match accounts() {
        Accounts::Read(held) => cleanup::plan(session, wallet, held, &chosen()),
        _ => Ok(TokenPlan::default()),
    };
    let held = match accounts() {
        Accounts::Read(held) => held,
        _ => Vec::new(),
    };
    let decimals = |held: &Held| held.mint.as_ref().map_or(0, |mint| mint.decimals);
    let burns: Vec<String> = match &plan {
        Ok(plan) => plan
            .accounts
            .iter()
            .filter_map(|planned| match planned.outcome {
                Outcome::Burned { amount } => {
                    let held = held.iter().find(|held| held.address == planned.address)?;
                    Some(format!(
                        "{} {}",
                        format_token_amount(amount, decimals(held)),
                        symbol_of(session, &planned.mint)
                    ))
                }
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let closing = plan.as_ref().map_or(0, TokenPlan::closing);
    let steps = plan.as_ref().map_or(0, |plan| plan.steps.len());
    let returned = plan.as_ref().map_or(0, TokenPlan::returned);
    let confirmed = burns.is_empty() || typed().trim().eq_ignore_ascii_case(BURN_WORD);
    let can_start = steps > 0 && confirmed && !busy();

    let close = move || session.close_cleanup();
    let start = move |_| {
        error.set(None);
        busy.set(true);
        let picks = chosen();
        spawn(async move {
            match cleanup::start(session, wallet, picks, done).await {
                Ok(true) => {}
                Ok(false) => {
                    error.set(Some("Nothing you chose is left to close.".into()));
                    reload();
                }
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let mut set_all = move |pick: fn(&Held) -> Option<Choice>| {
        if let Accounts::Read(held) = accounts() {
            chosen.with_mut(|chosen| {
                for held in &held {
                    if let Some(choice) = pick(held) {
                        chosen.insert(held.address, choice);
                    }
                }
            });
        }
    };

    let start_label = if !burns.is_empty() {
        "Burn and close"
    } else {
        "Close accounts"
    };
    let start_label = if done > 0 {
        format!("{start_label}: step {}", done + 1)
    } else {
        start_label.to_string()
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal wide cleanup",
                role: "dialog",
                aria_modal: "true",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Manage tokens" }
                    WalletPicker {
                        label: "Wallet".to_string(),
                        options: session
                            .wallet_summaries()
                            .into_iter()
                            .filter(|wallet| wallet.address.is_some())
                            .collect::<Vec<_>>(),
                        selected: Some(wallet),
                        none_label: "Choose a wallet".to_string(),
                        on_change: move |id: Option<WalletId>| {
                            if let Some(id) = id {
                                session.open_cleanup(id);
                            }
                        },
                    }
                    p { class: "lede",
                        "Each token account holds about 0.002 SOL of rent. Closing one gives it back to the wallet. An account with tokens in it can only be closed once they are burned."
                    }
                    if let Some(text) = notice() {
                        p { class: "note", role: "status", "{text}" }
                    }
                    match accounts() {
                        Accounts::Reading => rsx! {
                            p { class: "hint", role: "status", "Reading the wallet's token accounts…" }
                        },
                        Accounts::Failed(err) => rsx! {
                            p { class: "error", "{err}" }
                            div { class: "btn-row",
                                button { onclick: move |_| reload(), "Try again" }
                            }
                        },
                        Accounts::Read(list) if list.is_empty() => rsx! {
                            p { class: "hint", "This wallet has no token accounts." }
                        },
                        Accounts::Read(list) => rsx! {
                            div { class: "btn-row",
                                button { class: "sm",
                                    onclick: move |_| set_all(|held| {
                                        (held.account.amount == 0 && !is_native_mint(&held.account.mint))
                                            .then_some(Choice::Close)
                                    }),
                                    "Tick every empty one"
                                }
                                button { class: "sm ghost",
                                    onclick: move |_| set_all(|_| Some(Choice::Leave)),
                                    "Untick all"
                                }
                            }
                            ul { class: "book-list cleanup-list",
                                for held in list {
                                    CleanupRow {
                                        key: "{held.address}",
                                        held: held.clone(),
                                        choice: chosen().get(&held.address).cloned().unwrap_or(Choice::Leave),
                                        outcome: plan.as_ref().ok().and_then(|plan| {
                                            plan.accounts
                                                .iter()
                                                .find(|planned| planned.address == held.address)
                                                .map(|planned| planned.outcome.clone())
                                        }),
                                        on_pick: move |(address, choice): (Pubkey, Choice)| {
                                            chosen.with_mut(|chosen| {
                                                chosen.insert(address, choice);
                                            });
                                        },
                                    }
                                }
                            }
                        },
                    }
                    if let Err(err) = &plan {
                        p { class: "error", "{err}" }
                    }
                    if steps > 0 {
                        p { class: "hint",
                            {summary_line(closing, steps, returned)}
                        }
                    }
                    if !burns.is_empty() {
                        div { class: "callout danger-callout",
                            p {
                                "This burns {burns.join(\", \")}. Burned tokens are gone for good."
                            }
                            label { "Type {BURN_WORD} to confirm"
                                input {
                                    r#type: "text",
                                    autocomplete: "off",
                                    value: "{typed}",
                                    oninput: move |evt| typed.set(evt.value()),
                                }
                            }
                        }
                    }
                    if let Some(err) = error() {
                        p { class: "error", "{err}" }
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| close(),
                            "Cancel"
                        }
                        button { class: if burns.is_empty() { "primary" } else { "danger" },
                            disabled: !can_start,
                            onclick: start,
                            if busy() { "Preparing…" } else { "{start_label}" }
                        }
                    }
                }
            }
        }
    }
}

/// One token account: tick it to close it (burning what it holds first),
/// with what the plan does to it.
#[component]
fn CleanupRow(
    held: Held,
    choice: Choice,
    outcome: Option<Outcome>,
    on_pick: EventHandler<(Pubkey, Choice)>,
) -> Element {
    let session = use_context::<Session>();
    let address = held.address;
    let native = is_native_mint(&held.account.mint);
    let decimals = held.mint.as_ref().map_or(0, |mint| mint.decimals);
    let symbol = symbol_of(session, &held.account.mint);
    // The row names the mint already; a symbol adds to it only when the
    // market or the book knows one.
    let amount = if native {
        format_sol(held.account.amount)
    } else if symbol == short_pubkey(&held.account.mint) {
        format_token_amount(held.account.amount, decimals)
    } else {
        format!(
            "{} {symbol}",
            format_token_amount(held.account.amount, decimals)
        )
    };
    let ticked = choice != Choice::Leave;
    let ticked_choice = if held.account.amount == 0 || native {
        Choice::Close
    } else {
        Choice::Burn
    };
    let (chip_class, what) = match &outcome {
        Some(Outcome::Closed) => ("chip ok", "Close".to_string()),
        Some(Outcome::Unwrapped { .. }) => ("chip ok", "Unwrap and close".to_string()),
        Some(Outcome::Burned { .. }) => ("chip danger", "Burn and close".to_string()),
        Some(Outcome::Moved { .. }) => ("chip", "Move".to_string()),
        Some(Outcome::Left(Blocked::Chosen)) | None => ("chip", "Keep".to_string()),
        Some(Outcome::Left(why)) => ("chip warn", why.reason()),
    };
    let blocked = matches!(&outcome, Some(Outcome::Left(why)) if *why != Blocked::Chosen);

    rsx! {
        li { class: "book-row pick-row cleanup-row",
            input {
                r#type: "checkbox",
                checked: ticked,
                title: "Close this account",
                aria_label: "Close the {symbol} account {address}",
                onchange: move |evt| {
                    on_pick.call((address, if evt.checked() { ticked_choice.clone() } else { Choice::Leave }));
                },
            }
            AddressRow { pubkey: held.account.mint,
                span { class: "mono cleanup-amount", "{amount}" }
                span { class: "{chip_class}", title: if blocked { what.clone() } else { String::new() }, "{what}" }
            }
        }
    }
}

fn summary_line(closing: usize, steps: usize, returned: u64) -> String {
    let accounts = if closing == 1 {
        "One account closes".to_string()
    } else {
        format!("{closing} accounts close")
    };
    let steps = if steps == 1 {
        "one transaction".to_string()
    } else {
        format!("{steps} transactions, signed one after another")
    };
    format!(
        "{accounts} in {steps}. {} comes back to the wallet, less the fees.",
        format_sol(returned)
    )
}

/// The token accounts of `owner` whose token the user flagged red, or the
/// market says impersonates another.
fn flagged(session: Session, owner: Pubkey) -> BTreeSet<Pubkey> {
    let holdings = (session.holdings_view)();
    holdings
        .page(&owner, 0, usize::MAX)
        .into_iter()
        .filter(|held| {
            held.flag == Some(TokenFlag::Red)
                || holdings
                    .market
                    .check(&held.mint)
                    .is_some_and(TokenCheck::impersonates)
        })
        .map(|held| held.pubkey)
        .collect()
}

/// A mint's symbol from the Tokens API, else its label in the book, else
/// its short address.
fn symbol_of(session: Session, mint: &Pubkey) -> String {
    super::send::symbol_of(session, mint)
}
