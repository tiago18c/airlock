use super::lock::VaultUnlock;
use crate::state::forms::{AddWalletMode, SendOpen, WalletForm};
use crate::state::session::{Session, WalletSummary};
use crate::ui::wallets::kinds::{
    payers, ContractWalletRow, CurrentChip, ExportKeyButton, ExportKeysModal, KeyImportForm,
    OfflineForm, ProgrammaticForm, SquadsForm, VectorForm,
};
use crate::ui::{
    format_sol, Address, AddressHeader, CopyButton, Glyph, HelpTip, Icon, WalletPicker,
};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::{
    Category, DerivedAccount, NewSigner, OfflinePath, WalletClass, WalletKind, WalletRecord,
    DERIVE_BATCH, SOLANA_PATH_TEMPLATE,
};
use zeroize::Zeroize;

mod add_modal;
mod add_state;
mod confirm_remove;
pub mod kinds;
mod row;
mod seed_import;
mod seed_new;

pub(in crate::ui) use add_modal::*;
pub(in crate::ui) use add_state::*;
pub(in crate::ui) use confirm_remove::*;
pub(in crate::ui) use row::*;
pub(in crate::ui) use seed_import::*;
pub(in crate::ui) use seed_new::*;

/// Why a change to a wallet's settings asks for the password: they are kept
/// in the vault.
pub(in crate::ui) const CHANGE_WALLET: &str = "Unlock the vault to change this wallet's settings.";

#[component]
pub fn WalletsPanel() -> Element {
    let session = use_context::<Session>();
    kinds::use_vector_reads(&(session.wallets)().summaries);
    if !session.is_initialized() {
        return rsx! { VaultUnlock { show_skip: false } };
    }
    let locked = !session.is_unlocked();
    let form = session.forms.wallet;
    let mut modal_open = form.modal_open;
    let mut export_open = use_signal(|| false);
    let all = (session.wallets)().summaries.clone();
    let current = session.selected_wallet();
    let of_class = |class: WalletClass| -> Vec<WalletSummary> {
        all.iter()
            .filter(|wallet| wallet.class == class)
            .cloned()
            .collect()
    };
    let hot = of_class(WalletClass::Hot);
    let offline = of_class(WalletClass::Offline);
    let contracts = of_class(WalletClass::SmartContract);

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Wallets" }
                    }
                    div { class: "btn-row",
                        if !all.is_empty() {
                            button {
                                title: "Show the public keys of wallets here, for another device to add them",
                                onclick: move |_| export_open.set(true),
                                "Export keys"
                            }
                        }
                        button { class: "primary",
                            onclick: move |_| {
                                session.when_unlocked("Unlock the vault to add a wallet to it.", move || modal_open.set(true));
                            },
                            Icon { glyph: Glyph::Plus }
                            "Add wallet"
                        }
                    }
                }
                if locked {
                    p { class: "note",
                        "The vault is locked. Its wallets and balances are shown; signing, and adding, removing or changing a wallet, ask for the password."
                    }
                }

                if all.is_empty() {
                    div { class: "empty",
                        Icon { glyph: Glyph::Wallet }
                        p { "No wallets yet. Add one that exists, by its key or its address, or create a new one." }
                    }
                }

                if !hot.is_empty() {
                    h3 { class: "group-title",
                        "Hot wallets"
                        HelpTip { text: "A hot wallet's private key or seed phrase is stored in this device's vault, so the app can sign with it here." }
                    }
                    ul { class: "tree",
                        for wallet in hot {
                            WalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                        }
                    }
                }

                h3 { class: "group-title",
                    "Offline wallets"
                    HelpTip { text: "An offline wallet is the address of a key kept on another device. This device prepares its transactions and the other one signs them, from a QR code or pasted text. A hot wallet can pay its fees, and a programmatic signer lets it act without signing a transaction itself." }
                }
                if offline.is_empty() {
                    p { class: "note",
                        "None yet. Add the address of a key kept on another device to prepare its transactions here."
                    }
                }
                ul { class: "tree",
                    for wallet in offline {
                        WalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                    }
                }

                h3 { class: "group-title",
                    "Hardware wallets"
                    HelpTip { text: "A hardware wallet keeps its key on a separate signing device that is plugged in or paired when it is time to sign." }
                }
                p { class: "note", "Hardware wallets are not supported yet." }

                h3 { class: "group-title",
                    "Smart contract wallets"
                    HelpTip { text: "A smart contract wallet has no key of its own. A program on chain decides who may act: a programmatic signer acts when its authority key signs a message, and a Squads multisig acts when enough of its members approve." }
                }
                if contracts.is_empty() {
                    p { class: "note",
                        "None yet. Add a programmatic signer to sign offline and let a hot wallet pay, or a Squads multisig to share control."
                    }
                }
                ul { class: "tree",
                    for wallet in contracts {
                        ContractWalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                    }
                }
            }
        }
        if modal_open() {
            AddWalletModal {}
        }
        if export_open() {
            ExportKeysModal { on_close: move |_| export_open.set(false) }
        }
    }
}

/// A wallet's SOL balance, with the small button that sends from it. Shown
/// only where balances are: the online build.
#[component]
pub(in crate::ui) fn WalletBalance(
    lamports: Option<u64>,
    send_title: &'static str,
    on_send: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "wallet-balance",
            if let Some(lamports) = lamports {
                p { class: "balance", "{format_sol(lamports)}" }
            }
            button { class: "ghost icon-btn sm",
                title: send_title,
                aria_label: "Send SOL",
                onclick: move |_| on_send.call(()),
                Icon { glyph: Glyph::Send }
            }
        }
    }
}
