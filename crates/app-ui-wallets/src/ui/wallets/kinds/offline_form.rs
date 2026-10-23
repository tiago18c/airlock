//! Add-wallet form: an offline wallet, by the address of its key.

use super::*;

/// Add-wallet form: an offline wallet, by the address of a key kept on
/// another device.
#[component]
pub fn OfflineForm(on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let payer_wallets = payers(
        &(session.wallets)().summaries.clone(),
        crate::state::use_network().is_none(),
    );
    let mut label = use_signal(String::new);
    let mut address = use_signal(String::new);
    let mut fee_payer = use_signal(|| None::<WalletId>);
    let mut error = use_signal(|| None::<String>);

    let save = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let Ok(pubkey) = Pubkey::from_str(address().trim()) else {
            error.set(Some("The address is not valid base58.".into()));
            return;
        };
        match session.add_offline_wallet(name, pubkey, fee_payer()) {
            Ok(_) => {
                on_done.call(());
                crate::actions::network::start_refresh(session);
            }
            Err(err) => error.set(Some(err.to_string())),
        }
    };

    rsx! {
        p { class: "note",
            "An offline wallet is the address of a key kept on another device. This device prepares its transactions, and the other one signs them from a QR code or pasted text. No key is stored here."
        }
        label { "Label"
            input {
                r#type: "text",
                placeholder: "Cold wallet",
                value: "{label}",
                oninput: move |evt| label.set(evt.value()),
            }
        }
        label { "Address"
            input {
                r#type: "text",
                class: "mono",
                placeholder: "Base58 address of the key",
                value: "{address}",
                oninput: move |evt| address.set(evt.value()),
            }
        }
        WalletPicker {
            label: "Pays fees (optional)",
            options: payer_wallets,
            selected: fee_payer(),
            none_label: "The offline wallet itself",
            on_change: move |id| fee_payer.set(id),
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            button { class: "primary", onclick: save, "Add offline wallet" }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}
