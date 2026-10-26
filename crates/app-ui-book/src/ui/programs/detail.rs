//! One program's IDL: where it came from, and whether it matches the chain.

use super::*;
use solana_pubkey::Pubkey;

#[component]
pub(in crate::ui) fn ProgramDetail() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.program;
    let mut parsed = form.parsed;
    let selected = (form.selected)();
    let record = selected.and_then(|id| {
        session
            .idl_records()
            .into_iter()
            .find(|record| record.program_id == id)
    });
    let checking = use_signal(|| false);
    let check_note = use_signal(|| None::<String>);
    let view = use_memo(move || program_view(session, (form.selected)()));

    let Some(record) = record else {
        return rsx! {
            if !session.idl_records().is_empty() {
                div { class: "empty", p { "Select a program." } }
            }
        };
    };
    let shown = view();
    let spec = match record.spec {
        IdlSpec::Anchor => "Anchor",
        IdlSpec::Codama => "Codama",
    };
    let checksum = short_checksum(&record.sha256);
    let status = pin_status(&record);
    let program_id = record.program_id;
    let host_name = record.host.map(|host| {
        session
            .idl_records()
            .into_iter()
            .find(|row| row.program_id == host)
            .map_or_else(|| host.to_string(), |row| row.name)
    });
    let network = session.idl_network();
    let embedded = (record.origin == IdlOrigin::Builtin)
        .then(|| builtin_idl(network, &record.host.unwrap_or(record.program_id)))
        .flatten();

    rsx! {
        div { class: "card",
            div { class: "stack",
                AddressHeader {
                    pubkey: Some(record.program_id),
                    title: record.name.clone(),
                }
                p { class: "muted",
                    "{spec} · {checksum} · "
                    span { class: status_class(status), "{status.label()}" }
                    if record.origin == IdlOrigin::Builtin {
                        " "
                        span { class: "chip", "built in" }
                    }
                    if let Some(host) = host_name.clone() {
                        " "
                        span { class: "chip info", "from the {host} IDL" }
                    }
                    " "
                    span { class: "chip", title: "IDLs are kept per network: the cluster in use", "{network}" }
                }
                if let Some(embedded) = embedded {
                    p { class: "hint", "{source_sentence(embedded.source)} {embedded.changes}" }
                }
                if let Some(host) = host_name {
                    p { class: "hint",
                        "Read from the additional programs of the {host} IDL. Its checksum and online check are that file's, and it goes when that file goes."
                    }
                }
                div { class: "btn-row",
                    button {
                        disabled: checking(),
                        onclick: move |_| check_one(session, program_id, checking, check_note),
                        if checking() { "Checking…" } else { "Check online" }
                    }
                }
                if let Some(text) = check_note() {
                    p { class: "note", "{text}" }
                }
            }
            div { class: "segmented",
                button {
                    class: if parsed() { "tab on" } else { "tab" },
                    onclick: move |_| parsed.set(true),
                    "Parsed"
                }
                button {
                    class: if parsed() { "tab" } else { "tab on" },
                    onclick: move |_| parsed.set(false),
                    "Raw"
                }
            }
            if parsed() {
                if let Some(err) = shown.error.clone() {
                    p { class: "error", "{err}" }
                }
                if let Some(tree) = shown.outline.clone() {
                    ParsedIdl { tree }
                }
            } else {
                pre { class: "idl-raw", "{shown.raw}" }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ProgramView {
    raw: String,
    outline: Option<IdlOutline>,
    error: Option<String>,
}

fn program_view(session: Session, id: Option<Pubkey>) -> ProgramView {
    let Some(id) = id else {
        return ProgramView {
            raw: String::new(),
            outline: None,
            error: None,
        };
    };
    let Some(body) = session.idl_body(&id) else {
        return ProgramView {
            raw: String::new(),
            outline: None,
            error: Some("The IDL file is missing.".into()),
        };
    };
    let raw = pretty_idl(&body);
    match session.idl_outline(&id) {
        Ok(tree) => ProgramView {
            raw,
            outline: Some(tree),
            error: None,
        },
        Err(err) => ProgramView {
            raw,
            outline: None,
            error: Some(err.to_string()),
        },
    }
}

/// Where an embedded file was taken from.
fn source_sentence(source: IdlSource) -> String {
    match source {
        IdlSource::Chain(source) => {
            let account = match source.account {
                IdlOrigin::ProgramMetadata => "program-metadata",
                _ => "Anchor",
            };
            format!(
                "Made from the {account} IDL the program publishes on {} (SHA-256 {}…).",
                source.network,
                short_checksum(&source.sha256)
            )
        }
        IdlSource::Repository { url } => format!("Taken from {url}."),
        IdlSource::Hand => "Written by hand: the program publishes no IDL.".into(),
    }
}

pub(in crate::ui) fn pin_status(record: &IdlRecord) -> IdlPinStatus {
    IdlPinStatus::of(record.check.as_ref())
}

pub(in crate::ui) fn status_class(status: IdlPinStatus) -> &'static str {
    match status {
        IdlPinStatus::Matches | IdlPinStatus::MatchesSource => "chip ok",
        IdlPinStatus::Differs => "chip danger",
        _ => "chip",
    }
}
