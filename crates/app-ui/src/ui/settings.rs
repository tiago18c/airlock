use crate::state::forms::AddWalletMode;
use crate::state::session::Session;
use crate::ui::ThemeToggle;
use app_ui_kit::state::slices::VaultState;
use dioxus::prelude::*;
use wallet_core::{Cluster, LOCAL_RPC_URL};
use zeroize::{Zeroize, Zeroizing};

/// Auto-lock choices offered in Settings (seconds). Includes 1 minute for the
/// phase 1.5 Done-when check.
const AUTO_LOCK_CHOICES: &[(u64, &str)] = &[
    (60, "1 minute"),
    (5 * 60, "5 minutes"),
    (15 * 60, "15 minutes"),
    (30 * 60, "30 minutes"),
    (60 * 60, "1 hour"),
];

#[component]
pub fn SettingsPanel() -> Element {
    let session = use_context::<Session>();
    let cluster = session.cluster();
    let online = crate::state::use_network().is_some();
    let rpc_url = cluster.rpc_url().to_string();
    // The custom cluster in use, or the one typed last.
    let remembered = if cluster.is_custom() {
        Some(cluster.clone())
    } else {
        session.custom_cluster()
    };
    // Custom was picked in the list. It takes effect once a URL is applied.
    let mut picking_custom = use_signal(|| false);
    let custom_shown = cluster.is_custom() || picking_custom();
    let selected = if custom_shown {
        "custom"
    } else {
        cluster.name()
    }
    .to_string();
    let mut custom_name = use_signal({
        let remembered = remembered.clone();
        move || remembered.map(|c| c.name().to_string()).unwrap_or_default()
    });
    let mut custom_url = use_signal({
        let remembered = remembered.clone();
        move || {
            remembered
                .map(|c| c.rpc_url().to_string())
                .unwrap_or_else(|| LOCAL_RPC_URL.to_string())
        }
    });
    let mut custom_error = use_signal(|| None::<String>);
    let applied = cluster.is_custom()
        && cluster.rpc_url() == custom_url().trim()
        && (cluster.name() == custom_name().trim()
            || (custom_name().trim().is_empty() && cluster.name() == "custom"));
    let vector = session.vector_offered();
    let settings = (session.settings)();
    let auto_lock_secs = settings.auto_lock.as_secs();
    let close_to_tray = settings.close_to_tray;
    let unlocked = (session.vault_state)() == VaultState::Unlocked;

    rsx! {
        section { class: "page",
            div { class: "page-head",
                div {
                    h2 { "Settings" }
                }
            }

            if online {
                div { class: "card",
                    h3 { "Network" }
                    label { "Cluster"
                        select {
                            value: "{selected}",
                            onchange: move |e| {
                                custom_error.set(None);
                                let cluster = match e.value().as_str() {
                                    "custom" => {
                                        picking_custom.set(true);
                                        return;
                                    }
                                    "mainnet-beta" => Cluster::MainnetBeta,
                                    "testnet" => Cluster::Testnet,
                                    _ => Cluster::Devnet,
                                };
                                picking_custom.set(false);
                                crate::actions::network::use_cluster(session, cluster);
                            },
                            option { value: "devnet", selected: selected == "devnet", "Devnet" }
                            option { value: "testnet", selected: selected == "testnet", "Testnet" }
                            option { value: "mainnet-beta", selected: selected == "mainnet-beta", "Mainnet Beta" }
                            option { value: "custom", selected: selected == "custom", "Custom RPC" }
                        }
                    }
                    if custom_shown {
                        label { "RPC URL"
                            input {
                                r#type: "text",
                                placeholder: "{LOCAL_RPC_URL}",
                                value: "{custom_url}",
                                oninput: move |e| custom_url.set(e.value()),
                            }
                        }
                        label { "Name"
                            input {
                                r#type: "text",
                                placeholder: "custom",
                                value: "{custom_name}",
                                oninput: move |e| custom_name.set(e.value()),
                            }
                        }
                        if let Some(problem) = custom_error() {
                            p { class: "error", role: "alert", "{problem}" }
                        }
                        div { class: "btn-row",
                            button {
                                class: "primary",
                                disabled: applied,
                                onclick: move |_| match Cluster::custom(&custom_name(), &custom_url()) {
                                    Ok(cluster) => {
                                        custom_error.set(None);
                                        picking_custom.set(false);
                                        crate::actions::network::use_cluster(session, cluster);
                                    }
                                    Err(problem) => custom_error.set(Some(problem)),
                                },
                                if applied { "In use" } else { "Use this RPC" }
                            }
                        }
                        p { class: "hint",
                            "Any Solana JSON-RPC endpoint: a provider's URL, or a validator on this machine such as surfpool, which serves a local copy of a public network at {LOCAL_RPC_URL}. Everything the app reads and submits goes to this URL. The URL is stored in the workspace file as typed, including any API key in it."
                        }
                    }
                    if !picking_custom() || cluster.is_custom() {
                        p { class: "muted mono", "{rpc_url}" }
                    }
                    p { class: "hint",
                        "At launch and on a cluster change the app asks the node for its genesis hash, and may correct the cluster from the reply. A custom RPC is never corrected: it is used as typed, whatever network it reports."
                    }
                }
            }

            div { class: "card",
                h3 { "Appearance" }
                ThemeToggle { labels: true }
                p { class: "hint", "System follows the light or dark setting of this device." }
            }

            div { class: "card",
                h3 { "Vector wallets" }
                div { class: "segmented",
                    button {
                        class: if vector { "tab" } else { "tab on" },
                        onclick: move |_| {
                            session.edit_workspace(|ws| ws.vector_wallets = false);
                            let form = session.forms.wallet;
                            if (form.mode)() == AddWalletMode::Vector {
                                let mut mode = form.mode;
                                mode.set(if (form.creating)() {
                                    AddWalletMode::Seed
                                } else {
                                    AddWalletMode::PrivateKey
                                });
                            }
                        },
                        "Off"
                    }
                    button {
                        class: if vector { "tab on" } else { "tab" },
                        onclick: move |_| session.edit_workspace(|ws| ws.vector_wallets = true),
                        "On"
                    }
                }
                p { class: "hint",
                    "A Vector wallet is an account of the Vector program, controlled by a key of one of several schemes: Ed25519, secp256k1, an Ethereum address, or a post-quantum key. The program is new. Off, the app does not offer Vector wallets to add, does not show them to sites, and does not build transactions for them. Vector wallets already added stay listed, and a transaction that uses one is still explained."
                }
            }

            div { class: "card",
                h3 { "Vault" }
                label { "Auto-lock"
                    select {
                        value: "{auto_lock_secs}",
                        onchange: move |e| {
                            if let Ok(secs) = e.value().parse::<u64>() {
                                session.edit_workspace(|ws| ws.auto_lock_secs = secs.max(1));
                            }
                        },
                        for (secs, label) in AUTO_LOCK_CHOICES.iter().copied() {
                            option {
                                value: "{secs}",
                                selected: auto_lock_secs == secs,
                                "{label}"
                            }
                        }
                    }
                }
                p { class: "hint",
                    "Locks the vault after this time while the window is hidden, or after the same time with no input while it is open. Default is 5 minutes."
                }
                if unlocked {
                    PasswordChange {}
                }
            }

            // Tray and close-to-tray are online desktop only; the airgap
            // shell has no tray and always quits on close.
            if online {
                div { class: "card",
                    h3 { "Window" }
                    div { class: "segmented",
                        button {
                            class: if close_to_tray { "tab" } else { "tab on" },
                            onclick: move |_| session.edit_workspace(|ws| ws.close_to_tray = false),
                            "Quit on close"
                        }
                        button {
                            class: if close_to_tray { "tab on" } else { "tab" },
                            onclick: move |_| session.edit_workspace(|ws| ws.close_to_tray = true),
                            "Close to tray"
                        }
                    }
                    p { class: "hint",
                        "Close to tray keeps the app running when the window is closed. Use the tray icon to open the window again, lock the vault, or quit."
                    }
                }
            }

            if online {
                PricesCard {}
            }

            DiagnosticsCard {}
        }
    }
}

/// Change the vault's password: the current one, then the new one twice.
#[component]
fn PasswordChange() -> Element {
    let session = use_context::<Session>();
    let mut current = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut note = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    use_drop(move || {
        current.write().zeroize();
        new.write().zeroize();
        confirm.write().zeroize();
    });
    let mut change = move || {
        note.set(None);
        error.set(None);
        let (old, next) = (Zeroizing::new(current()), Zeroizing::new(new()));
        if next.len() < 8 {
            error.set(Some("Use at least 8 characters.".into()));
            return;
        }
        if *next != *Zeroizing::new(confirm()) {
            error.set(Some("The new passwords do not match.".into()));
            return;
        }
        let changed = session.change_password(&old, &next);
        current.write().zeroize();
        new.write().zeroize();
        confirm.write().zeroize();
        match changed {
            Ok(()) => note.set(Some(
                "Password changed. The vault is sealed under it, and the copy kept from before is deleted."
                    .into(),
            )),
            Err(err) => error.set(Some(err.to_string())),
        }
    };

    rsx! {
        h4 { "Change password" }
        label { "Current password"
            input {
                r#type: "password",
                autocomplete: "current-password",
                value: "{current}",
                oninput: move |e| current.set(e.value()),
            }
        }
        label { "New password"
            input {
                r#type: "password",
                autocomplete: "new-password",
                value: "{new}",
                oninput: move |e| new.set(e.value()),
            }
        }
        label { "Confirm new password"
            input {
                r#type: "password",
                autocomplete: "new-password",
                value: "{confirm}",
                oninput: move |e| confirm.set(e.value()),
            }
        }
        div { class: "btn-row",
            button { class: "primary",
                disabled: current().is_empty() || new().is_empty(),
                onclick: move |_| change(),
                "Change password"
            }
        }
        p { class: "hint",
            "Seals the vault under a new key and deletes the backup the old password still opens. A copy of the vault made elsewhere before, such as a backup of the data folder, still opens with the old one."
        }
        if let Some(text) = note() {
            p { class: "hint", "{text}" }
        }
        if let Some(text) = error() {
            p { class: "error", "{text}" }
        }
    }
}

/// The Tokens API key that prices the portfolio. Kept in the workspace, in
/// plain text like the cluster URL, so prices refresh while the vault is
/// locked.
#[component]
fn PricesCard() -> Element {
    let session = use_context::<Session>();
    let saved = (session.settings)().tokens_api_key.is_some();
    let mut typed = use_signal(String::new);
    let mut note = use_signal(|| None::<String>);

    rsx! {
        div { class: "card",
            h3 { "Prices" }
            p { class: "hint",
                "The Portfolio values holdings with the Tokens API (tokens.xyz). Only token mints are sent, never an address or an amount. Prices are for mainnet."
            }
            label { if saved { "API key (saved)" } else { "API key" }
                input {
                    r#type: "password",
                    autocomplete: "off",
                    placeholder: if saved { "Paste a new key to replace it" } else { "Paste the key" },
                    value: "{typed}",
                    oninput: move |e| typed.set(e.value()),
                }
            }
            div { class: "btn-row",
                button { class: "primary",
                    disabled: typed().trim().is_empty(),
                    onclick: move |_| {
                        let key = typed().trim().to_string();
                        session.edit_workspace(|ws| ws.tokens_api_key = Some(key));
                        typed.set(String::new());
                        note.set(Some("Saved. Prices refresh within a few minutes, or press Refresh on Portfolio.".into()));
                    },
                    "Save key"
                }
                if saved {
                    button { class: "ghost",
                        onclick: move |_| {
                            session.edit_workspace(|ws| ws.tokens_api_key = None);
                            note.set(Some("Removed. The Portfolio shows amounts without values.".into()));
                        },
                        "Remove key"
                    }
                }
            }
            if let Some(text) = note() {
                p { class: "hint", "{text}" }
            }
            p { class: "hint",
                "The key is stored in workspace.json in plain text, next to the cluster URL, so prices refresh while the vault is locked."
            }
        }
    }
}

#[component]
fn DiagnosticsCard() -> Element {
    let mut diagnostic = use_signal(crate::diagnostics::diagnostic_logging);
    let mut copy_note = use_signal(|| None::<String>);
    let mut copy_warned = use_signal(|| false);
    let log_var = crate::diagnostics::log_env_var();

    rsx! {
        div { class: "card",
            h3 { "Diagnostics" }
            div { class: "segmented",
                button {
                    class: if diagnostic() { "tab" } else { "tab on" },
                    onclick: move |_| {
                        crate::diagnostics::set_diagnostic_logging(false);
                        diagnostic.set(false);
                    },
                    "Off"
                }
                button {
                    class: if diagnostic() { "tab on" } else { "tab" },
                    onclick: move |_| {
                        crate::diagnostics::set_diagnostic_logging(true);
                        diagnostic.set(true);
                    },
                    "Diagnostic logging"
                }
            }
            p { class: "hint",
                "Off keeps the default info level. On raises logging to debug until the app restarts. {log_var} overrides the default when set."
            }
            div { class: "btn-row",
                {open_log_folder_button(copy_note)}
                button {
                    onclick: move |_| {
                        if !copy_warned() {
                            copy_warned.set(true);
                            copy_note.set(Some(
                                "Logs include full addresses and transaction history. Click again to copy them.".into(),
                            ));
                            return;
                        }
                        let text = crate::diagnostics::diagnostics_bundle();
                        crate::platform::copy_text(&text);
                        copy_note.set(Some("Diagnostics copied.".into()));
                    },
                    "Copy diagnostics"
                }
            }
            if let Some(text) = copy_note() {
                p { class: "hint", role: "status", "{text}" }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open_log_folder_button(mut copy_note: Signal<Option<String>>) -> Element {
    rsx! {
        button {
            onclick: move |_| {
                copy_note.set(match crate::diagnostics::open_log_folder() {
                    Ok(()) => Some("Opened the log folder.".into()),
                    Err(err) => Some(err),
                });
            },
            "Open log folder"
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn open_log_folder_button(_copy_note: Signal<Option<String>>) -> Element {
    rsx! {}
}
