//! Connections: the sites connected now, those authorized, and those blocked.
//! Opened from the extension chip's card and from Settings.

use app_ui_kit::{AddressRow, Session};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;

/// Whether the modal is open. Provided with the request modal's own.
#[derive(Clone, Copy)]
pub struct ConnectionsModal {
    open: Signal<bool>,
}

impl ConnectionsModal {
    pub(super) fn closed() -> Self {
        Self {
            open: Signal::new(false),
        }
    }

    pub fn show(mut self) {
        self.open.set(true);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum List {
    Connected,
    Authorized,
    Blocked,
}

/// The way in from Settings.
#[component]
pub fn ConnectionsCard() -> Element {
    let manage = use_context::<ConnectionsModal>();
    rsx! {
        div { class: "card",
            h3 { "Browser connections" }
            p { class: "hint",
                "Which sites are connected now, which you authorized, and which you blocked."
            }
            div { class: "btn-row",
                button {
                    onclick: move |_| manage.show(),
                    "Manage connections"
                }
            }
        }
    }
}

/// The modal, when it is open. The shell mounts this once.
#[component]
pub fn ConnectionsHost() -> Element {
    let session = use_context::<Session>();
    let ConnectionsModal { mut open } = use_context::<ConnectionsModal>();
    let mut list = use_signal(|| List::Connected);
    if !open() {
        return rsx! {};
    }
    let sites = (session.sites)();
    let shown = list();
    let tab = |which: List| if shown == which { "tab on" } else { "tab" };

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| open.set(false),
            div { class: "modal wide request", role: "dialog", aria_modal: "true",
                onclick: move |e| e.stop_propagation(),
                div { class: "request-head",
                    h3 { "Connections" }
                    button { class: "ghost sm",
                        onclick: move |_| open.set(false),
                        "Close"
                    }
                }
                div { class: "segmented",
                    button { class: tab(List::Connected),
                        onclick: move |_| list.set(List::Connected),
                        "Connected ({sites.connected.len()})"
                    }
                    button { class: tab(List::Authorized),
                        onclick: move |_| list.set(List::Authorized),
                        "Authorized ({sites.authorized.len()})"
                    }
                    button { class: tab(List::Blocked),
                        onclick: move |_| list.set(List::Blocked),
                        "Blocked ({sites.blocked.len()})"
                    }
                }
                div { class: "request-body",
                    match shown {
                        List::Connected => rsx! {
                            if sites.connected.is_empty() {
                                p { class: "muted", "No site is connected right now." }
                            }
                            for site in sites.connected.clone() {
                                SiteRow {
                                    key: "{site.origin}",
                                    origin: site.origin.clone(),
                                    accounts: site.accounts.clone(),
                                    chip: (!site.remembered).then(|| "Until the app closes".to_string()),
                                    action: Action::Disconnect,
                                }
                            }
                        },
                        List::Authorized => rsx! {
                            if sites.authorized.is_empty() {
                                p { class: "muted",
                                    "No site is remembered. Tick Remember this connection when one asks to connect."
                                }
                            }
                            for site in sites.authorized.clone() {
                                SiteRow {
                                    key: "{site.origin}",
                                    origin: site.origin.clone(),
                                    accounts: site.accounts.clone(),
                                    action: Action::Revoke,
                                }
                            }
                        },
                        List::Blocked => rsx! {
                            if sites.blocked.is_empty() {
                                p { class: "muted", "No site is blocked." }
                            }
                            for origin in sites.blocked.clone() {
                                SiteRow {
                                    key: "{origin}",
                                    origin: origin.clone(),
                                    accounts: Vec::new(),
                                    action: Action::Unblock,
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Disconnect,
    Revoke,
    Unblock,
}

#[component]
fn SiteRow(origin: String, accounts: Vec<Pubkey>, chip: Option<String>, action: Action) -> Element {
    let session = use_context::<Session>();
    let main_origin = origin.clone();
    let block_origin = origin.clone();
    let (label, hint) = match action {
        Action::Disconnect => (
            "Disconnect",
            "End the connection. The site is let back in when it next asks to connect.",
        ),
        Action::Revoke => (
            "Revoke",
            "Forget what this site was given, and disconnect it.",
        ),
        Action::Unblock => ("Unblock", "Let this site ask to connect again."),
    };
    rsx! {
        div { class: "site-row",
            div { class: "chip-row",
                p { class: "bridge-origin", "{origin}" }
                if let Some(chip) = chip {
                    span { class: "chip", "{chip}" }
                }
            }
            for account in accounts {
                AddressRow { key: "{account}", pubkey: account, no_copy: true }
            }
            div { class: "btn-row",
                if action != Action::Unblock {
                    button { class: "danger sm",
                        title: "Reject this site's requests from now on.",
                        onclick: move |_| session.site_block(&block_origin),
                        "Block"
                    }
                }
                button { class: "sm",
                    title: "{hint}",
                    onclick: move |_| match action {
                        Action::Disconnect => session.site_disconnect(&main_origin),
                        Action::Revoke => session.site_revoke(&main_origin),
                        Action::Unblock => session.site_unblock(&main_origin),
                    },
                    "{label}"
                }
            }
        }
    }
}
