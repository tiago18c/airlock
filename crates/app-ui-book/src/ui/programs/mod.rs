use crate::actions::programs::*;
use crate::state::session::Session;
use crate::ui::{short_checksum, AddressHeader, AddressRow, Glyph, Icon};
use dioxus::prelude::*;
use wallet_core::{
    pretty_idl, IdlAccountUse, IdlOrigin, IdlOutline, IdlPinStatus, IdlRecord, IdlSpec,
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
    let records = session.idl_records();

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Programs" }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            onclick: move |_| import_one(session, note, error),
                            "Import IDL"
                        }
                        button {
                            onclick: move |_| import_folder(session, note, error),
                            "Import bundle"
                        }
                        button {
                            onclick: move |_| export_folder(session, note, error),
                            "Export"
                        }
                    }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                if let Some(text) = note() {
                    p { class: "note", "{text}" }
                }
                if records.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::Code }
                        p { "No programs yet." }
                    }
                }
                div { class: "book-layout",
                    ul { class: "group-list programs",
                        for record in records {
                            ProgramItem { key: "{record.program_id}", record }
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
                AddressRow { pubkey: id, no_copy: true, no_card: true }
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
