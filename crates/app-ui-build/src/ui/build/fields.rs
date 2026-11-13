//! The field for one account or one argument of an instruction.

use super::*;

pub(in crate::ui) fn idl_account_field(
    form: crate::state::forms::BuildForm,
    id: u64,
    idl: &IdlDraft,
    account: IdlAccountField,
) -> Element {
    let name = account.name.clone();
    let manual = idl.is_manual(&account.name);
    let selected = idl.account(&account.name);
    let label = account_label(&account);

    if account.source == IdlAccountSource::Derived && !manual {
        let note = derived_note(&account);
        let address = account.address;
        let change_name = name.clone();
        return rsx! {
            div { class: "idl-derived",
                span { class: "idl-derived-name", "{name}" }
                span { class: "hint", "{note}" }
                if let Some(address) = address {
                    Address { pubkey: address }
                }
                // Such as Token-2022 where the IDL defaults to the Token program.
                button { class: "ghost sm",
                    r#type: "button",
                    onclick: move |_| {
                        form.patch_idl(id, |d| d.set_manual(&change_name, true));
                    },
                    "Change"
                }
            }
        };
    }
    let overridden = account.source == IdlAccountSource::Derived && manual;
    let default_name = name.clone();

    let note = if account.source == IdlAccountSource::Fill || manual {
        fill_note(&account)
    } else {
        String::new()
    };
    let signer = account.signer;
    let made_here =
        selected.is_some_and(|pk| form.generated.read().iter().any(|key| key.pubkey == pk));
    let generate_name = name.clone();
    rsx! {
        if !note.is_empty() {
            p { class: "hint", "{note}" }
        }
        AddressPicker {
            label,
            selected,
            reset: form.picker_reset,
            on_change: move |pk| {
                form.patch_idl(id, |d| {
                    d.set_account(&name, pk);
                });
            },
        }
        if overridden {
            div { class: "btn-row",
                button { class: "ghost sm",
                    r#type: "button",
                    onclick: move |_| {
                        form.patch_idl(id, |d| d.set_manual(&default_name, false));
                    },
                    "Use default"
                }
            }
        }
        // A new account signs for its own creation. A keypair made here
        // signs as it on Sign.
        if signer {
            div { class: "btn-row",
                button { class: "ghost sm",
                    r#type: "button",
                    onclick: move |_| {
                        let key = GeneratedKey::new();
                        let pubkey = key.pubkey;
                        let mut generated = form.generated;
                        generated.write().push(key);
                        form.patch_idl(id, |d| {
                            d.set_account(&generate_name, Some(pubkey));
                        });
                    },
                    "New keypair"
                }
                if made_here {
                    span { class: "hint",
                        "Made on this device. It signs in Review and Sign, and is kept in memory until the app closes."
                    }
                }
            }
        }
    }
}

pub(in crate::ui) fn idl_argument_field(
    form: crate::state::forms::BuildForm,
    id: u64,
    idl: &IdlDraft,
    argument: wallet_core::IdlArgField,
) -> Element {
    let name = argument.name.clone();
    let on_change = Callback::new(move |value: String| {
        form.patch_idl(id, |d| {
            d.set_argument(&name, value);
        });
    });
    argument_input(
        argument_label(&argument),
        argument_placeholder(&argument),
        input_mode(&argument.kind, &argument_places(&argument)),
        &argument.kind,
        idl.argument(&argument.name).to_string(),
        form.picker_reset,
        on_change,
    )
}

/// The widget for an argument of kind `kind`, holding `text`: a checkbox
/// for a boolean, a select for an enum, the address picker for a public
/// key, and a text field for the rest. Build's blocks and a template's
/// inputs both draw their arguments with it. An enum that is an option can
/// be left at None.
pub(in crate::ui) fn argument_input(
    label: String,
    placeholder: String,
    mode: Option<&'static str>,
    kind: &IdlArgKind,
    text: String,
    reset: Signal<u64>,
    on_change: Callback<String>,
) -> Element {
    if bare_bool(kind) {
        let checked = text == "true";
        return rsx! {
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e| {
                        let value = if e.checked() { "true" } else { "false" };
                        on_change.call(value.to_string());
                    },
                }
                " {label}"
            }
        };
    }

    if let Some(variants) = enum_variants(kind) {
        let variants = variants.to_vec();
        let optional = matches!(kind, IdlArgKind::Option(_));
        // A required choice with no answer yet, such as a template's question
        // with no default, says so, rather than showing its first variant
        // while it holds nothing: picking the variant shown would change
        // nothing. Build's own blocks start on a variant, so they never show
        // it.
        let unanswered = !optional && !variants.contains(&text);
        return rsx! {
            label { "{label}"
                select {
                    value: "{text}",
                    onchange: move |e| on_change.call(e.value()),
                    if optional {
                        option { value: "", selected: text.is_empty(), "None" }
                    }
                    if unanswered {
                        option { value: "", selected: true, "Choose one" }
                    }
                    for variant in variants {
                        option {
                            value: "{variant}",
                            selected: variant == text,
                            "{variant}"
                        }
                    }
                }
            }
        };
    }

    if arg_pubkey(kind) {
        let selected = Pubkey::from_str(text.trim()).ok();
        return rsx! {
            AddressPicker {
                label,
                selected,
                reset,
                on_change: move |pk: Option<Pubkey>| {
                    on_change.call(pk.map(|pk| pk.to_string()).unwrap_or_default());
                },
            }
        };
    }

    rsx! {
        label { "{label}"
            input {
                r#type: "text",
                inputmode: mode,
                placeholder: "{placeholder}",
                value: "{text}",
                oninput: move |e| on_change.call(e.value()),
            }
        }
    }
}

/// The keyboard a phone opens for a field of kind `kind` typed with
/// `places`: digits for a whole number, digits and a decimal point for an
/// amount with decimals. A signed or other field keeps the full keyboard,
/// which has the minus sign, the letters and the brackets it may need.
pub(in crate::ui) fn input_mode(kind: &IdlArgKind, places: &Places) -> Option<&'static str> {
    let inner = match kind {
        IdlArgKind::Option(inner) => inner.as_ref(),
        kind => kind,
    };
    match inner {
        IdlArgKind::Integer {
            float: true,
            signed: false,
            ..
        } => Some("decimal"),
        IdlArgKind::Integer { signed: false, .. } => match places {
            Places::BaseUnits | Places::Fixed(0) => Some("numeric"),
            Places::Fixed(_) | Places::Like(_) => Some("decimal"),
        },
        _ => None,
    }
}

/// An argument's name, and the unit it is typed in when its IDL names one:
/// "amount · SOL".
pub(in crate::ui) fn argument_label(argument: &IdlArgField) -> String {
    with_unit(&argument.name, argument.scale.unit())
}

/// A field's name, then its unit when it has one: "Fee · %".
pub(in crate::ui) fn with_unit(name: &str, unit: Option<&str>) -> String {
    match unit {
        Some(unit) => format!("{name} · {unit}"),
        None => name.to_string(),
    }
}

/// How many decimal places an argument is typed with.
pub(in crate::ui) fn argument_places(argument: &IdlArgField) -> Places {
    match &argument.scale {
        IdlArgScale::None => Places::BaseUnits,
        IdlArgScale::Fixed { decimals, .. } => Places::Fixed(*decimals),
        IdlArgScale::FromArgument { argument, .. } => Places::Like(argument.clone()),
    }
}

/// How many decimal places a number is typed with.
pub(in crate::ui) enum Places {
    /// In base units, as the program reads it.
    BaseUnits,
    Fixed(u8),
    /// As many as another field holds, not known yet.
    Like(String),
}

/// What a text field takes: an amount says how many decimal places, and a
/// number the IDL gives no unit for is in base units.
pub(in crate::ui) fn argument_placeholder(argument: &IdlArgField) -> String {
    let places = argument_places(argument);
    let blank = if argument.blank_uses_default {
        Some("blank for the default")
    } else if matches!(argument.kind, IdlArgKind::Option(_) | IdlArgKind::List(_)) {
        Some("blank for none")
    } else {
        None
    };
    kind_placeholder(&argument.kind, places, blank)
}

/// What a text field of kind `kind` takes, typed with `places`, and what
/// leaving it blank does, when it can be.
pub(in crate::ui) fn kind_placeholder(
    kind: &IdlArgKind,
    places: Places,
    blank: Option<&str>,
) -> String {
    let inner = match kind {
        IdlArgKind::Option(inner) => inner.as_ref(),
        kind => kind,
    };
    let takes = match inner {
        IdlArgKind::Bytes => "hex".to_string(),
        IdlArgKind::List(item) => match **item {
            IdlArgKind::String | IdlArgKind::Pubkey => r#"a JSON list, ["a", "b"]"#.to_string(),
            IdlArgKind::Bool => "a JSON list, [true, false]".to_string(),
            IdlArgKind::Struct { .. } | IdlArgKind::Variants { .. } => {
                r#"a JSON list, [{"a": 1}]"#.to_string()
            }
            _ => "a JSON list, [1, 2]".to_string(),
        },
        IdlArgKind::FixedBytes { len } => format!("text, or hex after 0x · up to {len} bytes"),
        IdlArgKind::Struct { fields } => format!(
            "a JSON object, {{{}}}",
            fields
                .iter()
                .map(|(name, _)| format!("\"{name}\": …"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        IdlArgKind::Variants { variants } => variants
            .iter()
            .map(|(name, held)| match held {
                IdlVariant::Empty => name.clone(),
                _ => format!("{{\"{name}\": …}}"),
            })
            .collect::<Vec<_>>()
            .join(" or "),
        IdlArgKind::Raw { note } => note.clone(),
        IdlArgKind::Integer { float: true, .. } => "number".to_string(),
        IdlArgKind::Integer { .. } => match places {
            Places::BaseUnits => "whole number, in base units".to_string(),
            Places::Fixed(0) => "whole number".to_string(),
            Places::Fixed(1) => "up to 1 decimal place".to_string(),
            Places::Fixed(decimals) => format!("up to {decimals} decimal places"),
            Places::Like(other) => format!("as many decimal places as {other}"),
        },
        _ => String::new(),
    };
    match (blank, takes.is_empty()) {
        (Some(blank), true) => blank.to_string(),
        (Some(blank), false) => format!("{takes} · {blank}"),
        (None, _) => takes,
    }
}

pub(in crate::ui) fn account_label(account: &IdlAccountField) -> String {
    let mut label = account.name.clone();
    if account.signer {
        label.push_str(" · signer");
    }
    if account.optional {
        label.push_str(" (optional)");
    }
    label
}

pub(in crate::ui) fn derived_note(account: &IdlAccountField) -> String {
    if account.seeds.is_empty() {
        "filled by the program".to_string()
    } else {
        format!("derived from {}", account.seeds.join(", "))
    }
}

pub(in crate::ui) fn fill_note(account: &IdlAccountField) -> String {
    if account.reason.is_empty() {
        "fill this address".to_string()
    } else {
        format!("fill this address · {}", account.reason)
    }
}

pub(in crate::ui) fn bare_bool(kind: &IdlArgKind) -> bool {
    matches!(kind, IdlArgKind::Bool)
}

pub(in crate::ui) fn arg_pubkey(kind: &IdlArgKind) -> bool {
    match kind {
        IdlArgKind::Pubkey => true,
        IdlArgKind::Option(inner) => arg_pubkey(inner),
        _ => false,
    }
}

pub(in crate::ui) fn enum_variants(kind: &IdlArgKind) -> Option<&[String]> {
    match kind {
        IdlArgKind::Enum { variants } => Some(variants),
        IdlArgKind::Option(inner) => enum_variants(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(signed: bool) -> IdlArgKind {
        IdlArgKind::Integer {
            signed,
            float: false,
            bits: 64,
        }
    }

    #[test]
    fn a_phone_opens_digits_for_a_number_and_a_decimal_point_for_an_amount() {
        assert_eq!(input_mode(&int(false), &Places::BaseUnits), Some("numeric"));
        assert_eq!(input_mode(&int(false), &Places::Fixed(0)), Some("numeric"));
        assert_eq!(input_mode(&int(false), &Places::Fixed(6)), Some("decimal"));
        assert_eq!(
            input_mode(&int(false), &Places::Like("$decimals".into())),
            Some("decimal")
        );
        let optional = IdlArgKind::Option(Box::new(int(false)));
        assert_eq!(input_mode(&optional, &Places::BaseUnits), Some("numeric"));
        // The minus sign, text, addresses and JSON keep the full keyboard.
        assert_eq!(input_mode(&int(true), &Places::BaseUnits), None);
        assert_eq!(input_mode(&IdlArgKind::String, &Places::BaseUnits), None);
        assert_eq!(input_mode(&IdlArgKind::Pubkey, &Places::BaseUnits), None);
        let list = IdlArgKind::List(Box::new(int(false)));
        assert_eq!(input_mode(&list, &Places::BaseUnits), None);
    }
}
