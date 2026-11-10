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
            InstructionList { instructions: tree.instructions }
        },
        IdlSection::Accounts => rsx! {
            div { class: "idl-section idl-list",
                for account in tree.accounts {
                    NamedItem { key: "{account.name}", name: account.name, detail: account.detail }
                }
            }
        },
        IdlSection::Types => rsx! {
            div { class: "idl-section idl-list",
                for kind in tree.types {
                    NamedItem { key: "{kind.name}", name: kind.name, detail: kind.detail }
                }
            }
        },
        IdlSection::Pdas => rsx! {
            div { class: "idl-section idl-list",
                for pda in tree.pdas {
                    details { class: "idl-item", key: "{pda.name}",
                        IdlSummary { name: pda.name.clone() }
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
            div { class: "idl-section idl-list",
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

/// The instructions tab: a filter by name over one list, each instruction
/// opening in place to its accounts and arguments.
#[component]
fn InstructionList(instructions: Vec<IdlIxOutline>) -> Element {
    let mut filter = use_signal(String::new);
    let any = !instructions.is_empty();
    let shown: Vec<_> = instructions
        .into_iter()
        .filter(|ix| name_matches(&ix.name, &filter()))
        .collect();

    rsx! {
        div { class: "idl-section",
            if !any {
                p { class: "muted", "This IDL describes no instructions." }
            } else {
                div { class: "filter-bar",
                    label {
                        span { class: "sr-only", "Filter instructions" }
                        input {
                            r#type: "search",
                            placeholder: "Instruction name",
                            value: "{filter}",
                            oninput: move |e| filter.set(e.value()),
                        }
                    }
                }
                if shown.is_empty() {
                    p { class: "muted", "No instructions match this filter." }
                }
            }
            div { class: "idl-list",
                for ix in shown {
                    details { class: "idl-item idl-ix", key: "{ix.name}",
                        summary {
                            span { class: "idl-chevron", aria_hidden: "true", Icon { glyph: Glyph::ChevronRight } }
                            span { class: "idl-name", "{ix.name}" }
                            if let Some(gist) = first_sentence(&ix.docs) {
                                span { class: "idl-gist", "{gist}" }
                            }
                            span { class: "idl-counts", "{shape(ix.accounts.len(), ix.arguments.len())}" }
                        }
                        div { class: "idl-body",
                            if !ix.docs.is_empty() {
                                p { class: "idl-docs", "{ix.docs}" }
                            }
                            if !ix.accounts.is_empty() {
                                div { class: "idl-block",
                                    h4 { class: "eyebrow", "Accounts" }
                                    ul { class: "idl-rows",
                                        for account in ix.accounts {
                                            li { class: "idl-row", key: "{account.name}",
                                                span { class: "idl-row-name", "{account.name}" }
                                                span { class: "idl-flags",
                                                    for flag in account_flags(&account) {
                                                        span { class: flag_class(&flag), key: "{flag}", "{flag}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if !ix.arguments.is_empty() {
                                div { class: "idl-block",
                                    h4 { class: "eyebrow", "Arguments" }
                                    ul { class: "idl-rows",
                                        for arg in ix.arguments {
                                            li { class: "idl-row", key: "{arg.name}",
                                                span { class: "idl-row-name", "{arg.name}" }
                                                code { class: "idl-row-type", "{arg.ty}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The row that opens an IDL item: the app's chevron, which turns when the
/// item is open, then the name.
#[component]
fn IdlSummary(name: String) -> Element {
    rsx! {
        summary {
            span { class: "idl-chevron", aria_hidden: "true", Icon { glyph: Glyph::ChevronRight } }
            span { class: "idl-name", "{name}" }
        }
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
            IdlSummary { name }
            div { class: "idl-body", p { "{detail}" } }
        }
    }
}

/// The pill a flag is drawn in: the account that signs in the accent, as
/// the transaction view's account list draws it.
pub(in crate::ui) fn flag_class(flag: &str) -> &'static str {
    match flag {
        "signer" | "signer optional" => "chip kind",
        _ => "chip",
    }
}

/// What a collapsed instruction says it takes: "4 accounts · 3 args".
pub(in crate::ui) fn shape(accounts: usize, args: usize) -> String {
    fn count(n: usize, one: &str) -> String {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    }
    format!("{} · {}", count(accounts, "account"), count(args, "arg"))
}

/// The first sentence of an instruction's description, for its collapsed
/// row; `None` when it has none. An IDL's docs are lines, often one
/// sentence wrapped over several, so the lines of the first paragraph are
/// read as one.
pub(in crate::ui) fn first_sentence(docs: &str) -> Option<String> {
    let paragraph = docs
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let end = paragraph
        .match_indices(". ")
        .next()
        .map_or(paragraph.len(), |(at, _)| at + 1);
    let sentence = paragraph[..end].trim();
    (!sentence.is_empty()).then(|| sentence.to_string())
}

/// What an instruction's account is drawn with, as the transaction view
/// draws them: writable or readonly, then whether it signs. An account that
/// may sign or not ("either" in the IDL) is "signer optional".
pub(in crate::ui) fn account_flags(account: &IdlAccountUse) -> Vec<String> {
    let mut flags = vec![if account.writable {
        "writable".to_string()
    } else {
        "readonly".to_string()
    }];
    match account.signer.trim() {
        "" | "not a signer" => {}
        "either" => flags.push("signer optional".into()),
        other => flags.push(other.to_string()),
    }
    if account.optional {
        flags.push("optional".into());
    }
    flags
}
