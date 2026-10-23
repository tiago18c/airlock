//! The wallets screen: the list, adding a wallet of each kind, and the
//! switcher in the top bar. Over [`app_ui_kit`], with no other screen.

mod ui;

use app_ui_kit::{actions, state};

pub use ui::wallets::kinds::{find_id, WalletSwitcher};
pub use ui::wallets::WalletsPanel;
