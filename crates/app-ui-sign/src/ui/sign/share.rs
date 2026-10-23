//! The transaction, its signatures and its context note, as QR codes and text,
//! apart or all in one transfer.

use super::*;

/// What another device needs: the transaction, the signatures made here, a
/// note saying which IDLs this device read it with, or all of them at once.
#[component]
pub(in crate::ui) fn Share(tx: std::rc::Rc<DecodedTx>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut tab = form.share;
    let known = (form.vector)();
    let ctx = wallet_core::relay::AuthContext { vector: &known };
    let mut signatures = signature_objects(tx.as_ref());
    let own = signatures.len();
    signatures.extend(authority_signature_objects(tx.as_ref()));
    signatures.extend(vector_signature_objects(tx.as_ref(), &ctx));
    let count = signatures.len();
    let all_signed: Vec<QrObject> = signatures
        .iter()
        .map(|item| QrObject::Signature(item.signed.clone()))
        .collect();
    let shown = tx.clone();

    rsx! {
        div { class: "segmented", role: "group", aria_label: "What to share",
            button {
                class: if tab() == ShareTab::Transaction { "tab on" } else { "tab" },
                onclick: move |_| tab.set(ShareTab::Transaction),
                "Transaction"
            }
            button {
                class: if tab() == ShareTab::Signatures { "tab on" } else { "tab" },
                onclick: move |_| tab.set(ShareTab::Signatures),
                "Signatures ({count})"
            }
            button {
                class: if tab() == ShareTab::Context { "tab on" } else { "tab" },
                onclick: move |_| tab.set(ShareTab::Context),
                "Context note"
            }
            button {
                class: if tab() == ShareTab::Everything { "tab on" } else { "tab" },
                onclick: move |_| tab.set(ShareTab::Everything),
                "Everything"
            }
        }
        match tab() {
            ShareTab::Transaction => rsx! {
                TransactionShare { tx: shown.clone() }
            },
            ShareTab::Signatures => rsx! {
                if signatures.is_empty() {
                    p { class: "hint",
                        "Nothing is signed yet. Each signature made here gets its own code, which carries the signer's address so the other device knows where it goes."
                    }
                } else {
                    p { class: "hint",
                        "Each is one signature and its signer's address. The device that holds the transaction adds them one at a time, or all at once from the first code."
                    }
                }
                if count > 1 {
                    div { class: "card",
                        div { class: "chip-row",
                            strong { "All {count} signatures" }
                            span { class: "chip", "one transfer" }
                        }
                        PayloadShare { objects: all_signed.clone(), short: true }
                    }
                }
                for (i, item) in signatures.into_iter().enumerate() {
                    SignatureCard {
                        key: "{item.pubkey}-{i}",
                        pubkey: item.pubkey,
                        signed: item.signed,
                        authorization: i >= own,
                    }
                }
            },
            ShareTab::Context => rsx! {
                ContextShare { tx: shown.clone() }
            },
            ShareTab::Everything => rsx! {
                EverythingShare { tx: shown.clone(), signatures: all_signed.clone() }
            },
        }
    }
}

/// The transaction, and in the same codes its context note unless left out.
/// The text is the transaction alone, as other tools read one.
#[component]
fn TransactionShare(tx: std::rc::Rc<DecodedTx>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut with_note = use_signal(|| true);
    let note = use_memo(use_reactive!(|(tx)| {
        let known = form.vector.peek().clone();
        context_note_for(&*session.idls.read(), &tx, &known).map_err(|err| err.to_string())
    }))();
    let mut objects = vec![QrObject::Transaction(tx.wire.clone())];
    if with_note() {
        if let Ok(context) = &note {
            objects.push(QrObject::Context(context.clone()));
        }
    }
    let tx_b64 = STANDARD.encode(&tx.wire);
    rsx! {
        p { class: "hint",
            "The whole transaction, with every signature it has so far. The other device loads it on its Sign screen."
        }
        label { class: "check",
            input {
                r#type: "checkbox",
                checked: with_note(),
                disabled: note.is_err(),
                onchange: move |_| with_note.set(!with_note()),
            }
            "With its context note, in the same codes"
        }
        if let Err(err) = &note {
            p { class: "error", "{err}" }
        }
        QrCarousel { objects }
        label { "Base64"
            textarea { readonly: true, value: "{tx_b64}" }
        }
        div { class: "btn-row",
            CopyButton { text: tx_b64.clone() }
        }
    }
}

/// The transaction, its context note and every signature, in one transfer
/// and one JSON array. The wire already carries the signatures; they ride
/// along as objects of their own for a device that reads them apart.
#[component]
fn EverythingShare(tx: std::rc::Rc<DecodedTx>, signatures: Vec<QrObject>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let note = use_memo(use_reactive!(|(tx)| {
        let known = form.vector.peek().clone();
        context_note_for(&*session.idls.read(), &tx, &known).map_err(|err| err.to_string())
    }))();
    let mut objects = vec![QrObject::Transaction(tx.wire.clone())];
    if let Ok(context) = &note {
        objects.push(QrObject::Context(context.clone()));
    }
    objects.extend(signatures);
    rsx! {
        p { class: "hint",
            "The transaction, its context note and every signature it has so far, in one transfer. The other device loads it on its Sign screen and takes each part in turn."
        }
        if let Err(err) = &note {
            p { class: "error", "Sent without its context note: {err}" }
        }
        PayloadShare { objects }
    }
}

#[component]
pub(in crate::ui) fn SignatureCard(pubkey: Pubkey, signed: Signed, authorization: bool) -> Element {
    let objects = vec![QrObject::Signature(signed)];
    rsx! {
        div { class: "card",
            div { class: "chip-row",
                Address { pubkey }
                if authorization {
                    span { class: "chip kind", "authorization" }
                } else {
                    span { class: "chip", "transaction signature" }
                }
            }
            PayloadShare { objects, short: true }
        }
    }
}

#[component]
pub(in crate::ui) fn ContextShare(tx: std::rc::Rc<DecodedTx>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let known = (form.vector)();
    let authorizations = known.len();
    let built = use_memo(use_reactive!(|(tx)| {
        let known = form.vector.peek().clone();
        context_note_for(&*session.idls.read(), &tx, &known)
            .map_err(|err| err.to_string())
            .map(|context| vec![QrObject::Context(context)])
    }))();
    rsx! {
        p { class: "hint",
            if authorizations > 0 {
                "Checksums of the IDLs this device read the transaction with, and the nonce of each Vector account it lists. The other device needs the note to place and sign those authorizations. A note is never signed."
            } else {
                "Checksums of the IDLs this device read the transaction with. The other device compares them with its own, after it has loaded the transaction. A note is never signed."
            }
        }
        p { class: "hint",
            "The Transaction tab sends the note in the same codes. Show it here on its own for a device that already has the transaction."
        }
        match built {
            Ok(objects) => rsx! {
                PayloadShare { objects, short: true }
            },
            Err(err) => rsx! { p { class: "error", "{err}" } },
        }
    }
}
