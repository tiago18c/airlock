//! The only crate that opens a hardware wallet. Each shell hands its
//! [`hub`] to the core through `Ports`; the core and the screens see only
//! `hw_protocol::link::DeviceHub`.
//!
//! On desktop a Ledger is a HID device, through `async-hid`: pure Rust over
//! each OS's own HID API, so nothing is built from C and no driver is
//! installed. A Trezor is a USB device, through `nusb`, also pure Rust.

use hw_protocol::link::DeviceHub;
#[cfg(feature = "fake")]
use hw_protocol::link::{Connected, LinkError, LinkFuture, PacketLink};

#[cfg(not(target_arch = "wasm32"))]
mod hid;
#[cfg(not(target_arch = "wasm32"))]
mod usb;
#[cfg(target_arch = "wasm32")]
mod web;

/// The hub this build opens devices with. In a browser, WebHID for Ledger
/// and WebUSB for Trezor, where the browser has them (Chromium, on a secure
/// page); none elsewhere.
pub fn hub() -> Option<std::sync::Arc<dyn DeviceHub>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(std::sync::Arc::new(hid::NativeHub::new()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        web::available().then(|| std::sync::Arc::new(web::WebHub) as std::sync::Arc<dyn DeviceHub>)
    }
}

/// A hub that sees one scripted device, for tests and screenshots.
#[cfg(feature = "fake")]
pub struct FakeHub {
    make: Box<dyn Fn() -> Box<dyn PacketLink> + Send + Sync>,
    device: Connected,
}

#[cfg(feature = "fake")]
impl FakeHub {
    pub fn new(
        device: Connected,
        make: impl Fn() -> Box<dyn PacketLink> + Send + Sync + 'static,
    ) -> Self {
        Self {
            make: Box::new(make),
            device,
        }
    }
}

#[cfg(feature = "fake")]
impl DeviceHub for FakeHub {
    fn list(&self) -> LinkFuture<'_, Result<Vec<Connected>, LinkError>> {
        let device = self.device.clone();
        Box::pin(async move { Ok(vec![device]) })
    }

    fn open<'a>(&'a self, id: &'a str) -> LinkFuture<'a, Result<Box<dyn PacketLink>, LinkError>> {
        Box::pin(async move {
            if id == self.device.id {
                Ok((self.make)())
            } else {
                Err(LinkError::Disconnected)
            }
        })
    }
}
