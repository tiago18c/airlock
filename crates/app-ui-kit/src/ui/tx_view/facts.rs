//! The transaction's limits, accounts and programs.

use super::*;

/// The numbers a v1 message states about itself.
#[component]
pub fn TxFacts(tx: DecodedTx) -> Element {
    let size = tx.wire.len();
    let limit = match tx.budget.compute_unit_limit {
        Some(limit) => format!("{limit} units"),
        None => "not set (zero)".to_string(),
    };
    let priority = match tx.budget.priority_fee {
        Some(fee) => format_sol(fee),
        None => "none".to_string(),
    };
    let loaded = match tx.budget.loaded_accounts_data_size_limit {
        Some(bytes) => format!("{bytes} bytes"),
        None => "not set (zero)".to_string(),
    };
    rsx! {
        div { class: "stats",
            div { class: "stat",
                span { class: "stat-label", "Size" }
                span { class: "stat-value", "{size} / {MAX_TRANSACTION_SIZE} bytes" }
            }
            div { class: "stat",
                span { class: "stat-label", "Compute limit" }
                span { class: "stat-value", "{limit}" }
            }
            div { class: "stat",
                span { class: "stat-label", "Priority fee" }
                span { class: "stat-value", "{priority}" }
            }
            div { class: "stat",
                span { class: "stat-label", "Account data limit" }
                span { class: "stat-value", "{loaded}" }
            }
            if let Some(heap) = tx.budget.heap_size {
                div { class: "stat",
                    span { class: "stat-label", "Heap" }
                    span { class: "stat-value", "{heap} bytes" }
                }
            }
        }
        p { class: "muted",
            if tx.uses_durable_nonce { "Lives on durable nonce " } else { "Lives on blockhash " }
            code { class: "mono", "{tx.lifetime}" }
        }
    }
}

/// The most the fee payer pays in fees: the base fee for each signature and
/// the priority fee.
#[component]
pub fn FeeLine(tx: DecodedTx) -> Element {
    rsx! {
        p { class: "fee-line muted",
            "Fee up to {format_sol(tx.fee_lamports())}"
            if let Some(payer) = tx.fee_payer() {
                " · paid by "
                Address { pubkey: payer }
            }
        }
    }
}

#[component]
pub fn AccountList(tx: DecodedTx) -> Element {
    rsx! {
        h3 { "Accounts" }
        ul { class: "book-list",
            for account in tx.accounts {
                li { class: "book-row",
                    key: "{account.pubkey}",
                    AddressRow { pubkey: account.pubkey, full: true,
                        if account.signer {
                            span { class: "chip kind", "signer" }
                        }
                        if account.writable {
                            span { class: "chip", "writable" }
                        } else {
                            span { class: "chip", "readonly" }
                        }
                    }
                }
            }
        }
    }
}

/// Every program the transaction calls, at any depth, with the IDL this
/// device reads it by.
#[component]
pub fn ProgramList(explained_programs: Vec<wallet_core::ProgramLine>) -> Element {
    let session = use_context::<Session>();
    let check_ids: Vec<Pubkey> = explained_programs
        .iter()
        .filter(|line| line.sha256.is_some())
        .map(|line| line.program_id)
        .collect();
    let no_idl_to_check = check_ids.is_empty();
    let checking = use_signal(|| false);
    let check_note = use_signal(|| None::<String>);
    rsx! {
        h3 { "Programs" }
        if let Some(text) = session.idl_pack_note() {
            p { class: "hint", "{text}" }
        }
        div { class: "btn-row",
            button {
                disabled: checking() || no_idl_to_check,
                onclick: move |_| {
                    check_many(session, check_ids.clone(), checking, check_note)
                },
                if checking() { "Checking…" } else { "Check programs" }
            }
            if let Some(text) = check_note() {
                span { class: "muted", "{text}" }
            }
        }
        ul { class: "book-list",
            for line in explained_programs {
                li { class: "book-row",
                    key: "{line.program_id}",
                    div { class: "chip-row",
                        if let Some(name) = line.name.clone() {
                            span { "{name}" }
                        }
                        Address { pubkey: line.program_id }
                        if let Some(sha) = line.sha256 {
                            code { class: "mono", "{short_checksum(&sha)}" }
                        }
                        match line.origin {
                            Some(IdlOrigin::Builtin) => rsx! { span { class: "chip", "built in" } },
                            Some(_) => rsx! { span { class: "chip info", "imported" } },
                            None => rsx! {},
                        }
                        if line.status != IdlPinStatus::NotChecked || line.origin != Some(IdlOrigin::Builtin) {
                            span { class: pin_class(line.status), "{line.status.label()}" }
                        }
                        if let Some(pin) = line.context_pin {
                            span { class: "chip", "{context_pin_label(pin)}" }
                        }
                    }
                }
            }
        }
    }
}

pub fn context_pin_label(pin: ContextPinMatch) -> &'static str {
    match pin {
        ContextPinMatch::Same => "same IDL",
        ContextPinMatch::Differs => "different IDL",
        ContextPinMatch::Missing => "no local IDL",
    }
}

pub fn pin_class(status: IdlPinStatus) -> &'static str {
    match status {
        IdlPinStatus::Matches | IdlPinStatus::MatchesSource => "chip ok",
        IdlPinStatus::Differs => "chip danger",
        _ => "chip",
    }
}
