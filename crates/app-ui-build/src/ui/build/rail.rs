//! The row of instruction blocks: pick one, reorder them, add another.
//! `parent` is the block that carries them, or `None` for the transaction's
//! own.

use super::*;

#[component]
pub(in crate::ui) fn InstructionRail(parent: Option<u64>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.build;
    let mut dragging = use_signal(|| None::<usize>);
    let drafts = form.blocks(parent);
    let selected_id = form.selected_in(parent);
    let new_tip = if parent.is_some() {
        "Add an instruction inside this one"
    } else {
        "Add a new instruction"
    };

    rsx! {
        div { class: "ix-rail",
            for (i, draft) in drafts.iter().cloned().enumerate() {
                {
                    let id = draft.id;
                    let is_on = selected_id == Some(id);
                    let program = draft.program_label();
                    let name = draft.instruction_label();
                    let carries = matches!(draft.body, IxBody::Carried { .. });
                    let last = i + 1 == drafts.len();
                    rsx! {
                        div {
                            class: match (is_on, carries) {
                                (true, true) => "ix-block carrier on",
                                (true, false) => "ix-block on",
                                (false, true) => "ix-block carrier",
                                (false, false) => "ix-block",
                            },
                            key: "{id}",
                            draggable: true,
                            ondragstart: move |e| {
                                // A block inside another is not the outer one.
                                e.stop_propagation();
                                dragging.set(Some(i));
                            },
                            ondragover: move |e| e.prevent_default(),
                            ondrop: move |e| {
                                e.stop_propagation();
                                if let Some(from) = dragging() {
                                    form.move_instruction(parent, from, i);
                                }
                                dragging.set(None);
                            },
                            onclick: move |_| form.select(parent, id),
                            button {
                                class: "ix-block-remove tip tip-end",
                                aria_label: "Remove this instruction",
                                "data-tip": "Remove this instruction",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    form.remove_instruction(id);
                                },
                                svg {
                                    view_box: "0 0 24 24",
                                    "aria-hidden": "true",
                                    path { d: "M4 7h16" }
                                    path { d: "M9 7V5h6v2" }
                                    path { d: "M7 7l1 13h8l1-13" }
                                    path { d: "M10 11v6M14 11v6" }
                                }
                            }
                            span { class: "ix-block-program", "{program}" }
                            span { class: "ix-block-name", "{name}" }
                            div { class: "ix-block-move",
                                button {
                                    class: "ghost icon-btn sm",
                                    title: "Move earlier",
                                    aria_label: "Move earlier",
                                    disabled: i == 0,
                                    onclick: move |e| {
                                        e.stop_propagation();
                                        if i > 0 {
                                            form.move_instruction(parent, i, i - 1);
                                        }
                                    },
                                    Icon { glyph: Glyph::ChevronLeft }
                                }
                                button {
                                    class: "ghost icon-btn sm",
                                    title: "Move later",
                                    aria_label: "Move later",
                                    disabled: last,
                                    onclick: move |e| {
                                        e.stop_propagation();
                                        form.move_instruction(parent, i, i + 1);
                                    },
                                    Icon { glyph: Glyph::ChevronRight }
                                }
                            }
                        }
                    }
                }
            }
            button {
                class: "ix-block ix-block-new tip",
                aria_label: "{new_tip}",
                "data-tip": "{new_tip}",
                ondragover: move |e| e.prevent_default(),
                ondrop: move |e| {
                    e.stop_propagation();
                    if let Some(from) = dragging() {
                        let len = form.blocks(parent).len();
                        if len > 0 {
                            form.move_instruction(parent, from, len - 1);
                        }
                    }
                    dragging.set(None);
                },
                onclick: move |_| {
                    form.add_instruction(
                        parent,
                        IxBody::Idl(pinned_draft(&session, &PINNED_PROGRAMS[0])),
                    );
                },
                span { class: "ix-plus", Icon { glyph: Glyph::Plus } }
            }
        }
    }
}
