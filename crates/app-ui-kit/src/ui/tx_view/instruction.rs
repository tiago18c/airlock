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
    let has_sentence = spans.is_some();

    rsx! {
        p { class: "ix-title", "{intent}" }
        if unnamed > 0 {
            p { class: "warn",
                "This instruction carries {unnamed} more account(s) than its IDL describes. The program may read a different layout, so the account names shown here may be wrong. Check the addresses in Inspect."
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
                                span { class: "chip", "signer" }
                            }
                        }
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
            let lines = intent_lines(&session, &value);
            let tip = lines.join("\n");
            let primary = value.primary.clone();
            // The scale came from bytes another device supplied, which nothing
            // here can check. Say so beside the amount, not only in the popover.
            let supplied = matches!(
                value.body,
                IntentBody::Amount {
                    scale_from: ScaleSource::Account(_),
                    ..
                }
            );
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

pub fn intent_lines(session: &Session, value: &IntentValue) -> Vec<String> {
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
