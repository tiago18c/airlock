//! One transaction from History, shown as Review and Sign shows one: what it
//! does in sentences, or everything it is made of, with what it did to each
//! balance and what the node kept of running it.

use std::collections::HashMap;

use app_core::{HistoryEntry, Outcome};
use app_ui_kit::actions::sentence;
use app_ui_kit::state::forms::SignView;
use app_ui_kit::ui::tx_view::{use_explained_landed, TxView};
use app_ui_kit::ui::{local_time, BalanceDeltas, CopyButton};
use app_ui_kit::{use_network, AddressRow, Glyph, Icon, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::{explorer, format_sol, landed_balances, DecodedTx};

use super::{decoded, OutcomeChip};

#[component]
pub(super) fn HistoryDetail(entry: HistoryEntry) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.history;
    let mut selected = form.selected;
    let mut view = form.view;
    let online = use_network().is_some();
    let inspect = view() == SignView::Inspect;
    let mut asked = use_signal(|| None::<solana_signature::Signature>);
    let mut read_error = use_signal(|| None::<String>);

    // The job reads the newest first. One opened before its turn is read now,
    // once, from a node of the cluster it ran on.
    let wants = entry.wants_details() && entry.cluster == session.cluster();
    let signature = entry.signature;
    let cluster = entry.cluster.clone();
    use_effect(use_reactive!(|(wants, signature, cluster)| {
        if !online || !wants || *asked.peek() == Some(signature) {
            return;
        }
        asked.set(Some(signature));
        read_error.set(None);
        let rpc = session.rpc();
        spawn(async move {
            let read = match rpc {
                Ok(rpc) => rpc.get_landed_transaction(&signature).await,
                Err(err) => Err(err),
            };
            match read {
                Ok(landed) => session.history_deferred("history_landed", |history| {
                    history.set_landed(&signature, &cluster, landed)
                }),
                Err(err) => read_error.set(Some(sentence(err))),
            }
        });
    }));

    let tx = decoded(&entry);
    let when = entry.block_time.unwrap_or(entry.seen);
    let date = local_time::date_text(when);
    let time = local_time::clock_text_seconds(when);
    let sig = signature.to_string();
    let explorer_url = explorer::tx_url(&signature, &entry.cluster);
    let cluster_name = entry.cluster.name().to_string();
    let landed = entry.landed.clone();
    let balances = landed.as_ref().map(landed_balances).unwrap_or_default();
    let logs = landed
        .as_ref()
        .map(|landed| landed.logs.clone())
        .unwrap_or_default();
    let failed = match &entry.outcome {
        Outcome::Failed { error } => Some(error.clone()),
        _ => None,
    };

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "page-head",
                    div {
                        button {
                            class: "ghost sm history-back",
                            r#type: "button",
                            onclick: move |_| selected.set(None),
                            Icon { glyph: Glyph::ChevronLeft }
                            "All transactions"
                        }
                        h2 { "Transaction" }
                    }
                    div { class: "btn-row",
                        div { class: "segmented", role: "group", aria_label: "How much to show",
                            button {
                                class: if !inspect { "tab on" } else { "tab" },
                                onclick: move |_| view.set(SignView::Summary),
                                "Summary"
                            }
                            button {
                                class: if inspect { "tab on" } else { "tab" },
                                onclick: move |_| view.set(SignView::Inspect),
                                "Inspect"
                            }
                        }
                    }
                }

                div { class: "card history-facts",
                    div { class: "meta-row",
                        OutcomeChip { outcome: entry.outcome.clone() }
                        span { class: "muted", "{date}, {time}" }
                        span { class: "chip", "{cluster_name}" }
                        if let Some(label) = entry.label.clone() {
                            span { class: "chip kind", title: "Sent from this app", "{label}" }
                        }
                    }
                    if let Some(error) = failed {
                        p { class: "error",
                            "It failed: {error}. Its fee was paid, and nothing else it did stayed."
                        }
                    } else if entry.outcome == Outcome::Pending {
                        p { class: "muted", role: "status", "Sent. Waiting for it to land." }
                    }
                    dl { class: "history-meta",
                        dt { "Signature" }
                        dd { class: "history-sig",
                            code { class: "mono", "{sig}" }
                            CopyButton { text: sig.clone(), label: "Copy signature" }
                        }
                        dt { "Wallets" }
                        dd {
                            for address in entry.addresses.iter().copied() {
                                AddressRow { key: "{address}", pubkey: address }
                            }
                        }
                        if let Some(landed) = landed.as_ref() {
                            dt { "Fee paid" }
                            dd { "{format_sol(landed.fee)}" }
                            dt { "Slot" }
                            dd { "{landed.slot}" }
                        }
                    }
                    if let Some(url) = explorer_url {
                        div { class: "btn-row",
                            button {
                                class: "ghost sm",
                                r#type: "button",
                                onclick: move |_| {
                                    if let Err(err) = session.open_link(&url) {
                                        session.show_problem(format!("Could not open link: {err}"));
                                    }
                                },
                                "View on explorer"
                            }
                        }
                    }
                }

                if let Some(err) = read_error() {
                    p { class: "error", "{err}" }
                }

                match tx {
                    Some(tx) => rsx! {
                        LandedView { tx, balances, inspect, logs }
                    },
                    None if entry.misses >= app_core::history::MAX_MISSES => rsx! {
                        p { class: "muted",
                            "The node did not return this transaction in full, so it cannot be shown in words. The explorer may have it."
                        }
                    },
                    None if online => rsx! {
                        p { class: "muted", role: "status", "Reading it from the node…" }
                    },
                    None => rsx! {
                        p { class: "muted",
                            "It has not been read from the node yet. It will be once the app is online."
                        }
                    },
                }
            }
        }
    }
}

/// The transaction itself, with what it did to each balance drawn beside the
/// address it changed.
#[component]
fn LandedView(
    tx: DecodedTx,
    balances: Vec<(Pubkey, u64, u64)>,
    inspect: bool,
    logs: Vec<String>,
) -> Element {
    let reading = use_explained_landed(tx.clone());
    let deltas = use_memo(use_reactive!(|balances| {
        balances
            .iter()
            .map(|(pubkey, pre, post)| (*pubkey, (*pre, *post)))
            .collect::<HashMap<_, _>>()
    }));
    use_context_provider(|| BalanceDeltas(deltas));
    rsx! {
        TxView { tx, reading, inspect }
        if inspect && !logs.is_empty() {
            div { class: "card",
                h3 { "Logs" }
                pre { class: "mono history-logs",
                    for (i, line) in logs.iter().enumerate() {
                        span { key: "{i}", "{line}\n" }
                    }
                }
            }
        }
    }
}
