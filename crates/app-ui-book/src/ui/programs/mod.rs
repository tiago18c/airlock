use crate::actions::programs::*;
use crate::state::forms::IdlSection;
use crate::state::session::Session;
use crate::ui::Empty;
use crate::ui::{
    short_checksum, use_phone_layout, AddressHeader, AddressRow, CopyButton, Glyph, Icon, PaneMenu,
};
use dioxus::prelude::*;
use wallet_core::{
    pretty_idl, IdlAccountUse, IdlIxOutline, IdlNamed, IdlOrigin, IdlOutline, IdlPinStatus,
    IdlRecord, IdlSource, IdlSpec,
};

mod detail;
mod parsed;

pub(in crate::ui) use detail::*;
pub(in crate::ui) use parsed::*;

#[component]
pub fn ProgramsPanel() -> Element {
    let session = use_context::<Session>();
    let note = use_signal(|| None::<String>);
    let error = use_signal(|| None::<String>);
    let mut filter = use_signal(String::new);
    let records = session.idl_records();
    let directory = (session.directory)();
    let any = !records.is_empty();
    // A phone has room for one of the two panes: the list, or the program
    // opened from it in its place, under a way back to the list.
    let phone = use_phone_layout();
    let mut selected = session.forms.program.selected;
    let opened = selected().and_then(|id| records.iter().find(|record| record.program_id == id));
    let can_remove = opened.is_some_and(removable);
    let open_id = opened.map(|record| record.program_id);
    let one_pane = phone && open_id.is_some();
    // Back to the list puts it where it was left: the filter is kept here,
    // the scroll offset in the page when the program was opened.
    let mut came_back = use_signal(|| false);
    let shown: Vec<IdlRecord> = records
        .into_iter()
        .filter(|record| {
            let label = directory.describe(&record.program_id).full_label();
            program_matches(
                &record.program_id.to_string(),
                [label.as_deref(), Some(record.name.as_str())],
                &filter(),
            )
        })
        .collect();
    // Android has no file picker for the app to open IDL files or folders with.
    let files = !cfg!(target_os = "android");
    // A file dialog is open: it is not modal to the window, so nothing here
    // opens another until it returns.
    let dialog_open = (session.forms.program.file_dialog)();

    rsx! {
        section { class: "page",
            div { class: "stack",
                if let (true, Some(id)) = (one_pane, open_id) {
                    // In place of the heading: the way back to the list, and
                    // Remove, which a phone's row leaves to the program it
                    // opens.
                    div { class: "program-back-row",
                        button {
                            class: "ghost program-back",
                            r#type: "button",
                            // What Android's Back presses (MainActivity).
                            "data-back": "true",
                            // The program starts at its top, wherever the
                            // list was scrolled to.
                            onmounted: move |_| {
                                let _ = app_ui_kit::platform::eval("window.scrollTo(0, 0);");
                            },
                            onclick: move |_| {
                                came_back.set(true);
                                selected.set(None);
                            },
                            Icon { glyph: Glyph::ChevronLeft }
                            "Programs"
                        }
                        if can_remove {
                            button {
                                class: "ghost danger program-remove",
                                r#type: "button",
                                onclick: move |_| {
                                    if session.remove_idl(&id).is_ok() {
                                        came_back.set(true);
                                        selected.set(None);
                                    }
                                },
                                Icon { glyph: Glyph::Trash }
                                "Remove"
                            }
                        }
                    }
                } else {
                    div { class: "pane-head",
                        div {
                            h2 { "Programs" }
                        }
                        if files {
                            // A screen mostly for reading: no primary action, and
                            // the bundle actions behind the menu.
                            div { class: "btn-row",
                                button {
                                    disabled: dialog_open,
                                    onclick: move |_| import_one(session, note, error),
                                    "Import IDL"
                                }
                                PaneMenu { label: "More program actions",
                                    button { class: "pane-menu-item",
                                        role: "menuitem",
                                        disabled: dialog_open,
                                        onclick: move |_| import_folder(session, note, error),
                                        "Import bundle"
                                    }
                                    button { class: "pane-menu-item",
                                        role: "menuitem",
                                        disabled: dialog_open,
                                        onclick: move |_| export_folder(session, note, error),
                                        "Export IDLs"
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(text) = session.idl_pack_note() {
                    p { class: "hint", "{text}" }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                if let Some(text) = note() {
                    p { class: "note", "{text}" }
                }
                if !any {
                    Empty { glyph: Glyph::Code, text: "No programs yet. The programs you add or import are listed here." }
                }
                div { class: "book-layout program-layout",
                    if any && !one_pane {
                        // One card: the filter as its first row, staying in
                        // place over the rows that scroll under it. On a
                        // phone the rows run down the page.
                        div { class: "program-side",
                            div { class: "filter-bar program-filter",
                                label {
                                    span { class: "sr-only", "Filter programs" }
                                    input {
                                        r#type: "search",
                                        placeholder: "Name or address",
                                        value: "{filter}",
                                        oninput: move |e| filter.set(e.value()),
                                    }
                                }
                            }
                            if shown.is_empty() {
                                p { class: "muted program-none", "No programs match this filter." }
                            }
                            ul { class: "group-list program-list",
                                // Back from a program, the list is scrolled
                                // to where it was when the program was opened.
                                onmounted: move |_| {
                                    if came_back() {
                                        came_back.set(false);
                                        let _ = app_ui_kit::platform::eval(
                                            "const y = window.__airlockProgramsY; \
                                             if (y != null) requestAnimationFrame(() => window.scrollTo(0, y));",
                                        );
                                    }
                                },
                                for record in shown {
                                    ProgramItem { key: "{record.program_id}", record }
                                }
                            }
                        }
                    }
                    // A phone shows the list until a program is opened, with
                    // no empty pane under it.
                    if !phone || one_pane {
                        ProgramDetail {}
                    }
                }
            }
        }
        if let Some(held) = (session.forms.program.pending_import)() {
            ReplaceShipped { replaces: held.replaces, note, error }
        }
        input {
            id: "idl-one",
            class: "file-hidden",
            r#type: "file",
            accept: ".json,application/json",
            onchange: move |evt| read_one(evt, session, note, error),
        }
        input {
            id: "idl-bundle",
            class: "file-hidden",
            r#type: "file",
            multiple: true,
            "webkitdirectory": "true",
            "directory": "true",
            onchange: move |evt| read_bundle(evt, session, note, error),
        }
    }
}

#[component]
fn ProgramItem(record: IdlRecord) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.program;
    let mut selected = form.selected;
    let id = record.program_id;
    let on = selected() == Some(id);
    // On a phone a row opens the program in the list's place, as the
    // chevron says; Remove is on the program it opens.
    let phone = use_phone_layout();

    rsx! {
        li { class: if on { "group-item on" } else { "group-item" },
            button {
                class: "group-pick",
                onclick: move |_| {
                    if phone {
                        let _ = app_ui_kit::platform::eval("window.__airlockProgramsY = window.scrollY;");
                    }
                    selected.set(Some(id));
                },
                // Every entry is a program, so the icon is left out.
                AddressRow { pubkey: id, no_copy: true, no_card: true, no_icon: true, class: "program-row" }
                if phone {
                    span { class: "program-chevron", aria_hidden: "true", Icon { glyph: Glyph::ChevronRight } }
                }
            }
            if !phone && removable(&record) {
            div { class: "group-actions",
                button {
                    class: "ghost danger icon-btn sm",
                    title: "Remove",
                    aria_label: "Remove",
                    onclick: move |_| {
                        if session.remove_idl(&id).is_ok() && selected() == Some(id) {
                            selected.set(None);
                        }
                    },
                    Icon { glyph: Glyph::Trash }
                }
            }
            }
        }
    }
}

/// Whether a program's IDL can be removed: one the app ships, or one read
/// from another program's IDL, goes only with what holds it.
fn removable(record: &IdlRecord) -> bool {
    record.origin != IdlOrigin::Builtin && record.host.is_none()
}

/// Whether a program answers the filter: one of its names, or its address,
/// holds the text, whatever the case.
fn program_matches(address: &str, names: [Option<&str>; 2], filter: &str) -> bool {
    let wanted = filter.trim().to_lowercase();
    wanted.is_empty()
        || names
            .into_iter()
            .flatten()
            .chain([address])
            .any(|text| text.to_lowercase().contains(&wanted))
}

/// Whether a name holds the filter, whatever the case.
fn name_matches(name: &str, filter: &str) -> bool {
    let wanted = filter.trim().to_lowercase();
    wanted.is_empty() || name.to_lowercase().contains(&wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

    #[test]
    fn a_program_is_found_by_either_name_or_its_address() {
        let names = [Some("Token Program"), Some("token")];
        assert!(program_matches(TOKEN, names, ""));
        assert!(program_matches(TOKEN, names, "  "));
        assert!(program_matches(TOKEN, names, "token prog"));
        assert!(program_matches(TOKEN, [None, Some("token")], "TOK"));
        assert!(program_matches(TOKEN, names, "kegqfe"));
        assert!(!program_matches(TOKEN, names, "memo"));
    }

    #[test]
    fn an_account_says_how_it_is_used_as_the_transaction_view_does() {
        let account = |writable, signer: &str, optional| IdlAccountUse {
            name: "authority".into(),
            writable,
            signer: signer.into(),
            optional,
        };
        assert_eq!(
            account_flags(&account(true, "not a signer", false)),
            ["writable"]
        );
        assert_eq!(
            account_flags(&account(false, "signer", false)),
            ["readonly", "signer"]
        );
        assert_eq!(
            account_flags(&account(false, "either", true)),
            ["readonly", "signer optional", "optional"]
        );
    }

    #[test]
    fn the_account_that_signs_is_drawn_in_the_accent() {
        assert_eq!(flag_class("signer"), "chip kind");
        assert_eq!(flag_class("signer optional"), "chip kind");
        assert_eq!(flag_class("readonly"), "chip");
        assert_eq!(flag_class("optional"), "chip");
    }

    #[test]
    fn a_collapsed_instruction_says_what_it_takes() {
        assert_eq!(shape(4, 3), "4 accounts · 3 args");
        assert_eq!(shape(1, 1), "1 account · 1 arg");
        assert_eq!(shape(0, 0), "0 accounts · 0 args");
    }

    #[test]
    fn a_collapsed_instruction_shows_the_first_sentence_of_its_docs() {
        assert_eq!(first_sentence(""), None);
        assert_eq!(first_sentence(" \n "), None);
        assert_eq!(
            first_sentence("Transfers tokens from one account to another. If this account"),
            Some("Transfers tokens from one account to another.".into())
        );
        assert_eq!(
            first_sentence("Transfers tokens from one\naccount to another. More."),
            Some("Transfers tokens from one account to another.".into())
        );
        assert_eq!(
            first_sentence("Closes an account\n\nSecond paragraph."),
            Some("Closes an account".into())
        );
    }

    #[test]
    fn the_check_chip_says_what_was_compared() {
        assert_eq!(
            status_words(IdlPinStatus::NotChecked),
            "not compared with chain"
        );
        assert_eq!(status_words(IdlPinStatus::Matches), "same as on chain");
        assert_eq!(status_words(IdlPinStatus::Differs), "differs from chain");
    }

    #[test]
    fn an_opened_instruction_sizes_its_name_column_to_its_longest_name() {
        let ix = IdlIxOutline {
            name: "transferChecked".into(),
            docs: String::new(),
            accounts: vec![IdlAccountUse {
                name: "destination".into(),
                writable: true,
                signer: "not a signer".into(),
                optional: false,
            }],
            arguments: vec![wallet_core::IdlArgOutline {
                name: "discriminator".into(),
                ty: "u8".into(),
                fixed: true,
                fixed_value: Some("12".into()),
            }],
        };
        assert_eq!(row_names_width(&ix), "discriminator".len());
        assert_eq!(longest(["burn", "transferChecked"].into_iter()), 15);
        assert_eq!(longest(std::iter::empty()), 1);
    }

    #[test]
    fn a_long_address_keeps_both_ends() {
        assert_eq!(middle_cut(TOKEN, 12), "TokenkegQfeZ…f9Ss623VQ5DA");
        assert_eq!(middle_cut("short", 12), "short");
    }

    #[test]
    fn a_forge_url_is_shown_as_its_path() {
        assert_eq!(
            short_source("https://github.com/solana-program/token"),
            "solana-program/token"
        );
        assert_eq!(
            short_source("https://gitlab.com/owner/repo.git/"),
            "owner/repo"
        );
        assert_eq!(
            short_source("https://example.org/owner/repo"),
            "https://example.org/owner/repo"
        );
        assert_eq!(short_source("https://github.com/"), "https://github.com/");
    }

    #[test]
    fn a_closed_account_says_what_it_holds() {
        assert_eq!(
            held(5, false, Some(82)).as_deref(),
            Some("5 fields · 82 bytes")
        );
        assert_eq!(held(1, true, None).as_deref(), Some("1 variant"));
        assert_eq!(held(0, false, None), None);
    }

    #[test]
    fn an_instruction_is_found_by_part_of_its_name() {
        assert!(name_matches("transferChecked", ""));
        assert!(name_matches("transferChecked", "checked"));
        assert!(!name_matches("transferChecked", "burn"));
    }
}

/// Asks before an import takes the place of IDLs that ship with the app,
/// naming each.
#[component]
fn ReplaceShipped(
    replaces: Vec<String>,
    note: Signal<Option<String>>,
    error: Signal<Option<String>>,
) -> Element {
    let session = use_context::<Session>();
    let one = replaces.len() == 1;
    rsx! {
        div { class: "modal-backdrop",
            div { class: "modal",
                role: "alertdialog",
                aria_modal: "true",
                aria_label: "Replace a built-in IDL",
                div { class: "stack",
                    h3 { if one { "Replace a built-in IDL?" } else { "Replace built-in IDLs?" } }
                    p {
                        if one {
                            "This file takes the place of the IDL that ships with Airlock for:"
                        } else {
                            "These files take the place of the IDLs that ship with Airlock for:"
                        }
                    }
                    ul {
                        for name in replaces.clone() {
                            li { key: "{name}", "{name}" }
                        }
                    }
                    p { class: "warn",
                        "Review and Sign then puts those programs' instructions in this file's words, and counts them as read with an IDL you chose. Replace only with a file you trust as much as the one that ships. Removing the import in Programs brings the built-in one back."
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| confirm_import(&session, false, note, error),
                            "Cancel"
                        }
                        button { class: "warn solid",
                            onclick: move |_| confirm_import(&session, true, note, error),
                            "Replace"
                        }
                    }
                }
            }
        }
    }
}
