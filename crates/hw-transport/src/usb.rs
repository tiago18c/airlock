//! Trezor over USB, through `nusb`: interface 0 and its two interrupt
//! endpoints, 64 bytes each way. Windows binds WinUSB from the device's own
//! descriptors, so no driver is installed.

use hw_protocol::link::{Connected, DeviceKind, LinkError, LinkFuture, PacketLink};
use hw_protocol::trezor;
use nusb::transfer::{In, Interrupt, Out};
use nusb::ErrorKind;

const INTERFACE: u8 = 0;
const ENDPOINT_OUT: u8 = 0x01;
const ENDPOINT_IN: u8 = 0x81;

fn link_error(error: nusb::Error) -> LinkError {
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
fn is_trezor(device: &nusb::DeviceInfo) -> bool {
    device.vendor_id() == trezor::VENDOR_ID && device.product_id() == trezor::PRODUCT_ID
}

fn id_text(device: &nusb::DeviceInfo) -> String {
    format!("usb:{:?}", device.id())
}

/// The Trezors this machine sees. The model comes from the device's own
/// Features once it is opened.
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

pub(crate) async fn open(id: &str) -> Result<Box<dyn PacketLink>, LinkError> {
    let info = nusb::list_devices()
        .await
        .map_err(link_error)?
        .find(|device| is_trezor(device) && id_text(device) == id)
        .ok_or(LinkError::Disconnected)?;
    let device = info.open().await.map_err(link_error)?;
    // Trezor Suite, or its bridge, holds the interface while it runs.
    let interface = device
        .detach_and_claim_interface(INTERFACE)
        .await
        .map_err(link_error)?;
    let out = interface
        .endpoint::<Interrupt, Out>(ENDPOINT_OUT)
        .map_err(link_error)?;
    let input = interface
        .endpoint::<Interrupt, In>(ENDPOINT_IN)
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
