//! One address: its row in the list, and the modal that adds or edits it.

use super::*;

#[component]
pub(in crate::ui) fn AddressModal() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.contact;
    let mut pubkey = form.pubkey;
    let mut label = form.label;
    let mut tags = form.tags;
    let mut error = form.error;
    let mut modal_open = form.modal_open;
    let editing = (form.editing)();
    let editing_group = form.editing_group;
    let mut new_kind = form.new_kind;
    // The name of a wallet's address is the wallet's own.
    let wallet = editing.and_then(|pk| session.wallet_at(&pk));
    let part_of_wallet = editing
        .filter(|_| wallet.is_none())
        .map(|pk| session.describe(&pk))
        .filter(|found| matches!(found.category, wallet_core::Category::Wallet(_)));
    let title = if editing.is_some() {
        "Edit address"
    } else {
        "Add address"
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| modal_open.set(false),
            div { class: "modal",
                onclick: move |e| e.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    p { class: "lede",
                        if editing.is_some() {
                            "Update the label and tags. The address itself stays put."
                        } else {
                            "Add a labeled contact. Signer wallets are created from the Wallets pane."
                        }
                    }
                    label { "Address"
                        input {
                            r#type: "text",
                            placeholder: "Base58 pubkey",
                            value: "{pubkey}",
                            disabled: editing.is_some(),
                            oninput: move |e| pubkey.set(e.value()),
                        }
                    }
                    if editing.is_none() {
                        label { "What it is"
                            select {
                                onchange: move |e| {
                                    new_kind.set(match e.value().as_str() {
                                        "mint" => AddressKind::Mint,
                                        "program" => AddressKind::Program,
                                        "validator" => AddressKind::Validator,
                                        _ => AddressKind::Account,
                                    });
                                },
                                option { value: "account", selected: new_kind() == AddressKind::Account, "Contact" }
                                option { value: "mint", selected: new_kind() == AddressKind::Mint, "Token mint" }
                                option { value: "program", selected: new_kind() == AddressKind::Program, "Program" }
                                option { value: "validator", selected: new_kind() == AddressKind::Validator, "Validator (vote account)" }
                            }
                        }
                    }
                    label { "Label"
                        input {
                            r#type: "text",
                            placeholder: "Alice",
                            value: "{label}",
                            disabled: part_of_wallet.is_some(),
                            oninput: move |e| label.set(e.value()),
                        }
                    }
                    if let Some(wallet) = wallet.as_ref() {
                        p { class: "hint", "This is the address of the wallet {wallet.label}. Renaming it here renames the wallet." }
                    }
                    if let Some(found) = part_of_wallet.as_ref() {
                        p { class: "hint",
                            "This address belongs to a wallet and is named after it"
                            if let Some(name) = found.label.as_ref() { ", as {name}" }
                            ". Rename the wallet instead."
                        }
                    }
                    label { "Tags (comma-separated)"
                        input {
                            r#type: "text",
                            placeholder: "ops, personal",
                            value: "{tags}",
                            oninput: move |e| tags.set(e.value()),
                        }
                    }
                    if let Some(err) = error() {
                        p { class: "error", "{err}" }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            onclick: move |_| {
                                error.set(None);
                                let label_text = label.read().trim().to_string();
                                if label_text.is_empty() && part_of_wallet.is_none() {
                                    error.set(Some("Give the address a label.".into()));
                                    return;
                                }
                                let tag_list = parse_tags(&tags());
                                if let Some(pk) = editing {
                                    let group_id = editing_group.read().clone();
                                    let renaming = wallet
                                        .as_ref()
                                        .filter(|wallet| wallet.label != label_text);
                                    if renaming.is_some() && !session.is_unlocked() {
                                        error.set(Some("A wallet's name is kept in the vault. Unlock it on the Wallets screen to rename the wallet.".into()));
                                        return;
                                    }
                                    let renamed = renaming
                                        .map(|wallet| session.rename_wallet(wallet.id, label_text.clone()));
                                    if let Some(Err(err)) = renamed {
                                        error.set(Some(err.to_string()));
                                        return;
                                    }
                                    let keep_label = part_of_wallet.is_some();
                                    if let Err(err) = session.mutate_workspace(|ws| {
                                        if !keep_label {
                                            ws.book.set_label_in(&group_id, &pk, label_text.clone());
                                        }
                                        ws.book.set_tags_in(&group_id, &pk, tag_list.clone());
                                    }) {
                                        error.set(Some(err.to_string()));
                                        return;
                                    }
                                } else {
                                    let pk = match Pubkey::from_str(pubkey.read().trim()) {
                                        Ok(pk) => pk,
                                        Err(_) => {
                                            error.set(Some("That is not a valid Solana address.".into()));
                                            return;
                                        }
                                    };
                                    if let Err(err) = session.mutate_workspace(|ws| {
                                        ws.book.insert_kind(
                                            pk,
                                            label_text.clone(),
                                            new_kind(),
                                            wallet_core::Origin::Imported,
                                            None,
                                        );
                                        ws.book.set_tags(&pk, tag_list.clone());
                                    }) {
                                        error.set(Some(err.to_string()));
                                        return;
                                    }
                                    crate::actions::network::start_refresh(session);
                                }
                                form.clear_draft();
                                modal_open.set(false);
                            },
                            if editing.is_some() { "Save" } else { "Add address" }
                        }
                        button { class: "ghost",
                            onclick: move |_| {
                                if let Some(pk) = editing {
                                    let group_id = editing_group.read().clone();
                                    if let Some(entry) = (session.book)().get_in(&group_id, &pk) {
                                        label.set(entry.label.clone());
                                        tags.set(join_tags(&entry.tags));
                                        error.set(None);
                                        return;
                                    }
                                }
                                form.clear_draft();
                            },
                            "Clear"
                        }
                        button { class: "ghost",
                            onclick: move |_| modal_open.set(false),
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn BookEntry(row: BookRow) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.contact;
    let mut modal_open = form.modal_open;
    let mut pubkey_field = form.pubkey;
    let mut label_field = form.label;
    let mut tags_field = form.tags;
    let mut error = form.error;
    let mut editing = form.editing;
    let mut editing_group = form.editing_group;
    let group_id = row.group_id.clone();
    let edit_group = group_id.clone();
    let remove_group = group_id;
    let entry = row.entry;
    let pubkey = entry.pubkey;
    let kind = entry.kind;
    let tags: Vec<String> = entry.tags.iter().cloned().collect();
    let tags_csv = join_tags(&entry.tags);
    let can_remove = kind != AddressKind::Program;
    let label = entry.label.clone();

    rsx! {
        li { class: "book-row",
            div { class: "book-head",
                AddressHeader { pubkey: Some(pubkey),
                    if let Some(lamports) = entry.lamports {
                        p { class: "balance", "{format_sol(lamports)}" }
                    }
                    if entry.kind == AddressKind::TokenAccount {
                        if let (Some(amount), Some(decimals)) = (entry.token_amount, entry.decimals) {
                            p { class: "balance", "{format_token_amount(amount, decimals)}" }
                        }
                    }
                    if let Some(hash) = entry.nonce_hash {
                        p { class: "muted mono", "nonce {hash}" }
                    }
                }
                div { class: "book-actions",
                    button { class: "sm",
                        onclick: move |_| {
                            pubkey_field.set(pubkey.to_string());
                            label_field.set(label.clone());
                            tags_field.set(tags_csv.clone());
                            error.set(None);
                            editing.set(Some(pubkey));
                            editing_group.set(edit_group.clone());
                            modal_open.set(true);
                        },
                        "Edit"
                    }
                    if can_remove {
                        button { class: "danger sm",
                            onclick: move |_| {
                                let group = remove_group.clone();
                                session.edit_workspace(|ws| {
                                    ws.book.remove_in(&group, &pubkey);
                                });
                            },
                            "Remove"
                        }
                    }
                }
            }
            if !tags.is_empty() {
                div { class: "chip-row",
                    for tag in tags {
                        span { class: "chip", key: "{tag}", "{tag}" }
                    }
                }
            }
            if matches!(kind, AddressKind::Account) {
                OwnerHoldings { owner: pubkey }
            }
        }
    }
}
