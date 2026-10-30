//! A transaction on screen. Summary says what it does in sentences. Inspect
//! adds everything it is made of: limits, accounts, programs, each field and
//! the raw bytes. Instructions carried inside other instructions are opened
//! in both.

use crate::actions::programs::check_many;
use crate::state::forms::SimState;
use crate::state::session::Session;
use crate::ui::{format_sol, short_checksum, Address, AddressRow, BalanceDeltas};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use wallet_core::{
    display_note, explain_with_note, ContextPinMatch, DecodedTx, ExplainedIx, IdlExplanation,
    IdlOrigin, IdlPinStatus, InnerKind, InnerMessage, IntentBody, IntentSpan, IntentValue, NextKey,
    ScaleSource, COMPUTE_BUDGET_PROGRAM_ID, MAX_TRANSACTION_SIZE,
};

mod facts;
mod instruction;

pub use facts::*;
pub use instruction::*;

/// Gives every address drawn in `children` the change the last simulation
/// found for it. A failed simulation changes nothing.
#[component]
pub fn SimulatedDeltas(children: Element) -> Element {
    let session = use_context::<Session>();
    let deltas = use_memo(move || match (session.forms.sign.simulation)() {
        SimState::Done(simulation) if simulation.error.is_none() => simulation
            .balance_changes
            .iter()
            .filter(|change| change.before != change.after)
            .map(|change| (change.pubkey, (change.before, change.after)))
            .collect(),
        _ => HashMap::new(),
    });
    use_context_provider(|| BalanceDeltas(deltas));
    rsx! { {children} }
}

/// `signers` are the addresses the screen lists under its signatures, inside
/// the same [`SimulatedDeltas`]. Their changes are drawn there, so Summary
/// does not repeat them. `children` sit beside the heading.
#[component]
pub fn TxView(
    tx: DecodedTx,
    inspect: bool,
    #[props(default)] signers: Vec<Pubkey>,
    children: Element,
) -> Element {
    let session = use_context::<Session>();
    let stored = (session.forms.sign.context)();
    let shown = tx.clone();
    // Decoding against the IDL catalog is the expensive part of this screen.
    // Redo it only when the transaction, the note or the catalog changes.
    // What an Advance does to a hash-based key is read with the accounts'
    // schemes: from a context note, a rebuild, or a Vector wallet in this
    // vault. A pasted or scanned transaction brings only the wallets'.
    let vector = (session.forms.sign.vector)();
    let wallets = (session.wallets)().summaries.clone();
    let explained = use_memo(use_reactive!(|(tx, stored, vector, wallets)| {
        let note = display_note(&tx, stored.as_ref())
            .with_vector_schemes(&vector)
            .with_wallet_schemes(&wallets);
        explain_with_note(&*session.idls.read(), &tx, &note)
    }))();
    // Summary names only some accounts. One that changes and is neither
    // named nor a signer gets a line of its own; Inspect lists every account.
    let deltas = try_use_context::<BalanceDeltas>();
    let unnamed: Vec<Pubkey> = match deltas {
        Some(deltas) if !inspect => {
            let mut named: HashSet<Pubkey> = signers.into_iter().collect();
            summary_addresses(&explained.instructions, &mut named);
            let deltas = deltas.0.read();
            shown
                .accounts
                .iter()
                .map(|account| account.pubkey)
                .filter(|pubkey| deltas.contains_key(pubkey) && !named.contains(pubkey))
                .collect()
        }
        _ => Vec::new(),
    };

    rsx! {
        if inspect {
            TxFacts { tx: shown.clone() }
        }
        div { class: "sign-status",
            h3 { if inspect { "Instructions" } else { "Transaction" } }
            {children}
        }
        InstructionList { instructions: explained.instructions.clone(), inspect }
        if !unnamed.is_empty() {
            p { class: "fee-line muted",
                "Also changes "
                for (i, pubkey) in unnamed.into_iter().enumerate() {
                    span { key: "{pubkey}",
                        if i > 0 {
                            ", "
                        }
                        Address { pubkey }
                    }
                }
            }
        }
        if inspect {
            AccountList { tx: shown.clone() }
            ProgramList { explained_programs: explained.programs.clone() }
        }
    }
}

/// Every address Summary draws for `instructions`, the way
/// [`InstructionBody`] decides what to draw.
fn summary_addresses(instructions: &[ExplainedIx], named: &mut HashSet<Pubkey>) {
    for ix in instructions {
        let envelope = matches!(
            ix.inner.as_ref().map(|inner| &inner.kind),
            Some(
                InnerKind::Authorization { .. }
                    | InnerKind::Execution { .. }
                    | InnerKind::Passthrough { .. }
            )
        );
        if !envelope {
            match &ix.idl {
                Some(IdlExplanation {
                    spans: Some(spans), ..
                }) => named.extend(spans.iter().filter_map(|span| match span {
                    IntentSpan::Value(IntentValue {
                        body: IntentBody::Address(pubkey),
                        ..
                    }) => Some(*pubkey),
                    _ => None,
                })),
                Some(idl) => named.extend(idl.accounts.iter().map(|account| account.pubkey)),
                None => named.extend(ix.accounts.iter().copied()),
            }
        }
        if let Some(inner) = &ix.inner {
            match &inner.kind {
                InnerKind::Authorization { authorities, .. } => {
                    for key in authorities {
                        named.insert(key.authority);
                        named.insert(key.address);
                    }
                }
                InnerKind::Execution { nonce_account, .. } => {
                    named.insert(*nonce_account);
                }
                InnerKind::SquadsTransaction { vault, .. } => {
                    named.insert(*vault);
                }
                InnerKind::ProposalTransaction {
                    governance,
                    proposal,
                    ..
                } => {
                    named.insert(*governance);
                    named.insert(*proposal);
                }
                InnerKind::Passthrough { signers } => {
                    for signer in signers {
                        named.insert(signer.account);
                        named.extend(signer.system);
                    }
                }
            }
            summary_addresses(&inner.instructions, named);
        }
    }
}

/// Instructions in order, each numbered within its own level. Summary leaves
/// out ComputeBudget instructions: what they cost is the fee line, and
/// Inspect states the limits. The rest keep their numbers, the ones a failed
/// simulation names.
#[component]
pub fn InstructionList(instructions: Vec<ExplainedIx>, inspect: bool) -> Element {
    rsx! {
        ol { class: "ix-list",
            for (i, ix) in instructions
                .into_iter()
                .enumerate()
                .filter(|(_, ix)| inspect || ix.program_id != COMPUTE_BUDGET_PROGRAM_ID)
            {
                li { class: "ix-card", key: "{i}",
                    span { class: "ix-num", "aria-hidden": "true", "{i + 1}" }
                    div { class: "ix-body",
                        InstructionBody { ix, inspect }
                    }
                }
            }
        }
    }
}

#[component]
fn InstructionBody(ix: ExplainedIx, inspect: bool) -> Element {
    // A Submit and the executor call inside it only carry other instructions.
    // In Summary they are a heading for what they carry.
    let envelope = matches!(
        ix.inner.as_ref().map(|inner| &inner.kind),
        Some(
            InnerKind::Authorization { .. }
                | InnerKind::Execution { .. }
                | InnerKind::Passthrough { .. }
        )
    );
    let inner = ix.inner.clone();
    rsx! {
        if inspect || !envelope {
            if let Some(idl) = ix.idl.clone() {
                IdlInstruction { idl, data: ix.data.clone(), inspect }
            } else {
                UnknownInstruction { ix: ix.clone(), inspect }
            }
        }
        if let Some(inner) = inner {
            InnerBlock { inner, inspect }
        }
    }
}

#[component]
fn InnerBlock(inner: InnerMessage, inspect: bool) -> Element {
    rsx! {
        div { class: "inner-block",
            match inner.kind.clone() {
                InnerKind::Authorization { authorities, forwarded } => rsx! {
                    for key in authorities {
                        div { class: "chip-row inner-head", key: "{key.authority}",
                            span { "Authorized by" }
                            Address { pubkey: key.authority }
                            span { "acting as" }
                            Address { pubkey: key.address }
                            if key.signed {
                                span { class: "chip ok", "signed" }
                            } else {
                                span { class: "chip warn", "not signed" }
                            }
                        }
                    }
                    if inspect && !forwarded.is_empty() {
                        div { class: "chip-row",
                            span { class: "muted", "also signing the transaction" }
                            for pubkey in forwarded {
                                Address { key: "{pubkey}", pubkey }
                            }
                        }
                    }
                },
                InnerKind::Execution { nonce_account, nonce_authority, nonce_value } => rsx! {
                    div { class: "chip-row inner-head",
                        span { "Runs once, when nonce" }
                        Address { pubkey: nonce_account }
                        span { "is next in line" }
                    }
                    if inspect {
                        div { class: "chip-row",
                            span { class: "muted", "nonce authority" }
                            Address { pubkey: nonce_authority }
                        }
                        p { class: "muted",
                            "signed against nonce value "
                            code { class: "mono", "{nonce_value}" }
                        }
                    }
                },
                InnerKind::SquadsTransaction { vault, vault_index } => rsx! {
                    div { class: "chip-row inner-head",
                        span { "Stored for vault" }
                        Address { pubkey: vault }
                        if inspect {
                            span { class: "muted", "(number {vault_index})" }
                        }
                        span { "to run after approval" }
                    }
                },
                InnerKind::ProposalTransaction {
                    governance,
                    proposal,
                    option_index,
                    transaction_index,
                } => rsx! {
                    div { class: "chip-row inner-head",
                        span { "Added to proposal" }
                        Address { pubkey: proposal }
                        if inspect {
                            span { class: "muted",
                                "(option {option_index}, transaction {transaction_index})"
                            }
                        }
                        span { "for governance" }
                        Address { pubkey: governance }
                        span { "to run if it passes" }
                    }
                },
                InnerKind::Passthrough { signers } => rsx! {
                    for signer in signers {
                        div { class: "chip-row inner-head", key: "{signer.account}",
                            span { "Runs as" }
                            Address { pubkey: signer.account }
                            if let Some(system) = signer.system {
                                span { "through its system account" }
                                Address { pubkey: system }
                            }
                        }
                        match signer.next_key {
                            Some(NextKey::Replaces(_)) => rsx! {
                                p { class: "warn",
                                    "This replaces the key that controls the account."
                                }
                            },
                            Some(NextKey::Names(_)) => rsx! {
                                p { class: "warn",
                                    "This names the key that controls the account next. It replaces the current key unless it is that same key, which this device cannot tell."
                                }
                            },
                            None => rsx! {},
                        }
                    }
                },
            }
            InstructionList { instructions: inner.instructions.clone(), inspect }
        }
    }
}

/// No stored IDL reads this instruction, so nothing is claimed about it.
#[component]
fn UnknownInstruction(ix: ExplainedIx, inspect: bool) -> Element {
    let session = use_context::<Session>();
    let known = session
        .idl_records()
        .iter()
        .any(|record| record.program_id == ix.program_id);
    // Explaining leaves a Passthrough closed, IDL and all, only when its
    // bytes are not what the program reads.
    let passthrough = ix.program_id == wallet_core::vector::PROGRAM_ID
        && ix.data.first() == Some(&wallet_core::vector::PASSTHROUGH_DISCRIMINATOR);
    let hex = hex::encode(&ix.data);
    let bytes = ix.data.len();
    let count = ix.accounts.len();
    rsx! {
        p { class: "warn",
            if passthrough {
                "This Vector Passthrough is not laid out the way the program reads one. It would fail on chain, and what it carries is not shown."
            } else if known {
                "The stored IDL for this program does not recognise this instruction."
            } else {
                "No IDL for this program. This instruction cannot be explained."
            }
        }
        div { class: "chip-row",
            span { class: "muted", "program" }
            Address { pubkey: ix.program_id }
            if !inspect {
                span { class: "muted", "{count} accounts · {bytes} bytes of data" }
            }
        }
        // Nothing names these accounts, so they are listed by position.
        if !ix.accounts.is_empty() {
            ul { class: "ix-accounts",
                for (i, account) in ix.accounts.iter().copied().enumerate() {
                    li { class: "chip-row", key: "{i}",
                        span { class: "muted ix-account-name", "#{i + 1}" }
                        AddressRow { pubkey: account, full: true }
                    }
                }
            }
        }
        if inspect {
            if !hex.is_empty() {
                code { class: "mono", "{hex}" }
            }
        }
    }
}
