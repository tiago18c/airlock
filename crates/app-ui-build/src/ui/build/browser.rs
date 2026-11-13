//! The Templates dialog: every template in a list, and the one picked in a
//! reader beside it, as Programs lists IDLs and reads the one picked. On a
//! phone it is one pane at a time: the list, or the template opened from it
//! under a way back.
//!
//! The reader's tabs ([`TemplateReading`]) are drawn from a
//! [`TemplateView`], read from a template and its text, so a template that
//! is not kept yet reads the same way as one that is: the editor
//! (`text.rs`), which takes the place of the list and the reader while a
//! template is written, imported or mended as text, shows its Questions
//! and Blocks from the same parts.

use super::*;
use crate::actions::kept_templates::{export_template, files_here, kept_templates, pick_import};
use crate::actions::template::{template_entries, TemplateEntry};
use crate::actions::template_browser::*;
use crate::actions::template_text::{edit_copy, edit_kept, new_template, TextEdit};
use crate::ui::{use_phone_layout, CopyButton};
use std::sync::Arc;

/// A way out of what the editor holds, which asks first when its text has
/// changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leave {
    /// Close the dialog.
    Close,
    /// Back to the list and the reader.
    Back,
    /// The editor on the starter text.
    New,
    /// A file picked for import, in the editor.
    Import,
}

/// The tabs of the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ReaderTab {
    Questions,
    Blocks,
    Text,
}

/// The entry `key` names, or on a wide window the first that can be used
/// when it names none (none picked yet, or the one picked was removed).
fn picked_entry(
    entries: &[TemplateEntry],
    key: Option<&str>,
    phone: bool,
) -> Option<TemplateEntry> {
    match key.and_then(|key| entries.iter().find(|entry| entry.source.key() == key)) {
        Some(entry) => Some(entry.clone()),
        None if phone => None,
        None => first_usable(entries).cloned(),
    }
}

/// The Templates dialog, from the button in Build's head and the link under
/// Build's short list.
#[component]
pub(in crate::ui) fn TemplateBrowser(
    mut open: Signal<bool>,
    on_pick: EventHandler<TemplateSource>,
) -> Element {
    let session = use_context::<Session>();
    let desk = use_context::<TemplateDesk>();
    let phone = use_phone_layout();
    let entries = use_memo(move || template_entries(&session));
    let mut filter = use_signal(String::new);
    // On a phone the dialog opens on the list; on a wide window on the
    // first template that can be used, which `picked_entry` gives.
    let mut selected = use_signal(|| None::<String>);
    let tab = use_signal(|| ReaderTab::Questions);
    // A template just kept from an import is shown, and its row brought
    // into sight.
    use_effect(move || {
        let Some(key) = (desk.show)() else {
            return;
        };
        selected.set(Some(key));
        let mut show = desk.show;
        show.set(None);
        let _ = app_ui_kit::platform::eval(
            "requestAnimationFrame(() => document.querySelector('.template-pick-item.on')?.scrollIntoView({ block: 'nearest' }));",
        );
    });
    let picked = use_memo(move || picked_entry(&entries(), selected().as_deref(), phone));
    let view = use_memo(move || {
        let entry = picked()?;
        let template = entry.source.template().ok()?;
        Some(template_view(
            &session,
            Arc::new(template),
            entry.source.text().map(str::to_string),
        ))
    });
    let kept = kept_templates(&session);
    let files = files_here();
    let busy = (desk.dialog)();
    let mut removing = desk.removing;
    let mut editing = desk.editing;
    let mut leaving = use_signal(|| None::<Leave>);
    let editor = editing
        .read()
        .as_ref()
        .map(|edit| (edit.opened, edit.target.title()));
    // Where each way out goes, once the person has agreed to lose any
    // changes.
    let mut go = move |leave: Leave| match leave {
        Leave::Close => {
            editing.set(None);
            open.set(false);
        }
        Leave::Back => editing.set(None),
        Leave::New => {
            desk.clear();
            editing.set(Some(new_template()));
        }
        Leave::Import => {
            desk.clear();
            pick_import(editing, desk.error, desk.dialog);
        }
    };
    // Each way out asks first when the editor holds changes.
    let mut leave = move |leave: Leave| {
        if editing.peek().as_ref().is_some_and(TextEdit::dirty) {
            leaving.set(Some(leave));
        } else {
            go(leave);
        }
    };
    let mut start = move |edit: Option<TextEdit>| {
        if let Some(edit) = edit {
            desk.clear();
            editing.set(Some(edit));
        }
    };

    let all = entries();
    let shown: Vec<TemplateEntry> = all
        .iter()
        .filter(|entry| template_matches(entry, &filter()))
        .cloned()
        .collect();
    let entry = picked();
    let picked_key = entry.as_ref().map(|entry| entry.source.key());
    // A phone shows the list until a template is opened from it.
    let show_list = !phone || entry.is_none();
    let show_reader = !phone || entry.is_some();
    let usable = entry
        .as_ref()
        .is_some_and(|entry| entry.problem.is_none() && !entry.is_broken());
    let removable = entry.as_ref().and_then(|entry| {
        entry
            .source
            .slug()
            .map(|slug| (slug.to_string(), entry.name.clone()))
    });
    let exportable = files && entry.as_ref().is_some_and(|entry| !entry.is_broken());

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| leave(Leave::Close),
            div {
                class: if phone { "modal template-browser phone" } else { "modal template-browser" },
                role: "dialog",
                aria_label: "Templates",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "template-browser-head",
                    if let Some((_, title)) = &editor {
                        h3 { class: "editor-title", "{title}" }
                    } else if phone && !show_list {
                        button {
                            class: "ghost template-back",
                            r#type: "button",
                            // What Android's Back presses (MainActivity).
                            "data-back": "true",
                            onclick: move |_| selected.set(None),
                            Icon { glyph: Glyph::ChevronLeft }
                            "Templates"
                        }
                    } else {
                        h3 { "Templates" }
                    }
                    if show_list || editor.is_some() {
                        div { class: "template-browser-ways",
                            button {
                                class: "sm",
                                r#type: "button",
                                title: "Write a template as text, from a short example",
                                onclick: move |_| leave(Leave::New),
                                "New template"
                            }
                            if files {
                                button {
                                    class: "sm",
                                    r#type: "button",
                                    disabled: busy,
                                    title: if busy { "A file dialog is open" } else { "Read a template file into the editor, and see what it does before it is kept" },
                                    onclick: move |_| leave(Leave::Import),
                                    "Import a template"
                                }
                            }
                        }
                    }
                }
                if let Some((opened, _)) = editor {
                    TemplateEditor { key: "{opened}", on_cancel: move |_| leave(Leave::Back) }
                } else {
                div { class: if phone { "template-browser-body one" } else { "template-browser-body" },
                    if show_list {
                        div { class: "template-side",
                            div { class: "filter-bar template-filter",
                                label {
                                    span { class: "sr-only", "Filter templates" }
                                    input {
                                        r#type: "search",
                                        placeholder: "Name or summary",
                                        value: "{filter}",
                                        oninput: move |e| filter.set(e.value()),
                                    }
                                }
                            }
                            if shown.is_empty() {
                                p { class: "muted template-none", "No templates match this filter." }
                            }
                            ul { class: "group-list template-pick-list",
                                for item in shown {
                                    TemplatePickRow {
                                        key: "{item.source.key()}",
                                        on: picked_key.as_deref() == Some(item.source.key().as_str()) && !phone,
                                        phone,
                                        entry: item.clone(),
                                        on_pick: move |key: String| selected.set(Some(key)),
                                    }
                                }
                            }
                            if kept.left_out > 0 {
                                p { class: "hint template-side-note",
                                    "{kept.left_out} more of your templates are kept and not listed: at most {app_ui_kit::platform::storage::MAX_STORED_TEMPLATES} are read."
                                }
                            }
                            if let Some(why) = kept.error.clone() {
                                p { class: "error template-side-note", "{why}" }
                            }
                        }
                    }
                    if show_reader {
                        div { class: "template-reader",
                            match (entry.clone(), view()) {
                                (Some(entry), Some(view)) => rsx! {
                                    ReaderHead {
                                        name: view.template.name.clone(),
                                        summary: view.template.summary.clone(),
                                        chip: entry.source.chip(),
                                        file: entry.source.slug().map(|slug| format!("{slug}.toml")),
                                        problems: view.problems.clone(),
                                    }
                                    div { class: "reader-actions",
                                        if let Some(edit) = edit_kept(&entry.source) {
                                            button { class: "sm",
                                                r#type: "button",
                                                title: "Change its text. Saving writes over it",
                                                onclick: move |_| start(Some(edit.clone())),
                                                "Edit"
                                            }
                                        }
                                        button { class: "sm",
                                            r#type: "button",
                                            title: "Write a template of your own from its text",
                                            onclick: {
                                                let source = entry.source.clone();
                                                move |_| start(edit_copy(&source))
                                            },
                                            "Edit a copy"
                                        }
                                    }
                                    TemplateReading { view, tab }
                                },
                                (Some(entry), None) => rsx! {
                                    BrokenReader {
                                        entry: entry.clone(),
                                        on_edit: move |_| start(edit_kept(&entry.source)),
                                    }
                                },
                                (None, _) => rsx! {
                                    div { class: "empty", p { "No template to show." } }
                                },
                            }
                        }
                    }
                }
                if let Some(text) = (desk.note)() {
                    p { class: "note", "{text}" }
                }
                if let Some(err) = (desk.error)() {
                    p { class: "error", "{err}" }
                }
                div { class: "btn-row",
                    if show_reader && entry.as_ref().is_some_and(|entry| entry.problem.is_some() && !entry.is_broken()) {
                        span { class: "hint btn-lead template-cannot", "Cannot be used on this network." }
                    }
                    button { class: "ghost",
                        r#type: "button",
                        onclick: move |_| leave(Leave::Close),
                        "Close"
                    }
                    if show_reader {
                        if let Some(target) = removable {
                            button {
                                r#type: "button",
                                onclick: move |_| removing.set(Some(target.clone())),
                                "Remove"
                            }
                        }
                        if exportable {
                            button {
                                r#type: "button",
                                disabled: busy,
                                title: if busy { "A file dialog is open" } else { "Save this template to a file" },
                                onclick: {
                                    let source = entry.as_ref().map(|entry| entry.source.clone());
                                    move |_| {
                                        if let Some(source) = &source {
                                            desk.clear();
                                            export_template(source, desk.note, desk.error, desk.dialog);
                                        }
                                    }
                                },
                                "Export"
                            }
                        }
                        if let Some(entry) = entry.clone().filter(|entry| !entry.is_broken()) {
                            button { class: "primary",
                                r#type: "button",
                                disabled: !usable,
                                title: entry.problem.clone().unwrap_or_default(),
                                onclick: {
                                    let source = entry.source.clone();
                                    move |_| {
                                        open.set(false);
                                        on_pick.call(source.clone());
                                    }
                                },
                                "Use this template"
                            }
                        }
                    }
                }
                }
            }
        }
        if let Some(pending) = leaving() {
            DiscardChanges {
                title: editor.as_ref().map(|(_, title)| title.clone()).unwrap_or_default(),
                on_keep: move |_| leaving.set(None),
                on_discard: move |_| {
                    leaving.set(None);
                    go(pending);
                },
            }
        }
    }
}

/// Asks before text the editor holds is left without being saved.
#[component]
fn DiscardChanges(
    title: String,
    on_keep: EventHandler<()>,
    on_discard: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| on_keep.call(()),
            div { class: "modal",
                role: "alertdialog",
                aria_label: "Discard your changes?",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Discard your changes?" }
                    p { class: "lede",
                        "{title}: the text has changed and is not saved. Leaving loses the changes."
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| on_keep.call(()),
                            "Cancel"
                        }
                        button { class: "danger solid",
                            r#type: "button",
                            onclick: move |_| on_discard.call(()),
                            "Discard"
                        }
                    }
                }
            }
        }
    }
}

/// One row of the dialog's list: the name, the summary cut short, and the
/// chip. Picking it shows it in the reader; it carries no buttons.
#[component]
fn TemplatePickRow(
    entry: TemplateEntry,
    on: bool,
    phone: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let broken = entry.is_broken();
    let off = entry.problem.is_some();
    let class = match (on, off) {
        (true, true) => "group-item template-pick-item on off",
        (true, false) => "group-item template-pick-item on",
        (false, true) => "group-item template-pick-item off",
        (false, false) => "group-item template-pick-item",
    };
    let key = entry.source.key();
    rsx! {
        li { class,
            button {
                class: "group-pick template-pick",
                r#type: "button",
                aria_current: on,
                onclick: move |_| on_pick.call(key.clone()),
                span { class: "template-pick-text",
                    span { class: "template-pick-head",
                        span { class: "template-pick-name", "{entry.name}" }
                        span { class: "chip", "{entry.source.chip()}" }
                    }
                    if broken {
                        span { class: "template-pick-problem", "Not a template" }
                    } else {
                        if let Some(summary) = &entry.summary {
                            span { class: "template-pick-summary", "{summary}" }
                        }
                        if off {
                            span { class: "template-pick-problem", "Cannot be used on this network" }
                        }
                    }
                }
                if phone {
                    span { class: "program-chevron", aria_hidden: "true", Icon { glyph: Glyph::ChevronRight } }
                }
            }
        }
    }
}

/// The head of the reader: the name, the summary, where it comes from,
/// and every problem it has on this network.
#[component]
pub(in crate::ui) fn ReaderHead(
    name: String,
    summary: Option<String>,
    /// "built-in" or "yours"; none for a file not kept yet.
    chip: Option<&'static str>,
    /// The file one of the user's is kept as.
    file: Option<String>,
    problems: Vec<String>,
) -> Element {
    rsx! {
        div { class: "reader-head",
            div { class: "reader-title",
                h3 { "{name}" }
                if let Some(chip) = chip {
                    span { class: "chip", "{chip}" }
                }
            }
            if let Some(summary) = summary {
                p { class: "muted reader-summary", "{summary}" }
            }
            if let Some(file) = file {
                p { class: "hint", "Kept on this device as " code { class: "mono", "{file}" } "." }
            }
            if !problems.is_empty() {
                div { class: "warn import-problems reader-problems",
                    p { "Cannot be used on this network." }
                    ul {
                        for problem in problems {
                            li { "{problem}" }
                        }
                    }
                }
            }
        }
    }
}

/// Why a file is not a template, as one sentence that says so.
fn not_a_template(why: &str) -> String {
    if why.starts_with("This ") {
        why.to_string()
    } else {
        format!("This file is not a template. {why}")
    }
}

/// A file of the user's that is not a template: why, and its text when it
/// could be read, which **Edit** opens in the editor to be mended.
#[component]
fn BrokenReader(entry: TemplateEntry, on_edit: EventHandler<()>) -> Element {
    let text = entry.source.text().map(str::to_string);
    rsx! {
        div { class: "reader-head",
            div { class: "reader-title",
                h3 { "{entry.name}" }
                span { class: "chip", "{entry.source.chip()}" }
            }
            p { class: "error", "{not_a_template(entry.problem.as_deref().unwrap_or_default())}" }
            p { class: "hint", "It is kept, and stays out of the list of templates you can use until it is removed or mended." }
        }
        if text.is_some() {
            div { class: "reader-actions",
                button { class: "sm",
                    r#type: "button",
                    title: "Mend its text in the editor. Saving writes over it",
                    onclick: move |_| on_edit.call(()),
                    "Edit"
                }
            }
        }
        if let Some(text) = text {
            div { class: "reader-panel",
                ReaderText { text }
            }
        } else {
            p { class: "muted", "Its text could not be read." }
        }
    }
}

/// The reader's tabs and the one open: the questions, the blocks, and the
/// text.
#[component]
pub(in crate::ui) fn TemplateReading(
    view: TemplateView,
    mut tab: Signal<ReaderTab>,
    /// What the Text tab says its text is.
    #[props(default = "The file as it is kept.")]
    text_note: &'static str,
) -> Element {
    let shown = if view.text.is_none() && tab() == ReaderTab::Text {
        ReaderTab::Questions
    } else {
        tab()
    };
    let tabs = [
        (
            ReaderTab::Questions,
            "Questions",
            Some(view.questions.count()),
        ),
        (ReaderTab::Blocks, "Blocks", Some(view.blocks.len())),
        (ReaderTab::Text, "Text", None),
    ];
    let has_text = view.text.is_some();
    rsx! {
        div { class: "tabs reader-tabs", role: "tablist",
            for (kind, title, count) in tabs.into_iter().filter(|(kind, _, _)| *kind != ReaderTab::Text || has_text) {
                button {
                    key: "{title}",
                    class: if shown == kind { "tab on" } else { "tab" },
                    role: "tab",
                    r#type: "button",
                    aria_selected: shown == kind,
                    onclick: move |_| tab.set(kind),
                    "{title}"
                    if let Some(count) = count {
                        span { class: "tab-count", "{count}" }
                    }
                }
            }
        }
        div { class: "reader-panel", role: "tabpanel",
            match shown {
                ReaderTab::Questions => rsx! { QuestionsTab { questions: view.questions.clone() } },
                ReaderTab::Blocks => rsx! { BlocksTab { blocks: view.blocks.clone() } },
                ReaderTab::Text => rsx! { ReaderText { text: view.text.clone().unwrap_or_default(), note: text_note } },
            }
        }
    }
}

/// Every question in the order the form asks them, then each option with
/// the questions asked only under it.
#[component]
pub(in crate::ui) fn QuestionsTab(questions: ReaderQuestions) -> Element {
    let count = questions.count();
    rsx! {
        if count == 0 && questions.options.is_empty() {
            p { class: "muted", "It asks nothing: everything it writes is fixed or worked out." }
        }
        if !questions.always.is_empty() {
            ul { class: "reader-questions",
                for question in questions.always.iter() {
                    QuestionRow { key: "{question.name}", question: question.clone() }
                }
            }
        }
        if !questions.options.is_empty() {
            h4 { class: "eyebrow reader-eyebrow", "Options · {questions.options.len()}" }
            ul { class: "reader-options",
                for option in questions.options.iter() {
                    li { class: "reader-option", key: "{option.name}",
                        div { class: "reader-option-head",
                            span { class: "reader-option-label", "{option.label}" }
                            if option.on_at_first {
                                span { class: "chip", "on at first" }
                            }
                        }
                        if let Some(hint) = &option.hint {
                            p { class: "hint", "{hint}" }
                        }
                        if let Some(why) = &option.not_here {
                            p { class: "muted", "Not offered on this network. {why}" }
                        }
                        if option.questions.is_empty() {
                            p { class: "muted reader-option-none", "It asks nothing more." }
                        } else {
                            ul { class: "reader-questions reader-option-questions",
                                for question in option.questions.iter() {
                                    QuestionRow { key: "{question.name}", question: question.clone() }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn QuestionRow(question: ReaderQuestion) -> Element {
    rsx! {
        li { class: "reader-question",
            div { class: "reader-question-head",
                span { class: "reader-question-label",
                    "{question.label}"
                    if question.optional {
                        span { class: "muted", " (optional)" }
                    }
                }
                if let Some(takes) = &question.takes {
                    span { class: "reader-takes", "{takes}" }
                }
            }
            if let Some(hint) = &question.hint {
                p { class: "hint", "{hint}" }
            }
            if let Some(until) = &question.until {
                p { class: "reader-until", "Until answered: {until}" }
            }
        }
    }
}

/// Every block in file order, numbered, with every field it sets and where
/// the value comes from.
#[component]
pub(in crate::ui) fn BlocksTab(blocks: Vec<ReaderBlock>) -> Element {
    rsx! {
        if blocks.is_empty() {
            p { class: "muted", "It writes no block." }
        }
        ol { class: "reader-blocks",
            for block in blocks {
                li { class: "reader-block", key: "{block.number}",
                    div { class: "reader-block-head",
                        span { class: "reader-block-number", "{block.number}" }
                        div { class: "reader-block-what",
                            p { class: "reader-block-title",
                                "{block.program} · "
                                code { class: "mono", "{block.instruction}" }
                            }
                            if let Some(id) = block.program_id {
                                p { class: "addr-full reader-program",
                                    code { class: "mono", "{id}" }
                                    CopyButton { text: id.to_string() }
                                }
                            }
                            if let Some(when) = &block.when {
                                p { class: "reader-when", "Only with “{when}”" }
                            }
                        }
                    }
                    if block.accounts.is_empty() && block.arguments.is_empty() {
                        p { class: "muted reader-block-none", "It sets no field: the IDL fills in every one." }
                    }
                    if !block.accounts.is_empty() {
                        FieldRows { title: "Accounts", rows: block.accounts.clone() }
                    }
                    if !block.arguments.is_empty() {
                        FieldRows { title: "Arguments", rows: block.arguments.clone() }
                    }
                }
            }
        }
    }
}

/// The accounts or the arguments a block sets, one row each.
#[component]
fn FieldRows(title: &'static str, rows: Vec<ReaderField>) -> Element {
    rsx! {
        div { class: "reader-fields-group",
            h5 { class: "eyebrow", "{title}" }
            ul { class: "reader-fields",
                for row in rows {
                    li { class: "reader-field", key: "{row.field}",
                        span { class: "reader-field-name mono", "{row.field}" }
                        span { class: "reader-field-from",
                            match row.origin.clone() {
                                FieldOrigin::Asked { label, .. } => rsx! {
                                    span { class: "reader-origin asked", "asked" }
                                    span { "“{label}”" }
                                },
                                FieldOrigin::Fixed { text, address, alias } => rsx! {
                                    span { class: "reader-origin", "fixed" }
                                    if let Some(address) = address {
                                        Address { pubkey: address }
                                    } else if let Some(unit) = &row.unit {
                                        span { class: "mono", "{text}" }
                                        " {unit}"
                                    } else {
                                        span { class: "mono", "{text}" }
                                    }
                                    if let Some(alias) = alias {
                                        code { class: "reader-ref", "${alias}" }
                                    }
                                },
                                FieldOrigin::Worked { value, words } => rsx! {
                                    span { class: "reader-origin worked", "worked out" }
                                    span { "{words}" }
                                    if let Some(value) = value {
                                        code { class: "reader-ref", "${value}" }
                                    }
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A template's file, with Copy.
#[component]
fn ReaderText(
    text: String,
    #[props(default = "The file as it is kept.")] note: &'static str,
) -> Element {
    rsx! {
        div { class: "reader-text",
            div { class: "reader-text-bar",
                span { class: "hint", "{note}" }
                CopyButton { text: text.clone() }
            }
            pre { class: "idl-raw reader-raw", "{text}" }
        }
    }
}
