//! Bridge request modal. Approval stays here; the socket is in [`crate::bridge`].

mod bridge;
mod extension_chip;

pub use bridge::{use_request_modal, BridgeRequests, RequestModal, RequestToast};
pub use extension_chip::ExtensionChip;
