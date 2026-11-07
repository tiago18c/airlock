//! In a browser: a Ledger over WebHID, a Trezor over WebUSB (through
//! `nusb`). A page sees only the devices the user chose for it, once each,
//! from a button: [`DeviceHub::request`].

use hw_protocol::link::{Connected, DeviceHub, DeviceKind, LinkError, LinkFuture, PacketLink};
use hw_protocol::{ledger, trezor};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::task::Waker;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{HidDevice, HidDeviceFilter, HidDeviceRequestOptions, HidInputReportEvent};

pub(crate) struct WebHub;

fn navigator() -> Option<web_sys::Navigator> {
    Some(web_sys::window()?.navigator())
}

/// `navigator.hid`, where the browser has it: Chromium on a secure page.
fn hid() -> Option<web_sys::Hid> {
    let navigator = navigator()?;
    js_sys::Reflect::has(&navigator, &JsValue::from_str("hid"))
        .unwrap_or(false)
        .then(|| navigator.hid())
}

fn has_usb() -> bool {
    navigator().is_some_and(|navigator| {
        js_sys::Reflect::has(&navigator, &JsValue::from_str("usb")).unwrap_or(false)
    })
}

/// Whether this browser can open a hardware wallet at all.
pub(crate) fn available() -> bool {
    hid().is_some() || has_usb()
}

fn js_error(error: JsValue) -> LinkError {
    let text = error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "the browser refused".into());
    if text.contains("NotAllowed") || text.to_ascii_lowercase().contains("permission") {
        LinkError::Denied(text)
    } else {
        LinkError::Io(text)
    }
}

/// Ledger's APDU interface, of the devices the page was given.
async fn ledgers() -> Result<Vec<HidDevice>, LinkError> {
    let Some(hid) = hid() else {
        return Ok(Vec::new());
    };
    let devices = JsFuture::from(hid.get_devices()).await.map_err(js_error)?;
    Ok(js_sys::Array::from(&devices)
        .iter()
        .filter_map(|device| device.dyn_into::<HidDevice>().ok())
        .filter(|device| device.vendor_id() == ledger::VENDOR_ID)
        .filter(|device| {
            device
                .collections()
                .iter()
                .any(|collection| collection.get_usage_page() == Some(ledger::USAGE_PAGE))
        })
        .collect())
}

fn ledger_id(at: usize, device: &HidDevice) -> String {
    format!("hid:{at}:{:04x}", device.product_id())
}

fn trezor_id(device: &nusb::DeviceInfo) -> String {
    format!("usb:{:?}", device.id())
}

impl DeviceHub for WebHub {
    fn list(&self) -> LinkFuture<'_, Result<Vec<Connected>, LinkError>> {
        Box::pin(async {
            let mut found: Vec<Connected> = ledgers()
                .await?
                .iter()
                .enumerate()
                .map(|(at, device)| Connected {
                    id: ledger_id(at, device),
                    kind: DeviceKind::Ledger,
                    model: ledger::model(device.product_id()).map(str::to_string),
                })
                .collect();
            if has_usb() {
                if let Ok(devices) = nusb::list_devices().await {
                    found.extend(
                        devices
                            .filter(|device| {
                                device.vendor_id() == trezor::VENDOR_ID
                                    && device.product_id() == trezor::PRODUCT_ID
                            })
                            .map(|device| Connected {
                                id: trezor_id(&device),
                                kind: DeviceKind::Trezor,
                                model: None,
                            }),
                    );
                }
            }
            Ok(found)
        })
    }

    fn open<'a>(&'a self, id: &'a str) -> LinkFuture<'a, Result<Box<dyn PacketLink>, LinkError>> {
        Box::pin(async move {
            if id.starts_with("usb:") {
                return open_trezor(id).await;
            }
            let device = ledgers()
                .await?
                .into_iter()
                .enumerate()
                .find(|(at, device)| ledger_id(*at, device) == id)
                .map(|(_, device)| device)
                .ok_or(LinkError::Disconnected)?;
            HidLink::open(device).await
        })
    }

    fn needs_request(&self) -> bool {
        true
    }

    fn request<'a>(&'a self, kind: DeviceKind) -> LinkFuture<'a, Result<bool, LinkError>> {
        Box::pin(async move {
            match kind {
                DeviceKind::Ledger => {
                    let Some(hid) = hid() else {
                        return Err(LinkError::Denied(
                            "this browser has no WebHID: use Chrome, Edge or Brave".into(),
                        ));
                    };
                    let filter = HidDeviceFilter::new();
                    filter.set_vendor_id(u32::from(ledger::VENDOR_ID));
                    filter.set_usage_page(ledger::USAGE_PAGE);
                    let options = HidDeviceRequestOptions::new(&[filter]);
                    let chosen = JsFuture::from(hid.request_device(&options))
                        .await
                        .map_err(js_error)?;
                    Ok(js_sys::Array::from(&chosen).length() > 0)
                }
                DeviceKind::Trezor => {
                    if !has_usb() {
                        return Err(LinkError::Denied(
                            "this browser has no WebUSB: use Chrome, Edge or Brave".into(),
                        ));
                    }
                    let selector = nusb::DeviceSelector::all()
                        .with_vid_pid(trezor::VENDOR_ID, trezor::PRODUCT_ID);
                    let chosen = nusb::request_device(&[selector])
                        .await
                        .map_err(|error| LinkError::Io(error.to_string()))?;
                    Ok(chosen.is_some())
                }
            }
        })
    }
}

/// Reports a Ledger sent, and who waits for the next one.
#[derive(Default)]
struct Inbox {
    reports: VecDeque<[u8; 64]>,
    waker: Option<Waker>,
}

struct HidLink {
    device: HidDevice,
    inbox: Rc<RefCell<Inbox>>,
    /// Kept for as long as the link, so the browser can call it.
    _on_report: Closure<dyn FnMut(HidInputReportEvent)>,
}

impl HidLink {
    async fn open(device: HidDevice) -> Result<Box<dyn PacketLink>, LinkError> {
        if !device.opened() {
            JsFuture::from(device.open()).await.map_err(js_error)?;
        }
        let inbox = Rc::new(RefCell::new(Inbox::default()));
        let filled = inbox.clone();
        let on_report =
            Closure::<dyn FnMut(HidInputReportEvent)>::new(move |event: HidInputReportEvent| {
                let data = event.data();
                let mut report = [0u8; 64];
                for (at, byte) in report.iter_mut().enumerate() {
                    if at < data.byte_length() {
                        *byte = data.get_uint8(at);
                    }
                }
                let mut inbox = filled.borrow_mut();
                inbox.reports.push_back(report);
                if let Some(waker) = inbox.waker.take() {
                    waker.wake();
                }
            });
        device.set_oninputreport(Some(on_report.as_ref().unchecked_ref()));
        Ok(Box::new(Self {
            device,
            inbox,
            _on_report: on_report,
        }))
    }
}

impl Drop for HidLink {
    fn drop(&mut self) {
        self.device.set_oninputreport(None);
        // The next job opens it again.
        let _ = self.device.close();
    }
}

impl PacketLink for HidLink {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>> {
        Box::pin(async move {
            let mut report = packet;
            let sent = self
                .device
                .send_report_with_u8_slice(0, &mut report)
                .map_err(js_error)?;
            JsFuture::from(sent).await.map_err(js_error)?;
            Ok(())
        })
    }

    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>> {
        let inbox = self.inbox.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            let mut inbox = inbox.borrow_mut();
            match inbox.reports.pop_front() {
                Some(report) => std::task::Poll::Ready(Ok(report)),
                None => {
                    inbox.waker = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            }
        }))
    }
}

async fn open_trezor(id: &str) -> Result<Box<dyn PacketLink>, LinkError> {
    use nusb::transfer::{In, Interrupt, Out};
    let usb_error = |error: nusb::Error| LinkError::Io(error.to_string());
    let info = nusb::list_devices()
        .await
        .map_err(usb_error)?
        .find(|device| trezor_id(device) == id)
        .ok_or(LinkError::Disconnected)?;
    let device = info.open().await.map_err(usb_error)?;
    let interface = device.claim_interface(0).await.map_err(usb_error)?;
    let out = interface
        .endpoint::<Interrupt, Out>(0x01)
        .map_err(usb_error)?;
    let input = interface
        .endpoint::<Interrupt, In>(0x81)
        .map_err(usb_error)?;
    Ok(Box::new(UsbLink { out, input }))
}

struct UsbLink {
    out: nusb::Endpoint<nusb::transfer::Interrupt, nusb::transfer::Out>,
    input: nusb::Endpoint<nusb::transfer::Interrupt, nusb::transfer::In>,
}

impl PacketLink for UsbLink {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>> {
        Box::pin(async move {
            self.out.submit(packet.to_vec().into());
            self.out
                .next_complete()
                .await
                .status
                .map_err(|error| LinkError::Io(error.to_string()))
        })
    }

    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>> {
        Box::pin(async move {
            let buffer = self.input.allocate(trezor::PACKET);
            self.input.submit(buffer);
            let done = self.input.next_complete().await;
            done.status
                .map_err(|error| LinkError::Io(error.to_string()))?;
            let bytes = done.buffer.into_vec();
            bytes
                .get(..trezor::PACKET)
                .and_then(|packet| <[u8; 64]>::try_from(packet).ok())
                .ok_or_else(|| LinkError::Io(format!("a packet of {} bytes", bytes.len())))
        })
    }
}
