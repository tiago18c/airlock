//! The Add wallet modal: bring in a wallet that exists, or create one.

use super::*;

pub(in crate::ui) const PATH_PRESETS: &[(&str, &str)] = &[
    (SOLANA_PATH_TEMPLATE, "Solana"),
    ("m/44'/501'/0'/0'", "First account only"),
];

pub(in crate::ui) const EXISTING_KINDS: &[(AddWalletMode, &str)] = &[
    (AddWalletMode::PrivateKey, "Private key"),
    (AddWalletMode::Seed, "Seed phrase"),
    (AddWalletMode::Offline, "Offline"),
    (AddWalletMode::FromDevice, "From another device"),
    (AddWalletMode::SeedVault, "Seed Vault"),
    (AddWalletMode::Hardware, "Hardware"),
    (AddWalletMode::Programmatic, "Programmatic signer"),
    (AddWalletMode::Vector, "Vector"),
    (AddWalletMode::Squads, "Squads"),
];

pub(in crate::ui) const NEW_KINDS: &[(AddWalletMode, &str)] = &[
    (AddWalletMode::Seed, "Seed phrase"),
    (AddWalletMode::Programmatic, "Programmatic signer"),
    (AddWalletMode::Vector, "Vector"),
    (AddWalletMode::Squads, "Squads"),
];

/// What the add-wallet modal can offer beyond the kinds every build has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Offered {
    /// Vector, when the setting is on.
    vector: bool,
    /// This phone's Seed Vault.
    seed_vault: bool,
    /// The Hardware tab: a Ledger, Trezor or Keystone can be added, or
    /// Settings can turn them on.
    hardware: bool,
}

/// The kind tabs for the add-wallet modal.
fn kinds_for(creating: bool, offered: Offered) -> Vec<(AddWalletMode, &'static str)> {
    let base = if creating { NEW_KINDS } else { EXISTING_KINDS };
    base.iter()
        .copied()
        .filter(|(kind, _)| match kind {
            AddWalletMode::Vector => offered.vector,
            AddWalletMode::SeedVault => offered.seed_vault,
            AddWalletMode::Hardware => offered.hardware,
            _ => true,
        })
        .collect()
}

#[component]
pub(in crate::ui) fn AddWalletModal() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.wallet;
    let mut label = form.label;
    let mut secret = form.secret;
    let passphrase = form.passphrase;
    let path_template = form.path_template;
    let mode = form.mode;
    let creating = form.creating;
    let error = form.error;
    let mut modal_open = form.modal_open;
    let tail = use_signal(SeedTail::default);
    let generation = use_signal(|| 0u64);
    let written = use_signal(|| false);
    let words = use_signal(|| 12usize);
    let mut path_custom = use_signal({
        move || {
            let current = path_template.peek().trim().to_string();
            !PATH_PRESETS.iter().any(|(path, _)| *path == current)
        }
    });
    let state = ModalState {
        tail,
        generation,
        written,
        words,
        path_custom,
    };
    // PBKDF2 only once the phrase is a whole mnemonic. Typing the words before
    // that changes nothing here.
    let accounts = use_memo(move || {
        seed_accounts(
            mode(),
            creating(),
            &secret(),
            &passphrase(),
            &path_template(),
        )
    });

    let vector_on = session.vector_offered();
    let devices = session.hardware_offered();
    let seed_vault = devices.contains(&wallet_core::HardwareVendor::SeedVault);
    let offered = Offered {
        vector: vector_on,
        seed_vault,
        // A phone has no Pre-release switch to point to: with nothing to add
        // there, the tab is left out.
        hardware: !crate::ui::use_phone_layout()
            || devices
                .iter()
                .any(|vendor| *vendor != wallet_core::HardwareVendor::SeedVault),
    };
    let kinds = kinds_for(creating(), offered);
    // Vector was selected, then the setting was turned off, or a tab this
    // build does not offer was left selected. Show another kind without
    // writing the signal: Settings already stores the switch.
    let stored = mode();
    let kind = if !creating() && !kinds.iter().any(|(kind, _)| *kind == stored) {
        AddWalletMode::PrivateKey
    } else if stored == AddWalletMode::Vector && !vector_on {
        if creating() {
            AddWalletMode::Seed
        } else {
            AddWalletMode::PrivateKey
        }
    } else {
        stored
    };
    let seed_error = accounts.read().error.clone();

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| modal_open.set(false),
            div { class: "modal wide add-wallet",
                onclick: move |e| e.stop_propagation(),
                div { class: "add-wallet-frame",
                    div { class: "add-wallet-head",
                        h3 { "Add wallet" }
                        div { class: "tabs",
                            button {
                                class: if creating() { "tab" } else { "tab on" },
                                onclick: move |_| show(form, state, false, kind),
                                "Existing wallet"
                            }
                            button {
                                class: if creating() { "tab on" } else { "tab" },
                                onclick: move |_| show(form, state, true, kind),
                                "Create new"
                            }
                        }
                        p { class: "lede wordy",
                            if creating() {
                                "Make a new wallet: a seed phrase generated on this device, or a wallet controlled through a key."
                            } else {
                                "Bring in a wallet that already exists: by its key, by its address when the key stays on another device, or one controlled through a key."
                            }
                        }
                        div { class: "segmented",
                            for (mode_kind, name) in kinds.iter().copied() {
                                button {
                                    key: "{name}",
                                    class: if kind == mode_kind { "tab on" } else { "tab" },
                                    onclick: move |_| show(form, state, creating(), mode_kind),
                                    "{name}"
                                }
                            }
                        }
                    }

                    div { class: "add-wallet-body",
                    if kind == AddWalletMode::Hardware {
                        HardwareForm { seed_vault: false, on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::SeedVault {
                        HardwareForm { seed_vault: true, on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::Offline {
                        OfflineForm { on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::FromDevice {
                        KeyImportForm { on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::Programmatic {
                        ProgrammaticForm { creating: creating(), on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::Vector {
                        VectorForm { creating: creating(), on_done: move |_| modal_open.set(false) }
                    } else if kind == AddWalletMode::Squads {
                        SquadsForm { creating: creating(), on_done: move |_| modal_open.set(false) }
                    } else {
                        label { "Label"
                            input {
                                r#type: "text",
                                placeholder: "Treasury",
                                value: "{label}",
                                oninput: move |e| label.set(e.value()),
                            }
                        }
                    }

                    if kind == AddWalletMode::PrivateKey {
                        label { "Private key"
                            textarea {
                                placeholder: "Base58 secret key, hex seed, or a JSON keypair array.",
                                value: "{secret}",
                                oninput: move |e| secret.set(e.value()),
                            }
                        }
                    }

                    if kind == AddWalletMode::Seed && creating() {
                        NewSeedFields { state }
                    }

                    if kind == AddWalletMode::Seed && !creating() {
                        SeedImportFields { state, accounts }
                    }

                    if let Some(err) = error().or(seed_error) {
                        p { class: "error", "{err}" }
                    }
                    }
                    if kind.imports_a_key() {
                    div { class: "btn-row",
                        if kind.imports_a_key() {
                            button { class: "primary",
                                disabled: creating() && !written(),
                                onclick: move |_| {
                                    let view = accounts.peek().clone();
                                    let extra = tail.peek().clone();
                                    let rows = shown_seed_rows(&view.rows, &extra, *generation.peek());
                                    save_wallet(session, form, &rows);
                                },
                                if creating() {
                                    "Create wallet"
                                } else if kind == AddWalletMode::Seed {
                                    "Add selected"
                                } else {
                                    "Save wallet"
                                }
                            }
                        }
                        if !creating() {
                            button { class: "ghost",
                                onclick: move |_| {
                                    form.clear();
                                    path_custom.set(false);
                                    invalidate_seed(tail, generation);
                                },
                                "Clear"
                            }
                        }
                        button { class: "ghost",
                            onclick: move |_| modal_open.set(false),
                            "Cancel"
                        }
                    }
                    }
                }
            }
        }
    }
}
