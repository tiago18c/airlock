//! What a transport gives the use cases: a connection to one device that
//! moves packets. `hw-transport` implements it over HID and USB, the tests
//! over a scripted fake. The framing is in [`crate::ledger`] and
//! [`crate::trezor`], so a link only moves 64-byte reports.

use std::future::Future;
use std::pin::Pin;

/// A boxed future that is `Send` on native, where the core's spawner needs
/// it, and not in a browser, where WebHID's callbacks hold `Rc`s.
#[cfg(not(target_arch = "wasm32"))]
pub type LinkFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type LinkFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// What can go wrong on the link itself, below what the device answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The device is gone: unplugged, or asleep.
    Disconnected,
    /// Another program holds it: Ledger Live, Trezor Suite.
    Busy,
    /// The OS refused to open it. On Linux, the udev rules are missing.
    Denied(String),
    Io(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "The device was disconnected."),
            Self::Busy => write!(f, "Another app is using the device. Close Ledger Live or Trezor Suite, then try again."),
            Self::Denied(why) => write!(f, "The device cannot be opened: {why}."),
            Self::Io(why) => write!(f, "The device stopped answering: {why}."),
        }
    }
}

impl std::error::Error for LinkError {}

/// One open device that sends and receives 64-byte reports.
#[cfg(not(target_arch = "wasm32"))]
pub trait PacketLink: Send {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>>;
    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>>;
}

#[cfg(target_arch = "wasm32")]
pub trait PacketLink {
    fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>>;
    fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>>;
}

/// Send one Ledger APDU and read its reply, status word included.
pub async fn exchange_apdu(
    link: &mut dyn PacketLink,
    apdu: &[u8],
) -> Result<Vec<u8>, ExchangeError> {
    for packet in crate::ledger::frame(apdu) {
        link.write(packet).await?;
    }
    let mut unframer = crate::ledger::Unframer::default();
    loop {
        let packet = link.read().await?;
        if let Some(reply) = unframer.push(&packet)? {
            return Ok(reply);
        }
    }
}

/// Send one Trezor message and read the next one back.
pub async fn exchange_message(
    link: &mut dyn PacketLink,
    message: &crate::trezor::Message,
) -> Result<crate::trezor::Message, ExchangeError> {
    for packet in crate::trezor::frame(message) {
        link.write(packet).await?;
    }
    let mut unframer = crate::trezor::Unframer::default();
    loop {
        let packet = link.read().await?;
        if let Some(reply) = unframer.push(&packet)? {
            return Ok(reply);
        }
    }
}

/// A link failed, or what came back was not framed as it should be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    Link(LinkError),
    Ledger(crate::ledger::LedgerError),
    Trezor(crate::trezor::TrezorError),
}

impl From<LinkError> for ExchangeError {
    fn from(error: LinkError) -> Self {
        Self::Link(error)
    }
}

impl From<crate::ledger::LedgerError> for ExchangeError {
    fn from(error: crate::ledger::LedgerError) -> Self {
        Self::Ledger(error)
    }
}

impl From<crate::trezor::TrezorError> for ExchangeError {
    fn from(error: crate::trezor::TrezorError) -> Self {
        Self::Trezor(error)
    }
}

impl std::fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Link(error) => error.fmt(f),
            Self::Ledger(error) => error.fmt(f),
            Self::Trezor(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ExchangeError {}

/// A device that answers from a script, for tests and screenshots. Each
/// expected request is matched whole, after it is unframed, and the scripted
/// reply is framed back.
#[cfg(any(test, feature = "fake"))]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;

    /// What the fake answers to one Ledger APDU.
    pub struct Step {
        /// The APDU it expects, or `None` for any.
        pub expect: Option<Vec<u8>>,
        /// Data and status word, or a link error.
        pub reply: Result<Vec<u8>, LinkError>,
    }

    /// A Ledger that plays `steps` in order, then disconnects.
    pub struct FakeLedger {
        steps: VecDeque<Step>,
        pending: Vec<u8>,
        expected_len: Option<usize>,
        outgoing: VecDeque<[u8; 64]>,
        /// Every APDU it was sent, in order.
        pub received: Vec<Vec<u8>>,
    }

    impl FakeLedger {
        pub fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: steps.into(),
                pending: Vec::new(),
                expected_len: None,
                outgoing: VecDeque::new(),
                received: Vec::new(),
            }
        }
    }

    impl PacketLink for FakeLedger {
        fn write(&mut self, packet: [u8; 64]) -> LinkFuture<'_, Result<(), LinkError>> {
            Box::pin(async move {
                let body = if self.expected_len.is_none() {
                    self.expected_len =
                        Some(usize::from(u16::from_be_bytes([packet[5], packet[6]])));
                    &packet[7..]
                } else {
                    &packet[5..]
                };
                let want = self.expected_len.unwrap_or(0);
                let take = (want - self.pending.len()).min(body.len());
                self.pending.extend_from_slice(&body[..take]);
                if self.pending.len() < want {
                    return Ok(());
                }
                self.expected_len = None;
                let apdu = std::mem::take(&mut self.pending);
                self.received.push(apdu.clone());
                let step = self.steps.pop_front().ok_or(LinkError::Disconnected)?;
                if let Some(expect) = step.expect {
                    assert_eq!(apdu, expect, "the fake Ledger got another APDU");
                }
                let reply = step.reply?;
                self.outgoing.extend(crate::ledger::frame(&reply));
                Ok(())
            })
        }

        fn read(&mut self) -> LinkFuture<'_, Result<[u8; 64], LinkError>> {
            Box::pin(async move { self.outgoing.pop_front().ok_or(LinkError::Disconnected) })
        }
    }
}

/// Which kind of device a connected one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Ledger,
    Trezor,
}

/// A device seen on the system, before it is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connected {
    /// What [`DeviceHub::open`] takes. Stable while the device stays plugged in.
    pub id: String,
    pub kind: DeviceKind,
    /// The model, when the USB ids tell it.
    pub model: Option<String>,
}

/// What the platform gives the core: the devices it sees, and a way to open
/// one. `hw-transport` implements it; the core and the screens only see this.
pub trait DeviceHub: Send + Sync {
    fn list(&self) -> LinkFuture<'_, Result<Vec<Connected>, LinkError>>;
    fn open<'a>(&'a self, id: &'a str) -> LinkFuture<'a, Result<Box<dyn PacketLink>, LinkError>>;

    /// A page sees a device only once the user chose it for the page, from
    /// a click: [`Self::request`]. A desktop sees every device.
    fn needs_request(&self) -> bool {
        false
    }

    /// Ask the user to choose a device of `kind` for this page. Call it from
    /// a click. Answers whether one was chosen.
    fn request<'a>(&'a self, kind: DeviceKind) -> LinkFuture<'a, Result<bool, LinkError>> {
        let _ = kind;
        Box::pin(async { Ok(true) })
    }

    /// Whether it opens Ledgers and Trezors over USB. A phone's hub opens
    /// only its Seed Vault.
    fn opens_cables(&self) -> bool {
        true
    }

    /// The phone's Seed Vault, on a Solana Mobile phone. None elsewhere.
    fn seed_vault(&self) -> Option<&dyn crate::seed_vault::SeedVault> {
        None
    }
}
