//! The transaction's limits, accounts and programs.

use super::*;

/// The numbers a v1 message states about itself.
#[component]
pub fn TxFacts(tx: DecodedTx) -> Element {
    let number = |n: u64| group_thousands(&n.to_string());
    let size = format!(
        "{} / {} bytes",
        number(tx.wire.len() as u64),
        number(MAX_TRANSACTION_SIZE as u64)
    );
    let limit = match tx.budget.compute_unit_limit {
        Some(limit) => format!("{} units", number(limit.into())),
        None => "not set (zero)".to_string(),
    };
    let priority = match tx.budget.priority_fee {
        Some(fee) => format_sol(fee),
        None => "none".to_string(),
    };
    let loaded = match tx.budget.loaded_accounts_data_size_limit {
        Some(bytes) => format!("{} bytes", number(bytes.into())),
        None => "not set (zero)".to_string(),
    };
    let heap = tx
        .budget
        .heap_size
        .map(|heap| format!("{} bytes", number(heap.into())));
    rsx! {
        div { class: "stats",
            div { class: "stat",
                span { class: "stat-label", "Size" }
                span { class: "stat-value", "{size}" }
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
            if let Some(heap) = heap {
                div { class: "stat",
                    span { class: "stat-label", "Heap" }
                    span { class: "stat-value", "{heap}" }
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
/// the priority fee, which is named when there is one. The payer is drawn
/// without its balance change, which is more than the fee: the sentences
/// say what else it pays.
#[component]
pub fn FeeLine(tx: DecodedTx) -> Element {
    let fee = match tx.budget.priority_fee.filter(|fee| *fee > 0) {
        Some(priority) => format!(
            "Fee up to {}, of which {} priority",
            format_sol(tx.fee_lamports()),
            format_sol(priority)
        ),
        None => format!("Fee up to {}", format_sol(tx.fee_lamports())),
    };
    rsx! {
        p { class: "fee-line muted",
            "{fee}"
            if let Some(payer) = tx.fee_payer() {
                " · paid by "
                Address { pubkey: payer }
            }
        }
    }
}

/// Address rows in `children` drawn without a balance change.
#[component]
pub fn WithoutDeltas(children: Element) -> Element {
    let none = use_memo(HashMap::new);
    use_context_provider(|| BalanceDeltas(none));
    rsx! { {children} }
}

#[component]
pub fn AccountList(tx: DecodedTx) -> Element {
    rsx! {
        h3 { "Accounts" }
        ul { class: "book-list tx-rows",
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
                        // The message names a table and a place in it. Which
                        // address is there is what the node says now.
                        if account.lookup.is_some() {
                            span { class: "chip warn", "from a lookup table" }
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
        // Check programs is the list's own action, at the end of its heading.
        div { class: "program-list-head",
            h3 { "Programs" }
            div { class: "program-list-check",
                if let Some(text) = check_note() {
                    span { class: "muted", "{text}" }
                }
                button { class: "sm",
                    disabled: checking() || no_idl_to_check,
                    onclick: move |_| {
                        check_many(session, check_ids.clone(), checking, check_note)
                    },
                    if checking() { "Checking…" } else { "Check programs" }
                }
            }
        }
        if let Some(text) = session.idl_pack_note() {
            p { class: "hint", "{text}" }
        }
        ul { class: "book-list tx-rows",
            for line in explained_programs {
                li { class: "book-row",
                    key: "{line.program_id}",
                    div { class: "chip-row",
                        // The chip names the program. The IDL's name is
                        // written only where nothing else names it.
                        if session.label_of(&line.program_id).is_none() {
                            if let Some(name) = line.name.clone() {
                                span { "{name}" }
                            }
                        }
                        Address { pubkey: line.program_id }
                        if let Some(sha) = line.sha256 {
                            span { class: "idl-checksum",
                                title: "The first bytes of the SHA-256 checksum of the IDL this device read the program's instructions with",
                                "IDL "
                                code { "{short_checksum(&sha)}" }
                            }
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
