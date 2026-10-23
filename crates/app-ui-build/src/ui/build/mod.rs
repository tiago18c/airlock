use crate::actions::build::*;
use crate::state::forms::{GeneratedKey, IxBody, IxDraft};
use crate::state::session::Session;
use crate::ui::{Accordion, Address, AddressPicker, AddressSelect, Glyph, Icon};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::edit::{CarrierKind, RawAccountDraft};
use wallet_core::{
    idl_instruction_form, pinned_program, AddressKind, IdlAccountField, IdlAccountSource,
    IdlArgKind, IdlDraft, IdlOrigin, IdlRecord, PinnedProgram, PINNED_PROGRAMS,
};

mod advanced;
mod carrier;
mod editor;
mod fields;
mod rail;
mod raw;

pub(in crate::ui) use advanced::*;
pub(in crate::ui) use carrier::*;
pub(in crate::ui) use editor::*;
pub(in crate::ui) use fields::*;
pub(in crate::ui) use rail::*;
pub(in crate::ui) use raw::*;

#[component]
pub fn BuildPanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let mut fee_payer = form.fee_payer;
    let error = form.error;
    let compiling = use_signal(|| false);
    let compiled_note = form.compiled_note;

    // The selected wallet's fee payer, once its record has loaded.
    use_effect(move || session.default_build_fee_payer());

    let drafts = (form.drafts)();
    let selected_id = (form.selected)();
    let selected_draft = drafts.iter().find(|d| Some(d.id) == selected_id).cloned();

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "page-head",
                    div {
                        h2 { "Build" }
                    }
                }

                AddressPicker {
                    label: "Fee payer".to_string(),
                    selected: fee_payer(),
                    reset: form.picker_reset,
                    on_change: move |pk| fee_payer.set(pk),
                }

                h3 { "Instructions" }
                InstructionRail { parent: None }
                AdvancedOptions {}

                if let Some(draft) = selected_draft {
                    InstructionEditor { key: "{draft.id}", draft }
                } else {
                    p { class: "muted", "Select a block to edit it, or add a new instruction." }
                }

                if let Some(note) = compiled_note() {
                    p { class: "note", "{note}" }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }

                div { class: "btn-row",
                    button { class: "primary",
                        disabled: compiling(),
                        onclick: move |_| compile(session, compiling),
                        if compiling() { "Compiling…" } else { "Review and Sign" }
                    }
                    button { class: "ghost",
                        onclick: move |_| session.clear_build(),
                        "Clear"
                    }
                }
            }
        }
    }
}
