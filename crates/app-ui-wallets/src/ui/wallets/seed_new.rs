//! Create new: a seed phrase made on this device, shown once to be written down.

use super::*;

#[component]
pub(in crate::ui) fn NewSeedFields(state: ModalState) -> Element {
    let session = use_context::<Session>();
    let mut secret = session.forms.wallet.secret;
    let mut written = state.written;
    let mut words = state.words;

    rsx! {
        p { class: "note",
            "Write these words down in order and keep them somewhere safe. They are the only backup of this wallet, anyone who has them controls it, and the app does not show them again."
        }
        ol { class: "seed-words",
            for (i, word) in secret().split_whitespace().enumerate() {
                li { key: "{i}",
                    span { class: "muted", "{i + 1}" }
                    span { class: "mono", "{word}" }
                }
            }
        }
        div { class: "btn-row",
            div { class: "segmented",
                for count in [12usize, 24] {
                    button {
                        key: "{count}",
                        class: if words() == count { "tab on" } else { "tab" },
                        onclick: move |_| {
                            if words() != count {
                                words.set(count);
                                secret.write().zeroize();
                                secret.set(wallet_core::new_phrase(count));
                                written.set(false);
                            }
                        },
                        "{count} words"
                    }
                }
            }
            button { class: "sm",
                onclick: move |_| {
                    secret.write().zeroize();
                    secret.set(wallet_core::new_phrase(words()));
                    written.set(false);
                },
                "Generate again"
            }
        }
        label { class: "check",
            input {
                r#type: "checkbox",
                checked: written(),
                onchange: move |_| written.set(!written()),
            }
            "I wrote the words down"
        }
    }
}
