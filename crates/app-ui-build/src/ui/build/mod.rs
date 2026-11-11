use crate::actions::build::*;
use crate::actions::kept_templates::{ensure_kept, read_import, save_plan, TEMPLATE_FILE_INPUT};
use crate::actions::template::{compile_template, show_as_blocks};
use crate::state::forms::{GeneratedKey, IxBody, IxDraft, TemplateSource};
use crate::state::session::Session;
use crate::ui::{Accordion, Address, AddressPicker, AddressSelect, Glyph, Icon};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::edit::{CarrierKind, RawAccountDraft};
use wallet_core::{
    idl_instruction_form, pinned_program, AddressKind, IdlAccountField, IdlAccountSource,
    IdlArgField, IdlArgKind, IdlArgScale, IdlDraft, IdlOrigin, IdlRecord, PinnedProgram,
    PINNED_PROGRAMS,
};

mod advanced;
mod carrier;
mod editor;
mod fields;
mod kept;
mod rail;
mod raw;
mod template;

pub(in crate::ui) use advanced::*;
pub(in crate::ui) use carrier::*;
pub(in crate::ui) use editor::*;
pub(in crate::ui) use fields::*;
pub(in crate::ui) use kept::*;
pub(in crate::ui) use rail::*;
pub(in crate::ui) use raw::*;
pub(in crate::ui) use template::*;

#[component]
pub fn BuildPanel() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let mut fee_payer = form.fee_payer;
    let error = form.error;
    let compiling = use_signal(|| false);
    let compiled_note = form.compiled_note;
    let mut picker_open = use_signal(|| false);
    let mut pending = use_signal(|| None::<TemplateSource>);
    let desk = use_context_provider(|| TemplateDesk {
        note: Signal::new(None),
        error: Signal::new(None),
        removing: Signal::new(None),
        preview: Signal::new(None),
        saving: Signal::new(None),
    });
    let mut saving = desk.saving;

    // The selected wallet's fee payer, once its record has loaded.
    use_effect(move || session.default_build_fee_payer());
    // The user's templates, read once, the first time Build is shown.
    use_effect(move || ensure_kept(&session));

    let drafts = (form.drafts)();
    let selected_id = (form.selected)();
    let selected_draft = drafts.iter().find(|d| Some(d.id) == selected_id).cloned();
    let templating = form.template.read().is_some();
    let holds_blocks = !drafts.is_empty();
    // Opening a template drops the blocks, so it asks first when there are any.
    let pick = move |source: TemplateSource| {
        if form.drafts.peek().is_empty() {
            pick_template(session, source);
        } else {
            pending.set(Some(source));
        }
    };

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "page-head",
                    div {
                        h2 { "Build" }
                    }
                    div { class: "btn-row",
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                desk.clear();
                                picker_open.set(true);
                            },
                            "Templates"
                        }
                    }
                }

                AddressPicker {
                    label: "Fee payer".to_string(),
                    selected: fee_payer(),
                    reset: form.picker_reset,
                    on_change: move |pk| fee_payer.set(pk),
                }

                if templating {
                    TemplateForm {}
                    AdvancedOptions {}
                } else {
                    h3 { "Instructions" }
                    InstructionRail { parent: None }
                    if !holds_blocks {
                        div { class: "stack template-start",
                            h3 { "Or start from a template" }
                            TemplateList { on_pick: pick }
                        }
                    }
                    AdvancedOptions {}

                    if let Some(draft) = selected_draft {
                        InstructionEditor { key: "{draft.id}", draft }
                    } else if holds_blocks {
                        p { class: "muted", "Select a block to edit it, or add a new instruction." }
                    }
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
                        onclick: move |_| {
                            if form.template.peek().is_some() {
                                compile_template(session, compiling);
                            } else {
                                compile(session, compiling);
                            }
                        },
                        if compiling() { "Compiling…" } else { "Review and Sign" }
                    }
                    if templating {
                        button {
                            r#type: "button",
                            title: "Write the template out as blocks, to change what it does not ask",
                            onclick: move |_| show_as_blocks(&session),
                            "Show as blocks"
                        }
                    } else if holds_blocks {
                        button {
                            r#type: "button",
                            title: "Keep these blocks as a template, choosing what it asks each time",
                            onclick: move |_| {
                                let mut error = form.error;
                                match save_plan(&session) {
                                    Ok(plan) => {
                                        error.set(None);
                                        saving.set(Some(plan));
                                    }
                                    Err(why) => error.set(Some(why)),
                                }
                            },
                            "Save as template"
                        }
                    }
                    button { class: "ghost",
                        onclick: move |_| session.clear_build(),
                        "Clear"
                    }
                }
            }
        }
        if picker_open() {
            TemplatePicker { open: picker_open, on_pick: pick }
        }
        DiscardBlocks { pending }
        SaveTemplateDialog {}
        ImportDialog {}
        RemoveTemplate {}
        // A browser picks the file to import through this field.
        input {
            id: TEMPLATE_FILE_INPUT,
            class: "file-hidden",
            r#type: "file",
            accept: ".toml",
            onchange: move |evt| read_import(evt, session, desk.preview, desk.error),
        }
    }
}
