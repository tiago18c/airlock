//! A pane's less used actions, behind one button at the end of its header.

use super::{Glyph, Icon};
use dioxus::prelude::*;

/// A menu button that opens `children`, each a `button.pane-menu-item`
/// with `role: "menuitem"`, in a list under it. A press on an item, outside
/// the list, or Escape closes it.
///
/// `glyph` is the button's icon, three lines unless given. `small` makes it
/// a small button, for a menu at the end of a row's buttons.
#[component]
pub fn PaneMenu(
    label: &'static str,
    #[props(default = Glyph::Menu)] glyph: Glyph,
    #[props(default)] small: bool,
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "pane-menu",
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Escape {
                    open.set(false);
                }
            },
            button { class: if small { "icon-btn sm" } else { "icon-btn" },
                title: label,
                aria_label: label,
                aria_haspopup: "menu",
                aria_expanded: "{open()}",
                onclick: move |_| open.toggle(),
                Icon { glyph }
            }
            if open() {
                div {
                    class: "pane-menu-backdrop",
                    onclick: move |_| open.set(false),
                }
                div { class: "pane-menu-list",
                    role: "menu",
                    aria_label: label,
                    // An item's press bubbles here once it has run.
                    onclick: move |_| open.set(false),
                    {children}
                }
            }
        }
    }
}
