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
                        button { class: "ghost",
                            onclick: move |_| modal_open.set(false),
                            "Cancel"
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
                    }
                }
            }
        }
    }
}

/// What an entry is called in the list: what the directory calls its
/// address, else the label the book gives it in its group.
pub(in crate::ui) fn entry_name(
    described: &Described,
    entry: &wallet_core::AddressEntry,
) -> Option<String> {
    described.full_label().or_else(|| {
        let label = entry.label.trim();
        (!label.is_empty()).then(|| label.to_string())
    })
}

/// What an address is, as the line under its name: "Hot wallet · private
/// key". `None` when the line would only repeat the kind the entry is listed
/// under, whose subheading or chip already says it: "Program", "Contact".
fn kind_line(described: &Described) -> Option<String> {
    let kind = described.facts.iter().find_map(|fact| match fact {
        Fact::WalletKind(kind) => Some(kind.clone()),
        _ => None,
    });
    match (described.category, kind) {
        (category, Some(kind)) => Some(format!("{} · {kind}", category.name())),
        (
            Category::Nonce
            | Category::Program
            | Category::Validator
            | Category::Token
            | Category::Contact,
            None,
        ) => None,
        (category, None) => Some(category.name().to_string()),
    }
}

/// The name a token's mark is drawn from: its own label, without the group
/// a full label adds.
fn token_mark_name(described: &Described, entry: &wallet_core::AddressEntry) -> Option<String> {
    described
        .label
        .clone()
        .or_else(|| Some(entry.label.clone()))
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
}

/// One entry as a row of the list: its kind, name with its tags after it,
/// address, balance and actions, each in a column that lines up from row to
/// row. An owner's
/// kind line ends in a toggle for its token accounts, which open under it.
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
    let holdings_open = use_signal(|| false);
    let group_id = row.group_id.clone();
    let edit_group = group_id.clone();
    let remove_group = group_id;
    let entry = row.entry;
    let pubkey = entry.pubkey;
    let kind = entry.kind;
    let owns_tokens = matches!(kind, AddressKind::Account);
    let tags: Vec<String> = entry.tags.iter().cloned().collect();
    let tags_csv = join_tags(&entry.tags);
    let can_remove = kind != AddressKind::Program;
    let label = entry.label.clone();
    let described = (session.directory)().describe(&pubkey);
    let category = described.category;
    let collides = !described.collides_with.is_empty();
    let name = entry_name(&described, &entry);
    let what = kind_line(&described);
    let phone = crate::ui::use_phone_layout();
    // The kind line is drawn when it says something, or ends in the toggle
    // of the entry's token accounts. On a phone the toggle is on the
    // address's line instead.
    let has_holdings = owns_tokens && session.holding_summary(&pubkey).total > 0;
    let show_kind = what.is_some() || (has_holdings && !phone);
    // A token is marked by its name on a tint of its mint, else a plain "?".
    let token_mark = (category == Category::Token).then(|| {
        let mint = entry.mint.unwrap_or(pubkey);
        (mint, token_mark_name(&described, &entry))
    });
    let has_balance = entry.lamports.is_some();
    // With no network the table has no Balance column, and a row draws a
    // balance only when it has one.
    let online = crate::state::use_network().is_some();
    let address = pubkey.to_string();
    let token_amount = match (kind, entry.token_amount, entry.decimals) {
        (AddressKind::TokenAccount, Some(amount), Some(decimals)) => {
            Some(format_token_amount(amount, decimals))
        }
        _ => None,
    };
    let edit_title = match name.as_deref() {
        Some(name) => format!("Edit {name}"),
        None => "Edit".to_string(),
    };
    let remove_title = match name.as_deref() {
        Some(name) => format!("Remove {name}"),
        None => "Remove".to_string(),
    };
    // On a phone every row has one shape: the name over its kind line, the
    // balance beside it when there is one, then the address shortened to one
    // line with copy right after it, and at the right end of that line the
    // toggle of its token accounts, if any, and the three dots, where they
    // are on every row. A row with nothing to weigh
    // draws nothing there rather than a dash, and its name takes the room.
    // The three dots open a bottom sheet titled with the entry's name, as a
    // wallet card's do.
    let mut sheet_open = use_signal(|| false);
    let weighed = has_balance || token_amount.is_some();
    let row_class = if phone && !weighed {
        "ab-row ab-unweighed"
    } else {
        "ab-row"
    };
    let sheet_title = name.clone().unwrap_or_else(|| "No name".to_string());
    let (item_class, item_role) = if phone {
        ("more-item", None)
    } else {
        ("pane-menu-item", Some("menuitem"))
    };
    let row_actions = rsx! {
        button { class: item_class,
            role: item_role,
            title: "{edit_title}",
            onclick: move |_| {
                pubkey_field.set(pubkey.to_string());
                label_field.set(label.clone());
                tags_field.set(tags_csv.clone());
                error.set(None);
                editing.set(Some(pubkey));
                editing_group.set(edit_group.clone());
                modal_open.set(true);
            },
            Icon { glyph: Glyph::Pencil }
            "Edit"
        }
        if can_remove {
            button { class: "{item_class} danger",
                role: item_role,
                title: "{remove_title}",
                onclick: move |_| {
                    let group = remove_group.clone();
                    session.edit_workspace(|ws| {
                        ws.book.remove_in(&group, &pubkey);
                    });
                },
                Icon { glyph: Glyph::Trash }
                "Remove"
            }
        }
    };

    rsx! {
        li { class: row_class,
            match token_mark {
                Some((mint, Some(token))) => rsx! {
                    TokenAvatar { mint, name: token, class: "ab-badge", title: category.name() }
                },
                Some((_, None)) => rsx! {
                    span { class: "ab-badge ab-token-mark", title: category.name(), "?" }
                },
                None => rsx! {
                    span { class: "ab-badge {category_class(category)}", title: category.name(),
                        Icon { glyph: category_glyph(category) }
                    }
                },
            }
            div { class: "ab-name",
                p { class: "ab-title",
                    if let Some(name) = name.as_ref() {
                        span { class: "ab-label", title: "{name}", "{name}" }
                    } else {
                        span { class: "ab-label unnamed", "No name" }
                    }
                    if collides {
                        span { class: "addr-alert", title: "Another address has this name",
                            Icon { glyph: Glyph::Alert }
                        }
                    }
                    if !tags.is_empty() {
                        span { class: "ab-tags",
                            for tag in tags {
                                span { class: "chip ab-tag", key: "{tag}", "{tag}" }
                            }
                        }
                    }
                }
                if show_kind {
                    // The same words on a desktop and a phone, leading with
                    // the kind; it wraps where it does not fit.
                    p { class: "ab-kind",
                        if let Some(what) = what.as_ref() {
                            "{what}"
                        }
                        if has_holdings && !phone {
                            HoldingsToggle {
                                owner: pubkey,
                                open: holdings_open,
                                alone: what.is_none(),
                            }
                        }
                    }
                }
            }
            div { class: "ab-address",
                if phone {
                    // Shortened in the middle, so both ends show, with the
                    // copy icon right after it: one button that copies the
                    // whole address. Every short address is as long, so the
                    // icon lands at one x on every row.
                    CopyAddress { pubkey, class: "ab-copy-addr" }
                } else {
                    code { class: "mono", title: "{address}", "{address}" }
                }
                if let Some(hash) = entry.nonce_hash {
                    p { class: "ab-nonce mono", title: "nonce {hash}", "nonce {hash}" }
                }
            }
            if !phone {
                div { class: "ab-copy",
                    CopyButton { text: address.clone() }
                }
            }
            if (!phone && online) || weighed {
                div { class: "ab-balance",
                    if let Some(lamports) = entry.lamports {
                        p { class: "balance", "{format_sol(lamports)}" }
                    }
                    if let Some(amount) = token_amount.as_ref() {
                        p { class: "balance", "{amount}" }
                    }
                    // Nothing to weigh: a dash, so the column does not read
                    // as missing data. A balance of zero still says "0 SOL".
                    if !weighed {
                        p { class: "ab-no-balance",
                            span { aria_hidden: "true", "–" }
                            span { class: "sr-only", "No balance" }
                        }
                    }
                }
            }
            div { class: "ab-actions",
                if phone {
                    if has_holdings {
                        HoldingsToggle { owner: pubkey, open: holdings_open, alone: true }
                    }
                    button { class: "ghost icon-btn ab-more",
                        title: "More actions",
                        aria_label: "More actions",
                        aria_haspopup: "dialog",
                        onclick: move |_| sheet_open.set(true),
                        Icon { glyph: Glyph::More }
                    }
                } else {
                    PaneMenu { label: "More actions", glyph: Glyph::More, small: true,
                        {row_actions.clone()}
                    }
                }
            }
            if owns_tokens {
                OwnerHoldings { owner: pubkey, open: holdings_open }
            }
        }
        if phone {
            ActionSheet { open: sheet_open, title: sheet_title.clone(), label: "Actions of {sheet_title}",
                {row_actions}
            }
        }
    }
}
