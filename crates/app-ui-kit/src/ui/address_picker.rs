//! The field that finds an address by label, tag or the address itself.

use super::AddressRow;
use crate::actions::addresses::*;
use crate::state::session::Session;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use wallet_core::TokenFlag;

#[component]
pub fn AddressPicker(
    label: String,
    selected: Option<Pubkey>,
    on_change: EventHandler<Option<Pubkey>>,
    reset: Signal<u64>,
) -> Element {
    let session = use_context::<Session>();
    let mut query = use_signal(String::new);
    let mut open = use_signal(|| false);
    let mut focused = use_signal(|| false);
    let mut picking = use_signal(|| false);
    let mut active = use_signal(|| 0usize);
    // Cards of the entries open beside the list, not over it.
    let beside = super::address::use_card_beside();
    // Not a signal: the effect below must only rerun when `reset` moves.
    let mut seen_reset = use_hook(|| CopyValue::new(*reset.peek()));
    use_effect(move || {
        let current_reset = reset();
        if *seen_reset.peek() == current_reset {
            return;
        }
        seen_reset.set(current_reset);
        query.set(String::new());
        open.set(false);
        focused.set(false);
        picking.set(false);
        active.set(0);
    });

    let book = (session.book)();
    // Choices are only built while the menu is open (P4).
    let choices = if open() {
        let holding_hits = session.search_holdings(&query(), 8);
        picker_choices(&query(), book.as_ref(), &holding_hits, |pk| {
            session.flag_of(pk)
        })
    } else {
        Vec::new()
    };
    let choices_key = choices.clone();
    let editing = focused();
    let place = selected.and_then(|pk| book.place(&pk));
    let selected_flag = selected.and_then(|pk| session.flag_of(&pk));
    let show_display = !editing && selected.is_some();
    let leftover_pubkey =
        !editing && selected.is_none() && Pubkey::from_str(query.read().trim()).is_ok();
    let invalid =
        !editing && selected.is_none() && !query.read().trim().is_empty() && !leftover_pubkey;
    let field_class = if show_display && selected_flag == Some(TokenFlag::Red) {
        "picker-field flagged"
    } else if show_display && place.as_ref().is_some_and(|found| found.imported) {
        "picker-field imported"
    } else if show_display {
        "picker-field valid"
    } else if invalid {
        "picker-field invalid"
    } else {
        "picker-field"
    };
    let input_value = if editing {
        query()
    } else if show_display || leftover_pubkey {
        String::new()
    } else {
        query()
    };

    let commit = move |raw: String| match resolve_address(raw.trim(), (session.book)().as_ref()) {
        Ok(pk) => on_change.call(pk),
        Err(()) => on_change.call(None),
    };

    rsx! {
        div { class: if open() { "picker open" } else { "picker" },
            if open() {
                div {
                    class: "picker-backdrop",
                    onclick: move |_| {
                        if !picking() {
                            commit(query());
                            open.set(false);
                            focused.set(false);
                            let _ = crate::platform::eval(
                                "document.activeElement && document.activeElement.blur();",
                            );
                        }
                    },
                }
            }
            label { "{label}"
                div { class: "{field_class}",
                    input {
                        r#type: "text",
                        class: if show_display { "picker-input masked" } else { "picker-input" },
                        placeholder: if show_display {
                            ""
                        } else {
                            "Address, label, or tag"
                        },
                        value: "{input_value}",
                        spellcheck: false,
                        autocomplete: "off",
                        onfocus: move |_| {
                            focused.set(true);
                            open.set(true);
                            active.set(0);
                            if let Some(pk) = selected {
                                query.set(pk.to_string());
                            } else if Pubkey::from_str(query.read().trim()).is_ok() {
                                query.set(String::new());
                            }
                        },
                        onblur: move |_| {
                            if picking() {
                                return;
                            }
                            commit(query());
                            focused.set(false);
                            open.set(false);
                        },
                        oninput: move |e| {
                            query.set(e.value());
                            open.set(true);
                            focused.set(true);
                            active.set(0);
                        },
                        onkeydown: move |e| {
                            let key = e.key().to_string();
                            let n = choices_key.len();
                            match key.as_str() {
                                "ArrowDown" | "Down" => {
                                    e.prevent_default();
                                    if n == 0 {
                                        open.set(true);
                                        return;
                                    }
                                    let next = if open() {
                                        (active() + 1) % n
                                    } else {
                                        0
                                    };
                                    open.set(true);
                                    active.set(next);
                                    scroll_active_item(next);
                                }
                                "ArrowUp" | "Up" => {
                                    e.prevent_default();
                                    if n == 0 {
                                        open.set(true);
                                        return;
                                    }
                                    let next = if open() {
                                        (active() + n - 1) % n
                                    } else {
                                        n - 1
                                    };
                                    open.set(true);
                                    active.set(next);
                                    scroll_active_item(next);
                                }
                                "Enter" => {
                                    e.prevent_default();
                                    if open() {
                                        if let Some(choice) =
                                            choices_key.get(active().min(n.saturating_sub(1)))
                                        {
                                            apply_pick(
                                                choice.pubkey,
                                                query,
                                                open,
                                                focused,
                                                picking,
                                                on_change,
                                            );
                                            return;
                                        }
                                    }
                                    commit(query());
                                    open.set(false);
                                    focused.set(false);
                                    let _ = crate::platform::eval(
                                        "document.activeElement && document.activeElement.blur();",
                                    );
                                }
                                "Escape" => open.set(false),
                                _ => {}
                            }
                        },
                    }
                    if show_display {
                        if let Some(pk) = selected {
                            div { class: "picker-display",
                                AddressRow { pubkey: pk, no_copy: true }
                            }
                        }
                    }
                }
            }
            if open() {
                div {
                    class: "picker-menu",
                    onmounted: move |e| beside.mounted(e),
                    onmousedown: move |e| {
                        picking.set(true);
                        e.prevent_default();
                    },
                    if choices.is_empty() {
                        p { class: "muted picker-empty", "No matches in the address book." }
                    }
                    for (i, choice) in choices_key.clone().into_iter().enumerate() {
                            button {
                                class: picker_item_class(i == active(), &choice),
                                key: "{choice.group_id}:{choice.pubkey}",
                                "data-index": "{i}",
                                onclick: move |_| {
                                    apply_pick(
                                        choice.pubkey,
                                        query,
                                        open,
                                        focused,
                                        picking,
                                        on_change,
                                    );
                                },
                                AddressRow { pubkey: choice.pubkey, no_copy: true }
                            }
                    }
                }
            }
        }
    }
}

fn picker_item_class(active: bool, choice: &PickerChoice) -> String {
    let mut class = String::from("picker-item");
    if active {
        class.push_str(" active");
    }
    if choice.flag == Some(TokenFlag::Red) {
        class.push_str(" flagged");
    } else if choice.group_label.is_some() {
        class.push_str(" imported");
    }
    class
}

fn apply_pick(
    pubkey: Pubkey,
    mut query: Signal<String>,
    mut open: Signal<bool>,
    mut focused: Signal<bool>,
    mut picking: Signal<bool>,
    on_change: EventHandler<Option<Pubkey>>,
) {
    query.set(pubkey.to_string());
    on_change.call(Some(pubkey));
    picking.set(false);
    open.set(false);
    focused.set(false);
    let _ = crate::platform::eval("document.activeElement && document.activeElement.blur();");
}

fn scroll_active_item(index: usize) {
    let _ = crate::platform::eval(format!(
        r#"requestAnimationFrame(() => {{
            const el = document.querySelector('.picker.open .picker-item[data-index="{index}"]');
            if (el) el.scrollIntoView({{ block: 'nearest' }});
        }});"#
    ));
}
