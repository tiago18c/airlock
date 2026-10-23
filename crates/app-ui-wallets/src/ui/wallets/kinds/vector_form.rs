//! Add-wallet form: a Vector account.

use super::*;
use wallet_core::vector::{
    address, new_held_key, new_key_phrase, normalize_key_phrase, parse_public_key, phrase_key,
    scheme_offered, signs_here, SchemeId, PROGRAM_ID,
};
use wallet_core::{format_sol, Authority, SecretBytes};

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthoritySource {
    /// A new key kept in this wallet's record.
    NewKey,
    /// A hot or offline Ed25519 wallet of this device.
    Wallet,
    /// A key kept elsewhere: paste its public key or Ethereum address.
    Elsewhere,
    /// An XMSS wallet made before, typed in again from its recovery phrase.
    Phrase,
}

/// Whether `scheme`'s keys made here come from a recovery phrase.
fn from_phrase(scheme: SchemeId) -> bool {
    scheme == SchemeId::Xmss
}

/// The key the account is made with, from a phrase: key number 0. Rebuilding
/// an XMSS key takes a moment, so a form asks once per phrase.
fn first_key(words: &str) -> Option<Vec<u8>> {
    let words = normalize_key_phrase(words).ok()?;
    phrase_key(&words, SchemeId::Xmss, 0)
        .ok()
        .map(|(key, _)| key)
}

/// Add a Vector wallet. Every offered scheme is listed; one this build cannot
/// keep a key of is offered only with its key elsewhere. Nothing goes on
/// chain here: Initialize is a button on the row.
#[component]
pub fn VectorForm(creating: bool, on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = (session.wallets)().summaries.clone();
    let payer_wallets = payers(&wallets, crate::state::use_network().is_none());
    let key_wallets = keys(&wallets);
    let mut label = use_signal(String::new);
    let mut scheme = use_signal(|| SchemeId::Ed25519);
    let mut source = use_signal(|| {
        if creating {
            AuthoritySource::NewKey
        } else {
            AuthoritySource::Wallet
        }
    });
    let mut existing = use_signal(|| None::<WalletId>);
    let mut pasted = use_signal(String::new);
    // A new key made from a phrase: the words, shown once to write down.
    let made_phrase = use_signal(new_key_phrase);
    let mut written = use_signal(|| false);
    let mut typed_phrase = use_signal(String::new);
    let phrase_in_use = use_memo(move || match source() {
        AuthoritySource::NewKey if from_phrase(scheme()) => Some(made_phrase()),
        AuthoritySource::Phrase => Some(typed_phrase()),
        AuthoritySource::NewKey | AuthoritySource::Wallet | AuthoritySource::Elsewhere => None,
    });
    let phrase_key_preview = use_memo(move || phrase_in_use().as_deref().and_then(first_key));
    let mut fee_payer = use_signal(|| None::<WalletId>);
    let mut error = use_signal(|| None::<String>);
    let mut rent = use_signal(|| None::<u64>);
    let network = crate::state::use_network();

    // Rent for the chosen scheme, when a node can say.
    use_effect(move || {
        let chosen = scheme();
        rent.set(None);
        let Some(network) = network else {
            return;
        };
        spawn(async move {
            let Ok(rpc) = network.rpc() else {
                return;
            };
            if let Ok(lamports) = wallet_flows::vector::rent_for(rpc.as_ref(), chosen).await {
                if *scheme.peek() == chosen {
                    rent.set(Some(lamports));
                }
            }
        });
    });

    let key_preview = match source() {
        AuthoritySource::Wallet => existing()
            .filter(|_| scheme() == SchemeId::Ed25519)
            .and_then(|id| key_wallets.iter().find(|wallet| wallet.id == id))
            .and_then(|wallet| wallet.address)
            .map(|pubkey| pubkey.to_bytes().to_vec()),
        AuthoritySource::Elsewhere => parse_public_key(scheme(), &pasted()).ok(),
        AuthoritySource::NewKey | AuthoritySource::Phrase => phrase_key_preview(),
    };
    let address_preview = key_preview
        .as_ref()
        .and_then(|key| address(&PROGRAM_ID, scheme(), key).ok());

    let paste_placeholder = match scheme() {
        SchemeId::Ed25519 => "Base58 Ed25519 address",
        SchemeId::Secp256k1 => "Compressed secp256k1 pubkey (66 hex chars)",
        SchemeId::Eip191 => "0x… Ethereum address",
        SchemeId::MlDsa44 => "ML-DSA-44 public key (hex)",
        SchemeId::Falcon512 => "Falcon-512 public key (hex)",
        SchemeId::Winternitz => "Winternitz public key (hex)",
        SchemeId::Xmss => "XMSS public key (hex)",
    };
    let elsewhere_label = match scheme() {
        SchemeId::Eip191 => "An Ethereum address (signed with personal_sign)",
        _ => "A key kept on another device",
    };

    let save = move |_| {
        error.set(None);
        let name = label().trim().to_string();
        if name.is_empty() {
            error.set(Some("Label is required.".into()));
            return;
        }
        let chosen = scheme();
        if !scheme_offered(chosen) {
            error.set(Some(format!("{} wallets are not offered.", chosen.label())));
            return;
        }
        if matches!(source(), AuthoritySource::NewKey | AuthoritySource::Phrase)
            && !signs_here(chosen)
        {
            error.set(Some(format!(
                "This build cannot keep a {} key. Add it with its key on another device.",
                chosen.label()
            )));
            return;
        }
        // A key made from a phrase: new, once its words are written down, or
        // typed in again.
        let made = match source() {
            AuthoritySource::NewKey if from_phrase(chosen) => {
                if !written() {
                    error.set(Some(
                        "Write the recovery phrase down first, then tick that you did.".into(),
                    ));
                    return;
                }
                Some(WalletRecord::vector_from_phrase(
                    name.clone(),
                    chosen,
                    PROGRAM_ID,
                    &made_phrase(),
                    false,
                ))
            }
            AuthoritySource::Phrase => Some(WalletRecord::vector_from_phrase(
                name.clone(),
                chosen,
                PROGRAM_ID,
                &typed_phrase(),
                true,
            )),
            AuthoritySource::NewKey | AuthoritySource::Wallet | AuthoritySource::Elsewhere => None,
        };
        if let Some(made) = made {
            let mut wallet = match made {
                Ok(wallet) => wallet,
                Err(err) => {
                    error.set(Some(err.to_string()));
                    return;
                }
            };
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
            return;
        }
        let (key, authority) = match source() {
            AuthoritySource::NewKey => {
                let (key, secret) = new_held_key(chosen);
                (
                    key,
                    Authority::Held {
                        secret: SecretBytes(secret),
                        next_leaf: 0,
                    },
                )
            }
            AuthoritySource::Wallet => {
                let known = keys(&(session.wallets)().summaries.clone());
                let found = existing()
                    .filter(|_| chosen == SchemeId::Ed25519)
                    .and_then(|id| {
                        known
                            .iter()
                            .find(|wallet| wallet.id == id)
                            .and_then(|wallet| wallet.address.map(|address| (address, id)))
                    });
                match found {
                    Some((pubkey, id)) => {
                        (pubkey.to_bytes().to_vec(), Authority::Wallet { wallet: id })
                    }
                    None => {
                        error.set(Some(
                            "Pick the wallet whose key signs for this account.".into(),
                        ));
                        return;
                    }
                }
            }
            AuthoritySource::Phrase => return,
            AuthoritySource::Elsewhere => match parse_public_key(chosen, &pasted()) {
                Ok(key) => {
                    // An Ed25519 key that is a wallet here signs as that wallet.
                    let authority = keys(&(session.wallets)().summaries.clone())
                        .iter()
                        .find(|wallet| {
                            chosen == SchemeId::Ed25519
                                && wallet.address.map(|address| address.to_bytes().to_vec())
                                    == Some(key.clone())
                        })
                        .map(|wallet| Authority::Wallet { wallet: wallet.id })
                        .unwrap_or(Authority::Outside);
                    (key, authority)
                }
                Err(err) => {
                    error.set(Some(err.to_string()));
                    return;
                }
            },
        };
        let mut wallet = WalletRecord::vector(name, chosen, PROGRAM_ID, key, authority);
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
            "A Vector wallet is an account of the Vector program. Its key signs a digest of whole transactions off chain; a hot wallet relays them. After you add it, Initialize on its row creates the account on this cluster."
        }
        label { "Label"
            input {
                r#type: "text",
                placeholder: "Quantum",
                value: "{label}",
                oninput: move |evt| label.set(evt.value()),
            }
        }
        label { "Scheme"
            select {
                onchange: move |evt| {
                    if let Some(next) = SchemeId::ALL.iter().copied().find(|scheme| scheme.label() == evt.value()) {
                        scheme.set(next);
                        if creating && signs_here(next) {
                            source.set(AuthoritySource::NewKey);
                        } else if next == SchemeId::Ed25519 {
                            source.set(AuthoritySource::Wallet);
                        } else {
                            source.set(AuthoritySource::Elsewhere);
                        }
                    }
                },
                for option in SchemeId::ALL.into_iter().filter(|option| scheme_offered(*option)) {
                    option {
                        value: "{option.label()}",
                        selected: scheme() == option,
                        if option == SchemeId::Falcon512 {
                            "Falcon-512 (experimental)"
                        } else if option == SchemeId::Xmss {
                            "XMSS (one device only)"
                        } else if signs_here(option) {
                            "{option.label()}"
                        } else {
                            "{option.label()} — key on another device only"
                        }
                    }
                }
            }
        }
        if let Some(lamports) = rent() {
            p { class: "wallet-fact",
                span { class: "muted", "Rent to initialize" }
                span { "{format_sol(lamports)}" }
            }
        }
        label { "Authority key"
            select {
                onchange: move |evt| {
                    source.set(match evt.value().as_str() {
                        "new" => AuthoritySource::NewKey,
                        "elsewhere" => AuthoritySource::Elsewhere,
                        "phrase" => AuthoritySource::Phrase,
                        _ => AuthoritySource::Wallet,
                    });
                },
                if signs_here(scheme()) {
                    option {
                        value: "new",
                        selected: source() == AuthoritySource::NewKey,
                        if from_phrase(scheme()) {
                            "A new key made here from a new recovery phrase"
                        } else {
                            "A new key made here and kept with this wallet"
                        }
                    }
                }
                if signs_here(scheme()) && from_phrase(scheme()) {
                    option {
                        value: "phrase",
                        selected: source() == AuthoritySource::Phrase,
                        "A recovery phrase written down before"
                    }
                }
                option {
                    value: "wallet",
                    selected: source() == AuthoritySource::Wallet,
                    disabled: scheme() != SchemeId::Ed25519,
                    "A hot or offline wallet of this device"
                }
                option {
                    value: "elsewhere",
                    selected: source() == AuthoritySource::Elsewhere,
                    "{elsewhere_label}"
                }
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
        if source() == AuthoritySource::Elsewhere && scheme().key_len() > 33 {
            p { class: "hint",
                "A long key is easier to bring over with Export key on the other device, then Add wallet, From another device here."
            }
        }
        if source() == AuthoritySource::Elsewhere {
            label { "Public key"
                input {
                    r#type: "text",
                    class: "mono",
                    placeholder: "{paste_placeholder}",
                    value: "{pasted}",
                    oninput: move |evt| pasted.set(evt.value()),
                }
            }
        }
        if source() == AuthoritySource::NewKey && from_phrase(scheme()) {
            p { class: "note",
                "Every key this account will hold is made from these 24 words, so they bring it back if this device is lost. Write them down, in order, and keep them offline. Anyone with them controls the account."
            }
            ol { class: "seed-words",
                for (i, word) in made_phrase().split_whitespace().map(str::to_string).enumerate() {
                    li { key: "{i}",
                        span { class: "muted", "{i + 1}" }
                        span { class: "mono", "{word}" }
                    }
                }
            }
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked: written(),
                    onchange: move |evt| written.set(evt.checked()),
                }
                "I wrote the words down"
            }
        } else if source() == AuthoritySource::NewKey {
            p { class: "muted",
                "The key is stored in this wallet's record. It has no Solana address of its own and cannot pay fees."
            }
        }
        if source() == AuthoritySource::Phrase {
            label { "Recovery phrase"
                textarea {
                    class: "mono",
                    placeholder: "The words, in order",
                    value: "{typed_phrase}",
                    oninput: move |evt| typed_phrase.set(evt.value()),
                }
            }
            p { class: "note",
                "The wallet starts at the key the account was made with. Before it signs anything, scan the account's state from the online device: this device finds the key the account holds now among the phrase's keys. It then signs nothing but a move to the next key, at the last leaf of that key, since a lost copy may have used the others."
            }
        }
        WalletPicker {
            label: "Pays fees",
            options: payer_wallets.clone(),
            selected: fee_payer(),
            none_label: "Choose a hot wallet",
            on_change: move |id| fee_payer.set(id),
        }
        if let Some(address) = address_preview {
            p { class: "wallet-fact",
                span { class: "muted", "Account address" }
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
                if creating { "Create Vector wallet" } else { "Add Vector wallet" }
            }
            button { class: "ghost", onclick: move |_| on_done.call(()), "Cancel" }
        }
    }
}
