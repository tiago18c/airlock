use super::lock::VaultUnlock;
use crate::state::forms::{AddWalletMode, SendOpen, WalletForm};
use crate::state::session::{Session, Tab, WalletSummary};
use crate::ui::wallets::kinds::{
    ContractWalletRow, CurrentChip, ExportKeysModal, HardwareForm, KeyImportForm, OfflineForm,
    ProgrammaticForm, SquadsForm, VectorForm, WalletDialogButtons, WalletRowMenu,
};
use crate::ui::{format_sol, AddressHeader, CopyButton, Glyph, HelpTip, Icon, PaneMenu};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::{
    Category, DerivedAccount, NewSigner, WalletClass, DERIVE_BATCH, SOLANA_PATH_TEMPLATE,
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
    let hardware = of_class(WalletClass::Hardware);
    let contracts = of_class(WalletClass::SmartContract);
    // A phone lists only the groups that hold a wallet.
    let phone = app_ui_kit::ui::use_phone_layout();
    // What reads the chain and sends is only offered in the online build.
    let online = crate::state::use_network().is_some();
    // Burn and close opens on the wallet in use, and picks another there.
    let first_wallet = current.or_else(|| all.first().map(|wallet| wallet.id));

    rsx! {
        section { class: "page",
            div { class: "stack",
                div { class: "pane-head",
                    div {
                        h2 { "Wallets" }
                    }
                    div { class: "btn-row",
                        button { class: "primary",
                            onclick: move |_| {
                                session.when_unlocked("Unlock the vault to add a wallet to it.", move || modal_open.set(true));
                            },
                            Icon { glyph: Glyph::Plus }
                            "Add wallet"
                        }
                        if !all.is_empty() {
                            PaneMenu { label: "More wallet actions",
                                button { class: "pane-menu-item",
                                    role: "menuitem",
                                    title: "Show the public keys of wallets here, for another device to add them",
                                    onclick: move |_| export_open.set(true),
                                    Icon { glyph: Glyph::Qr }
                                    "Export keys"
                                }
                                if online {
                                    button { class: "pane-menu-item",
                                        role: "menuitem",
                                        title: "Move everything a wallet holds to another address: to rotate a key, or to move to a programmatic signer, a Vector account or a multisig",
                                        onclick: move |_| session.open_migrate(current),
                                        Icon { glyph: Glyph::Send }
                                        "Migrate wallet"
                                    }
                                    button { class: "pane-menu-item",
                                        role: "menuitem",
                                        title: "Burn and close a wallet's token accounts, to get their rent back",
                                        onclick: move |_| {
                                            if let Some(wallet) = first_wallet {
                                                session.open_cleanup(wallet);
                                            }
                                        },
                                        Icon { glyph: Glyph::Trash }
                                        "Manage tokens"
                                    }
                                }
                            }
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
                        if phone {
                            p { "No wallets yet." }
                        } else {
                            p { "No wallets yet. Add one that exists, by its key or its address, or create a new one." }
                        }
                    }
                }

                if !hot.is_empty() {
                    h3 { class: "group-title wallets-group",
                        "Hot wallets"
                        HelpTip { text: "A hot wallet's private key or seed phrase is stored in this device's vault, so the app can sign with it here." }
                    }
                    ul { class: "tree",
                        for wallet in hot {
                            WalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                        }
                    }
                }

                if !phone || !offline.is_empty() {
                h3 { class: "group-title wallets-group",
                    "Offline wallets"
                    HelpTip { text: "An offline wallet is the address of a key kept on another device. This device prepares its transactions and the other one signs them, from a QR code or pasted text. A hot wallet can pay its fees, and a programmatic signer lets it act without signing a transaction itself." }
                }
                if offline.is_empty() {
                    p { class: "wallets-empty",
                        "None yet. Add the address of a key kept on another device to prepare its transactions here."
                    }
                }
                ul { class: "tree",
                    for wallet in offline {
                        WalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                    }
                }
                }

                if !phone || !hardware.is_empty() {
                h3 { class: "group-title wallets-group",
                    "Hardware wallets"
                    HelpTip { text: "A hardware wallet keeps its key on a separate signing device that is plugged in or paired when it is time to sign." }
                }
                if hardware.is_empty() {
                    if session.hardware_offered().is_empty() {
                        div { class: "wallets-empty",
                            p {
                                "No hardware wallets yet. Ledger support is pre-release until Ledger ships the Solana app 1.18."
                            }
                            button { class: "sm",
                                title: "Go to Settings and turn on Pre-release hardware wallets, to add one",
                                onclick: move |_| session.goto(Tab::Settings),
                                "Turn on in Settings"
                            }
                        }
                    } else {
                        p { class: "wallets-empty",
                            "None yet. Add one with Add wallet, Hardware or Seed Vault."
                        }
                    }
                }
                ul { class: "tree",
                    for wallet in hardware {
                        WalletRow { key: "{wallet.id}", current: current == Some(wallet.id), wallet }
                    }
                }
                }

                if !phone || !contracts.is_empty() {
                h3 { class: "group-title wallets-group",
                    "Smart contract wallets"
                    HelpTip { text: "A smart contract wallet has no key of its own. A program on chain decides who may act: a programmatic signer acts when its authority key signs a message, and a Squads multisig acts when enough of its members approve." }
                }
                if contracts.is_empty() {
                    p { class: "wallets-empty",
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
        }
        if modal_open() {
            AddWalletModal {}
        }
        if export_open() {
            ExportKeysModal { on_close: move |_| export_open.set(false) }
        }
    }
}

/// A wallet's SOL balance and the small button that sends from it. Shown only
/// where balances are: the online build. What a balance is worth belongs to
/// Portfolio, not here.
#[component]
pub(in crate::ui) fn WalletBalance(
    lamports: Option<u64>,
    send_title: &'static str,
    on_send: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "wallet-balance",
            if let Some(lamports) = lamports {
                span { class: "balance", "{format_sol(lamports)}" }
            }
            button { class: "ghost sm wallet-send",
                title: send_title,
                onclick: move |_| on_send.call(()),
                Icon { glyph: Glyph::Send }
                "Send"
            }
        }
    }
}
