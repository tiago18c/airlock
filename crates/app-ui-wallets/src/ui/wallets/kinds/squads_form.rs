//! Add-wallet form: a Squads multisig, existing or new.

use super::*;

pub(in crate::ui) fn program_from(value: &str) -> SquadsProgram {
    if value == "smart" {
        SquadsProgram::SmartAccount
    } else {
        SquadsProgram::V4
    }
}

pub(in crate::ui) fn parse_addresses(raw: &str) -> Result<Vec<Pubkey>, String> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty())
        .map(|part| Pubkey::from_str(part).map_err(|_| format!("{part} is not a valid address.")))
        .collect()
}

/// Add-wallet form: watch an existing Squads multisig, or create a new one.
#[component]
pub fn SquadsForm(creating: bool, on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let payer_wallets = payers(&wallets, crate::state::use_network().is_none() && !creating);
    let candidates = member_candidates(&wallets);
    let mut label = use_signal(String::new);
    let mut program = use_signal(|| SquadsProgram::V4);
    let mut address = use_signal(String::new);
    let mut chosen = use_signal(Vec::<WalletId>::new);
    let mut others = use_signal(String::new);
    let mut threshold = use_signal(|| "1".to_string());
    let mut payer = use_signal(|| None::<WalletId>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let add_existing = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let Ok(multisig) = Pubkey::from_str(address().trim()) else {
            error.set(Some("The multisig address is not valid base58.".into()));
            return;
        };
        let squad = Squad {
            program: program(),
            address: multisig,
        };
        let fee_payer = payer();
        busy.set(true);
        spawn(async move {
            match crate::actions::wallets::add_existing_squad(session, name, squad, fee_payer).await
            {
                Ok(()) => on_done.call(()),
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    };

    let create = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let known = (session.wallets)().summaries.clone();
        let mut members: Vec<Pubkey> = chosen()
            .iter()
            .filter_map(|id| known.iter().find(|wallet| wallet.id == *id))
            .filter_map(|wallet| wallet.address)
            .collect();
        match parse_addresses(&others()) {
            Ok(more) => members.extend(more),
            Err(err) => {
                error.set(Some(err));
                return;
            }
        }
        members.sort();
        members.dedup();
        if members.is_empty() {
            error.set(Some("Pick at least one member.".into()));
            return;
        }
        let threshold = match threshold().trim().parse::<u16>() {
            Ok(value) if value >= 1 && usize::from(value) <= members.len() => value,
            _ => {
                error.set(Some(format!(
                    "The threshold must be between 1 and {}.",
                    members.len()
                )));
                return;
            }
        };
        let Some(creator) = payer().and_then(|id| {
            known
                .iter()
                .find(|wallet| wallet.id == id)
                .and_then(|wallet| wallet.address.map(|address| (id, address)))
        }) else {
            error.set(Some(
                "Pick the hot wallet that pays for the creation.".into(),
            ));
            return;
        };
        let program = program();
        busy.set(true);
        spawn(async move {
            let result = crate::actions::wallets::create_squad(
                session, program, name, members, threshold, creator,
            )
            .await;
            busy.set(false);
            match result {
                Ok(()) => on_done.call(()),
                Err(err) => error.set(Some(err)),
            }
        });
    };

    rsx! {
        label { "Label"
            input {
                r#type: "text",
                placeholder: "Treasury",
                value: "{label}",
                oninput: move |evt| label.set(evt.value()),
            }
        }
        label { "Program"
            select {
                onchange: move |evt| program.set(program_from(&evt.value())),
                option { value: "v4", selected: program() == SquadsProgram::V4, "Squads v4" }
                option { value: "smart", selected: program() == SquadsProgram::SmartAccount,
                    "Squads smart account"
                }
            }
        }
        if creating {
            p { class: "muted", "Members on this device" }
            if candidates.is_empty() {
                p { class: "note", "Add a hot wallet, an offline wallet or a programmatic signer first, or list member addresses below." }
            }
            ul { class: "derive-list",
                for wallet in candidates.clone() {
                    li { class: "derive-row", key: "{wallet.id}",
                        input {
                            r#type: "checkbox",
                            checked: chosen().contains(&wallet.id),
                            onchange: move |_| {
                                let mut list = chosen.write();
                                if let Some(at) = list.iter().position(|id| *id == wallet.id) {
                                    list.remove(at);
                                } else {
                                    list.push(wallet.id);
                                }
                            },
                        }
                        match wallet.address {
                            Some(pubkey) => rsx! { AddressRow { pubkey } },
                            None => rsx! { p { "{wallet.label}" } },
                        }
                    }
                }
            }
            label { "Other member addresses (optional)"
                textarea {
                    class: "mono",
                    placeholder: "One address per line",
                    value: "{others}",
                    oninput: move |evt| others.set(evt.value()),
                }
            }
            label { "Approvals needed"
                input {
                    r#type: "text",
                    inputmode: "numeric",
                    value: "{threshold}",
                    oninput: move |evt| threshold.set(evt.value()),
                }
            }
            p { class: "note",
                "Every member may propose, approve and execute. Creating needs a connection: the app reads the program's settings and a recent blockhash, then opens the transaction in Sign."
            }
        } else {
            label { "Multisig address"
                input {
                    r#type: "text",
                    class: "mono",
                    placeholder: "The multisig account, not the vault",
                    value: "{address}",
                    oninput: move |evt| address.set(evt.value()),
                }
            }
        }
        WalletPicker {
            label: if creating { "Pays for the creation" } else { "Pays fees (optional)" },
            options: payer_wallets.clone(),
            selected: payer(),
            none_label: if creating { "Choose a hot wallet" } else { "The member's own setting" },
            on_change: move |id| payer.set(id),
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            if creating {
                button { class: "primary", disabled: busy(), onclick: create,
                    if busy() { "Preparing…" } else { "Create multisig" }
                }
            } else {
                button { class: "primary", disabled: busy(), onclick: add_existing,
                    if busy() { "Reading…" } else { "Add multisig" }
                }
            }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}
