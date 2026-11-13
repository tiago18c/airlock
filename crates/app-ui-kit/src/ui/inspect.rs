//! The account inspector: an account read from the node, its header, then its
//! data decoded with its owner's IDL, or the bytes in hex or base64 when no
//! IDL reads them. An account of the upgradeable loader also shows the hash
//! of the code it holds; a program, the hash in its program-data account.
//! Any address card opens it through `Session::inspect`; the shell mounts it
//! once.

use crate::actions::idl_check::fetch_missing_idls;
use crate::rt::off_the_window;
use crate::state::session::Session;
use crate::ui::{format_sol, Address, AddressHeader, CopyButton, Glyph, Icon};
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use std::sync::Arc;
use wallet_core::program_code::{loader_account, DeployedProgram, LoaderAccount};
use wallet_core::{
    base64_text, group_thousands, hex_dump, inspect_account, AccountReading, InspectedAccount,
    Leaf, ValueNode, SHOWN_BYTES,
};

#[component]
pub fn AccountInspector() -> Element {
    let session = use_context::<Session>();
    let mut inspect = session.inspect;
    // The accounts opened before this one from inside the inspector, for Back.
    let mut trail = use_signal(Vec::<Pubkey>::new);
    let mut last = use_signal(|| None::<Pubkey>);
    use_effect(move || {
        let now = inspect();
        let before = *last.peek();
        if now == before {
            return;
        }
        match (before, now) {
            (_, None) => trail.write().clear(),
            (Some(before), Some(now)) => {
                if trail.peek().last() == Some(&now) {
                    trail.write().pop();
                } else {
                    trail.write().push(before);
                }
            }
            (None, Some(_)) => {}
        }
        last.set(now);
    });
    let Some(pubkey) = inspect() else {
        return rsx! {};
    };
    let back = trail().last().copied();

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| inspect.set(None),
            div { class: "modal inspect",
                role: "dialog",
                aria_label: "Inspect an account",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        inspect.set(None);
                    }
                },
                InspectBody { pubkey, back }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum Inspection {
    Fetching,
    /// The owner has no IDL here: looking for one on chain.
    LookingForIdl,
    Missing,
    Failed(String),
    /// With what the upgradeable loader's layout says of it, when the
    /// loader owns it.
    Read(Arc<InspectedAccount>, Option<LoaderAccount>),
}

/// A program's program-data account, read after the program.
#[derive(Clone, PartialEq)]
enum ProgramDataRead {
    Reading,
    Read(DeployedProgram),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DataView {
    Decoded,
    Hex,
    Base64,
}

#[component]
fn InspectBody(pubkey: Pubkey, back: Option<Pubkey>) -> Element {
    let session = use_context::<Session>();
    let mut inspect = session.inspect;
    let mut state = use_signal(|| Inspection::Fetching);
    let mut round = use_signal(|| 0u32);
    let mut view = use_signal(|| None::<DataView>);
    let mut program_data = use_signal(|| None::<ProgramDataRead>);
    let mut running = use_signal(|| None::<Task>);
    // Read again for another address, or on Refresh. A read still going for
    // the address before is dropped.
    use_effect(use_reactive!(|(pubkey)| {
        round();
        if let Some(earlier) = running.peek().as_ref() {
            earlier.cancel();
        }
        view.set(None);
        state.set(Inspection::Fetching);
        program_data.set(None);
        let task = spawn(load(session, pubkey, state, program_data));
        running.set(Some(task));
    }));
    let shown = state();
    let busy = matches!(shown, Inspection::Fetching | Inspection::LookingForIdl)
        || program_data() == Some(ProgramDataRead::Reading);
    let cluster = session.cluster().name().to_string();

    rsx! {
        div { class: "stack",
            AddressHeader { pubkey: Some(pubkey) }
            match shown {
                Inspection::Fetching => rsx! { p { class: "muted", "Reading the account from {cluster}…" } },
                Inspection::LookingForIdl => rsx! { p { class: "muted", "Looking for the IDL of its owner…" } },
                Inspection::Missing => rsx! {
                    p { class: "note", "No account is at this address on {cluster}: it holds no lamports and no data." }
                },
                Inspection::Failed(why) => rsx! { p { class: "error", "The account could not be read: {why}" } },
                Inspection::Read(read, loader) => rsx! {
                    AccountRead { pubkey, read, loader, program_data: program_data(), view }
                },
            }
            div { class: "btn-row",
                if let Some(back) = back {
                    button { class: "ghost",
                        // What Android's Back presses (MainActivity).
                        "data-back": "true",
                        onclick: move |_| inspect.set(Some(back)),
                        Icon { glyph: Glyph::ChevronLeft }
                        "Back"
                    }
                }
                button { class: "ghost",
                    onclick: move |_| inspect.set(None),
                    "Close"
                }
                crate::ui::RefreshButton {
                    busy,
                    labelled: true,
                    busy_text: "Reading…",
                    onclick: move |_| round += 1,
                }
            }
        }
    }
}

/// Read the account into `state`, then, for a program of the upgradeable
/// loader, its program-data account into `program_data`.
async fn load(
    session: Session,
    pubkey: Pubkey,
    mut state: Signal<Inspection>,
    mut program_data: Signal<Option<ProgramDataRead>>,
) {
    let rpc = match session.rpc() {
        Ok(rpc) => rpc,
        Err(err) => return state.set(Inspection::Failed(err.to_string())),
    };
    let account = match rpc.get_account(&pubkey).await {
        Ok(Some(account)) => account,
        Ok(None) => return state.set(Inspection::Missing),
        Err(err) => return state.set(Inspection::Failed(err.to_string())),
    };
    let owner = account.owner;
    let known = session
        .idl_records()
        .iter()
        .any(|record| record.program_id == owner);
    if !account.data.is_empty() && !account.executable && !known {
        state.set(Inspection::LookingForIdl);
        fetch_missing_idls(&session, &[owner]).await;
    }
    let catalog = session.idls.read().view();
    // A buffer or program-data account is hashed here, megabytes at most.
    let (read, loader) = off_the_window(move || {
        let loader = loader_account(&account.owner, &account.data);
        (inspect_account(&catalog, account), loader)
    })
    .await;
    state.set(Inspection::Read(Arc::new(read), loader));
    let Some(LoaderAccount::Program {
        program_data: address,
    }) = loader
    else {
        return;
    };
    program_data.set(Some(ProgramDataRead::Reading));
    let done = match wallet_flows::loader::program_data(rpc.as_ref(), address).await {
        Ok(found) => off_the_window(move || found.read()).await,
        Err(err) => Err(err),
    };
    program_data.set(Some(match done {
        Ok(deployed) => ProgramDataRead::Read(deployed),
        Err(err) => ProgramDataRead::Failed(err.to_string()),
    }));
}

#[component]
fn AccountRead(
    pubkey: Pubkey,
    read: Arc<InspectedAccount>,
    loader: Option<LoaderAccount>,
    program_data: Option<ProgramDataRead>,
    view: Signal<Option<DataView>>,
) -> Element {
    let header = &read.header;
    let decoded = matches!(read.reading, AccountReading::Decoded { .. });
    let current = view().unwrap_or(if decoded {
        DataView::Decoded
    } else {
        DataView::Hex
    });
    let rent_epoch = if header.rent_epoch == u64::MAX {
        "none (rent exempt)".to_string()
    } else {
        header.rent_epoch.to_string()
    };
    let executable = if header.executable { "yes" } else { "no" };
    let size = bytes_text(header.data_len);

    rsx! {
        dl { class: "addr-facts inspect-facts",
            div { dt { "Balance" } dd { "{format_sol(header.lamports)}" } }
            div { dt { "Owner" } dd { Address { pubkey: header.owner } } }
            div { dt { "Executable" } dd { "{executable}" } }
            div { dt { "Rent epoch" } dd { "{rent_epoch}" } }
            div { dt { "Data" } dd { "{size}" } }
        }
        if let Some(loader) = loader {
            LoaderFacts { pubkey, loader, program_data }
        }
        section { class: "idl-section",
            h4 { "Data" }
            match &read.reading {
                AccountReading::Empty => rsx! { p { class: "muted", "The account holds no data." } },
                AccountReading::Decoded { program, account, .. } => rsx! {
                    p { class: "muted",
                        "Read as "
                        span { class: "chip kind", "{account}" }
                        " with the {program} IDL."
                    }
                },
                AccountReading::Raw(why) => rsx! { p { class: "hint", "{why} The bytes are shown as they are." } },
            }
            if !read.data.is_empty() {
                DataViews { read: read.clone(), view, current, decoded }
            }
        }
    }
}

#[component]
fn DataViews(
    read: Arc<InspectedAccount>,
    mut view: Signal<Option<DataView>>,
    current: DataView,
    decoded: bool,
) -> Element {
    let tab = |which: DataView| {
        if current == which {
            "tab on"
        } else {
            "tab"
        }
    };
    let text = match current {
        DataView::Decoded => String::new(),
        DataView::Hex => hex_dump(&read.data),
        DataView::Base64 => base64_text(&read.data),
    };
    let cut = read.data.len() > SHOWN_BYTES;
    let copy = match current {
        DataView::Base64 if !cut => Some(text.replace('\n', "")),
        _ => None,
    };
    let fields = match &read.reading {
        AccountReading::Decoded { fields, .. } => fields.clone(),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "inspect-views",
            div { class: "segmented",
                if decoded {
                    button { class: tab(DataView::Decoded),
                        onclick: move |_| view.set(Some(DataView::Decoded)),
                        "Decoded"
                    }
                }
                button { class: tab(DataView::Hex),
                    onclick: move |_| view.set(Some(DataView::Hex)),
                    "Hex"
                }
                button { class: tab(DataView::Base64),
                    onclick: move |_| view.set(Some(DataView::Base64)),
                    "Base64"
                }
            }
            if let Some(copy) = copy {
                CopyButton { text: copy, label: "Copy the data in base64" }
            }
        }
        if current == DataView::Decoded {
            ValueTree { nodes: fields }
        } else {
            if cut {
                p { class: "hint",
                    "The first {bytes_text(SHOWN_BYTES)} of {bytes_text(read.data.len())} are shown."
                }
            }
            pre { class: "idl-raw inspect-raw", "{text}" }
        }
    }
}

/// What the upgradeable loader keeps in a program, its program data or a
/// buffer: the hash of the code, to compare with `solana-verify`, who can
/// change it, and when it was deployed.
#[component]
fn LoaderFacts(
    pubkey: Pubkey,
    loader: LoaderAccount,
    program_data: Option<ProgramDataRead>,
) -> Element {
    let session = use_context::<Session>();
    match loader {
        LoaderAccount::Uninitialized => rsx! {},
        LoaderAccount::Program {
            program_data: address,
        } => rsx! {
            section { class: "idl-section",
                h4 { "Program" }
                dl { class: "addr-facts inspect-facts",
                    div {
                        dt { "Program data" }
                        dd { class: "addr-card-account",
                            Address { pubkey: address }
                            button { class: "ghost sm icon-btn tip tip-end",
                                r#type: "button",
                                "data-tip": "Inspect the program data account",
                                aria_label: "Inspect the program data account",
                                onclick: move |_| {
                                    let mut inspect = session.inspect;
                                    inspect.set(Some(address));
                                },
                                Icon { glyph: Glyph::Search }
                            }
                        }
                    }
                    if let Some(ProgramDataRead::Read(deployed)) = program_data.clone() {
                        DeployedRows { deployed }
                    }
                }
                match program_data {
                    Some(ProgramDataRead::Read(_)) => rsx! {
                        p { class: "hint",
                            code { "solana-verify get-program-hash {pubkey}" }
                            " prints the same hash. Compare it with the hash of a build you trust."
                        }
                    },
                    Some(ProgramDataRead::Failed(why)) => rsx! {
                        p { class: "note", "Its program data could not be read, so its code has no hash here. {why}" }
                    },
                    Some(ProgramDataRead::Reading) | None => rsx! {
                        p { class: "muted", "Reading its program data…" }
                    },
                }
            }
        },
        LoaderAccount::ProgramData(deployed) => rsx! {
            section { class: "idl-section",
                h4 { "Program" }
                dl { class: "addr-facts inspect-facts",
                    DeployedRows { deployed }
                }
                p { class: "hint",
                    code { "solana-verify get-program-hash" }
                    " prints the same hash for the program this account holds. Compare it with the hash of a build you trust."
                }
            }
        },
        LoaderAccount::Buffer { authority, hash } => rsx! {
            section { class: "idl-section",
                h4 { "Buffer" }
                dl { class: "addr-facts inspect-facts",
                    HashRow { term: "Buffer hash", hash, copy: "Copy the buffer hash" }
                    div {
                        dt { "Authority" }
                        dd {
                            match authority {
                                Some(authority) => rsx! { Address { pubkey: authority } },
                                None => rsx! { "None: nobody can write to it or deploy from it." },
                            }
                        }
                    }
                }
                p { class: "hint",
                    code { "solana-verify get-buffer-hash {pubkey}" }
                    " prints the same hash. Compare it with the hash of a build you trust."
                }
            }
        },
    }
}

/// The hash, upgrade authority and deploy slot of a deployed program.
#[component]
fn DeployedRows(deployed: DeployedProgram) -> Element {
    let slot = group_thousands(&deployed.slot.to_string());
    rsx! {
        HashRow { term: "Program hash", hash: deployed.hash, copy: "Copy the program hash" }
        div {
            dt { "Upgrade authority" }
            dd {
                match deployed.authority {
                    Some(authority) => rsx! { Address { pubkey: authority } },
                    None => rsx! { "None: nobody can upgrade this program." },
                }
            }
        }
        div { dt { "Last deployed" } dd { "In slot {slot}" } }
    }
}

/// A code hash in full, in lowercase hex, with a copy button.
#[component]
fn HashRow(term: &'static str, hash: [u8; 32], copy: &'static str) -> Element {
    let text = hex::encode(hash);
    rsx! {
        div {
            dt { "{term}" }
            dd { class: "addr-card-account",
                code { class: "mono", "{text}" }
                CopyButton { text: text.clone(), label: copy }
            }
        }
    }
}

/// Decoded values as a tree, one line each.
#[component]
fn ValueTree(nodes: Vec<ValueNode>) -> Element {
    rsx! {
        ul { class: "inspect-tree",
            for (i, node) in nodes.into_iter().enumerate() {
                li { key: "{i}",
                    div { class: "inspect-row",
                        span { class: "inspect-key", "{node.label}" }
                        match node.leaf {
                            Leaf::Text(text) => rsx! { span { class: "inspect-value", "{text}" } },
                            Leaf::Address(pubkey) => rsx! { Address { pubkey } },
                            Leaf::Bytes(bytes) => rsx! {
                                code { class: "inspect-value inspect-bytes", title: "{bytes_text(bytes.len())}",
                                    "{hex::encode(&bytes)}"
                                }
                            },
                            Leaf::Group(word) => rsx! { span { class: "muted", "{word}" } },
                        }
                    }
                    if !node.children.is_empty() {
                        ValueTree { nodes: node.children }
                    }
                }
            }
        }
    }
}

fn bytes_text(n: usize) -> String {
    match n {
        1 => "1 byte".into(),
        n if n >= 1024 && n % 1024 == 0 => format!("{} KiB", n / 1024),
        n => format!("{n} bytes"),
    }
}
