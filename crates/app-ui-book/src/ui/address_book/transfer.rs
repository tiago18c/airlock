//! Exporting the book to a file, and importing one.

use super::*;

#[component]
pub(in crate::ui) fn ExportModal(mut open: Signal<bool>, mut label: Signal<String>) -> Element {
    let session = use_context::<Session>();
    let json = (session.book)().export_json(&label()).unwrap_or_default();

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| open.set(false),
            div { class: "modal",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Export this device" }
                    p { class: "lede",
                        "The name is written into the file for the other machine. It is not a label on this device."
                    }
                    label { "Group name"
                        input {
                            r#type: "text",
                            value: "{label}",
                            oninput: move |evt| label.set(evt.value()),
                        }
                    }
                    textarea { readonly: true, value: "{json}" }
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| open.set(false),
                            "Close"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn ImportModal(
    mut open: Signal<bool>,
    mut text: Signal<String>,
    mut error: Signal<Option<String>>,
) -> Element {
    let session = use_context::<Session>();
    let mut group = session.forms.contact.group;
    let mut page = session.forms.contact.page;

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| open.set(false),
            div { class: "modal",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Import a group" }
                    p { class: "lede",
                        "Paste an exported address book. The addresses stay under the name in the file, separate from this device."
                    }
                    textarea {
                        placeholder: "Paste the exported JSON",
                        value: "{text}",
                        oninput: move |evt| text.set(evt.value()),
                    }
                    if let Some(message) = error() {
                        p { class: "error", "{message}" }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            onclick: move |_| {
                                let raw = text.read().clone();
                                let mut failure = None;
                                let saved = session.mutate_workspace(|ws| {
                                    if let Err(err) = ws.book.import_json(&raw) {
                                        failure = Some(err);
                                    }
                                });
                                if let Some(err) = failure {
                                    error.set(Some(err.to_string()));
                                    return;
                                }
                                if let Err(err) = saved {
                                    error.set(Some(err.to_string()));
                                    return;
                                }
                                text.set(String::new());
                                error.set(None);
                                if let Some(imported) = (session.book)()
                                    .groups()
                                    .into_iter()
                                    .rev()
                                    .find(|group| group.imported)
                                {
                                    group.set(imported.id);
                                    page.set(0);
                                }
                                open.set(false);
                            },
                            "Import"
                        }
                        button { class: "ghost",
                            onclick: move |_| open.set(false),
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}
