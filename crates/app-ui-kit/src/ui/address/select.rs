//! A dropdown over a fixed list of addresses, each drawn as a row.

use super::*;

/// Pick one of `options`, or none when `none_label` is given. Each entry is
/// an [`AddressRow`], so the list reads like every other list of addresses.
#[component]
pub fn AddressSelect(
    label: String,
    options: Vec<Pubkey>,
    selected: Option<Pubkey>,
    /// The entry for "none". Without it, nothing can be unpicked.
    #[props(default)]
    none_label: Option<String>,
    on_change: EventHandler<Option<Pubkey>>,
) -> Element {
    let mut open = use_signal(|| false);
    // Cards of the entries open beside the list, not over it.
    let beside = crate::ui::address::use_card_beside();
    let item_class = |on: bool| {
        if on {
            "picker-item active"
        } else {
            "picker-item"
        }
    };
    let placeholder = none_label
        .clone()
        .unwrap_or_else(|| "Choose an address".to_string());
    rsx! {
        div { class: if open() { "picker open" } else { "picker" },
            if open() {
                div { class: "picker-backdrop", onclick: move |_| open.set(false) }
            }
            label { "{label}"
                button {
                    r#type: "button",
                    class: "picker-button",
                    "aria-haspopup": "listbox",
                    "aria-expanded": "{open()}",
                    onclick: move |_| open.set(!open()),
                    onkeydown: move |evt| {
                        if evt.key() == Key::Escape {
                            open.set(false);
                        }
                    },
                    match selected {
                        Some(pubkey) => rsx! { AddressRow { pubkey, no_copy: true } },
                        None => rsx! { span { class: "picker-none", "{placeholder}" } },
                    }
                }
            }
            if open() {
                div { class: "picker-menu", role: "listbox",
                    onmounted: move |e| beside.mounted(e),
                    if let Some(none) = none_label.clone() {
                        button {
                            r#type: "button",
                            class: item_class(selected.is_none()),
                            onclick: move |_| {
                                open.set(false);
                                on_change.call(None);
                            },
                            span { class: "picker-label", "{none}" }
                        }
                    }
                    for pubkey in options {
                        button {
                            r#type: "button",
                            key: "{pubkey}",
                            class: item_class(selected == Some(pubkey)),
                            onclick: move |_| {
                                open.set(false);
                                on_change.call(Some(pubkey));
                            },
                            AddressRow { pubkey, no_copy: true }
                        }
                    }
                }
            }
        }
    }
}
