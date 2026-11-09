//! An IDL to browse: a tab for each kind of thing it describes.

use super::*;

/// The tabs over a program's IDL: each kind it has, with how many, then the
/// raw file. A tab the IDL has nothing for is left out.
/// The kinds a program's IDL has, each with how many. Instructions always,
/// the others when the IDL has any.
fn idl_tabs(outline: Option<&IdlOutline>) -> Vec<(IdlSection, &'static str, usize)> {
    let Some(tree) = outline else {
        return Vec::new();
    };
    [
        (
            IdlSection::Instructions,
            "Instructions",
            tree.instructions.len(),
        ),
        (IdlSection::Accounts, "Accounts", tree.accounts.len()),
        (IdlSection::Types, "Types", tree.types.len()),
        (IdlSection::Pdas, "PDAs", tree.pdas.len()),
        (IdlSection::Events, "Events", tree.events.len()),
        (IdlSection::Errors, "Errors", tree.errors.len()),
    ]
    .into_iter()
    .filter(|(kind, _, count)| *count > 0 || *kind == IdlSection::Instructions)
    .collect()
}

/// The tab shown: the one chosen, or the first when this program does not
/// have that one.
pub(in crate::ui) fn shown_section(outline: Option<&IdlOutline>, chosen: IdlSection) -> IdlSection {
    let tabs = idl_tabs(outline);
    if chosen == IdlSection::Raw || tabs.iter().any(|(kind, _, _)| *kind == chosen) {
        chosen
    } else {
        tabs.first().map_or(IdlSection::Raw, |(kind, _, _)| *kind)
    }
}

/// The tabs over a program's IDL: each kind it has, with how many, then the
/// raw file.
#[component]
pub(in crate::ui) fn IdlTabs(
    outline: Option<IdlOutline>,
    mut section: Signal<IdlSection>,
) -> Element {
    let shown = shown_section(outline.as_ref(), section());
    let tabs = idl_tabs(outline.as_ref());

    rsx! {
        div { class: "tabs idl-tabs", role: "tablist",
            for (kind, title, count) in tabs {
                button {
                    key: "{title}",
                    class: if shown == kind { "tab on" } else { "tab" },
                    role: "tab",
                    aria_selected: shown == kind,
                    onclick: move |_| section.set(kind),
                    "{title}"
                    span { class: "tab-count", "{count}" }
                }
            }
            button {
                class: if shown == IdlSection::Raw { "tab on" } else { "tab" },
                role: "tab",
                aria_selected: shown == IdlSection::Raw,
                onclick: move |_| section.set(IdlSection::Raw),
                "Raw"
            }
        }
    }
}

/// One tab of an IDL.
#[component]
pub(in crate::ui) fn ParsedIdl(tree: IdlOutline, section: IdlSection) -> Element {
    match section {
        IdlSection::Instructions => rsx! {
            div { class: "idl-section",
                if tree.instructions.is_empty() {
                    p { class: "muted", "This IDL describes no instructions." }
                }
                for ix in tree.instructions {
                    details { class: "idl-item", key: "{ix.name}",
                        summary { "{ix.name}" }
                        div { class: "idl-body",
                            if !ix.docs.is_empty() {
                                p { "{ix.docs}" }
                            }
                            if !ix.accounts.is_empty() {
                                p { class: "muted", "Accounts" }
                                ul { class: "idl-lines",
                                    for account in ix.accounts {
                                        li { key: "{account.name}", "{account_line(&account)}" }
                                    }
                                }
                            }
                            if !ix.arguments.is_empty() {
                                p { class: "muted", "Arguments" }
                                ul { class: "idl-lines",
                                    for arg in ix.arguments {
                                        li { key: "{arg.name}",
                                            span { "{arg.name}" }
                                            span { class: "muted", " {arg.ty}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        IdlSection::Accounts => rsx! {
            div { class: "idl-section",
                for account in tree.accounts {
                    NamedItem { key: "{account.name}", name: account.name, detail: account.detail }
                }
            }
        },
        IdlSection::Types => rsx! {
            div { class: "idl-section",
                for kind in tree.types {
                    NamedItem { key: "{kind.name}", name: kind.name, detail: kind.detail }
                }
            }
        },
        IdlSection::Pdas => rsx! {
            div { class: "idl-section",
                for pda in tree.pdas {
                    details { class: "idl-item", key: "{pda.name}",
                        summary { "{pda.name}" }
                        if !pda.seeds.is_empty() {
                            div { class: "idl-body",
                                ul { class: "idl-lines",
                                    for seed in pda.seeds {
                                        li { key: "{seed}", "{seed}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        IdlSection::Events => rsx! {
            div { class: "idl-section",
                for event in tree.events {
                    NamedItem { key: "{event.name}", name: event.name, detail: event.detail }
                }
            }
        },
        IdlSection::Errors => rsx! {
            div { class: "idl-errors",
                for err in tree.errors {
                    div { class: "idl-error", key: "{err.code}",
                        div { class: "idl-error-code",
                            code { "{err.code}" }
                            code { class: "muted", "0x{err.code:x}" }
                        }
                        p { class: "idl-error-name", "{err.name}" }
                        if !err.message.is_empty() {
                            p { class: "muted", "{err.message}" }
                        }
                    }
                }
            }
        },
        IdlSection::Raw => rsx! {},
    }
}

/// An account, type or event: its name, opening to what it holds.
#[component]
fn NamedItem(name: String, detail: String) -> Element {
    if detail.is_empty() {
        return rsx! { div { class: "idl-item idl-leaf", "{name}" } };
    }
    rsx! {
        details { class: "idl-item",
            summary { "{name}" }
            div { class: "idl-body", p { "{detail}" } }
        }
    }
}

pub(in crate::ui) fn account_line(account: &IdlAccountUse) -> String {
    let access = if account.writable {
        "writable"
    } else {
        "readonly"
    };
    if account.optional {
        format!(
            "{} · {access} · {} · optional",
            account.name, account.signer
        )
    } else {
        format!("{} · {access} · {}", account.name, account.signer)
    }
}
