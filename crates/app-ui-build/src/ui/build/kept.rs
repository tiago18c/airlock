//! The user's own templates on Build: saving the blocks as one, and
//! removing one. An imported file is read in the Templates dialog's editor
//! before it is kept (`text.rs`).

use super::*;
use crate::actions::kept_templates::*;
use crate::actions::template_text::TextEdit;
use std::collections::BTreeMap;
use wallet_core::template::{Choice, FieldAt, Locked, PlannedInput, SaveField, SavePlan};

/// What the lists of templates and their dialogs share, provided by Build.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) struct TemplateDesk {
    /// What the last export, import or removal did.
    pub note: Signal<Option<String>>,
    pub error: Signal<Option<String>>,
    /// The template whose removal waits to be confirmed: its slug and name.
    pub removing: Signal<Option<(String, String)>>,
    /// The Templates dialog's editor, while it is open: Build's form's, so
    /// its text outlives the screen.
    pub editing: Signal<Option<TextEdit>>,
    /// Build's blocks, read for saving as a template.
    pub saving: Signal<Option<SavePlan>>,
    /// The template the Templates dialog shows next, by its key: one just
    /// saved from the editor.
    pub show: Signal<Option<String>>,
    /// A file dialog for importing or exporting is open. It is awaited,
    /// not modal to the window, so the buttons that open one wait for it.
    pub dialog: Signal<bool>,
}

impl TemplateDesk {
    pub fn clear(&self) {
        let (mut note, mut error) = (self.note, self.error);
        note.set(None);
        error.set(None);
    }
}

/// A field's choice in words, for a field holding `blank` or not.
fn choice_words(choice: Choice, blank: bool) -> &'static str {
    match choice {
        Choice::Keep if blank => "Leave it blank",
        Choice::Keep => "Keep this value",
        Choice::Ask => "Ask each time",
        Choice::Wallet => "The wallet in use",
        Choice::FeePayer => "The fee payer",
    }
}

fn choice_value(choice: Choice) -> &'static str {
    match choice {
        Choice::Keep => "keep",
        Choice::Ask => "ask",
        Choice::Wallet => "wallet",
        Choice::FeePayer => "fee-payer",
    }
}

fn choice_of(value: &str) -> Choice {
    match value {
        "ask" => Choice::Ask,
        "wallet" => Choice::Wallet,
        "fee-payer" => Choice::FeePayer,
        _ => Choice::Keep,
    }
}

/// The program of a block, as Build names it.
fn program_label(program_id: &Pubkey, name: &str) -> String {
    match pinned_program(program_id) {
        Some(pinned) => pinned.label.to_string(),
        None if !name.is_empty() => name.to_string(),
        None => "Program".to_string(),
    }
}

/// Save as template: a name and a summary, then each block's fields with
/// what becomes of each. Fields asked that hold one value are one question,
/// and say so.
#[component]
pub(in crate::ui) fn SaveTemplateDialog() -> Element {
    let session = use_context::<Session>();
    let desk = use_context::<TemplateDesk>();
    let mut saving = desk.saving;
    let mut name = use_signal(String::new);
    let mut summary = use_signal(String::new);
    let mut choices = use_signal(BTreeMap::<FieldAt, Choice>::new);
    let mut replacing = use_signal(|| None::<String>);
    let mut errors = use_signal(Vec::<String>::new);
    let Some(plan) = saving() else {
        return rsx! {};
    };
    let mut close = move || {
        saving.set(None);
        name.set(String::new());
        summary.set(String::new());
        choices.set(BTreeMap::new());
        replacing.set(None);
        errors.set(Vec::new());
    };
    let inputs = plan.inputs(&choices());
    let save_plan = plan.clone();
    let mut save = move |replace: bool| match save_as_template(
        &session,
        &save_plan,
        &name(),
        &summary(),
        &choices(),
        replace,
    ) {
        Ok(Kept::Saved(_)) => close(),
        Ok(Kept::WouldReplace(existing)) => {
            errors.set(Vec::new());
            replacing.set(Some(existing));
        }
        Err(reasons) => {
            replacing.set(None);
            errors.set(reasons);
        }
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal wide save-template",
                role: "dialog",
                aria_label: "Save as template",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Save as template" }
                    p { class: "lede",
                        "A template asks what you choose each time and fills in the rest. Whoever opens it sees every value it keeps."
                    }
                    label { "Name"
                        input {
                            r#type: "text",
                            value: "{name}",
                            maxlength: "80",
                            oninput: move |e| {
                                name.set(e.value());
                                replacing.set(None);
                            },
                        }
                    }
                    label { "Summary (optional)"
                        input {
                            r#type: "text",
                            value: "{summary}",
                            maxlength: "300",
                            oninput: move |e| summary.set(e.value()),
                        }
                    }
                    for (index, block) in plan.blocks.iter().enumerate() {
                        div { class: "save-block", key: "{index}",
                            h4 {
                                "{index + 1}. {program_label(&block.program_id, &block.program_name)} · {block.instruction}"
                            }
                            if block.fields.is_empty() {
                                p { class: "hint", "Nothing to fill in: the IDL fills in every field." }
                            }
                            ul { class: "save-fields",
                                for field in block.fields.iter() {
                                    SaveFieldRow {
                                        key: "{field.at.field}",
                                        field: field.clone(),
                                        choice: plan.choice(&choices(), field),
                                        joined: joined_with(&plan, &inputs, field),
                                        on_choice: {
                                            let at = field.at.clone();
                                            move |choice: Choice| {
                                                choices.write().insert(at.clone(), choice);
                                                replacing.set(None);
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                    for reason in errors() {
                        p { class: "error", "{reason}" }
                    }
                    if let Some(existing) = replacing() {
                        p { class: "warn",
                            "You already have a template called “{existing}”. Saving replaces it."
                        }
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| close(),
                            "Cancel"
                        }
                        if replacing().is_some() {
                            button { class: "danger solid",
                                r#type: "button",
                                onclick: move |_| save(true),
                                "Replace"
                            }
                        } else {
                            button { class: "primary",
                                r#type: "button",
                                onclick: move |_| save(false),
                                "Save"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The question a field is asked in, when other fields are asked in it
/// too: its label, and the others, by block and name.
fn joined_with(plan: &SavePlan, inputs: &[PlannedInput], field: &SaveField) -> Option<String> {
    let input = inputs
        .iter()
        .find(|input| input.fields.contains(&field.at))?;
    if input.fields.len() < 2 {
        return None;
    }
    let others: Vec<String> = input
        .fields
        .iter()
        .filter(|at| **at != field.at)
        .filter_map(|at| {
            plan.field(at)
                .map(|other| format!("{} · {}", other.at.block + 1, other.at.field))
        })
        .collect();
    Some(format!(
        "Asked once as “{}”, with {}.",
        input.label,
        others.join(", ")
    ))
}

/// One field of a block in the Save dialog: its name, what it holds, and
/// what becomes of it.
#[component]
fn SaveFieldRow(
    field: SaveField,
    choice: Choice,
    joined: Option<String>,
    on_choice: EventHandler<Choice>,
) -> Element {
    let blank = field.is_blank();
    let label = with_unit(&field.at.field, field.unit.as_deref());
    rsx! {
        li { class: "save-field",
            div { class: "save-field-what",
                span { class: "save-field-name", "{label}" }
                span { class: "save-field-value",
                    if let Some(address) = field.address {
                        Address { pubkey: address }
                    } else if blank {
                        span { class: "hint", "blank" }
                    } else {
                        "{field.text}"
                    }
                }
            }
            div { class: "save-field-choice",
                match field.locked {
                    Some(Locked::NewKeypair) => rsx! {
                        span { class: "save-field-fixed", "A new keypair each time" }
                    },
                    Some(Locked::MustAsk) => rsx! {
                        span { class: "save-field-fixed", "Ask each time" }
                        span { class: "hint", "It is blank, and the program needs it." }
                    },
                    None => rsx! {
                        label {
                            span { class: "sr-only", "What becomes of {field.at.field}" }
                            select {
                                value: choice_value(choice),
                                onchange: move |e| on_choice.call(choice_of(&e.value())),
                                for offer in field.offers.iter().copied() {
                                    option {
                                        value: choice_value(offer),
                                        selected: offer == choice,
                                        {choice_words(offer, blank)}
                                    }
                                }
                            }
                        }
                    },
                }
                if let Some(joined) = joined {
                    span { class: "hint", "{joined}" }
                }
            }
        }
    }
}

/// Asks before one of the user's templates is removed.
#[component]
pub(in crate::ui) fn RemoveTemplate() -> Element {
    let session = use_context::<Session>();
    let desk = use_context::<TemplateDesk>();
    let mut removing = desk.removing;
    let (mut note, mut error) = (desk.note, desk.error);
    let Some((slug, name)) = removing() else {
        return rsx! {};
    };
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| removing.set(None),
            div { class: "modal",
                role: "dialog",
                aria_label: "Remove a template",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Remove “{name}”?" }
                    p { class: "lede",
                        "It goes from this device. A file you exported it to stays where you saved it."
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| removing.set(None),
                            "Cancel"
                        }
                        button { class: "danger solid",
                            r#type: "button",
                            onclick: move |_| {
                                match remove_kept(&session, &slug) {
                                    Ok(()) => {
                                        error.set(None);
                                        note.set(Some(format!("Removed “{name}”.")));
                                    }
                                    Err(err) => {
                                        note.set(None);
                                        error.set(Some(err));
                                    }
                                }
                                removing.set(None);
                            },
                            "Remove"
                        }
                    }
                }
            }
        }
    }
}
