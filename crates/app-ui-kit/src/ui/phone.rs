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
/// family: the same scrim and radius, and the same edge, flush on the tab
/// bar wherever it sits (the shell measures the bar into `--dock-h`).
#[component]
pub fn PhoneSheet(
    /// The sheet's accessible name.
    label: String,
    on_close: EventHandler<()>,
    /// `dialog`, or `menu` for a list of places to go.
    #[props(default = "dialog")]
    role: &'static str,
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
            {children}
        }
    }
}
