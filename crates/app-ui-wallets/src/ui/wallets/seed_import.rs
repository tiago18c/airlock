//! Existing wallet: a seed phrase typed in, and the accounts derived from it.

use super::*;

#[component]
pub(in crate::ui) fn SeedImportFields(state: ModalState, accounts: Memo<SeedAccounts>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.wallet;
    let mut secret = form.secret;
    let mut passphrase = form.passphrase;
    let mut path_template = form.path_template;
    let mut error = form.error;
    let mut path_custom = state.path_custom;
    let generation = (state.generation)();
    let tail = (state.tail)();
    let rows = shown_seed_rows(&accounts().rows, &tail, generation);

    rsx! {
        label { "Seed phrase"
            textarea {
                placeholder: "12 or 24 words",
                value: "{secret}",
                oninput: move |e| {
                    secret.set(e.value());
                    error.set(None);
                    invalidate_seed(state.tail, state.generation);
                },
            }
        }
        label { "Passphrase (optional)"
            input {
                r#type: "password",
                value: "{passphrase}",
                oninput: move |e| {
                    passphrase.set(e.value());
                    error.set(None);
                    invalidate_seed(state.tail, state.generation);
                },
            }
        }
        label { "Derivation path"
            select {
                onchange: move |evt| {
                    let value = evt.value();
                    error.set(None);
                    invalidate_seed(state.tail, state.generation);
                    if value == "custom" {
                        path_custom.set(true);
                    } else {
                        path_custom.set(false);
                        path_template.set(value);
                    }
                },
                for (path, name) in PATH_PRESETS {
                    option {
                        value: "{path}",
                        selected: !path_custom() && path_template() == *path,
                        "{name} ({path})"
                    }
                }
                option {
                    value: "custom",
                    selected: path_custom(),
                    "Custom"
                }
            }
        }
        if path_custom() {
            label { "Custom path"
                input {
                    r#type: "text",
                    placeholder: "m/44'/501'/*'/0'",
                    value: "{path_template}",
                    oninput: move |evt| {
                        path_template.set(evt.value());
                        error.set(None);
                        invalidate_seed(state.tail, state.generation);
                    },
                }
            }
        }
        if !rows.is_empty() {
            ul { class: "derive-list",
                for row in rows {
                    li { class: "derive-row", key: "{row.path}",
                        input {
                            r#type: "checkbox",
                            checked: row.checked,
                            onchange: move |_| {
                                toggle_seed(state.tail, *state.generation.peek(), row.index)
                            },
                        }
                        div {
                            code { class: "mono", "{row.path}" }
                            div { class: "addr-full",
                                code { class: "mono", "{row.pubkey}" }
                                CopyButton { text: row.pubkey.to_string() }
                            }
                        }
                    }
                }
            }
            if path_template.read().contains('*') {
                button {
                    onclick: move |_| {
                        let generation = *state.generation.peek();
                        let start = next_seed_index(
                            accounts.peek().rows.len(),
                            &state.tail.peek(),
                            generation,
                        );
                        match derive_batch(
                            &secret.read(),
                            &passphrase.read(),
                            path_template.read().trim(),
                            start,
                            DERIVE_BATCH,
                        ) {
                            Ok(more) if !more.is_empty() => {
                                push_seed_rows(state.tail, generation, more);
                                error.set(None);
                            }
                            Ok(_) => error.set(Some("This path has no * to continue from.".into())),
                            Err(err) => error.set(Some(err)),
                        }
                    },
                    "Derive more"
                }
            }
        }
    }
}
