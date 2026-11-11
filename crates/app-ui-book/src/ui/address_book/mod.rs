use crate::actions::addresses::*;
use crate::actions::network::refresh_from_chain;
use crate::state::session::Session;
use crate::ui::{
    category_class, category_glyph, format_sol, format_token_amount, ActionSheet, Address,
    AddressRow, CopyAddress, CopyButton, Glyph, Icon, PaneMenu, RefreshButton,
};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::{
    AddressKind, BookGroupInfo, BookRow, Category, Described, Fact, Group, TokenFlag, TokenHolding,
    WalletSort, LOCAL_GROUP_ID,
};

mod entry;
mod groups;
mod holdings;
mod transfer;

pub(in crate::ui) use entry::*;
pub(in crate::ui) use groups::*;
pub(in crate::ui) use holdings::*;
pub(in crate::ui) use transfer::*;

const PAGE_SIZE: usize = 25;

/// One line of the list: a kind's subheading, or an entry.
enum ListItem {
    Heading { name: &'static str, count: usize },
    Entry(Box<BookRow>),
}

/// Where a kind comes in the list with every kind shown: the order of the
/// chips, and what has no kind last.
fn group_rank(group: Option<Group>) -> usize {
    group
        .and_then(|group| Group::ALL.iter().position(|of| *of == group))
        .unwrap_or(Group::ALL.len())
}

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
    // On a phone the header keeps only the title, Add address and the three
    // lines: Refresh joins Export and Import in a bottom sheet,
    // as the Wallets pane's menu does, and a line under the header says
    // while it runs, since the sheet is closed by then.
    let phone = crate::ui::use_phone_layout();
    let mut pane_sheet_open = use_signal(|| false);
    let refresh = move |_: Event<MouseData>| {
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
    };
    let (item_class, item_role) = if phone {
        ("more-item", None)
    } else {
        ("pane-menu-item", Some("menuitem"))
    };
    let pane_actions = rsx! {
        button { class: item_class,
            role: item_role,
            title: "Write this device's address book out, for another device to import",
            onclick: move |_| export_open.set(true),
            Icon { glyph: Glyph::Export }
            "Export"
        }
        button { class: item_class,
            role: item_role,
            title: "Paste another device's export, as a group of its own",
            onclick: move |_| {
                import_error.set(None);
                import_open.set(true);
            },
            Icon { glyph: Glyph::Import }
            "Import"
        }
    };

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Address Book" }
                    }
                    div { class: "btn-row",
                        if online {
                            RefreshButton {
                                busy: fetching(),
                                title: "Read balances, token accounts and nonce hashes again",
                                onclick: refresh,
                            }
                        }
                        button { class: "primary",
                            onclick: move |_| {
                                form.clear_draft();
                                modal_open.set(true);
                            },
                            Icon { glyph: Glyph::Plus }
                            "Add address"
                        }
                        if phone {
                            button { class: "icon-btn",
                                title: "More address book actions",
                                aria_label: "More address book actions",
                                aria_haspopup: "dialog",
                                onclick: move |_| pane_sheet_open.set(true),
                                Icon { glyph: Glyph::Menu }
                            }
                        } else {
                            PaneMenu { label: "More address book actions", {pane_actions.clone()} }
                        }
                    }
                }

                if phone && fetching() {
                    p { class: "hint pane-refreshing", role: "status", "Refreshing…" }
                }
                if !online {
                    p { class: "hint", "Offline: this app never connects, so it looks up no balances, token accounts or nonce hashes." }
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
        ActionSheet { open: pane_sheet_open, title: "Address Book", label: "More address book actions",
            {pane_actions}
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
    let online = crate::state::use_network().is_some();
    let form = session.forms.contact;
    let mut filter = form.filter;
    let mut kind_filter = form.kind;
    let mut page = form.page;

    let group_id = form.group;
    let directory = (session.directory)();
    let wanted = kind_filter();
    let grouped = wanted.is_none();
    // Each entry of the group and text filter with the kind it is listed
    // under.
    let matching: Vec<(Option<Group>, BookRow)> = (session.book)()
        .rows_in(&group_id(), &filter(), None)
        .into_iter()
        .map(|row| {
            let group = directory.describe(&row.entry.pubkey).category.group();
            (group, row)
        })
        .collect();
    // How many of them each chip would show.
    let all_count = matching.len();
    let chip_counts: Vec<(Group, usize)> = Group::ALL
        .iter()
        .map(|of| {
            let count = matching
                .iter()
                .filter(|(group, _)| *group == Some(*of))
                .count();
            (*of, count)
        })
        .collect();
    let mut entries: Vec<(Option<Group>, BookRow)> = matching
        .into_iter()
        .filter(|(group, _)| grouped || *group == wanted)
        .collect();
    // With every kind listed, by kind in the order of the chips; then by
    // name, ignoring case; the unnamed last, by address.
    entries.sort_by_cached_key(|(group, row)| {
        let rank = if grouped { group_rank(*group) } else { 0 };
        let name = entry_name(&directory.describe(&row.entry.pubkey), &row.entry);
        match name {
            Some(name) => (rank, false, name.to_lowercase(), String::new()),
            None => (rank, true, String::new(), row.entry.pubkey.to_string()),
        }
    });
    let collisions = directory.collisions();
    let total = entries.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let page_i = page().min(pages - 1);
    let start = page_i * PAGE_SIZE;
    let counts: Vec<(Option<Group>, usize)> = if grouped {
        let mut counts: Vec<(Option<Group>, usize)> = Vec::new();
        for (group, _) in &entries {
            match counts.last_mut() {
                Some((last, count)) if last == group => *count += 1,
                _ => counts.push((*group, 1)),
            }
        }
        counts
    } else {
        Vec::new()
    };
    let shown: Vec<(Option<Group>, BookRow)> =
        entries.into_iter().skip(start).take(PAGE_SIZE).collect();
    let from = if total == 0 { 0 } else { start + 1 };
    let to = (start + shown.len()).min(total);
    // The page's rows, each kind's first under its subheading: the kind's
    // name and how many entries of it the whole list holds.
    let mut items: Vec<ListItem> = Vec::with_capacity(shown.len() + counts.len());
    let mut previous: Option<Option<Group>> = None;
    for (group, row) in shown {
        if grouped && previous != Some(group) {
            let count = counts
                .iter()
                .find(|(of, _)| *of == group)
                .map_or(0, |(_, count)| *count);
            items.push(ListItem::Heading {
                name: group.map_or("Other", Group::name),
                count,
            });
        }
        previous = Some(group);
        items.push(ListItem::Entry(Box::new(row)));
    }

    rsx! {
        div { class: "stack",
            GroupBar {}
            div { class: "filter-bar",
                // The placeholder says what to type; the label is for screen
                // readers.
                label {
                    span { class: "sr-only", "Filter" }
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
                // On a phone the chips are one row that scrolls sideways.
                div { class: "chip-row ab-chips",
                    button {
                        class: if kind_filter().is_none() { "tab on" } else { "tab" },
                        onclick: move |_| {
                            kind_filter.set(None);
                            page.set(0);
                        },
                        "All"
                        if all_count > 0 {
                            " "
                            span { class: "ab-chip-count", "{all_count}" }
                        }
                    }
                    // Each kind with how many entries of it the group and the
                    // text filter leave; none when it has none.
                    for (group, count) in chip_counts {
                        button {
                            class: if kind_filter() == Some(group) { "tab on" } else { "tab" },
                            key: "{group.name()}",
                            onclick: move |_| {
                                kind_filter.set(Some(group));
                                page.set(0);
                            },
                            "{group.name()}"
                            if count > 0 {
                                " "
                                span { class: "ab-chip-count", "{count}" }
                            }
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

            if total > 0 {
                // One card: the column labels as its first row, then the
                // entries.
                // With no network there is no balance to show, so there is
                // no Balance column: the others take its room.
                div { class: if online { "ab-table" } else { "ab-table no-balance" },
                    // What each column holds, in the columns of the rows.
                    // Hidden where the rows wrap.
                    div { class: "ab-columns", aria_hidden: "true",
                        span { class: "ab-col-name", "Name" }
                        span { class: "ab-col-address", "Address" }
                        if online {
                            span { class: "ab-col-balance", "Balance" }
                        }
                    }
                    ul { class: "book-list ab-list",
                        for item in items {
                            match item {
                                ListItem::Heading { name, count } => rsx! {
                                    li { class: "ab-kind-head", key: "kind:{name}",
                                        span { class: "ab-kind-head-name", "{name}" }
                                        span { class: "ab-kind-head-count", "{count}" }
                                    }
                                },
                                ListItem::Entry(row) => rsx! {
                                    BookEntry {
                                        key: "{row.group_id}:{row.entry.pubkey}",
                                        row: *row,
                                    }
                                },
                            }
                        }
                    }
                }
            }

            // Only with more than one page: each kind's subheading already
            // carries its count.
            if pages > 1 {
                div { class: "pager ab-pager",
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
