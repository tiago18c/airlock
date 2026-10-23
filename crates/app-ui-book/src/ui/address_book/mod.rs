use crate::actions::addresses::*;
use crate::actions::network::refresh_from_chain;
use crate::state::session::Session;
use crate::ui::{format_sol, format_token_amount, Address, AddressHeader, AddressRow, Glyph, Icon};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::{
    AddressKind, BookGroupInfo, BookRow, Group, TokenFlag, TokenHolding, LOCAL_GROUP_ID,
};

mod entry;
mod groups;
mod holdings;
mod transfer;

pub(in crate::ui) use entry::*;
pub(in crate::ui) use groups::*;
pub(in crate::ui) use holdings::*;
pub(in crate::ui) use transfer::*;

const PAGE_SIZE: usize = 10;

#[component]
pub fn AddressBookPanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.contact;
    let mut modal_open = form.modal_open;
    let online = crate::state::use_network().is_some();
    let mut fetching = use_signal(|| false);
    let mut fetch_note = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut export_open = use_signal(|| false);
    let mut import_open = use_signal(|| false);
    let export_label = use_signal(String::new);
    let import_text = use_signal(String::new);
    let mut import_error = use_signal(|| None::<String>);

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Address Book" }
                    }
                    div { class: "btn-row",
                        button {
                            onclick: move |_| export_open.set(true),
                            "Export"
                        }
                        button {
                            onclick: move |_| {
                                import_error.set(None);
                                import_open.set(true);
                            },
                            "Import"
                        }
                        button { class: "primary",
                            onclick: move |_| {
                                form.clear_draft();
                                modal_open.set(true);
                            },
                            Icon { glyph: Glyph::Plus }
                            "Add address"
                        }
                    }
                }

                if online {
                    div { class: "btn-row",
                        button {
                            disabled: fetching(),
                            onclick: move |_| {
                                error.set(None);
                                fetch_note.set(None);
                                fetching.set(true);
                                spawn(async move {
                                    match refresh_from_chain(session).await {
                                        Ok(note) => fetch_note.set(Some(note)),
                                        Err(err) => error.set(Some(err)),
                                    }
                                    fetching.set(false);
                                });
                            },
                            if fetching() { "Refreshing…" } else { "Refresh from chain" }
                        }
                    }
                } else {
                    p { class: "hint", "Offline: token accounts and nonce hashes stay local until getGenesisHash succeeds." }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                if let Some(note) = fetch_note() {
                    p { class: "note", "{note}" }
                }

                AddressBookList {}
            }
        }
        if modal_open() {
            AddressModal {}
        }
        if export_open() {
            ExportModal { open: export_open, label: export_label }
        }
        if import_open() {
            ImportModal { open: import_open, text: import_text, error: import_error }
        }
    }
}

#[component]
fn AddressBookList() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.contact;
    let mut filter = form.filter;
    let mut kind_filter = form.kind;
    let mut page = form.page;

    let group_id = form.group;
    let directory = (session.directory)();
    let wanted = kind_filter();
    let entries: Vec<BookRow> = (session.book)()
        .rows_in(&group_id(), &filter(), None)
        .into_iter()
        .filter(|row| {
            wanted.is_none() || directory.describe(&row.entry.pubkey).category.group() == wanted
        })
        .collect();
    let collisions = directory.collisions();
    let total = entries.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let page_i = page().min(pages - 1);
    let start = page_i * PAGE_SIZE;
    let shown: Vec<BookRow> = entries.into_iter().skip(start).take(PAGE_SIZE).collect();
    let from = if total == 0 { 0 } else { start + 1 };
    let to = (start + shown.len()).min(total);

    rsx! {
        div { class: "book-layout",
            GroupSidebar {}
            div { class: "stack",
            div { class: "filter-bar",
                label { "Filter"
                    input {
                        r#type: "search",
                        placeholder: "Address, label, or tag",
                        value: "{filter}",
                        oninput: move |e| {
                            filter.set(e.value());
                            page.set(0);
                        },
                    }
                }
                div { class: "chip-row",
                    button {
                        class: if kind_filter().is_none() { "tab on" } else { "tab" },
                        onclick: move |_| {
                            kind_filter.set(None);
                            page.set(0);
                        },
                        "All"
                    }
                    for group in Group::ALL {
                        button {
                            class: if kind_filter() == Some(group) { "tab on" } else { "tab" },
                            key: "{group.name()}",
                            onclick: move |_| {
                                kind_filter.set(Some(group));
                                page.set(0);
                            },
                            "{group.name()}"
                        }
                    }
                }
            }

            if !collisions.is_empty() {
                div { class: "callout warn-callout", role: "status",
                    p {
                        Icon { glyph: Glyph::Alert }
                        " Some names are used for more than one address. Wherever they appear, those addresses are marked until one is renamed."
                    }
                    for (label, addresses) in collisions {
                        p { class: "chip-row", key: "{label}",
                            strong { "{label}" }
                            for pubkey in addresses {
                                Address { key: "{pubkey}", pubkey }
                            }
                        }
                    }
                }
            }

            if total == 0 {
                div { class: "empty",
                    Icon { glyph: Glyph::Book }
                    p { "No addresses match this filter." }
                }
            }

            ul { class: "book-list",
                for row in shown {
                    BookEntry {
                        key: "{row.group_id}:{row.entry.pubkey}",
                        row,
                    }
                }
            }

            if total > 0 {
                div { class: "pager",
                    button {
                        disabled: page_i == 0,
                        onclick: move |_| page.set(page_i.saturating_sub(1)),
                        "Previous"
                    }
                    p { class: "muted", "{from}–{to} of {total}" }
                    button {
                        disabled: page_i + 1 >= pages,
                        onclick: move |_| page.set(page_i + 1),
                        "Next"
                    }
                }
            }
            }
        }
    }
}
