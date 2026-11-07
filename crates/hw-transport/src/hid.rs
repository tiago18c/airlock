//! Ledger over HID on Windows, macOS and Linux, and the hub that also
//! lists the Trezors `usb` opens.

use async_hid::{AsyncHidRead, AsyncHidWrite, DeviceReaderWriter, HidBackend, HidError};
use hw_protocol::ledger;
use hw_protocol::link::{Connected, DeviceHub, DeviceKind, LinkError, LinkFuture, PacketLink};

pub(crate) struct NativeHub {
    backend: HidBackend,
}

impl NativeHub {
    pub(crate) fn new() -> Self {
        Self {
            backend: HidBackend::default(),
        }
    }
}

fn link_error(error: HidError) -> LinkError {
    match error {
        HidError::Disconnected | HidError::NotConnected => LinkError::Disconnected,
        other => {
            let text = other.to_string();
            // Windows answers "access denied" while another app holds the
            // interface exclusively.
            if text.to_ascii_lowercase().contains("denied") || text.contains("0x80070005") {
                LinkError::Busy
            } else {
                LinkError::Io(text)
            }
        }
    }
}

/// Ledger's APDU interface, not its FIDO or keyboard ones.
fn is_ledger_apdu(device: &async_hid::DeviceInfo) -> bool {
    device.vendor_id == ledger::VENDOR_ID && device.usage_page == ledger::USAGE_PAGE
}

fn id_text(device: &async_hid::DeviceInfo) -> String {
    format!("hid:{:?}", device.id)
}

impl DeviceHub for NativeHub {
    fn list(&self) -> LinkFuture<'_, Result<Vec<Connected>, LinkError>> {
        Box::pin(async move {
            use futures_lite::StreamExt;
            let devices: Vec<async_hid::Device> = self
                .backend
                .enumerate()
                .await
                .map_err(link_error)?
                .collect()
                .await;
            let mut found: Vec<Connected> = devices
                .iter()
                .filter(|device| is_ledger_apdu(device))
                .map(|device| Connected {
                    id: id_text(device),
                    kind: DeviceKind::Ledger,
                    model: ledger::model(device.product_id).map(str::to_string),
                })
                .collect();
            // A machine whose USB stack cannot be listed still has its Ledgers.
            if let Ok(trezors) = crate::usb::list().await {
                found.extend(trezors);
            }
            Ok(found)
        })
    }

    fn open<'a>(&'a self, id: &'a str) -> LinkFuture<'a, Result<Box<dyn PacketLink>, LinkError>> {
        Box::pin(async move {
            use futures_lite::StreamExt;
            if id.starts_with("usb:") {
                return crate::usb::open(id).await;
            }
            let device = self
                .backend
                .enumerate()
                .await
                .map_err(link_error)?
                .find(|device| is_ledger_apdu(device) && id_text(device) == id)
                .await
                .ok_or(LinkError::Disconnected)?;
            let handle = device.open().await.map_err(link_error)?;
            Ok(Box::new(HidLink { handle }) as Box<dyn PacketLink>)
        })
    }
}

struct HidLink {
    handle: DeviceReaderWriter,
}

impl PacketLink for HidLink {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>> {
        Box::pin(async move {
            // The first byte is the report id: Ledger uses none.
            let mut report = [0u8; 65];
            report[1..].copy_from_slice(&packet);
            self.handle
                .write_output_report(&report)
                .await
                .map_err(link_error)
        })
    }

    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>> {
        Box::pin(async move {
            let mut buffer = [0u8; 65];
            let len = self
                .handle
                .read_input_report(&mut buffer)
                .await
                .map_err(link_error)?;
            // Some backends hand the report back with its id, some without.
            let report = match len {
                65 => &buffer[1..],
                64 => &buffer[..64],
                _ => return Err(LinkError::Io(format!("a report of {len} bytes"))),
            };
            Ok(report.try_into().expect("64 bytes"))
        })
    }
}
