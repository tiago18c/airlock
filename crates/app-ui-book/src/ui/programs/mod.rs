use crate::actions::programs::*;
use crate::state::forms::IdlSection;
use crate::state::session::Session;
use crate::ui::{short_checksum, AddressHeader, AddressRow, CopyButton, Glyph, Icon, PaneMenu};
use dioxus::prelude::*;
use wallet_core::{
    pretty_idl, IdlAccountUse, IdlIxOutline, IdlOrigin, IdlOutline, IdlPinStatus, IdlRecord,
    IdlSource, IdlSpec,
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

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Programs" }
                    }
                    if files {
                        // A screen mostly for reading: no primary action, and
                        // the bundle actions behind the menu.
                        div { class: "btn-row",
                            button {
                                onclick: move |_| import_one(session, note, error),
                                "Import IDL"
                            }
                            PaneMenu { label: "More program actions",
                                button { class: "pane-menu-item",
                                    role: "menuitem",
                                    onclick: move |_| import_folder(session, note, error),
                                    "Import bundle"
                                }
                                button { class: "pane-menu-item",
                                    role: "menuitem",
                                    onclick: move |_| export_folder(session, note, error),
                                    "Export IDLs"
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
                    div { class: "empty",
                        Icon { glyph: Glyph::Code }
                        p { "No programs yet." }
                    }
                }
                div { class: "book-layout program-layout",
                    if any {
                        // One card: the filter as its first row, staying in
                        // place over the rows that scroll under it.
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
                                for record in shown {
                                    ProgramItem { key: "{record.program_id}", record }
                                }
                            }
                        }
                    }
                    ProgramDetail {}
                }
            }
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

    rsx! {
        li { class: if on { "group-item on" } else { "group-item" },
            button {
                class: "group-pick",
                onclick: move |_| selected.set(Some(id)),
                // Every entry is a program, so the icon is left out.
                AddressRow { pubkey: id, no_copy: true, no_card: true, no_icon: true, class: "program-row" }
            }
            if record.origin != IdlOrigin::Builtin && record.host.is_none() {
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
    fn an_instruction_is_found_by_part_of_its_name() {
        assert!(name_matches("transferChecked", ""));
        assert!(name_matches("transferChecked", "checked"));
        assert!(!name_matches("transferChecked", "burn"));
    }
}
