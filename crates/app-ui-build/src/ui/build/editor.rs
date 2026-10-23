//! One instruction block: its program, its instruction, and its fields.

use super::*;

#[component]
pub(in crate::ui) fn InstructionEditor(draft: IxDraft) -> Element {
    let session = use_context::<Session>();
    if matches!(draft.body, IxBody::Carried { .. }) {
        return rsx! { CarrierEditor { draft } };
    }
    let form = session.forms.build;
    let id = draft.id;
    let raw = matches!(draft.body, IxBody::Raw(_));
    let current: Option<PinnedProgram> = draft
        .idl()
        .and_then(|idl| idl.program_id)
        .and_then(|program_id| pinned_program(&program_id))
        .copied();
    let replace = move |next: IxBody| {
        form.patch(id, |d| d.body = next);
        let mut reset = form.picker_reset;
        reset.set(reset() + 1);
    };
    let as_raw = draft.clone();

    rsx! {
        div { class: "ix-editor",
            h3 { "Instruction" }
            div { class: "program-tabs",
                for program in PINNED_PROGRAMS {
                    button {
                        class: if !raw && current == Some(program) { "tab on" } else { "tab" },
                        key: "{program.label}",
                        onclick: move |_| {
                            if raw || current != Some(program) {
                                replace(IxBody::Idl(pinned_draft(&session, &program)));
                            }
                        },
                        "{program.label}"
                    }
                }
                button {
                    class: if !raw && current.is_none() { "tab on" } else { "tab" },
                    onclick: move |_| {
                        if raw || current.is_some() {
                            replace(IxBody::Idl(other_draft(&session)));
                        }
                    },
                    "Other programs"
                }
                button {
                    class: if raw { "tab on" } else { "tab" },
                    title: "Its program, accounts and bytes, written by hand",
                    onclick: move |_| {
                        if !raw {
                            replace(IxBody::Raw(raw_draft(&session, &as_raw)));
                        }
                    },
                    "Raw"
                }
            }
            if raw {
                RawFields { draft: draft.clone() }
            } else {
                IdlFields { draft: draft.clone() }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn IdlFields(draft: IxDraft) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let id = draft.id;
    let Some(idl) = draft.idl().cloned() else {
        return rsx! {};
    };
    let program_id = idl.program_id;
    let pinned = program_id.and_then(|program_id| pinned_program(&program_id));
    // Pinned programs have their own tabs. This list is everything else.
    let records: Vec<IdlRecord> = session
        .idl_records()
        .into_iter()
        .filter(|record| pinned_program(&record.program_id).is_none())
        .collect();
    let origin = program_id.and_then(|program_id| {
        session
            .idl_records()
            .into_iter()
            .find(|record| record.program_id == program_id)
            .map(|record| record.origin)
    });
    let prepared = program_id.and_then(|program_id| {
        let client = session.idls.read().program_client(&program_id)?;
        let names: Vec<String> = client
            .instruction_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let spec = idl_instruction_form(&client, &idl.instruction, &idl.input());
        Some((names, spec))
    });
    let names = prepared
        .as_ref()
        .map(|(names, _)| names.clone())
        .unwrap_or_default();
    let accounts = prepared
        .as_ref()
        .and_then(|(_, spec)| spec.as_ref().ok())
        .map(|spec| spec.accounts.clone())
        .unwrap_or_default();
    let arguments = prepared
        .as_ref()
        .and_then(|(_, spec)| spec.as_ref().ok())
        .map(|spec| spec.arguments.clone())
        .unwrap_or_default();
    let spec_error = prepared
        .as_ref()
        .and_then(|(_, spec)| spec.as_ref().err())
        .cloned();

    rsx! {
        p { class: "hint",
            "Amounts are in the program's base units, such as lamports. PDA accounts are filled in. If a default cannot be derived, that account asks for an address."
        }
        if let Some(origin) = origin {
            div { class: "chip-row",
                if origin == IdlOrigin::Builtin {
                    span { class: "chip", "built-in IDL" }
                } else {
                    span { class: "chip info", "imported IDL" }
                }
            }
        }
        if pinned.is_none() && records.is_empty() {
            p { class: "hint",
                "Import an IDL on the Programs pane. This tab builds one instruction from that document."
            }
        } else {
            if pinned.is_none() {
            label { "Program"
                select {
                    value: program_id.map(|pk| pk.to_string()).unwrap_or_default(),
                    onchange: move |e| {
                        let value = e.value();
                        let Some(record) = session
                            .idl_records()
                            .into_iter()
                            .find(|record| record.program_id.to_string() == value)
                        else {
                            return;
                        };
                        let next = draft_for_record(&session, &record, None);
                        form.patch(id, |d| d.body = IxBody::Idl(next));
                        let mut reset = form.picker_reset;
                        reset.set(reset() + 1);
                    },
                    for record in records.iter() {
                        option {
                            value: "{record.program_id}",
                            selected: Some(record.program_id) == program_id,
                            "{record.name}"
                        }
                    }
                }
            }
            }
            if !names.is_empty() {
                label { "Instruction"
                    select {
                        value: "{idl.instruction}",
                        onchange: move |e| {
                            let value = e.value();
                            let Some(program_id) = program_id else { return };
                            let Some(record) = session
                                .idl_records()
                                .into_iter()
                                .find(|record| record.program_id == program_id)
                            else {
                                return;
                            };
                            let next = draft_for_record(&session, &record, Some(&value));
                            form.patch(id, |d| d.body = IxBody::Idl(next));
                            let mut reset = form.picker_reset;
                            reset.set(reset() + 1);
                        },
                        for name in names.iter() {
                            option {
                                value: "{name}",
                                selected: name == &idl.instruction,
                                "{name}"
                            }
                        }
                    }
                }
            }
            if let Some(err) = spec_error {
                p { class: "error", "{err}" }
            }
            for account in accounts {
                { idl_account_field(form, id, &idl, account) }
            }
            for argument in arguments {
                { idl_argument_field(form, id, &idl, argument) }
            }
        }
    }
}
