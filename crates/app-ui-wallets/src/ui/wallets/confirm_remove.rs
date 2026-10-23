//! Shared confirmation when removing a wallet from the vault.

use dioxus::prelude::*;

/// Ask before removing a wallet. When `require_label` is set, the user must
/// type the wallet's label to enable Remove. `on_confirm` says whether the
/// wallet was removed; the modal stays open when it was not.
#[component]
pub(in crate::ui) fn ConfirmRemoveModal(
    open: Signal<bool>,
    label: String,
    require_label: bool,
    lede: String,
    on_confirm: Callback<(), bool>,
) -> Element {
    let mut confirm_text = use_signal(String::new);
    let expected = label.clone();
    let confirmed = move || !require_label || confirm_text().trim() == expected.trim();
    if !open() {
        return rsx! {};
    }
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| {
                confirm_text.set(String::new());
                open.set(false);
            },
            div { class: "modal",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Remove {label}" }
                    p { class: "lede", "{lede}" }
                    if require_label {
                        label { "Type {label} to confirm"
                            input {
                                r#type: "text",
                                value: "{confirm_text}",
                                oninput: move |evt| confirm_text.set(evt.value()),
                            }
                        }
                    }
                    div { class: "btn-row",
                        button {
                            class: "danger solid",
                            disabled: !confirmed(),
                            onclick: move |_| {
                                if confirmed() && on_confirm.call(()) {
                                    confirm_text.set(String::new());
                                    open.set(false);
                                }
                            },
                            "Remove wallet"
                        }
                        button { class: "ghost",
                            onclick: move |_| {
                                confirm_text.set(String::new());
                                open.set(false);
                            },
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}
