//! Add-wallet form: a programmatic signer.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum AuthoritySource {
    /// A new key, made here and kept in the vault as a hot wallet.
    NewKey,
    /// A hot or offline wallet of this device.
    Wallet,
    /// A key kept on another device, by its address.
    Elsewhere,
}

/// Add-wallet form: a programmatic signer. When `creating`, its authority is
/// a key made here or a wallet of this device; otherwise a wallet of this
/// device or a key kept elsewhere. Nothing goes on chain here: the address
/// is derived from the authority key.
#[component]
pub fn ProgrammaticForm(creating: bool, on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let payer_wallets = payers(&wallets, crate::state::use_network().is_none());
    let key_wallets = keys(&wallets);
    let mut label = use_signal(String::new);
    let mut source = use_signal(|| {
        if creating {
            AuthoritySource::NewKey
        } else {
            AuthoritySource::Wallet
        }
    });
    let mut existing = use_signal(|| None::<WalletId>);
    let mut pasted = use_signal(String::new);
    let mut fee_payer = use_signal(|| None::<WalletId>);
    let mut error = use_signal(|| None::<String>);

    let authority_preview = match source() {
        AuthoritySource::NewKey => None,
        AuthoritySource::Wallet => existing()
            .and_then(|id| key_wallets.iter().find(|wallet| wallet.id == id))
            .and_then(|wallet| wallet.address),
        AuthoritySource::Elsewhere => Pubkey::from_str(pasted().trim()).ok(),
    };
    let address_preview =
        authority_preview.map(|authority| programmatic_signer_address(&authority));

    let save = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let known = keys(&(session.wallets)().summaries.clone());
        let (authority, authority_wallet) = match source() {
            AuthoritySource::NewKey => {
                let key = KeypairSigner::new_random();
                let key_label = format!("{name} key");
                let wallet = WalletRecord::from_keypair_seed(key_label.clone(), key.seed_bytes());
                let id = wallet.id;
                let stored = session.add_signers(vec![NewSigner {
                    wallet,
                    pubkey: key.pubkey(),
                    label: key_label,
                    origin: Origin::Imported,
                }]);
                if let Err(err) = stored {
                    error.set(Some(err.to_string()));
                    return;
                }
                (key.pubkey(), Some(id))
            }
            AuthoritySource::Wallet => {
                let found = existing().and_then(|id| {
                    known
                        .iter()
                        .find(|wallet| wallet.id == id)
                        .and_then(|wallet| wallet.address.map(|address| (address, Some(id))))
                });
                match found {
                    Some(found) => found,
                    None => {
                        error.set(Some("Pick the wallet whose key is the authority.".into()));
                        return;
                    }
                }
            }
            AuthoritySource::Elsewhere => match Pubkey::from_str(pasted().trim()) {
                // A wallet already here for that key is the one to sign through.
                Ok(authority) => (
                    authority,
                    known
                        .iter()
                        .find(|wallet| wallet.address == Some(authority))
                        .map(|wallet| wallet.id),
                ),
                Err(_) => {
                    error.set(Some("The authority address is not valid base58.".into()));
                    return;
                }
            },
        };
        let mut wallet = WalletRecord::programmatic(name, authority, authority_wallet);
        wallet.kind.set_fee_payer(fee_payer());
        let id = wallet.id;
        match session.add_contract_wallet(wallet) {
            Ok(()) => {
                let _ = session.select_wallet(id);
                on_done.call(());
                crate::actions::network::start_refresh(session);
            }
            Err(err) => error.set(Some(err.to_string())),
        }
    };

    rsx! {
        p { class: "note",
            "A programmatic signer is an address controlled by an Ed25519 key that never signs a transaction. The key signs a message, and any funded wallet can submit it later. Use the signer's address, not the key's, as the multisig member or the account owner."
        }
        label { "Label"
            input {
                r#type: "text",
                placeholder: "Cold signer",
                value: "{label}",
                oninput: move |evt| label.set(evt.value()),
            }
        }
        label { "Authority key"
            select {
                onchange: move |evt| {
                    source.set(match evt.value().as_str() {
                        "new" => AuthoritySource::NewKey,
                        "elsewhere" => AuthoritySource::Elsewhere,
                        _ => AuthoritySource::Wallet,
                    });
                },
                if creating {
                    option { value: "new", selected: source() == AuthoritySource::NewKey,
                        "A new key made here"
                    }
                }
                option { value: "wallet", selected: source() == AuthoritySource::Wallet,
                    "A hot or offline wallet of this device"
                }
                if !creating {
                    option { value: "elsewhere", selected: source() == AuthoritySource::Elsewhere,
                        "A key kept on another device, by address"
                    }
                }
            }
        }
        if source() == AuthoritySource::NewKey {
            p { class: "muted",
                "A new key is made on this device and kept in the vault as a hot wallet. It is the signer's authority."
            }
        }
        if source() == AuthoritySource::Wallet {
                WalletPicker {
                    label: "Wallet",
                    options: key_wallets.clone(),
                    selected: existing(),
                    none_label: "Choose a wallet",
                    on_change: move |id| existing.set(id),
                }
            }
        if source() == AuthoritySource::Elsewhere {
            label { "Authority address"
                input {
                    r#type: "text",
                    class: "mono",
                    placeholder: "Base58 address of the key",
                    value: "{pasted}",
                    oninput: move |evt| pasted.set(evt.value()),
                }
            }
        }
        WalletPicker {
            label: "Pays fees (optional)",
            options: payer_wallets.clone(),
            selected: fee_payer(),
            none_label: if source() == AuthoritySource::NewKey { "The new key" } else { "The authority key, if it is on this device" },
            on_change: move |id| fee_payer.set(id),
        }
        if let Some(address) = address_preview {
            p { class: "wallet-fact",
                span { class: "muted", "Signer address" }
                span { class: "addr-full",
                    code { class: "mono", "{address}" }
                    CopyButton { text: address.to_string() }
                }
            }
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        div { class: "btn-row",
            button { class: "primary", onclick: save,
                if creating { "Create programmatic signer" } else { "Add programmatic signer" }
            }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}
