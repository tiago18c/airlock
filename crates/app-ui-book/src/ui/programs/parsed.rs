//! An IDL as a tree to browse.

use super::*;

#[component]
pub(in crate::ui) fn ParsedIdl(tree: IdlOutline) -> Element {
    rsx! {
        div { class: "stack",
            if !tree.version.is_empty() {
                p { class: "muted", "Version {tree.version}" }
            }
            IdlGroup { title: "Instructions", count: tree.instructions.len(),
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
            if !tree.accounts.is_empty() {
                IdlGroup { title: "Accounts", count: tree.accounts.len(),
                    for account in tree.accounts {
                        details { class: "idl-item", key: "{account.name}",
                            summary { "{account.name}" }
                            if !account.detail.is_empty() {
                                div { class: "idl-body",
                                    p { "{account.detail}" }
                                }
                            }
                        }
                    }
                }
            }
            if !tree.pdas.is_empty() {
                IdlGroup { title: "PDAs", count: tree.pdas.len(),
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
            }
            if !tree.types.is_empty() {
                IdlGroup { title: "Types", count: tree.types.len(),
                    for kind in tree.types {
                        details { class: "idl-item", key: "{kind.name}",
                            summary { "{kind.name}" }
                            if !kind.detail.is_empty() {
                                div { class: "idl-body", p { "{kind.detail}" } }
                            }
                        }
                    }
                }
            }
            if !tree.events.is_empty() {
                IdlGroup { title: "Events", count: tree.events.len(),
                    for event in tree.events {
                        p { key: "{event.name}", "{event.name}" }
                    }
                }
            }
            if !tree.errors.is_empty() {
                IdlGroup { title: "Errors", count: tree.errors.len(),
                    for err in tree.errors {
                        p { key: "{err.code}", "{err.name} · {err.code} · {err.message}" }
                    }
                }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn IdlGroup(title: &'static str, count: usize, children: Element) -> Element {
    rsx! {
        section { class: "idl-section",
            h4 { "{title} · {count}" }
            {children}
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
