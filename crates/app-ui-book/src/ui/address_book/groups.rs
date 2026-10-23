//! The list of groups beside the address list.

use super::*;

#[component]
pub(in crate::ui) fn GroupSidebar() -> Element {
    let session = use_context::<Session>();
    let groups = (session.book)().groups();

    rsx! {
        ul { class: "group-list",
            for group in groups {
                GroupItem { key: "{group.id}", info: group }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn GroupItem(info: BookGroupInfo) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.contact;
    let mut selected = form.group;
    let mut page = form.page;
    let mut renaming = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let id = info.id.clone();
    let pick_id = id.clone();
    let rename_id = id.clone();
    let delete_id = id.clone();
    let stored_label = info.label.clone();
    let edit_label = stored_label.clone();
    let on = selected() == id;
    let name = stored_label.unwrap_or_else(|| "Local".to_string());
    let entries = info.entries;
    let imported = info.imported;

    rsx! {
        li { class: if on { "group-item on" } else { "group-item" },
            if renaming() {
                div { class: "group-rename",
                    input {
                        r#type: "text",
                        value: "{draft}",
                        oninput: move |evt| draft.set(evt.value()),
                    }
                    button {
                        onclick: move |_| {
                            let name = draft.read().trim().to_string();
                            if name.is_empty() {
                                return;
                            }
                            let id = rename_id.clone();
                            session.edit_workspace(|ws| {
                                ws.book.rename_group(&id, name);
                            });
                            renaming.set(false);
                        },
                        "Save"
                    }
                }
            } else {
                button {
                    class: "group-pick",
                    onclick: move |_| {
                        selected.set(pick_id.clone());
                        page.set(0);
                    },
                    span { class: "group-name", "{name}" }
                    span { class: "muted", "{entries}" }
                }
                if imported {
                    div { class: "group-actions",
                        button {
                            class: "ghost icon-btn sm",
                            title: "Edit",
                            aria_label: "Edit",
                            onclick: move |_| {
                                draft.set(edit_label.clone().unwrap_or_default());
                                renaming.set(true);
                            },
                            Icon { glyph: Glyph::Pencil }
                        }
                        button {
                            class: "ghost danger icon-btn sm",
                            title: "Delete",
                            aria_label: "Delete",
                            onclick: move |_| {
                                let id = delete_id.clone();
                                let viewing = selected() == id;
                                session.edit_workspace(|ws| {
                                    ws.book.remove_group(&id);
                                });
                                if viewing {
                                    selected.set(LOCAL_GROUP_ID.to_string());
                                    page.set(0);
                                }
                            },
                            Icon { glyph: Glyph::Trash }
                        }
                    }
                }
            }
        }
    }
}
