//! Templates on Build: the short list under the rail, and the form of the
//! one open, which takes the place of the blocks until it is shown as
//! blocks, compiled, or closed. The Templates dialog is in `browser.rs`.

use super::*;
use crate::actions::template::*;
use crate::state::forms::{FactReadState, OpenTemplate, TemplateSource};
use std::collections::HashMap;
use wallet_core::template::{
    ExpandedBlock, Expansion, Fact, FormInput, InputKind, InputScale, Missing, Row, TemplateOption,
};
use wallet_core::{TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

/// Build's short list of templates, under its instruction rail while it
/// holds no block: one row each, its name, its summary and where it comes
/// from, which opens it at once. One that does not fit the IDLs of the
/// network in use is listed, greyed, with the reason. A file of the user's
/// that is not a template is left to the Templates dialog, which says why.
#[component]
pub(in crate::ui) fn TemplateList(on_pick: EventHandler<TemplateSource>) -> Element {
    let session = use_context::<Session>();
    let entries = use_memo(move || template_entries(&session));
    rsx! {
        ul { class: "book-list template-list",
            for entry in entries().into_iter().filter(|entry| !entry.is_broken()) {
                li {
                    key: "{entry.source.key()}",
                    class: if entry.problem.is_some() { "template-item off" } else { "template-item" },
                    button {
                        class: "template-row",
                        r#type: "button",
                        disabled: entry.problem.is_some(),
                        onclick: {
                            let source = entry.source.clone();
                            move |_| on_pick.call(source.clone())
                        },
                        span { class: "template-row-head",
                            span { class: "template-row-name", "{entry.name}" }
                            span { class: "chip", "{entry.source.chip()}" }
                        }
                        if let Some(summary) = &entry.summary {
                            span { class: "template-row-summary", "{summary}" }
                        }
                        if let Some(problem) = &entry.problem {
                            span { class: "template-row-problem",
                                "Cannot be used on this network. {problem}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Asks before a template takes the place of the blocks on Build.
#[component]
pub(in crate::ui) fn DiscardBlocks(mut pending: Signal<Option<TemplateSource>>) -> Element {
    let session = use_context::<Session>();
    let Some(source) = pending() else {
        return rsx! {};
    };
    let name = source
        .template()
        .map(|template| template.name)
        .unwrap_or_default();
    let count = session.forms.build.drafts.read().len();
    let blocks = if count == 1 {
        "the block".to_string()
    } else {
        format!("the {count} blocks")
    };
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| pending.set(None),
            div { class: "modal",
                role: "dialog",
                aria_label: "Discard the blocks on Build",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Discard the blocks on Build?" }
                    p { class: "lede",
                        "“{name}” takes the place of {blocks} on Build. They are not kept."
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| pending.set(None),
                            "Cancel"
                        }
                        button { class: "danger solid",
                            r#type: "button",
                            onclick: move |_| {
                                if let Some(source) = pending() {
                                    pick_template(session, source);
                                }
                                pending.set(None);
                            },
                            "Discard and open"
                        }
                    }
                }
            }
        }
    }
}

/// Opens `source` on Build, or says why it could not.
pub(in crate::ui) fn pick_template(session: Session, source: TemplateSource) {
    if let Err(err) = open_template(&session, source) {
        let mut error = session.forms.build.error;
        error.set(Some(err));
    }
}

/// The open template's form: its questions, its options with theirs, the
/// chain facts it needs, and everything it writes itself.
#[component]
pub(in crate::ui) fn TemplateForm() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let opened = use_memo(move || expand_open(&session));
    let fixed_open = use_signal(|| false);
    let worked_open = use_signal(|| false);

    // Online, the facts the answers decide are read from the node once the
    // answers settle.
    use_effect(move || {
        let needs = opened
            .read()
            .as_ref()
            .and_then(|opened| opened.expansion.as_ref().ok())
            .map(|expansion| expansion.needs.clone())
            .unwrap_or_default();
        request_facts(session, needs);
    });

    let Some(OpenExpansion {
        open,
        env,
        form: questions,
        expansion,
    }) = opened()
    else {
        return rsx! {};
    };
    let expansion = match expansion {
        Ok(expansion) => expansion,
        Err(problems) => {
            let reason = problems
                .first()
                .map(|problem| problem.to_string())
                .unwrap_or_default();
            return rsx! {
                div { class: "ix-editor template-form",
                    TemplateHead { open: open.clone() }
                    p { class: "error", "This template does not fit the IDLs of this network. {reason}" }
                }
            };
        }
    };

    // A blank input is marked once Review and Sign was asked for; an answer
    // a function cannot take, at once, with why.
    let marked: Vec<(String, Option<String>)> = expansion
        .missing
        .iter()
        .filter_map(|missing| match missing {
            Missing::Input(name) if open.marked.contains(name) => Some((name.clone(), None)),
            Missing::Invalid { input, message } => Some((input.clone(), Some(message.clone()))),
            _ => None,
        })
        .collect();
    let field = |input: &FormInput| {
        template_input(
            session,
            &open,
            &env,
            &expansion,
            input,
            marked
                .iter()
                .find(|(name, _)| *name == input.name)
                .map(|(_, why)| why.as_deref().unwrap_or("Fill this in.")),
        )
    };
    let plain: Vec<Element> = questions
        .inputs
        .iter()
        .filter(|input| input.option.is_none())
        .map(field)
        .collect();
    let options: Vec<(TemplateOption, bool, Vec<Element>)> = questions
        .options
        .iter()
        .map(|option| {
            let on = open.answers.is_on(&option.name);
            let inputs = if on {
                questions
                    .inputs
                    .iter()
                    .filter(|input| input.option.as_deref() == Some(option.name.as_str()))
                    .map(field)
                    .collect()
            } else {
                Vec::new()
            };
            (option.clone(), on, inputs)
        })
        .collect();

    let online = session.rpc().is_ok();
    let read = (form.fact_read)();
    let failed = match &read.state {
        FactReadState::Failed(why) => Some(why.clone()),
        _ => None,
    };
    let reading = online && read.state == FactReadState::Reading && !expansion.needs.is_empty();
    // Offline, or when the node would not say, the facts come from the
    // person, one field each.
    let ask_facts = !online || failed.is_some();
    let facts: Vec<Element> = if ask_facts {
        expansion
            .uses
            .iter()
            .map(|fact| fact_field(session, &open, *fact))
            .collect()
    } else {
        Vec::new()
    };

    let units = block_units(&session, &expansion);
    let fixed = expansion
        .blocks
        .iter()
        .map(|block| block.fixed.len())
        .sum::<usize>();
    let worked = expansion
        .blocks
        .iter()
        .map(|block| block.worked_out.len())
        .sum::<usize>();

    rsx! {
        div { class: "ix-editor template-form",
            TemplateHead { open: open.clone() }
            div { class: "template-inputs",
                for input in plain { {input} }
            }
            if !options.is_empty() {
                div { class: "template-options",
                    h4 { "Options" }
                    for (option, on, inputs) in options {
                        div { class: "template-option", key: "{option.name}",
                            label { class: "check",
                                input {
                                    r#type: "checkbox",
                                    checked: on,
                                    onchange: {
                                        let name = option.name.clone();
                                        move |e: FormEvent| set_option(&session, &name, e.checked())
                                    },
                                }
                                " {option.label}"
                            }
                            if let Some(hint) = &option.hint {
                                p { class: "hint template-option-hint", "{hint}" }
                            }
                            if on && !inputs.is_empty() {
                                div { class: "template-option-inputs",
                                    for input in inputs { {input} }
                                }
                            }
                        }
                    }
                }
            }
            if !facts.is_empty() {
                div { class: "template-facts",
                    h4 { "From the chain" }
                    p { class: "hint",
                        if online {
                            "The node did not answer these. Type them as the chain has them."
                        } else {
                            "Without a node these come from you. Type them as the chain has them."
                        }
                    }
                    for field in facts { {field} }
                }
            }
            if reading {
                p { class: "hint", role: "status", "Reading from the node…" }
            }
            if let Some(why) = failed {
                p { class: "error", "{why}" }
            }
            if fixed > 0 {
                Accordion {
                    title: format!("Fixed by this template · {fixed}"),
                    open: fixed_open,
                    for (place, block) in expansion.blocks.iter().enumerate().filter(|(_, block)| !block.fixed.is_empty()) {
                        WrittenBlock {
                            key: "{block.number}",
                            heading: format!("{}. {}", place + 1, block_heading(&session, &block.draft)),
                            rows: block.fixed.clone(),
                            units: units.get(&block.number).cloned().unwrap_or_default(),
                        }
                    }
                }
            }
            if worked > 0 {
                Accordion {
                    title: format!("Worked out · {worked}"),
                    open: worked_open,
                    for (place, block) in expansion.blocks.iter().enumerate().filter(|(_, block)| !block.worked_out.is_empty()) {
                        WrittenBlock {
                            key: "{block.number}",
                            heading: format!("{}. {}", place + 1, block_heading(&session, &block.draft)),
                            rows: block.worked_out.clone(),
                            units: units.get(&block.number).cloned().unwrap_or_default(),
                        }
                    }
                }
            }
        }
    }
}

/// What became of the file of one of the user's templates since the form
/// opened it, in words: it changed, or it went (renamed or removed). The
/// form keeps working on what it opened, and says so.
fn file_since(session: &Session, open: &OpenTemplate) -> Option<&'static str> {
    let TemplateSource::Yours { slug, text, .. } = &open.source else {
        return None;
    };
    let kept = crate::actions::kept_templates::kept_templates(session);
    match kept.kept.iter().find(|now| now.slug() == Some(slug.as_str())) {
        Some(now) if now.text() == Some(&**text) => None,
        Some(_) => Some(
            "Its file has changed since this form opened it. The form keeps what it opened: close it and open the template again for the new text.",
        ),
        None => Some(
            "Its file has been renamed or removed since this form opened it. The form keeps what it opened.",
        ),
    }
}

/// The template's name and summary, and the way out of it.
#[component]
fn TemplateHead(open: OpenTemplate) -> Element {
    let session = use_context::<Session>();
    let since = file_since(&session, &open);
    rsx! {
        div { class: "template-head",
            div { class: "template-title",
                h3 { "{open.template.name}" }
                if let Some(summary) = &open.template.summary {
                    p { class: "muted", "{summary}" }
                }
                if let Some(since) = since {
                    p { class: "hint template-since", "{since}" }
                }
            }
            button { class: "ghost sm",
                r#type: "button",
                title: "Close the template, and start Build again empty",
                onclick: move |_| close_template(&session),
                "Close template"
            }
        }
    }
}

/// One question, with the widget of the fields it fills. Its default shows
/// as its value until it is changed, and it can be cleared.
fn template_input(
    session: Session,
    open: &OpenTemplate,
    env: &wallet_core::template::Env,
    expansion: &Expansion,
    input: &FormInput,
    missing: Option<&str>,
) -> Element {
    let text = open.input_text(&input.name, env);
    let name = input.name.clone();
    let on_change = Callback::new(move |value: String| answer(&session, &name, value));
    let optional = if input.optional { " (optional)" } else { "" };
    let widget = match &input.kind {
        InputKind::Address => {
            let selected = Pubkey::from_str(text.trim()).ok();
            rsx! {
                AddressPicker {
                    label: format!("{}{optional}", input.label),
                    selected,
                    reset: session.forms.build.picker_reset,
                    on_change: move |pk: Option<Pubkey>| {
                        on_change.call(pk.map(|pk| pk.to_string()).unwrap_or_default());
                    },
                }
            }
        }
        InputKind::Argument { kind, scale } => {
            let (unit, places) = match scale {
                InputScale::None => (None, Places::BaseUnits),
                InputScale::Fixed { decimals, unit } => (unit.as_deref(), Places::Fixed(*decimals)),
                InputScale::FromArgument { argument, unit, .. } => (
                    unit.as_deref(),
                    match expansion.decimals_of(scale) {
                        Some(decimals) => Places::Fixed(decimals),
                        None => Places::Like(argument.clone()),
                    },
                ),
                InputScale::FromValue { value, unit } => (
                    unit.as_deref(),
                    match expansion.decimals_of(scale) {
                        Some(decimals) => Places::Fixed(decimals),
                        None => Places::Like(format!("${value}")),
                    },
                ),
            };
            // An optional input can be left blank, as an optional field can.
            let kind = if input.optional {
                IdlArgKind::Option(Box::new(kind.clone()))
            } else {
                kind.clone()
            };
            let blank = input.optional.then_some("blank for none");
            argument_input(
                format!("{}{optional}", with_unit(&input.label, unit)),
                kind_placeholder(&kind, places, blank),
                &kind,
                text,
                session.forms.build.picker_reset,
                on_change,
            )
        }
    };
    rsx! {
        div {
            class: if missing.is_some() { "template-input missing" } else { "template-input" },
            key: "{input.name}",
            {widget}
            if let Some(hint) = &input.hint {
                p { class: "hint", "{hint}" }
            }
            if let Some(why) = missing {
                p { class: "field-missing", "{why}" }
            }
        }
    }
}

/// The field for a chain fact typed by hand, labelled as what it is.
fn fact_field(session: Session, open: &OpenTemplate, fact: Fact) -> Element {
    let text = open.typed_facts.get(&fact).cloned().unwrap_or_default();
    let label = fact.to_string();
    let key = format!("{fact:?}");
    match fact {
        Fact::TokenProgram(_) => rsx! {
            label { key: "{key}", "{label}"
                select {
                    value: "{text}",
                    onchange: move |e| type_fact(&session, fact, e.value()),
                    option { value: "", selected: text.is_empty(), "Choose one" }
                    option {
                        value: "{TOKEN_PROGRAM_ID}",
                        selected: text == TOKEN_PROGRAM_ID.to_string(),
                        "Token"
                    }
                    option {
                        value: "{TOKEN_2022_PROGRAM_ID}",
                        selected: text == TOKEN_2022_PROGRAM_ID.to_string(),
                        "Token-2022"
                    }
                }
            }
        },
        Fact::Rent(_) | Fact::MintDecimals(_) => rsx! {
            label { key: "{key}", "{label}"
                input {
                    r#type: "text",
                    inputmode: "numeric",
                    placeholder: "whole number",
                    value: "{text}",
                    oninput: move |e| type_fact(&session, fact, e.value()),
                }
            }
        },
    }
}

/// The unit each argument of each block is typed in, by block number, from
/// its IDL: "SOL" for createAccount's lamports.
fn block_units(
    session: &Session,
    expansion: &Expansion,
) -> HashMap<usize, HashMap<String, String>> {
    let idls = session.idls.read();
    expansion
        .blocks
        .iter()
        .filter_map(|block: &ExpandedBlock| {
            let client = idls.program_client(&block.draft.program_id?)?;
            let form =
                idl_instruction_form(&client, &block.draft.instruction, &block.draft.input())
                    .ok()?;
            let units = form
                .arguments
                .iter()
                .filter_map(|field| Some((field.name.clone(), field.scale.unit()?.to_string())))
                .collect();
            Some((block.number, units))
        })
        .collect()
}

/// What a template writes into one block, a row a field. Blocks are
/// numbered by their place in the transaction, as Review and Sign numbers
/// its instructions.
#[component]
pub(in crate::ui) fn WrittenBlock(
    heading: String,
    rows: Vec<Row>,
    units: HashMap<String, String>,
) -> Element {
    rsx! {
        div { class: "template-written",
            h4 { "{heading}" }
            ul { class: "template-written-rows",
                for row in rows {
                    li { key: "{row.field}",
                        span { class: "template-written-field", "{row.field}" }
                        span { class: "template-written-value",
                            if row.text.trim().is_empty() {
                                span { class: "hint", "not known yet" }
                            } else if let Ok(address) = Pubkey::from_str(row.text.trim()) {
                                Address { pubkey: address }
                            } else if let Some(unit) = units.get(&row.field) {
                                "{row.text} {unit}"
                            } else {
                                "{row.text}"
                            }
                        }
                    }
                }
            }
        }
    }
}
