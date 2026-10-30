//! What app-ui's screens and the app's other crates share: the session state
//! and the pump, the actions, platform glue, the CSS, and the widgets in
//! [`ui`]. No screen, no `wallet-rpc` and no bridge host.

pub mod actions;
pub mod diagnostics;
pub mod platform;
pub mod rt;
pub mod state;
pub mod ui;

pub use state::forms::{use_forms, Forms, TxSource};
pub use state::online::{use_network, Network};
pub use state::proposals::ProposalsOpen;
pub use state::pump::{use_core, Pump};
pub use state::session::{OnlineHooks, Session, Tab, WalletSummary};
pub use state::submit::{SiteSendFut, SubmitFut, SubmitHandle, TxSubmit};
pub use state::theme::use_theme;
pub use state::{open_core, open_core_with, use_session};
pub use ui::{
    format_sol, format_token_amount, Address, AddressHeader, AddressPicker, AddressRow, Glyph,
    Icon, LockScreen, PhraseView, ThemeToggle, VaultUnlock, WalletPicker,
};

use dioxus::prelude::*;

/// CSS shipped with this library (`asset!` from a lib is collected into
/// whatever package `dx` builds).
pub const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
pub const MAIN_CSS: Asset = asset!("/assets/main.css");
