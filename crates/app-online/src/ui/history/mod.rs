//! History: the latest transactions of every wallet, newest first, kept by
//! the `history` sync job. A row says in a sentence what each did, read with
//! the IDLs on this device. Opening one shows it as Review and Sign does, with
//! what it did to each balance.

use app_core::{HistoryEntry, Outcome};
use app_ui_kit::ui::address::name_or_short;
use app_ui_kit::ui::local_time;
use app_ui_kit::ui::tx_view::{use_worked_out, Worked, READING_DEADLINE};
use app_ui_kit::ui::{use_phone_layout, RefreshButton};
use app_ui_kit::{Glyph, Icon, PhraseView, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use wallet_core::{
    change_for, decode_landed, decode_transaction, explain_with_note, format_sol,
    format_token_amount, CatalogView, DecodedTx, DisplayNote, ExplainedIx, IntentBody, IntentSpan,
    IntentValue, LookupTables, Phrase, COMPUTE_BUDGET_PROGRAM_ID,
};

use crate::sync::HISTORY_JOB;
use crate::Jobs;

mod detail;

use detail::HistoryDetail;

/// How many rows are drawn at first, and added by Show older.
const PAGE: usize = 50;

#[component]
pub fn HistoryPanel() -> Element {
    let session = use_context::<Session>();
    let phone = use_phone_layout();
    let form = session.forms.history;
    let mut wallet = form.wallet;
    let mut selected = form.selected;
    let mut shown = use_signal(|| PAGE);
    let jobs = try_use_context::<Jobs>();
    let can_refresh = jobs.is_some();
    let mut refreshing = use_signal(|| false);
    let mut refresh_error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);

    let cluster = session.cluster();
    let history = (session.history)();
    let wallets: Vec<(Pubkey, String)> = session
        .wallet_summaries()
        .into_iter()
        .filter_map(|summary| Some((summary.address?, summary.label)))
        .collect();
    let only = wallet().filter(|pk| wallets.iter().any(|(address, _)| address == pk));

    if let Some(signature) = selected() {
        if let Some(entry) = history.get(&signature).cloned() {
            return rsx! {
                HistoryDetail { entry }
            };
        }
    }

    let entries: Vec<HistoryEntry> = history
        .on(&cluster)
        .filter(|entry| only.is_none_or(|only| entry.addresses.contains(&only)))
        .cloned()
        .collect();
    let total = entries.len();
    let page: Vec<HistoryEntry> = entries.into_iter().take(shown()).collect();
    // The wallets shown, and whether a node may have older transactions of
    // them than are kept.
    let whose: Vec<Pubkey> = match only {
        Some(only) => vec![only],
        None => wallets.iter().map(|(address, _)| *address).collect(),
    };
    let more_kept = total > shown();
    let more_on_chain = jobs.is_some()
        && whose.iter().any(|address| {
            !history.reached_start(&cluster, address)
                && history.oldest_listed(&cluster, address).is_some()
        });
    let every_one_kept = whose
        .iter()
        .all(|address| history.reached_start(&cluster, address));
    let show_older = {
        let jobs = jobs.clone();
        move |_| {
            if more_kept {
                shown += PAGE;
                return;
            }
            let (Some(Jobs { core, .. }), Ok(rpc)) = (jobs.clone(), session.rpc()) else {
                return;
            };
            let whose = whose.clone();
            refresh_error.set(None);
            loading.set(true);
            spawn(async move {
                match crate::sync::load_older(&core, rpc, &whose).await {
                    Ok(_) => shown += PAGE,
                    Err(err) => refresh_error.set(Some(app_ui_kit::actions::sentence(err))),
                }
                loading.set(false);
            });
        }
    };
    let job = (session.sync)().job(HISTORY_JOB).cloned();
    let read_once = job
        .as_ref()
        .is_some_and(|job| job.last_success_ms.is_some());
    let job_error = job.and_then(|job| job.last_error);

    let refresh = move |_| {
        let Some(Jobs { scheduler, .. }) = jobs.clone() else {
            return;
        };
        refresh_error.set(None);
        refreshing.set(true);
        spawn(async move {
            if let Err(err) = scheduler.run_now(HISTORY_JOB).await {
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
                        h2 { "History" }
                        p { class: "muted",
                            "The latest transactions of this device's wallets on {cluster.name()}. What has been read is kept, so it is not asked for again."
                        }
                    }
                    div { class: "btn-row",
                        select {
                            aria_label: "Whose transactions",
                            onchange: move |e| {
                                wallet.set(e.value().parse().ok());
                                shown.set(PAGE);
                            },
                            option { value: "", selected: only.is_none(), "All wallets" }
                            for (address, label) in wallets.iter().cloned() {
                                option {
                                    key: "{address}",
                                    value: "{address}",
                                    selected: only == Some(address),
                                    "{label}"
                                }
                            }
                        }
                        if can_refresh {
                            RefreshButton { busy: refreshing(), onclick: refresh }
                        }
                    }
                }
                if phone && refreshing() {
                    p { class: "hint pane-refreshing", role: "status", "Refreshing…" }
                }

                if let Some(err) = refresh_error().or(job_error) {
                    p { class: "error", "{err}" }
                }

                if wallets.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::History }
                        p { "Add a wallet to see what it did on chain." }
                    }
                } else if page.is_empty() && !read_once {
                    div { class: "empty",
                        Icon { glyph: Glyph::History }
                        p { role: "status", "Reading the wallets' transactions…" }
                    }
                } else if page.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::History }
                        p { "No transactions on {cluster.name()} yet." }
                    }
                } else {
                    HistoryList {
                        entries: page,
                        only,
                        onopen: move |signature| selected.set(Some(signature)),
                    }
                    if more_kept || more_on_chain {
                        div { class: "btn-row",
                            button {
                                disabled: loading(),
                                onclick: show_older,
                                if loading() { "Loading older…" } else { "Show older" }
                            }
                            if more_kept {
                                span { class: "muted", "{shown()} of {total}" }
                            }
                        }
                    } else if every_one_kept {
                        p { class: "hint", "That is every transaction of these wallets on {cluster.name()}." }
                    }
                }
            }
        }
    }
}

/// What a row says a transaction did.
#[derive(Clone, Debug, PartialEq)]
struct RowWords {
    /// The first thing it does, in a sentence, when an IDL reads it.
    sentence: Option<Vec<IntentSpan>>,
    /// The instruction's name, when an IDL reads it.
    title: Option<String>,
    /// The program it calls first, and its name if known.
    program: Option<(Pubkey, Option<String>)>,
    /// How many more things it does.
    more: usize,
}

/// The transaction of `entry`, read from the node's copy or from what was
/// sent. `None` until there are bytes to read.
pub(super) fn decoded(entry: &HistoryEntry) -> Option<DecodedTx> {
    match (&entry.landed, &entry.sent_wire) {
        (Some(landed), _) => decode_landed(landed).ok(),
        (None, Some(wire)) => decode_transaction(wire, &LookupTables::new()).ok(),
        (None, None) => None,
    }
}

/// The rows, each with its sentence, read away from the window together.
#[component]
fn HistoryList(
    entries: Vec<HistoryEntry>,
    only: Option<Pubkey>,
    onopen: EventHandler<Signature>,
) -> Element {
    let session = use_context::<Session>();
    let txs: Vec<(Signature, DecodedTx)> = entries
        .iter()
        .filter_map(|entry| Some((entry.signature, decoded(entry)?)))
        .collect();
    let answer = use_worked_out::<Vec<(Signature, DecodedTx)>, Vec<(Signature, RowWords)>>(
        txs,
        READING_DEADLINE,
        |asked, now| asked == now,
        read_rows,
    );
    let words: Vec<(Signature, RowWords)> = match answer.worked {
        Worked::Done(words) => words,
        Worked::Waiting | Worked::GaveUp => Vec::new(),
    };
    let now = app_ui_kit::actions::unix_now();
    let mut last_day = None;
    let rows: Vec<Row> = entries
        .into_iter()
        .map(|entry| {
            let when = entry.block_time.unwrap_or(entry.seen);
            let day = day_label(when, now);
            let head = (last_day.as_ref() != Some(&day)).then(|| day.clone());
            last_day = Some(day);
            let row_words = words
                .iter()
                .find(|(signature, _)| *signature == entry.signature)
                .map(|(_, words)| words.clone());
            let wallets: Vec<String> = entry
                .addresses
                .iter()
                .filter(|address| only.is_none_or(|only| only == **address))
                .map(|address| name_or_short(&session, address))
                .collect();
            let mut sub = local_time::clock_text(when);
            for name in wallets {
                sub.push_str(" · ");
                sub.push_str(&name);
            }
            Row {
                head,
                signature: entry.signature,
                text: row_text(&entry, row_words.as_ref()),
                sub,
                changes: change_lines(&session, &entry, only),
                outcome: entry.outcome,
            }
        })
        .collect();

    rsx! {
        div { class: "history-list",
            for row in rows {
                div { class: "history-item", key: "{row.signature}",
                if let Some(head) = row.head.clone() {
                    h3 { class: "history-day", "{head}" }
                }
                // Not a `button`: its title holds addresses, which open their
                // own card, and a button may not hold anything pressable.
                div {
                    class: "history-row",
                    role: "button",
                    tabindex: "0",
                    onclick: move |_| onopen.call(row.signature),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                            e.prevent_default();
                            onopen.call(row.signature);
                        }
                    },
                    div { class: "history-row-main",
                        span { class: "history-row-text", PhraseView { phrase: row.text.clone() } }
                        span { class: "history-row-sub muted", "{row.sub}" }
                    }
                    div { class: "history-row-side",
                        for (i, line) in row.changes.iter().enumerate() {
                            span {
                                key: "{i}",
                                class: if line.starts_with('+') { "history-amount gain" } else { "history-amount" },
                                "{line}"
                            }
                        }
                        OutcomeChip { outcome: row.outcome.clone() }
                    }
                }
                }
            }
        }
    }
}

/// One row, worked out before it is drawn.
struct Row {
    /// The day heading drawn above it, when it starts a day.
    head: Option<String>,
    signature: Signature,
    /// What it did, its addresses drawn as addresses everywhere are.
    text: Phrase,
    sub: String,
    changes: Vec<String>,
    outcome: Outcome,
}

#[component]
pub(super) fn OutcomeChip(outcome: Outcome) -> Element {
    match outcome {
        Outcome::Pending => rsx! {
            span { class: "chip info",
                span { class: "toast-spin", "aria-hidden": "true" }
                "waiting"
            }
        },
        Outcome::Succeeded => rsx! { span { class: "chip ok", "succeeded" } },
        Outcome::Failed { .. } => rsx! { span { class: "chip danger", "failed" } },
    }
}

/// "Today", "Yesterday", or the date.
fn day_label(secs: i64, now: i64) -> String {
    let today = local_time::day(now);
    let day = local_time::day(secs);
    if day == today {
        "Today".into()
    } else if Some(day) == today.previous_day() {
        "Yesterday".into()
    } else {
        local_time::date_text(secs)
    }
}

/// Each row's words, with the catalog as it is. Away from the window.
// `use_worked_out` hands its key back as it was given: a `Vec`.
#[allow(clippy::ptr_arg)]
fn read_rows(view: &CatalogView, txs: &Vec<(Signature, DecodedTx)>) -> Vec<(Signature, RowWords)> {
    txs.iter()
        .map(|(signature, tx)| {
            let explained = explain_with_note(view, tx, &DisplayNote::default());
            let name_of = |program: &Pubkey| {
                explained
                    .programs
                    .iter()
                    .find(|line| line.program_id == *program)
                    .and_then(|line| line.name.clone())
            };
            let doing: Vec<&ExplainedIx> = explained
                .instructions
                .iter()
                .filter(|ix| ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
                .collect();
            // An envelope says what it carries: a proposal, an authorization.
            let first = doing.first().map(|ix| {
                ix.inner
                    .as_ref()
                    .and_then(|inner| {
                        inner
                            .instructions
                            .iter()
                            .find(|inner| inner.program_id != COMPUTE_BUDGET_PROGRAM_ID)
                    })
                    .filter(|_| ix.idl.as_ref().and_then(|idl| idl.spans.as_ref()).is_none())
                    .unwrap_or(ix)
            });
            let words = RowWords {
                sentence: first.and_then(|ix| ix.idl.as_ref()?.spans.clone()),
                title: first.and_then(|ix| Some(ix.idl.as_ref()?.intent.clone())),
                program: first.map(|ix| (ix.program_id, name_of(&ix.program_id))),
                more: doing.len().saturating_sub(1),
            };
            (*signature, words)
        })
        .collect()
}

/// One line for a row: what it did, in words when they are known, each
/// address in it an address, as in every other sentence of the app.
fn row_text(entry: &HistoryEntry, words: Option<&RowWords>) -> Phrase {
    let Some(words) = words else {
        // The node said it does not have it, so often it is not asked again.
        if decoded(entry).is_none() && entry.misses >= app_core::history::MAX_MISSES {
            return Phrase::text("The node did not return it in full");
        }
        return Phrase::text(match (&entry.label, decoded(entry).is_some()) {
            (_, true) => "Reading it…".into(),
            (Some(label), false) => format!("{label} · reading it from the node…"),
            (None, false) => "Reading it from the node…".into(),
        });
    };
    let mut text = match (&words.sentence, &words.title, &words.program) {
        (Some(spans), _, _) => sentence_phrase(spans),
        (None, Some(title), _) => Phrase::text(title.clone()),
        (None, None, Some((program, _))) => Phrase::text("Calls ").a(*program),
        (None, None, None) => Phrase::text("Does nothing"),
    };
    if words.more > 0 {
        text.push_text(format!(" · and {} more", words.more));
    }
    text
}

/// A sentence as a phrase: its words and values as text, its addresses as
/// addresses.
fn sentence_phrase(spans: &[IntentSpan]) -> Phrase {
    let mut phrase = Phrase::default();
    for span in spans {
        match span {
            IntentSpan::Text(text) => phrase.push_text(text.clone()),
            IntentSpan::Value(IntentValue {
                body: IntentBody::Address(pubkey),
                ..
            }) => phrase.push_address(*pubkey),
            IntentSpan::Value(value) => phrase.push_text(value.primary.clone()),
        };
    }
    phrase
}

/// What it did to the balances of the wallet shown, or of each wallet it
/// names: "−0.001 SOL", "+2.5 USDC". Fees count.
fn change_lines(session: &Session, entry: &HistoryEntry, only: Option<Pubkey>) -> Vec<String> {
    let Some(landed) = &entry.landed else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for owner in entry
        .addresses
        .iter()
        .filter(|address| only.is_none_or(|only| only == **address))
    {
        let change = change_for(landed, owner);
        if change.lamports != 0 {
            lines.push(format!(
                "{}{}",
                sign(change.lamports),
                format_sol(change.lamports.unsigned_abs() as u64)
            ));
        }
        for token in &change.tokens {
            lines.push(format!(
                "{}{} {}",
                sign(token.amount),
                format_token_amount(token.amount.unsigned_abs() as u64, token.decimals),
                name_or_short(session, &token.mint)
            ));
        }
    }
    lines
}

fn sign(amount: i128) -> &'static str {
    if amount < 0 {
        "−"
    } else {
        "+"
    }
}
