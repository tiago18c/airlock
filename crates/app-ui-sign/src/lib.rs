//! The sign screen, and the QR transfer it sends and scans with. Over
//! [`app_ui_kit`], with no other screen.

mod ui;

use app_ui_kit::{actions, state};

pub use ui::sign::SignPanel;
