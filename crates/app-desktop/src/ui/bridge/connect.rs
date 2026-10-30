//! A site asking which accounts it may see.

use super::*;
use wallet_core::ShareableAccount;

#[component]
pub(in crate::ui) fn ConnectRequest(current: BridgePrompt, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let origin = current.origin.clone();
    let start = use_hook(move || session.site_choice(&origin));
    let order = start.choice.order.clone();
    let accounts = in_order(session.shareable_accounts(), &order);
    let current_key = start.choice.current;
    let current_is_new = start.choice.current_is_new;
    // Kept in the order they were ticked: the first goes to the site first.
    let mut chosen = use_signal(|| start.choice.ticked.clone());
    let mut remember = use_signal(|| start.remembered);
    let shares_contract = accounts
        .iter()
        .any(|account| account.contract && chosen.read().contains(&account.pubkey));
    let approve_id = current.id.clone();
    let reject_id = current.id.clone();
    let block_id = current.id.clone();

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
                    if current_key == Some(account.pubkey) {
                        span { class: "chip", "Current" }
                    }
                }
            }
            if current_is_new {
                p { class: "hint",
                    "This site has not seen your current wallet. Tick it to show it."
                }
            }
            if shares_contract {
                p { class: "hint",
                    "No key signs as a programmatic signer or a Squads vault. A transaction this site builds for one is rebuilt here before you read it: as an authorization, or as a proposal."
                }
            }
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked: remember(),
                    onchange: move |evt| remember.set(evt.checked()),
                }
                span { "Remember this connection" }
            }
        }
        div { class: "btn-row",
            button {
                class: "danger",
                title: "Reject this request and every request this site makes from now on.",
                onclick: move |_| {
                    session.bridge_block(&block_id);
                },
                "Block site"
            }
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
                    let held = session.shareable_accounts();
                    let shared: Vec<ShareableAccount> = chosen
                        .read()
                        .iter()
                        .filter_map(|key| held.iter().find(|account| account.pubkey == *key))
                        .cloned()
                        .collect();
                    session.bridge_connect(&approve_id, shared, remember());
                },
                "Approve"
            }
        }
    }
}

/// `accounts` in `order`, and any added since it was worked out after them.
fn in_order(mut accounts: Vec<ShareableAccount>, order: &[Pubkey]) -> Vec<ShareableAccount> {
    accounts.sort_by_key(|account| {
        order
            .iter()
            .position(|key| *key == account.pubkey)
            .unwrap_or(order.len())
    });
    accounts
}
