mod notices;
mod settings;
mod shell;
mod toasts;
pub use app_ui_kit::ui::*;
pub use app_ui_kit::ui::{address, clock, local_time, lock, tx_view};

pub use app_ui_book::{AddressBookPanel, ProgramsPanel};
pub use app_ui_build::BuildPanel;
pub use app_ui_kit::use_theme;
pub use app_ui_sign::SignPanel;
pub use app_ui_wallets::{find_id, WalletSwitcher, WalletsPanel};
pub use notices::NoticeBell;
pub use settings::SettingsPanel;
pub use shell::{AppBrand, AppShell, Sidebar, TopBar};
pub use toasts::ToastHost;

use app_core::TxStatus;

/// Chip class, title-case label, and whether the chip spins (in flight).
pub fn tx_status_chip(status: &TxStatus) -> (&'static str, &'static str, bool) {
    match status {
        TxStatus::Sent => ("chip info", status.label(), true),
        TxStatus::Confirmed { .. } => ("chip ok", status.label(), false),
        TxStatus::Failed { .. } | TxStatus::Rejected { .. } => {
            ("chip danger", status.label(), false)
        }
        TxStatus::Expired | TxStatus::Unknown | TxStatus::NotSent => {
            ("chip warn", status.label(), false)
        }
    }
}
