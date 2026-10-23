//! The app shell both builds share: the chrome around the panes, and the pane
//! switch. A build passes in what only it has: its brand, its tabs, and the
//! slots it fills (the Stake pane, top bar chips, the sidebar's status bar,
//! the browser-request bar, modals).

use crate::state::session::{Session, Tab};
use crate::ui::{
    AddressBookPanel, BuildPanel, Glyph, Icon, LockScreen, ProgramsPanel, SettingsPanel, SignPanel,
    ThemeToggle, UnlockPrompt, WalletSwitcher, WalletsPanel,
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
    /// The Stake pane. A build without it lists no [`Tab::Stake`].
    stake: Option<Element>,
    /// The Portfolio pane. A build without it lists no [`Tab::Portfolio`].
    portfolio: Option<Element>,
    /// Top bar items between the wallet switcher and the theme toggle: the
    /// cluster, status chips and the notification bell.
    top_bar_extra: Option<Element>,
    /// The status bar at the bottom of the sidebar: the network chip.
    sidebar_status: Option<Element>,
    /// Above the pane: the browser-request bar.
    above_panes: Option<Element>,
    /// Outside the pane switch: modals and toasts a build hosts.
    overlays: Option<Element>,
    /// The window's own title bar buttons, at the right end of the top bar.
    /// Only a desktop window without the native frame passes them; the
    /// shell then keeps the top bar in place and scrolls the pane under it.
    window_controls: Option<Element>,
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
    let class = match (gated, frameless) {
        (false, false) => "app",
        (true, false) => "app locked",
        (false, true) => "app frameless",
        (true, true) => "app locked frameless",
    };

    rsx! {
        document::Title { "{brand.name}" }
        document::Link { rel: "icon", href: brand.mark }
        document::Stylesheet { href: TOKENS_CSS }
        document::Stylesheet { href: MAIN_CSS }

        div {
            class,
            onmousemove: move |_| session.ui_activity(),
            onkeydown: move |_| session.ui_activity(),
            onclick: move |_| session.ui_activity(),
            if !gated {
                Sidebar { tabs, status: sidebar_status }
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
                            LockScreen {}
                        } else {
                            match session.current_tab() {
                                Tab::Portfolio => portfolio.unwrap_or_else(|| rsx! { WalletsPanel {} }),
                                Tab::Wallets => rsx! { WalletsPanel {} },
                                Tab::Stake => stake.unwrap_or_else(|| rsx! { WalletsPanel {} }),
                                Tab::AddressBook => rsx! { AddressBookPanel {} },
                                Tab::Programs => rsx! { ProgramsPanel {} },
                                Tab::Build => rsx! { BuildPanel {} },
                                Tab::Sign => rsx! { SignPanel {} },
                                Tab::Settings => rsx! { SettingsPanel {} },
                            }
                        }
                    }
                }
            }
            {overlays}
            UnlockPrompt {}
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
        Tab::Settings => ("Settings", Glyph::Sliders),
    }
}

#[component]
pub fn Sidebar(tabs: &'static [Tab], status: Option<Element>) -> Element {
    let session = use_context::<Session>();
    rsx! {
        aside { class: "sidebar",
            Brand {}
            nav { class: "nav", aria_label: "Sections",
                for &tab in tabs {
                    {
                        let (label, glyph) = tab_entry(tab);
                        rsx! {
                            button {
                                class: if session.current_tab() == tab { "nav-item on" } else { "nav-item" },
                                key: "{label}",
                                onclick: move |_| session.goto(tab),
                                Icon { glyph }
                                span { class: "nav-label", "{label}" }
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
    rsx! {
        // `data-drag`: in a frameless window, empty space here moves it.
        header { class: "topbar", "data-drag": "true",
            Brand {}
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
    if !session.is_initialized() {
        return rsx! {};
    }
    let unlocked = session.is_unlocked();
    let (glyph, label) = if unlocked {
        (Glyph::Unlock, "Lock vault")
    } else {
        (Glyph::Lock, "Unlock vault")
    };
    rsx! {
        button {
            class: if unlocked { "icon-btn sm lock-toggle unlocked" } else { "icon-btn sm lock-toggle" },
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
        }
    }
}
