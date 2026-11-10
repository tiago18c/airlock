//! One program's IDL: where it came from, and whether it matches the chain.

use super::*;
use solana_pubkey::Pubkey;

#[component]
pub(in crate::ui) fn ProgramDetail() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.program;
    let section = form.section;
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
    let tab = shown_section(shown.outline.as_ref(), section());
    let version = shown
        .outline
        .as_ref()
        .map(|tree| tree.version.clone())
        .filter(|version| !version.is_empty());
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
    // The name the list gives it; the IDL's own name beside it when that
    // says something else.
    let label = (session.directory)()
        .describe(&record.program_id)
        .full_label();
    let idl_name = label
        .as_ref()
        .is_some_and(|label| *label != record.name)
        .then(|| record.name.clone());
    let title = label.unwrap_or_else(|| record.name.clone());
    let network = session.idl_network();
    let embedded = (record.origin == IdlOrigin::Builtin)
        .then(|| session.embedded_idl(&record.program_id))
        .flatten();

    rsx! {
        div { class: "card program-detail",
            // The name stays pinned while the rest scrolls, the tabs under it.
            // Every entry here is a program, so the kind line is left out.
            div { class: "program-head",
                AddressHeader {
                    pubkey: Some(record.program_id),
                    title,
                    subtitle: String::new(),
                    no_address: true,
                    title_after: idl_name.map(|name| rsx! {
                        code { class: "program-idl-name", title: "Its name in the IDL", "{name}" }
                    }),
                }
            }
            div { class: "stack program-about",
                div { class: "addr-full program-address",
                    code { class: "mono", "{program_id}" }
                    CopyButton { text: program_id.to_string() }
                }
                // Two lines: what the file is, then the state of the online
                // check with the action that runs it, as one group.
                div { class: "program-facts-block",
                    p { class: "muted program-facts",
                        span { class: "program-facts-text",
                            "{spec} · "
                            if let Some(version) = version {
                                "version {version} · "
                            }
                            "{checksum}"
                        }
                        if record.origin == IdlOrigin::Builtin {
                            span { class: "chip", "built in" }
                        }
                        if let Some(host) = host_name.clone() {
                            span { class: "chip info", "from the {host} IDL" }
                        }
                        span { class: "chip", title: "IDLs are kept per network: the cluster in use", "IDL for {network}" }
                    }
                    div { class: "program-check-group",
                        span { class: status_class(status), title: status_tip(status), "{status_words(status)}" }
                        button {
                            class: "sm program-check",
                            disabled: checking(),
                            onclick: move |_| check_one(session, program_id, checking, check_note),
                            if checking() { "Checking…" } else { "Check online" }
                        }
                    }
                }
                if let Some(embedded) = embedded {
                    p { class: "hint", "{source_sentence(&embedded.source)} {embedded.changes}" }
                }
                if let Some(host) = host_name {
                    p { class: "hint",
                        "Read from the additional programs of the {host} IDL. Its checksum and online check are that file's, and it goes when that file goes."
                    }
                }
                if let Some(text) = check_note() {
                    p { class: "note", "{text}" }
                }
            }
            IdlTabs { outline: shown.outline.clone(), section }
            if tab == IdlSection::Raw {
                pre { class: "idl-raw", "{shown.raw}" }
            } else if let Some(err) = shown.error.clone() {
                p { class: "error", "{err}" }
            } else if let Some(tree) = shown.outline.clone() {
                ParsedIdl { tree, section: tab }
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
fn source_sentence(source: &IdlSource) -> String {
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

/// What a check state means, in one sentence, for the chip's tooltip. The
/// check compares the stored file with the IDL the program publishes on
/// chain, in its program-metadata or Anchor IDL account.
pub(in crate::ui) fn status_tip(status: IdlPinStatus) -> &'static str {
    match status {
        IdlPinStatus::NotChecked => {
            "Not yet compared with the IDL the program publishes on chain: Check online compares them."
        }
        IdlPinStatus::Matches => {
            "The last online check found this exact file published on chain by the program."
        }
        IdlPinStatus::MatchesSource => {
            "The last online check found the IDL this file was made from published on chain, so the file is current."
        }
        IdlPinStatus::Differs => {
            "The last online check found a different IDL published on chain by the program than this file."
        }
        IdlPinStatus::NoIdl => "No IDL is stored on this device for this program.",
    }
}

/// What a check state says, for the chip: what the stored file was compared
/// with, the IDL the program publishes on chain, and what was found.
pub(in crate::ui) fn status_words(status: IdlPinStatus) -> &'static str {
    match status {
        IdlPinStatus::NotChecked => "not compared with chain",
        IdlPinStatus::Matches => "same as on chain",
        IdlPinStatus::MatchesSource => "made from the IDL on chain",
        IdlPinStatus::Differs => "differs from chain",
        IdlPinStatus::NoIdl => "no IDL stored",
    }
}

pub(in crate::ui) fn status_class(status: IdlPinStatus) -> &'static str {
    match status {
        IdlPinStatus::Matches | IdlPinStatus::MatchesSource => "chip ok",
        IdlPinStatus::Differs => "chip danger",
        IdlPinStatus::NotChecked | IdlPinStatus::NoIdl => "chip warn",
    }
}
