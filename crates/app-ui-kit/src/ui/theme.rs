use super::{Glyph, Icon};
use crate::state::theme::{apply, Theme};
use dioxus::prelude::*;

fn glyph(theme: Theme) -> Glyph {
    match theme {
        Theme::System => Glyph::Monitor,
        Theme::Light => Glyph::Sun,
        Theme::Dark => Glyph::Moon,
    }
}

/// System, light or dark. `labels` adds the name beside each icon.
#[component]
pub fn ThemeToggle(#[props(default)] labels: bool) -> Element {
    let mut theme = use_context::<Signal<Theme>>();

    rsx! {
        div { class: "segmented", role: "group", aria_label: "Theme",
            for option in Theme::ALL {
                button {
                    class: match (theme() == option, labels) {
                        (true, true) => "tab on",
                        (false, true) => "tab",
                        (true, false) => "tab icon-btn on",
                        (false, false) => "tab icon-btn",
                    },
                    key: "{option.value()}",
                    title: "{option.label()} theme",
                    aria_label: "{option.label()} theme",
                    aria_pressed: theme() == option,
                    onclick: move |_| {
                        theme.set(option);
                        apply(option);
                    },
                    Icon { glyph: glyph(option) }
                    if labels {
                        "{option.label()}"
                    }
                }
            }
        }
    }
}
