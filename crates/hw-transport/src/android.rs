//! A Ledger or a Trezor that Android handed over. An app cannot list the
//! phone's USB devices itself: Android's `UsbManager` lists them, asks the
//! user to let the app use one, and opens it. What it gives back is the
//! file descriptor of the open device, which `nusb` takes from there.
//!
//! A Ledger is a HID device, but Android binds no HID driver an app can use,
//! so its APDU interface is read here as plain USB: the first HID interface
//! with an interrupt endpoint each way, which is the APDU one (FIDO comes
//! after it). The packets are the same 64 bytes as over HID, without the
//! report id. A Trezor is opened as on desktop.

use std::os::fd::OwnedFd;

use hw_protocol::link::{DeviceKind, LinkError, PacketLink};

use crate::usb::{self, apdu_interface, InterfaceSeen};

/// Open the device behind `fd`, a `kind` that Android already let the app
/// use.
pub async fn open_fd(kind: DeviceKind, fd: OwnedFd) -> Result<Box<dyn PacketLink>, LinkError> {
    let device = nusb::Device::from_fd(fd).await.map_err(usb::link_error)?;
    let (interface, out, input) = match kind {
        DeviceKind::Trezor => (usb::INTERFACE, usb::ENDPOINT_OUT, usb::ENDPOINT_IN),
        DeviceKind::Ledger => ledger_endpoints(&device)?,
    };
    usb::claim(&device, interface, out, input).await
}

/// The Ledger's APDU interface and its OUT and IN endpoints.
fn ledger_endpoints(device: &nusb::Device) -> Result<(u8, u8, u8), LinkError> {
    let config = device
        .active_configuration()
        .map_err(|error| LinkError::Io(error.to_string()))?;
    let interfaces: Vec<InterfaceSeen> = config
        .interfaces()
        .flat_map(|interface| {
            let number = interface.interface_number();
            interface
                .alt_settings()
                .map(|alt| {
                    let endpoints = alt
                        .endpoints()
                        .map(|endpoint| (endpoint.transfer_type(), endpoint.address()))
                        .collect();
                    (number, alt.class(), endpoints)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    apdu_interface(&interfaces)
        .ok_or_else(|| LinkError::Io("the Ledger shows no HID interface to send APDUs to".into()))
}
