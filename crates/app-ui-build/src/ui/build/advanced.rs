//! Advanced options: compute budget, durable nonce and the blockhash.

use super::*;

#[component]
pub(in crate::ui) fn AdvancedOptions() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let advanced_open = form.advanced_open;
    let mut cu_limit = form.cu_limit;
    let mut cu_price = form.cu_price;
    let mut loaded_accounts = form.loaded_accounts;
    let mut use_nonce = form.use_nonce;
    let mut nonce = form.nonce;
    let mut nonce_authority = form.nonce_authority;
    let mut nonce_hash = form.nonce_hash;
    let mut blockhash = form.blockhash;
    let fetching = use_signal(|| false);

    let online = crate::state::use_network().is_some();
    let nonce_accounts: Vec<_> = (session.book)()
        .iter()
        .filter(|e| e.kind == AddressKind::NonceAccount)
        .cloned()
        .collect();

    rsx! {
        Accordion {
            title: "Advanced options".to_string(),
            open: advanced_open,
                    label { "Compute unit limit"
                        input {
                            r#type: "text",
                            placeholder: if online { "blank: simulated on compile, plus 10%" } else { "omit to use the runtime default" },
                            value: "{cu_limit}",
                            oninput: move |e| cu_limit.set(e.value()),
                        }
                    }
                    label { "Priority fee (µ-lamports / CU)"
                        input {
                            r#type: "text",
                            placeholder: "omit to skip SetComputeUnitPrice",
                            value: "{cu_price}",
                            oninput: move |e| cu_price.set(e.value()),
                        }
                    }
                    label { "Loaded accounts data size (bytes)"
                        input {
                            r#type: "text",
                            placeholder: if online { "blank: simulated on compile, plus 10%" } else { "omit to use the runtime default" },
                            value: "{loaded_accounts}",
                            oninput: move |e| loaded_accounts.set(e.value()),
                        }
                    }
                    label { class: "check",
                        input {
                            r#type: "checkbox",
                            checked: use_nonce(),
                            onchange: move |e| use_nonce.set(e.checked()),
                        }
                        " Advance nonce (ix 0)"
                    }
                    if use_nonce() {
                        AddressSelect {
                            label: "Nonce account".to_string(),
                            options: nonce_accounts.iter().map(|entry| entry.pubkey).collect::<Vec<_>>(),
                            selected: nonce(),
                            none_label: "Select a nonce account".to_string(),
                            on_change: move |picked: Option<Pubkey>| {
                                nonce.set(picked);
                                let Some(pk) = picked else { return; };
                                if let Some(entry) = (session.book)().get(&pk) {
                                    if let Some(owner) = entry.owner {
                                        nonce_authority.set(Some(owner));
                                    }
                                    if let Some(hash) = entry.nonce_hash {
                                        nonce_hash.set(hash.to_string());
                                    }
                                }
                            },
                        }
                        if nonce_accounts.is_empty() {
                            p { class: "hint",
                                "No nonce accounts in the book. Refresh from chain, or initialize one as an instruction."
                            }
                        }
                        AddressPicker {
                            label: "Nonce authority".to_string(),
                            selected: nonce_authority(),
                            reset: form.picker_reset,
                            on_change: move |pk| nonce_authority.set(pk),
                        }
                        label { "Current nonce hash (base58)"
                            input {
                                r#type: "text",
                                placeholder: "Required when advancing a nonce",
                                value: "{nonce_hash}",
                                oninput: move |e| nonce_hash.set(e.value()),
                            }
                        }
                    }

                    if online {
                        label { "Recent blockhash (unused if nonce is set)"
                            input {
                                r#type: "text",
                                placeholder: "blank: fetched on compile",
                                value: "{blockhash}",
                                oninput: move |e| blockhash.set(e.value()),
                            }
                        }
                        div { class: "btn-row",
                            button {
                                disabled: fetching(),
                                onclick: move |_| fetch_lifetime(session, fetching),
                                if fetching() {
                                    "Fetching…"
                                } else if use_nonce() {
                                    "Fetch nonce"
                                } else {
                                    "Fetch blockhash"
                                }
                            }
                        }
                    } else {
                        p { class: "hint",
                            "Offline: enable Advance nonce and fill the nonce fields. Recent-blockhash compiles are blocked."
                        }
                    }
        }
    }
}
