//! The user's own templates on Build: saving the blocks as one, removing
//! one, and the preview an imported file gets before it is kept.

use super::*;
use crate::actions::kept_templates::*;
use std::collections::{BTreeMap, HashMap};
use wallet_core::template::{Choice, FieldAt, Locked, PlannedInput, SaveField, SavePlan};

/// What the lists of templates and their dialogs share, provided by Build.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) struct TemplateDesk {
    /// What the last export, import or removal did.
    pub note: Signal<Option<String>>,
    pub error: Signal<Option<String>>,
    /// The template whose removal waits to be confirmed: its slug and name.
    pub removing: Signal<Option<(String, String)>>,
    /// A file read for import, shown before it is kept.
    pub preview: Signal<Option<ImportPreview>>,
    /// Build's blocks, read for saving as a template.
    pub saving: Signal<Option<SavePlan>>,
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

/// An imported file before it is kept: what it asks, its options, and for
/// every block everything it fixes and works out, open. A file that is not
/// a template says why and can only be closed.
#[component]
pub(in crate::ui) fn ImportDialog() -> Element {
    let session = use_context::<Session>();
    let desk = use_context::<TemplateDesk>();
    let mut preview = desk.preview;
    let (mut note, mut error) = (desk.note, desk.error);
    let mut replacing = use_signal(|| None::<String>);
    let mut refused = use_signal(|| None::<String>);
    let fixed_open = use_signal(|| true);
    let worked_open = use_signal(|| true);
    let Some(ImportPreview { read, problems }) = preview() else {
        return rsx! {};
    };
    let mut close = move || {
        preview.set(None);
        replacing.set(None);
        refused.set(None);
    };

    let template = match read {
        Ok(template) => template,
        Err(why) => {
            return rsx! {
                div { class: "modal-backdrop",
                    onclick: move |_| close(),
                    div { class: "modal",
                        role: "dialog",
                        aria_label: "Import a template",
                        onclick: move |evt| evt.stop_propagation(),
                        div { class: "stack",
                            h3 { "This file is not a template" }
                            p { class: "error", "{why}" }
                            p { class: "hint", "Nothing was kept." }
                            div { class: "btn-row",
                                button { class: "ghost",
                                    r#type: "button",
                                    onclick: move |_| close(),
                                    "Close"
                                }
                            }
                        }
                    }
                }
            };
        }
    };

    let blocks = preview_blocks(&session, &template);
    let fixed: usize = blocks.iter().map(|block| block.fixed.len()).sum();
    let worked: usize = blocks.iter().map(|block| block.worked_out.len()).sum();
    let option_label = |name: &str| {
        template
            .option(name)
            .map_or(name.to_string(), |option| option.label.clone())
    };
    let questions: Vec<(String, String, Option<String>)> = template
        .inputs
        .iter()
        .map(|input| {
            let mut label = input.label.clone();
            if input.optional {
                label.push_str(" (optional)");
            }
            let mut notes = Vec::new();
            if let Some(default) = &input.default {
                notes.push(format!(
                    "Until answered: {}",
                    term_words(&template, default)
                ));
            }
            if let Some(option) = &input.option {
                notes.push(format!("Asked only with “{}”", option_label(option)));
            }
            let notes = (!notes.is_empty()).then(|| notes.join(" · "));
            (label, input.hint.clone().unwrap_or_default(), notes)
        })
        .collect();
    let keep_template = template.clone();
    let mut keep = move |replace: bool| match keep_import(&session, &keep_template, replace) {
        Ok(Kept::Saved(name)) => {
            error.set(None);
            note.set(Some(format!(
                "Kept “{name}”. It is listed with your templates."
            )));
            close();
        }
        Ok(Kept::WouldReplace(existing)) => {
            refused.set(None);
            replacing.set(Some(existing));
        }
        Err(why) => {
            replacing.set(None);
            refused.set(Some(why));
        }
    };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal wide import-template",
                role: "dialog",
                aria_label: "Import a template",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Import a template" }
                    p { class: "lede",
                        "Check what it does before you keep it. Everything it fixes and works out is listed here."
                    }
                    div { class: "template-title",
                        p { class: "import-name", "{template.name}" }
                        if let Some(summary) = &template.summary {
                            p { class: "muted", "{summary}" }
                        }
                    }
                    if !problems.is_empty() {
                        div { class: "warn import-problems",
                            p { "It cannot be used on this network. You can keep it: it is listed greyed until it can." }
                            ul {
                                for problem in problems.iter() {
                                    li { "{problem}" }
                                }
                            }
                        }
                    }
                    div { class: "import-section",
                        h4 { "Questions · {questions.len()}" }
                        if questions.is_empty() {
                            p { class: "hint", "It asks nothing." }
                        }
                        ul { class: "import-list",
                            for (label, hint, notes) in questions {
                                li {
                                    span { class: "import-item-name", "{label}" }
                                    if !hint.is_empty() {
                                        span { class: "hint", "{hint}" }
                                    }
                                    if let Some(notes) = notes {
                                        span { class: "hint", "{notes}" }
                                    }
                                }
                            }
                        }
                    }
                    if !template.options.is_empty() {
                        div { class: "import-section",
                            h4 { "Options · {template.options.len()}" }
                            ul { class: "import-list",
                                for option in template.options.iter() {
                                    li {
                                        span { class: "import-item-name",
                                            "{option.label}"
                                            if option.default {
                                                span { class: "chip", "on at first" }
                                            }
                                        }
                                        if let Some(hint) = &option.hint {
                                            span { class: "hint", "{hint}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "import-section",
                        h4 { "Blocks · {blocks.len()}" }
                        ol { class: "import-blocks",
                            for block in blocks.iter() {
                                li { key: "{block.number}",
                                    span { class: "import-item-name",
                                        "{block.number}. {block.program} · {block.instruction}"
                                    }
                                    if let Some(id) = block.program_id {
                                        span { class: "mono import-program", "{id}" }
                                    }
                                    if let Some(when) = &block.when {
                                        span { class: "hint", "Only with “{when}”" }
                                    }
                                }
                            }
                        }
                    }
                    if fixed > 0 {
                        Accordion {
                            title: format!("Fixed by this template · {fixed}"),
                            open: fixed_open,
                            for block in blocks.iter().filter(|block| !block.fixed.is_empty()) {
                                WrittenBlock {
                                    key: "{block.number}",
                                    heading: format!("{}. {} · {}", block.number, block.program, block.instruction),
                                    rows: block.fixed.clone(),
                                    units: block.units.clone(),
                                }
                            }
                        }
                    }
                    if worked > 0 {
                        Accordion {
                            title: format!("Worked out · {worked}"),
                            open: worked_open,
                            for block in blocks.iter().filter(|block| !block.worked_out.is_empty()) {
                                WrittenBlock {
                                    key: "{block.number}",
                                    heading: format!("{}. {} · {}", block.number, block.program, block.instruction),
                                    rows: block.worked_out.clone(),
                                    units: HashMap::new(),
                                }
                            }
                        }
                    }
                    if let Some(why) = refused() {
                        p { class: "error", "{why}" }
                    }
                    if let Some(existing) = replacing() {
                        p { class: "warn",
                            "You already have a template called “{existing}”. Keeping this one replaces it."
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
                                onclick: move |_| keep(true),
                                "Replace"
                            }
                        } else {
                            button { class: "primary",
                                r#type: "button",
                                onclick: move |_| keep(false),
                                "Keep this template"
                            }
                        }
                    }
                }
            }
        }
    }
}
