//! The box that takes a pasted or scanned transaction, signature or note.

use super::*;

/// The paste box and the camera, one tab each. Either takes a transaction, a
/// signature or a context note.
#[component]
pub(in crate::ui) fn Inbox(loaded: bool) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut import = form.import;
    let mut error = form.error;
    let mut scanning = use_signal(|| false);
    let label = if loaded {
        "Signature, context note or another transaction (JSON, or a transaction in base64 or base58)"
    } else {
        "Transaction (base64 or base58)"
    };

    rsx! {
        div { class: "segmented", role: "group", aria_label: "How to load it",
            button {
                class: if scanning() { "tab" } else { "tab on" },
                onclick: move |_| scanning.set(false),
                "Paste"
            }
            button {
                class: if scanning() { "tab on" } else { "tab" },
                onclick: move |_| scanning.set(true),
                "Scan a QR code"
            }
        }
        if scanning() {
            QrScanner {
                on_payload: move |bytes: Vec<u8>| match decode_payload(&bytes) {
                    Ok(objects) => accept_all(session, objects, TxSource::Scanned),
                    Err(err) => error.set(Some(sentence(err))),
                },
            }
        } else {
            label { "{label}"
                textarea {
                    value: "{import}",
                    placeholder: "Paste here…",
                    oninput: move |evt| {
                        let text = evt.value();
                        // Text that reads is taken as it lands. Anything
                        // else waits for Load, which says what is wrong with it.
                        let ready = parse_payload_text(&text).ok();
                        import.set(text);
                        if let Some(objects) = ready {
                            accept_all(session, objects, TxSource::Pasted);
                        }
                    },
                }
            }
            div { class: "btn-row",
                button {
                    class: if loaded { "" } else { "primary" },
                    onclick: move |_| match parse_payload_text(&import()) {
                        Ok(objects) => accept_all(session, objects, TxSource::Pasted),
                        Err(err) => error.set(Some(sentence(err))),
                    },
                    "Load"
                }
            }
        }
    }
}
