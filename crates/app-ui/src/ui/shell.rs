//! The app shell both builds share: the chrome around the panes, and the pane
//! switch. A build passes in what only it has: its brand, its tabs, and the
//! slots it fills (the Stake pane, top bar chips, the sidebar's status bar,
//! the browser-request bar, modals).

use crate::state::session::{Session, Tab};
use crate::ui::{
    use_phone_layout, AccountInspector, AddressBookPanel, BuildPanel, DeviceDialog,
    FingerprintOffer, Glyph, Icon, LockScreen, PhoneLayout, PhoneSheet, ProgramsPanel,
    SettingsPanel, SignPanel, ThemeToggle, UnlockPrompt, WalletSwitcher, WalletsPanel,
};
use crate::{MAIN_CSS, TOKENS_CSS};
use dioxus::prelude::*;

/// The brand mark: the app's icon and name. Online and airgap each provide one.
#[derive(Clone, Copy, PartialEq)]
pub struct AppBrand {
    pub name: &'static str,
    pub sub: &'static str,
    /// The app icon, shown beside the name and as the page's favicon.
    pub mark: Asset,
}

/// Everything but the window: the tabs, top bar, banners, the pane on
/// screen, and the slots a build fills. Mount it once, as the whole
/// of the root component, after [`use_session`](crate::use_session).
#[component]
pub fn AppShell(
    brand: AppBrand,
    /// The navigation, in order. A tab a build lacks is not listed.
    tabs: &'static [Tab],
    /// A phone's further sections, listed under More at the end of the tab
    /// bar. A shell that passes them is laid out for a phone: short tab
    /// labels, and the long hints under the controls left out.
    more: Option<&'static [Tab]>,
    /// The Stake pane. A build without it lists no [`Tab::Stake`].
    stake: Option<Element>,
    /// The Portfolio pane. A build without it lists no [`Tab::Portfolio`].
    portfolio: Option<Element>,
    /// The History pane. A build without it lists no [`Tab::History`].
    history: Option<Element>,
    /// Top bar items between the wallet switcher and the theme toggle: the
    /// cluster, status chips and the notification bell.
    top_bar_extra: Option<Element>,
    /// The status bar at the bottom of the sidebar: the network chip.
    sidebar_status: Option<Element>,
    /// In Settings, for what only this build has: browser connections.
    settings_extra: Option<Element>,
    /// Above the pane: the browser-request bar.
    above_panes: Option<Element>,
    /// Outside the pane switch: modals and toasts a build hosts.
    overlays: Option<Element>,
    /// The window's own title bar buttons, at the right end of the top bar.
    /// Only a desktop window without the native frame passes them; the
    /// shell then keeps the top bar in place and scrolls the pane under it.
    window_controls: Option<Element>,
    /// Shown in place of the lock screen while the vault is locked: on a
    /// phone's first launch, how the app is to be used, before its vault is
    /// created.
    setup: Option<Element>,
) -> Element {
    use_context_provider(|| brand);
    let session = use_context::<Session>();
    use_drop(app_core::diagnostics::log_stopped);

    // A store that could not be read at startup is reported once, not replaced.
    use_effect(move || {
        let problems = session.load_problems();
        if !problems.is_empty() {
            session.show_problem(problems.join(" "));
        }
    });

    let gated = (session.gate)();
    let frameless = window_controls.is_some();
    let phone = more.is_some();
    use_context_provider(|| PhoneLayout(phone));
    let mut class = String::from("app");
    if gated {
        class.push_str(" locked");
    }
    if frameless {
        class.push_str(" frameless");
    }
    if phone {
        class.push_str(" phone");
    }
    // A phone's More sheet sits flush on its tab bar, whose height changes
    // with the status line and the system's gesture bar, so the bar is
    // measured. Locked, there is no tab bar.
    let dock = use_signal(|| 0.0_f64);
    let style = if phone && !gated {
        format!("--dock-h: {}px", dock())
    } else {
        String::new()
    };

    rsx! {
        document::Title { "{brand.name}" }
        document::Link { rel: "icon", href: brand.mark }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }

        div {
            class,
            style,
            onmousemove: move |_| session.ui_activity(),
            onkeydown: move |_| session.ui_activity(),
            onclick: move |_| session.ui_activity(),
            ontouchstart: move |_| session.ui_activity(),
            if !gated {
                Sidebar { tabs, more, status: sidebar_status, dock }
            }
            div { class: "main",
                TopBar { extra: top_bar_extra, controls: window_controls }
                // Scrolls the pane under the top bar in a frameless window;
                // otherwise it is not a box, and the page scrolls.
                div { class: "pane-scroll",
                    main { class: "content",
                        if let Some(message) = (session.save_error)() {
                            div { class: "save-error", role: "alert",
                                p { class: "error", "{message}" }
                                button { class: "ghost sm",
                                    onclick: move |_| {
                                        let mut shown = session.save_error;
                                        shown.set(None);
                                    },
                                    "Dismiss"
                                }
                            }
                        }

                        {above_panes}

                        if gated {
                            if let Some(setup) = setup {
                                {setup}
                            } else {
                                LockScreen {}
                            }
                        } else {
                            match session.current_tab() {
                                Tab::Portfolio => portfolio.unwrap_or_else(|| rsx! { WalletsPanel {} }),
                                Tab::Wallets => rsx! { WalletsPanel {} },
                                Tab::Stake => stake.unwrap_or_else(|| rsx! { WalletsPanel {} }),
                                Tab::AddressBook => rsx! { AddressBookPanel {} },
                                Tab::Programs => rsx! { ProgramsPanel {} },
                                Tab::Build => rsx! { BuildPanel {} },
                                Tab::Sign => rsx! { SignPanel {} },
                                Tab::History => history.unwrap_or_else(|| rsx! { SignPanel {} }),
                                Tab::Settings => rsx! { SettingsPanel { extra: settings_extra.clone() } },
                            }
                        }
                    }
                }
            }
            {overlays}
            UnlockPrompt {}
            FingerprintOffer {}
            AccountInspector {}
            DeviceDialog {}
        }
    }
}

#[component]
pub fn Brand() -> Element {
    let brand = use_context::<AppBrand>();
    rsx! {
        // One line, at the same place in every layout; the subtitle is
        // the tooltip.
        div { class: "brand", "data-drag": "true", title: "{brand.sub}",
            img { class: "brand-mark", src: brand.mark, alt: "", draggable: "false" }
            span { class: "brand-name", "{brand.name}" }
        }
    }
}

/// A tab's label in a phone's tab bar, where a long one would wrap.
fn short_label(tab: Tab) -> &'static str {
    match tab {
        Tab::Sign => "Sign",
        Tab::AddressBook => "Contacts",
        other => tab_entry(other).0,
    }
}

/// The label and icon of a tab in the sidebar.
fn tab_entry(tab: Tab) -> (&'static str, Glyph) {
    match tab {
        Tab::Portfolio => ("Portfolio", Glyph::Pie),
        Tab::Wallets => ("Wallets", Glyph::Wallet),
        Tab::Stake => ("Stake", Glyph::Growth),
        Tab::AddressBook => ("Address Book", Glyph::Book),
        Tab::Programs => ("Programs", Glyph::Code),
        Tab::Build => ("Build", Glyph::Layers),
        Tab::Sign => ("Review and Sign", Glyph::Pen),
        Tab::History => ("History", Glyph::History),
        Tab::Settings => ("Settings", Glyph::Sliders),
    }
}

#[component]
pub fn Sidebar(
    tabs: &'static [Tab],
    more: Option<&'static [Tab]>,
    status: Option<Element>,
    /// Given on a phone: kept at the tab bar's height, for the More sheet
    /// that sits on it.
    #[props(default)]
    dock: Option<Signal<f64>>,
) -> Element {
    let session = use_context::<Session>();
    let mut more_open = use_signal(|| false);
    let current = session.current_tab();
    let in_more = more.is_some_and(|more| more.contains(&current));
    let phone = more.is_some();
    rsx! {
        aside {
            class: "sidebar",
            onresize: move |e| {
                if let (Some(mut dock), Ok(size)) = (dock, e.get_border_box_size()) {
                    dock.set(size.height);
                }
            },
            Brand {}
            nav { class: "nav", aria_label: "Sections",
                for &tab in tabs {
                    {
                        let (label, glyph) = tab_entry(tab);
                        let shown = if phone { short_label(tab) } else { label };
                        rsx! {
                            button {
                                // While More is open it is the one selected:
                                // two lit tabs would leave which one the
                                // sheet belongs to unclear.
                                class: if current == tab && !more_open() { "nav-item on" } else { "nav-item" },
                                key: "{label}",
                                onclick: move |_| {
                                    more_open.set(false);
                                    session.goto(tab);
                                },
                                Icon { glyph }
                                span { class: "nav-label", "{shown}" }
                            }
                        }
                    }
                }
                if let Some(more) = more {
                    button {
                        class: if in_more || more_open() { "nav-item on" } else { "nav-item" },
                        aria_expanded: "{more_open()}",
                        onclick: move |_| more_open.toggle(),
                        Icon { glyph: Glyph::More }
                        span { class: "nav-label", "More" }
                    }
                    if more_open() {
                        PhoneSheet {
                            label: "More",
                            role: "menu",
                            title: "More",
                            class: "more-sheet",
                            on_close: move |_| more_open.set(false),
                            for &tab in more {
                                {
                                    let (label, glyph) = tab_entry(tab);
                                    rsx! {
                                        button {
                                            class: if current == tab { "more-item on" } else { "more-item" },
                                            key: "{label}",
                                            role: "menuitem",
                                            onclick: move |_| {
                                                more_open.set(false);
                                                session.goto(tab);
                                            },
                                            Icon { glyph }
                                            span { "{label}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(status) = status {
                div { class: "sidebar-status", {status} }
            }
        }
    }
}

#[component]
pub fn TopBar(extra: Option<Element>, controls: Option<Element>) -> Element {
    // A phone's bar has no room to spare for a mark that says which app
    // this is: the wallet picker takes its place and the bar's whole width.
    let phone = use_phone_layout();
    rsx! {
        // `data-drag`: in a frameless window, empty space here moves it.
        header { class: "topbar", "data-drag": "true",
            if !phone {
                Brand {}
            }
            div { class: "topbar-tools",
                WalletSwitcher {}
                LockToggle {}
                {extra}
                ThemeToggle {}
            }
            {controls}
        }
    }
}

/// Locks or unlocks the vault from the top bar. Absent until a vault exists.
#[component]
fn LockToggle() -> Element {
    let session = use_context::<Session>();
    let phone = use_phone_layout();
    if !session.is_initialized() {
        return rsx! {};
    }
    let unlocked = session.is_unlocked();
    let (glyph, label) = if unlocked {
        (Glyph::Unlock, "Lock vault")
    } else {
        (Glyph::Lock, "Unlock vault")
    };
    // On a phone the locked vault is the state to notice, and not by colour
    // alone: a filled button that says so. Unlocked is the usual state, a
    // quiet icon like the bell beside it.
    let class = match (phone, unlocked) {
        (true, true) => "icon-btn sm lock-toggle",
        (true, false) => "sm lock-toggle locked",
        (false, true) => "icon-btn sm lock-toggle unlocked",
        (false, false) => "icon-btn sm lock-toggle",
    };
    rsx! {
        button {
            class,
            title: "{label}",
            aria_label: "{label}",
            onclick: move |_| {
                if unlocked {
                    session.lock();
                } else {
                    session.when_unlocked("Unlock the vault to sign with its keys and change its wallets.", || {});
                }
            },
            Icon { glyph }
            if phone && !unlocked {
                span { "Locked" }
            }
        }
    }
}
