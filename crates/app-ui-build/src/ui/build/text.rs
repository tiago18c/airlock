//! The Templates dialog's editor: a template written as text, read as it
//! is typed. It takes the place of the list and the reader while it is
//! open.
//!
//! Rule 3 holds here: what Save keeps is shown beside it, read from the
//! text as it stands, the template's Questions and Blocks one after the
//! other, with no tab to open. On a wide dialog the text is on the left and
//! the reading on the right, each scrolling on its own; a narrow dialog and
//! a phone put the reading under the text on one scrolling page. The
//! markup is one; the stylesheet places it.
//!
//! The field is a plain `textarea`, given its text once when it is made
//! (`initial_value`) and never set back while it is typed in, so the caret
//! stays where it is. Its line numbers are a column beside it that follows
//! its scrolling.

use super::*;
use crate::actions::template_browser::TemplateView;
use crate::actions::template_text::*;

/// Keeps the line numbers beside the field in step with its scrolling.
const FOLLOW_SCROLL_JS: &str = r#"
(() => {
    const field = document.getElementById(ID);
    if (!field || field.dataset.followed) return;
    field.dataset.followed = "1";
    const numbers = field.parentElement.querySelector(".editor-gutter");
    const follow = () => { numbers.scrollTop = field.scrollTop; };
    field.addEventListener("scroll", follow, { passive: true });
    follow();
})();
"#;

/// Selects line LINE in the field and brings it into sight.
const SHOW_LINE_JS: &str = r#"
(() => {
    const field = document.getElementById(ID);
    if (!field) return;
    const lines = field.value.split("\n");
    const line = Math.min(Math.max(LINE, 1), lines.length);
    let start = 0;
    for (let i = 0; i < line - 1; i++) start += lines[i].length + 1;
    field.focus();
    field.setSelectionRange(start, start + lines[line - 1].length);
    const height = parseFloat(getComputedStyle(field).lineHeight) || 20;
    field.scrollTop = Math.max(0, (line - 1) * height - field.clientHeight / 2);
    field.scrollIntoView({ block: "nearest" });
})();
"#;

fn field_id(opened: u64) -> String {
    format!("template-text-{opened}")
}

fn show_line(opened: u64, line: usize) {
    let script = SHOW_LINE_JS
        .replace("ID", &format!("\"{}\"", field_id(opened)))
        .replace("LINE", &line.to_string());
    let _ = app_ui_kit::platform::eval(&script);
}

/// The editor, on the text in `desk.editing`, with its own last row of
/// buttons. `on_cancel` asks before leaving when the text has changed.
#[component]
pub(in crate::ui) fn TemplateEditor(on_cancel: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let desk = use_context::<TemplateDesk>();
    let mut editing = desk.editing;
    let (mut note, mut show) = (desk.note, desk.show);
    let mut failed = use_signal(|| None::<String>);
    // The reading of the last text that read as a template, shown, marked
    // as not current, while the text as it stands does not.
    let mut last_read = use_signal(|| None::<TemplateView>);
    let read = use_memo(move || {
        let edit = editing.read();
        edit.as_ref()
            .map(|edit| read_text(&session, &edit.target, &edit.text))
    });
    use_effect(move || {
        if let Some(TextRead::Reads { view, .. }) = read() {
            last_read.set(Some(view));
        }
    });

    let Some(edit) = editing() else {
        return rsx! {};
    };
    let Some(read) = read() else {
        return rsx! {};
    };
    let opened = edit.opened;
    let id = field_id(opened);
    let numbers: String = (1..=edit.text.split('\n').count())
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let unsaved = edit.dirty() || !matches!(edit.target, EditTarget::Edit { .. });
    let replaces = read.replaces().map(str::to_string);
    let can_save = read.can_save();
    let target = edit.target.clone();
    let text = edit.text.clone();
    let mut save = move |replace: bool| match save_text(&session, &target, &text, replace) {
        Ok(TextSaved::Saved(saved)) => {
            note.set(Some(saved.note));
            show.set(Some(saved.key));
            editing.set(None);
        }
        Ok(TextSaved::WouldReplace(existing)) => failed.set(Some(format!(
            "You already have a template called “{existing}”. Saving replaces it."
        ))),
        Err(why) => failed.set(Some(why)),
    };
    let (current, stale) = match (read.view(), last_read()) {
        (Some(view), _) => (Some(view.clone()), false),
        (None, Some(view)) => (Some(view), true),
        (None, None) => (None, false),
    };

    rsx! {
        div { class: "template-editor",
            div { class: "editor-text",
                div { class: "editor-field",
                    pre { class: "editor-gutter", aria_hidden: "true", "{numbers}" }
                    textarea {
                        id: "{id}",
                        class: "editor-input",
                        aria_label: "Template text",
                        wrap: "off",
                        spellcheck: "false",
                        autocapitalize: "off",
                        autocomplete: "off",
                        "autocorrect": "off",
                        initial_value: "{edit.original}",
                        oninput: move |e| {
                            if let Some(edit) = editing.write().as_mut() {
                                edit.text = e.value();
                            }
                            failed.set(None);
                        },
                        onmounted: move |_| {
                            let script = FOLLOW_SCROLL_JS.replace("ID", &format!("\"{}\"", field_id(opened)));
                            let _ = app_ui_kit::platform::eval(&script);
                        },
                    }
                }
            }
            div { class: "editor-reading",
                div { class: "editor-status", role: "status", aria_live: "polite",
                    EditorStatus { read: read.clone(), opened }
                    if let Some(why) = failed() {
                        p { class: "error", "{why}" }
                    }
                }
                if let Some(view) = current {
                    div { class: if stale { "editor-view stale" } else { "editor-view" },
                        if stale {
                            p { class: "editor-stale-note",
                                "As of the last text that read as a template. The text as it stands does not, so this is not what it would keep."
                            }
                        }
                        ReaderHead {
                            name: view.template.name.clone(),
                            summary: view.template.summary.clone(),
                            problems: Vec::new(),
                        }
                        h4 { class: "editor-section",
                            "Questions"
                            span { class: "tab-count", "{view.questions.count()}" }
                        }
                        div { class: "reader-panel",
                            QuestionsTab { questions: view.questions.clone() }
                        }
                        h4 { class: "editor-section",
                            "Blocks"
                            span { class: "tab-count", "{view.blocks.len()}" }
                        }
                        div { class: "reader-panel",
                            BlocksTab { blocks: view.blocks.clone() }
                        }
                    }
                } else {
                    p { class: "muted editor-nothing",
                        "Nothing reads yet. What the template asks and writes shows here once its text reads as a template."
                    }
                }
            }
        }
        if let Some(err) = (desk.error)() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            if unsaved {
                span { class: "hint btn-lead", "Not saved yet" }
            }
            button { class: "ghost",
                r#type: "button",
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
            if replaces.is_some() && can_save {
                button { class: "danger solid",
                    r#type: "button",
                    title: "Keep this text in place of your template of that name",
                    onclick: move |_| save(true),
                    "Replace"
                }
            } else {
                button { class: "primary",
                    r#type: "button",
                    disabled: !can_save,
                    title: if can_save { "Keep this text as it is, as one of your templates" } else { "It can be saved once it reads as a template under a name of its own" },
                    onclick: move |_| save(false),
                    "Save"
                }
            }
        }
    }
}

/// What the editor says of the text as it stands: its size, then why it
/// does not read as a template with the line it points at, then what it
/// asks and writes, every problem it has on this network, and its name.
#[component]
fn EditorStatus(read: TextRead, opened: u64) -> Element {
    match read {
        TextRead::Refused(Refusal::TooLarge { bytes }) => rsx! {
            p { class: "error", "{too_large(bytes)}" }
        },
        TextRead::Refused(Refusal::NotATemplate { reason, excerpt }) => rsx! {
            div { class: "editor-error",
                p { class: "error",
                    "It does not read as a template. "
                    span { class: "editor-reason", "{reason}" }
                }
                if let Some(excerpt) = excerpt {
                    pre { class: "editor-excerpt",
                        span { class: "editor-excerpt-line", "{excerpt.line}" }
                        span { class: "editor-excerpt-text",
                            "{excerpt.before}"
                            // Past the end of the line, a space is marked.
                            mark { if excerpt.at.is_empty() { "\u{a0}" } else { "{excerpt.at}" } }
                            "{excerpt.after}"
                        }
                    }
                    button { class: "link-btn",
                        r#type: "button",
                        onclick: move |_| show_line(opened, excerpt.line),
                        "Show line {excerpt.line} in the text"
                    }
                }
            }
        },
        TextRead::Reads { view, name } => rsx! {
            p { class: "editor-reads", "{reads_as(&view)}" }
            if !view.problems.is_empty() {
                div { class: "warn import-problems",
                    p { "Cannot be used on this network. It can still be saved: it is listed greyed until it can be used." }
                    ul {
                        for problem in view.problems.iter() {
                            li { "{problem}" }
                        }
                    }
                }
            }
            match name {
                NameCheck::Builtin(why) => rsx! { p { class: "error", "{why}" } },
                NameCheck::Replaces(existing) => rsx! {
                    p { class: "warn", "You already have a template called “{existing}”. Saving replaces it." }
                },
                NameCheck::Free | NameCheck::Own => rsx! {},
            }
        },
    }
}
