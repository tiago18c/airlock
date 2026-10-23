//! A site asking which accounts it may see.

use super::*;

#[component]
pub(in crate::ui) fn ConnectRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let accounts = session.shareable_accounts();
    // One account needs no choosing. With several, nothing is shared by default.
    let mut chosen = use_signal(|| match session.shareable_accounts().as_slice() {
        [only] => vec![only.pubkey],
        _ => Vec::new(),
    });
    let shares_contract = accounts
        .iter()
        .any(|account| account.contract && chosen.read().contains(&account.pubkey));
    let approve_id = current.id.clone();
    let reject_id = current.id.clone();

    rsx! {
        RequestHead { current: current.clone(), title: "Connect".to_string(), on_close }
        div { class: "request-body",
            p { class: "muted", "{current.detail}" }
            if accounts.is_empty() {
                p { class: "warn", "This device has no account to share. Add a wallet first." }
            }
            for account in accounts {
                label { class: "check", key: "{account.pubkey}",
                    input {
                        r#type: "checkbox",
                        checked: chosen.read().contains(&account.pubkey),
                        onchange: move |evt| {
                            let mut list = chosen.write();
                            list.retain(|known| *known != account.pubkey);
                            if evt.checked() {
                                list.push(account.pubkey);
                            }
                        },
                    }
                    AddressRow { pubkey: account.pubkey, no_copy: true }
                }
            }
            if shares_contract {
                p { class: "hint",
                    "No key signs as a programmatic signer or a Squads vault. A transaction this site builds for one is rebuilt here before you read it: as an authorization, or as a proposal."
                }
            }
        }
        div { class: "btn-row",
            button {
                onclick: move |_| {
                    session.bridge_answer(&reject_id, BridgeAnswer::Rejected);
                },
                "Reject"
            }
            button {
                class: "primary",
                disabled: chosen.read().is_empty(),
                onclick: move |_| {
                    let shared: Vec<([u8; 32], String)> = session
                        .shareable_accounts()
                        .into_iter()
                        .filter(|account| chosen.read().contains(&account.pubkey))
                        .map(|account| (account.pubkey.to_bytes(), account.label))
                        .collect();
                    session.bridge_answer(&approve_id, BridgeAnswer::Connect(shared));
                },
                "Approve"
            }
        }
    }
}
