//! The address book and programs screens. Over [`app_ui_kit`], with no
//! other screen.

mod ui;

use app_ui_kit::{actions, state};

pub use ui::address_book::AddressBookPanel;
pub use ui::programs::ProgramsPanel;
