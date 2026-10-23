//! Shared UI for the online and airgap shells: the shell, notices, toasts
//! and settings, over [`app_ui_kit`] and the screen crates, which it
//! re-exports under the same paths. No `wallet-rpc` or bridge host.

pub mod ui;

pub use app_ui_kit::{actions, diagnostics, platform, rt, state};
pub use app_ui_kit::{
    open_core, open_core_with, use_core, use_forms, use_network, use_session, Forms, Network,
    OnlineHooks, ProposalsOpen, Pump, Session, SubmitFut, SubmitHandle, Tab, TxSource, TxSubmit,
    WalletSummary, MAIN_CSS, TOKENS_CSS,
};
pub use ui::{
    find_id, format_sol, format_token_amount, use_theme, Address, AddressBookPanel, AddressHeader,
    AddressPicker, AddressRow, AppBrand, AppShell, BuildPanel, Glyph, Icon, LockScreen, NoticeBell,
    PhraseView, ProgramsPanel, SettingsPanel, Sidebar, SignPanel, ThemeToggle, ToastHost, TopBar,
    VaultUnlock, WalletPicker, WalletsPanel,
};
