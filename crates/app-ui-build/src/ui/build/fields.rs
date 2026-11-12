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
    let text = idl.argument(&argument.name).to_string();
    let kind = argument.kind.clone();
    let label = argument_label(&argument);

    if bare_bool(&kind) {
        let checked = text == "true";
        return rsx! {
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e| {
                        let value = if e.checked() { "true" } else { "false" }.to_string();
                        form.patch_idl(id, |d| {
                            d.set_argument(&name, value);
                        });
                    },
                }
                " {label}"
            }
        };
    }

    if let Some(variants) = enum_variants(&kind) {
        let variants = variants.to_vec();
        let optional = matches!(kind, IdlArgKind::Option(_));
        return rsx! {
            label { "{label}"
                select {
                    value: "{text}",
                    onchange: move |e| {
                        let value = e.value();
                        form.patch_idl(id, |d| {
                            d.set_argument(&name, value);
                        });
                    },
                    if optional {
                        option { value: "", selected: text.is_empty(), "None" }
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

    if arg_pubkey(&kind) {
        let selected = Pubkey::from_str(text.trim()).ok();
        return rsx! {
            AddressPicker {
                label,
                selected,
                reset: form.picker_reset,
                on_change: move |pk: Option<Pubkey>| {
                    let value = pk.map(|pk| pk.to_string()).unwrap_or_default();
                    form.patch_idl(id, |d| {
                        d.set_argument(&name, value);
                    });
                },
            }
        };
    }

    let placeholder = argument_placeholder(&argument);
    rsx! {
        label { "{label}"
            input {
                r#type: "text",
                placeholder: "{placeholder}",
                value: "{text}",
                oninput: move |e| {
                    let value = e.value();
                    form.patch_idl(id, |d| {
                        d.set_argument(&name, value);
                    });
                },
            }
        }
    }
}

/// An argument's name, and the unit it is typed in when its IDL names one:
/// "amount · SOL".
pub(in crate::ui) fn argument_label(argument: &IdlArgField) -> String {
    match argument.scale.unit() {
        Some(unit) => format!("{} · {unit}", argument.name),
        None => argument.name.clone(),
    }
}

/// What a text field takes: an amount says how many decimal places, and a
/// number the IDL gives no unit for is in base units.
pub(in crate::ui) fn argument_placeholder(argument: &IdlArgField) -> String {
    let (optional, inner) = match &argument.kind {
        IdlArgKind::Option(inner) => (true, inner.as_ref()),
        kind => (false, kind),
    };
    let takes = match inner {
        IdlArgKind::Bytes => "hex".to_string(),
        IdlArgKind::Raw { note } => note.clone(),
        IdlArgKind::Integer { float: true, .. } => "number".to_string(),
        IdlArgKind::Integer { .. } => match &argument.scale {
            IdlArgScale::None => "whole number, in base units".to_string(),
            IdlArgScale::Fixed { decimals: 0, .. } => "whole number".to_string(),
            IdlArgScale::Fixed { decimals: 1, .. } => "up to 1 decimal place".to_string(),
            IdlArgScale::Fixed { decimals, .. } => format!("up to {decimals} decimal places"),
            IdlArgScale::FromArgument { argument, .. } => {
                format!("as many decimal places as {argument}")
            }
        },
        _ => String::new(),
    };
    let blank = if argument.blank_uses_default {
        Some("blank for the default")
    } else if optional {
        Some("blank for none")
    } else {
        None
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
