//! Whether the window is laid out for a phone. The shell provides it; panes
//! read it to leave out what a phone has no room for.

use dioxus::prelude::*;

/// Provided by the shell: true when it was given a phone's More list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PhoneLayout(pub bool);

/// Whether this window is laid out for a phone. False outside the shell.
pub fn use_phone_layout() -> bool {
    try_use_context::<PhoneLayout>().is_some_and(|layout| layout.0)
}

/// A phone's bottom sheet, over a scrim that closes it: the wallet picker,
/// the notifications and More. All three share it so they look like one
/// family: the same scrim, radius, grab handle and title. A sheet opened
/// from the top bar runs to the bottom of the screen, over the tab bar; More
/// belongs to the tab bar and stays docked on it (the shell measures the bar
/// into `--dock-h`).
#[component]
pub fn PhoneSheet(
    /// The sheet's accessible name.
    label: String,
    on_close: EventHandler<()>,
    /// `dialog`, or `menu` for a list of places to go.
    #[props(default = "dialog")]
    role: &'static str,
    /// The heading at the top of the sheet. A sheet with a head of its own,
    /// such as the notifications with their buttons, leaves it out and gives
    /// its heading the `phone-sheet-title` class.
    #[props(default)]
    title: Option<String>,
    /// Extra classes for the sheet.
    #[props(default)]
    class: String,
    children: Element,
) -> Element {
    rsx! {
        // `data-drag: false`: a sheet opened from the top bar is inside it,
        // but a tap on it is not a window move.
        div {
            class: "phone-scrim",
            "data-drag": "false",
            onclick: move |_| on_close.call(()),
        }
        div {
            class: if class.is_empty() { "phone-sheet".to_string() } else { format!("phone-sheet {class}") },
            "data-drag": "false",
            role,
            aria_label: "{label}",
            onclick: move |e| e.stop_propagation(),
            // Only a cue that the sheet can be put away, as on other phone
            // sheets; a tap on the scrim closes it.
            div { class: "phone-sheet-handle", aria_hidden: "true" }
            if let Some(title) = title {
                // A menu holds only its items to a screen reader, and its
                // label already names it.
                h2 {
                    class: "phone-sheet-title",
                    aria_hidden: if role == "menu" { "true" },
                    "{title}"
                }
            }
            {children}
        }
    }
}

/// A bottom sheet of actions on a phone, in place of a menu that would
/// open over what it acts on: a title, one row per action (`children`, each
/// a `button.more-item` with its icon and name) and Close right under them.
/// A pane's menu and a row's three dots open one, whatever the pane.
///
/// A press on a row runs it, then bubbles up to the list and closes the
/// sheet, as a menu closes on a choice: only one sheet is up at a time, and
/// what the row opens is never under this one.
#[component]
pub fn ActionSheet(
    open: Signal<bool>,
    title: String,
    /// What a screen reader calls the sheet.
    label: String,
    children: Element,
) -> Element {
    let mut open = open;
    if !open() {
        return rsx! {};
    }
    rsx! {
        div { class: "modal-backdrop", onclick: move |_| open.set(false),
            div {
                class: "modal action-sheet",
                role: "dialog",
                aria_modal: "true",
                aria_label: "{label}",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    div { class: "action-list",
                        onclick: move |_| open.set(false),
                        {children}
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            r#type: "button",
                            onclick: move |_| open.set(false),
                            "Close"
                        }
                    }
                }
            }
        }
    }
}
