//! An instruction written by hand: its program, each account with its
//! flags, and its data as hex.

use super::*;

#[component]
pub(in crate::ui) fn RawFields(draft: IxDraft) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let mut unread = use_signal(|| false);
    let id = draft.id;
    let IxBody::Raw(raw) = draft.body.clone() else {
        return rsx! {};
    };
    let has_idl = raw.program_id.is_some_and(|program_id| {
        session
            .idl_records()
            .iter()
            .any(|record| record.program_id == program_id)
    });
    let bump = move || {
        let mut reset = form.picker_reset;
        reset.set(reset() + 1);
    };
    let read_raw = raw.clone();

    rsx! {
        p { class: "hint",
            "Accounts are listed in the order the program reads them. Data is hex."
        }
        AddressPicker {
            label: "Program".to_string(),
            selected: raw.program_id,
            reset: form.picker_reset,
            on_change: move |pk| {
                unread.set(false);
                form.patch_raw(id, |d| d.program_id = pk);
            },
        }
        if has_idl {
            div { class: "btn-row",
                button { class: "ghost sm",
                    r#type: "button",
                    onclick: move |_| match read_with_idl(&session, &read_raw) {
                        Some(idl) => {
                            form.patch(id, |d| d.body = IxBody::Idl(idl));
                            bump();
                        }
                        None => unread.set(true),
                    },
                    "Read with its IDL"
                }
                if unread() {
                    span { class: "hint",
                        "Its IDL does not read these accounts and bytes back to the same instruction."
                    }
                }
            }
        }
        div { class: "raw-accounts",
            for (i, account) in raw.accounts.iter().cloned().enumerate() {
                div { class: "raw-account", key: "{id}-{i}",
                    AddressPicker {
                        label: format!("Account {}", i + 1),
                        selected: account.pubkey,
                        reset: form.picker_reset,
                        on_change: move |pk| {
                            form.patch_raw(id, |d| {
                                if let Some(slot) = d.accounts.get_mut(i) {
                                    slot.pubkey = pk;
                                }
                            });
                        },
                    }
                    div { class: "btn-row",
                        label { class: "check",
                            input {
                                r#type: "checkbox",
                                checked: account.signer,
                                onchange: move |e| {
                                    let on = e.checked();
                                    form.patch_raw(id, |d| {
                                        if let Some(slot) = d.accounts.get_mut(i) {
                                            slot.signer = on;
                                        }
                                    });
                                },
                            }
                            " signer"
                        }
                        label { class: "check",
                            input {
                                r#type: "checkbox",
                                checked: account.writable,
                                onchange: move |e| {
                                    let on = e.checked();
                                    form.patch_raw(id, |d| {
                                        if let Some(slot) = d.accounts.get_mut(i) {
                                            slot.writable = on;
                                        }
                                    });
                                },
                            }
                            " writable"
                        }
                        button { class: "ghost sm",
                            r#type: "button",
                            onclick: move |_| {
                                form.patch_raw(id, |d| {
                                    if i < d.accounts.len() {
                                        d.accounts.remove(i);
                                    }
                                });
                                // The pickers below now show other accounts.
                                bump();
                            },
                            "Remove"
                        }
                    }
                }
            }
        }
        div { class: "btn-row",
            button { class: "ghost sm",
                r#type: "button",
                onclick: move |_| {
                    form.patch_raw(id, |d| d.accounts.push(RawAccountDraft::default()));
                },
                "Add account"
            }
        }
        label { "Data (hex)"
            textarea {
                class: "mono",
                rows: 3,
                spellcheck: false,
                placeholder: "such as 02000000e803000000000000",
                value: "{raw.data}",
                oninput: move |e| {
                    let value = e.value();
                    form.patch_raw(id, |d| d.data = value);
                },
            }
        }
    }
}
