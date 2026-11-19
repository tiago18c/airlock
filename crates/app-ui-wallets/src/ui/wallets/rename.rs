//! Renaming a wallet from its card.

use crate::state::session::Session;
use dioxus::prelude::*;
use wallet_core::WalletId;

/// A wallet's name in a field, and Save. The session gives the wallet's
/// address in the address book the same name.
#[component]
pub(in crate::ui) fn RenameWalletModal(open: Signal<bool>, id: WalletId, label: String) -> Element {
    let session = use_context::<Session>();
    let mut open = open;
    let mut text = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // The field starts from the current name each time the dialog opens.
    let current = label.clone();
    use_effect(use_reactive!(|(current)| {
        if open() {
            text.set(current.clone());
            error.set(None);
        }
    }));
    if !open() {
        return rsx! {};
    }
    let name = text().trim().to_string();
    let ready = !name.is_empty() && name != label;
    let mut save = move || {
        let name = text.peek().trim().to_string();
        if name.is_empty() {
            return;
        }
        match session.rename_wallet(id, name) {
            Ok(()) => open.set(false),
            Err(err) => error.set(Some(err.to_string())),
        }
    };
    rsx! {
        div { class: "modal-backdrop", onclick: move |_| open.set(false),
            div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                form { class: "stack",
                    onsubmit: move |evt| {
                        evt.prevent_default();
                        save();
                    },
                    h3 { "Rename {label}" }
                    label { "Name"
                        input {
                            r#type: "text",
                            value: "{text}",
                            autofocus: true,
                            onmounted: move |evt| async move {
                                let _ = evt.set_focus(true).await;
                            },
                            oninput: move |evt| text.set(evt.value()),
                        }
                    }
                    if let Some(err) = error() {
                        p { class: "error", "{err}" }
                    }
                    div { class: "btn-row",
                        button { class: "ghost", r#type: "button",
                            onclick: move |_| open.set(false),
                            "Cancel"
                        }
                        button { class: "primary", r#type: "submit", disabled: !ready, "Save" }
                    }
                }
            }
        }
    }
}
