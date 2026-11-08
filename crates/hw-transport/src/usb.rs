//! Trezor over USB, through `nusb`: interface 0 and its two interrupt
//! endpoints, 64 bytes each way. Windows binds WinUSB from the device's own
//! descriptors, so no driver is installed.

#[cfg(not(target_os = "android"))]
use hw_protocol::link::{Connected, DeviceKind};
use hw_protocol::link::{LinkError, LinkFuture, PacketLink};
use hw_protocol::trezor;
use nusb::transfer::{In, Interrupt, Out};
use nusb::ErrorKind;

pub(crate) const INTERFACE: u8 = 0;
pub(crate) const ENDPOINT_OUT: u8 = 0x01;
pub(crate) const ENDPOINT_IN: u8 = 0x81;

pub(crate) fn link_error(error: nusb::Error) -> LinkError {
    match error.kind() {
        ErrorKind::Disconnected | ErrorKind::NotFound => LinkError::Disconnected,
        ErrorKind::Busy => LinkError::Busy,
        ErrorKind::PermissionDenied => LinkError::Denied(error.to_string()),
        _ => LinkError::Io(error.to_string()),
    }
}

fn transfer_error(error: nusb::transfer::TransferError) -> LinkError {
    match error {
        nusb::transfer::TransferError::Disconnected => LinkError::Disconnected,
        other => LinkError::Io(other.to_string()),
    }
}

/// A Trezor running its firmware, not its bootloader.
#[cfg(not(target_os = "android"))]
fn is_trezor(device: &nusb::DeviceInfo) -> bool {
    device.vendor_id() == trezor::VENDOR_ID && device.product_id() == trezor::PRODUCT_ID
}

#[cfg(not(target_os = "android"))]
fn id_text(device: &nusb::DeviceInfo) -> String {
    format!("usb:{:?}", device.id())
}

/// The Trezors this machine sees. The model comes from the device's own
/// Features once it is opened.
#[cfg(not(target_os = "android"))]
pub(crate) async fn list() -> Result<Vec<Connected>, LinkError> {
    let devices = nusb::list_devices().await.map_err(link_error)?;
    Ok(devices
        .filter(is_trezor)
        .map(|device| Connected {
            id: id_text(&device),
            kind: DeviceKind::Trezor,
            model: None,
        })
        .collect())
}

#[cfg(not(target_os = "android"))]
pub(crate) async fn open(id: &str) -> Result<Box<dyn PacketLink>, LinkError> {
    let info = nusb::list_devices()
        .await
        .map_err(link_error)?
        .find(|device| is_trezor(device) && id_text(device) == id)
        .ok_or(LinkError::Disconnected)?;
    let device = info.open().await.map_err(link_error)?;
    claim(&device, INTERFACE, ENDPOINT_OUT, ENDPOINT_IN).await
}

/// Claim `interface` of an open device, and talk to it in 64-byte packets
/// over its interrupt endpoints `out` and `input`.
pub(crate) async fn claim(
    device: &nusb::Device,
    interface: u8,
    out: u8,
    input: u8,
) -> Result<Box<dyn PacketLink>, LinkError> {
    // Trezor Suite, or its bridge, holds the interface while it runs. On
    // Android the kernel's HID driver holds a Ledger's.
    let interface = device
        .detach_and_claim_interface(interface)
        .await
        .map_err(link_error)?;
    let out = interface
        .endpoint::<Interrupt, Out>(out)
        .map_err(link_error)?;
    let input = interface
        .endpoint::<Interrupt, In>(input)
        .map_err(link_error)?;
    Ok(Box::new(UsbLink { out, input }))
}

struct UsbLink {
    out: nusb::Endpoint<Interrupt, Out>,
    input: nusb::Endpoint<Interrupt, In>,
}

impl PacketLink for UsbLink {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>> {
        Box::pin(async move {
            self.out.submit(packet.to_vec().into());
            self.out
                .next_complete()
                .await
                .status
                .map_err(transfer_error)
        })
    }

    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>> {
        Box::pin(async move {
            let buffer = self.input.allocate(trezor::PACKET);
            self.input.submit(buffer);
            let done = self.input.next_complete().await;
            done.status.map_err(transfer_error)?;
            let bytes = done.buffer.into_vec();
            bytes
                .get(..trezor::PACKET)
                .and_then(|packet| <[u8; 64]>::try_from(packet).ok())
                .ok_or_else(|| LinkError::Io(format!("a packet of {} bytes", bytes.len())))
        })
    }
}

/// The HID interface class.
const HID: u8 = 0x03;

/// An interface's number, class, and endpoints by transfer type and address.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) type InterfaceSeen = (u8, u8, Vec<(nusb::descriptors::TransferType, u8)>);

/// Of `interfaces` (number, class, endpoints), the first HID one with an
/// interrupt endpoint each way: its number, OUT and IN addresses.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn apdu_interface(interfaces: &[InterfaceSeen]) -> Option<(u8, u8, u8)> {
    interfaces.iter().find_map(|(number, class, endpoints)| {
        if *class != HID {
            return None;
        }
        let interrupt = |inward: bool| {
            endpoints.iter().find_map(|(kind, address)| {
                (*kind == nusb::descriptors::TransferType::Interrupt
                    && (address & 0x80 != 0) == inward)
                    .then_some(*address)
            })
        };
        Some((*number, interrupt(false)?, interrupt(true)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nusb::descriptors::TransferType;

    #[test]
    fn the_first_hid_interface_with_both_endpoints_carries_apdus() {
        let interrupt = TransferType::Interrupt;
        // A Nano S Plus: APDUs on 0, then FIDO on 1, then a WebUSB one.
        let nano = vec![
            (0, HID, vec![(interrupt, 0x82), (interrupt, 0x02)]),
            (1, HID, vec![(interrupt, 0x81), (interrupt, 0x01)]),
            (2, 0xff, vec![(interrupt, 0x83), (interrupt, 0x03)]),
        ];
        assert_eq!(apdu_interface(&nano), Some((0, 0x02, 0x82)));
        // Something before it that is not HID, or lacks an endpoint, is
        // passed over.
        let other = vec![
            (0, 0xff, vec![(interrupt, 0x81), (interrupt, 0x01)]),
            (1, HID, vec![(interrupt, 0x83)]),
            (
                2,
                HID,
                vec![
                    (TransferType::Bulk, 0x84),
                    (interrupt, 0x84),
                    (interrupt, 0x04),
                ],
            ),
        ];
        assert_eq!(apdu_interface(&other), Some((2, 0x04, 0x84)));
        assert_eq!(apdu_interface(&[]), None);
    }
}
