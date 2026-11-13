//! One instruction, in words when an IDL reads it.

use super::*;

#[component]
pub fn IdlInstruction(idl: IdlExplanation, data: Vec<u8>, inspect: bool) -> Element {
    let hex = hex::encode(&data);
    let spans = idl.spans.clone();
    let accounts = idl.accounts.clone();
    // The IDL's fields name its accounts too, as bare addresses. Those are
    // drawn below as rows, so only the arguments are listed here.
    let fields: Vec<_> = idl
        .fields
        .iter()
        .filter(|field| {
            !accounts
                .iter()
                .any(|account| account.pubkey.to_string() == field.value)
        })
        .cloned()
        .collect();
    let intent = idl.intent.clone();
    let unnamed = idl.unnamed_accounts;
    let undescribed = idl.undescribed_bytes.len();
    let undescribed_hex = hex::encode(&idl.undescribed_bytes);
    let has_sentence = spans.is_some();

    rsx! {
        p { class: "ix-title", "{intent}" }
        if unnamed > 0 {
            p { class: "warn",
                "This instruction carries {unnamed} more account(s) than its IDL describes. The program may read a different layout, so the account names shown here may be wrong. Check the addresses in Inspect."
            }
        }
        if undescribed > 0 {
            p { class: "warn",
                "This instruction's data is {undescribed} byte(s) longer than its IDL describes: {undescribed_hex}. If the program reads them, they say something that is not shown here."
            }
        }
        if let Some(spans) = spans {
            IntentSentence { spans }
        }
        if (inspect || !has_sentence) && !fields.is_empty() {
            ul { class: "idl-lines",
                for field in fields {
                    li { key: "{field.label}", "{field.label} · {field.value}" }
                }
            }
        }
        // Without a sentence, the accounts are what says what it touches.
        if inspect || !has_sentence {
            AccountRowDeltas { inspect,
                ul { class: "ix-accounts",
                    for account in accounts {
                        li { class: "chip-row", key: "{account.name}-{account.pubkey}",
                            span { class: "muted ix-account-name", "{account.name}" }
                            AddressRow { pubkey: account.pubkey, full: true,
                                if account.writable {
                                    span { class: "chip", "writable" }
                                } else {
                                    span { class: "chip", "readonly" }
                                }
                                if account.signer {
                                    span { class: "chip kind", "signer" }
                                }
                            }
                        }
                    }
                }
            }
        }
        if inspect {
            DataBytes { hex }
        }
    }
}

/// An instruction's account rows. In Inspect they are drawn without a balance
/// change: an account's change is in the Accounts list, and would repeat on
/// every row it fills. In Summary, rows stand in for a sentence that would
/// say what moves, so they keep it.
#[component]
pub fn AccountRowDeltas(inspect: bool, children: Element) -> Element {
    if inspect {
        rsx! {
            WithoutDeltas { {children} }
        }
    } else {
        rsx! { {children} }
    }
}

/// An instruction's raw data, as hex behind a "Data" disclosure that starts
/// closed. Nothing is drawn for an instruction with no data.
#[component]
pub fn DataBytes(hex: String) -> Element {
    rsx! {
        if !hex.is_empty() {
            details { class: "ix-data",
                summary { "Data" }
                code { class: "mono", "{hex}" }
            }
        }
    }
}

#[component]
pub fn IntentSentence(spans: Vec<IntentSpan>) -> Element {
    rsx! {
        p { class: "intent-sentence",
            for (i, span) in spans.into_iter().enumerate() {
                IntentPiece { key: "{i}", span }
            }
        }
    }
}

#[component]
pub fn IntentPiece(span: IntentSpan) -> Element {
    let session = use_context::<Session>();
    let open = use_signal(|| false);
    // The accounts this device's own node gave the reading above.
    let fetched = try_use_context::<FetchedForReading>().map(|held| (held.0)());
    let from_node = |pk: &Pubkey| fetched.as_ref().is_some_and(|held| held.contains_key(pk));
    match span {
        IntentSpan::Text(text) => rsx! { span { "{text}" } },
        // An address reads as it does everywhere else: what it is, its name,
        // and its short form, with the whole address on the card.
        IntentSpan::Value(IntentValue {
            body: IntentBody::Address(pubkey),
            ..
        }) => rsx! { Address { pubkey } },
        IntentSpan::Value(value) if matches!(value.body, IntentBody::Text(_)) => {
            let text = value.primary;
            rsx! { span { "{text}" } }
        }
        IntentSpan::Value(value) => {
            let lines = intent_lines(&session, &value, &from_node);
            let tip = lines.join("\n");
            let primary = value.primary.clone();
            // The scale came from bytes another device supplied, which nothing
            // here can check. Say so beside the amount, not only in the popover.
            // Bytes this device's own node gave are as good as its simulation.
            let supplied = match &value.body {
                IntentBody::Amount {
                    scale_from: ScaleSource::Account(pk),
                    ..
                } => !from_node(pk),
                _ => false,
            };
            rsx! {
                span { class: "intent-value",
                    button {
                        class: if supplied { "intent-hit supplied" } else { "intent-hit" },
                        title: "{tip}",
                        onclick: move |_| {
                            let mut flag = open;
                            flag.set(!flag());
                        },
                        "{primary}"
                    }
                    if supplied {
                        span { class: "chip warn", "unverified scale" }
                    }
                    if open() {
                        div { class: "intent-pop",
                            for (i, line) in lines.iter().enumerate() {
                                p { key: "{i}", "{line}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// What the popover of a value says. `from_node` tells an account this
/// device's own node gave from one another device supplied.
pub fn intent_lines(
    session: &Session,
    value: &IntentValue,
    from_node: &dyn Fn(&Pubkey) -> bool,
) -> Vec<String> {
    match &value.body {
        // Drawn by `Address`, which has its own card.
        IntentBody::Address(pk) => vec![pk.to_string()],
        IntentBody::Amount {
            raw,
            decimals,
            unit,
            scale_from,
        } => {
            let mut lines = vec![value.primary.clone(), format!("raw {raw}")];
            if let Some(decimals) = decimals {
                lines.push(format!("{decimals} decimals"));
            }
            if let Some(unit) = unit {
                lines.push(unit.clone());
            }
            lines.push(match scale_from {
                ScaleSource::Instruction => "scale from the instruction".into(),
                ScaleSource::Account(pk) if from_node(pk) => {
                    format!(
                        "scale read from {} on the node",
                        crate::ui::address::name_or_short(session, pk)
                    )
                }
                ScaleSource::Account(pk) => {
                    format!(
                        "scale supplied from {}",
                        crate::ui::address::name_or_short(session, pk)
                    )
                }
                ScaleSource::Missing => "scale missing".into(),
            });
            lines
        }
        IntentBody::Text(text) => vec![text.clone()],
    }
}
