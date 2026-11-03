//! Ledger: the Solana app's APDUs and the dashboard's, the HID framing that
//! carries them, and the status words that come back. See `doc/api.md` in
//! `LedgerHQ/app-solana`.

/// Ledger's USB vendor id.
pub const VENDOR_ID: u16 = 0x2c97;
/// The HID usage page of the interface APDUs go to (not the FIDO one).
pub const USAGE_PAGE: u16 = 0xffa0;
/// Every HID report is this long, without the report id hidapi adds.
pub const PACKET: usize = 64;
const CHANNEL: u16 = 0x0101;
const TAG_APDU: u8 = 0x05;

const CLA: u8 = 0xe0;
const INS_GET_CONFIGURATION: u8 = 0x04;
const INS_GET_PUBKEY: u8 = 0x05;
const INS_SIGN_MESSAGE: u8 = 0x06;
const INS_SIGN_OFFCHAIN_MESSAGE: u8 = 0x07;
/// A chunk carries at most this many bytes.
pub const MAX_CHUNK: usize = 255;
const P2_EXTEND: u8 = 0x01;
const P2_MORE: u8 = 0x02;

/// The model, from the high byte of the product id outside the bootloader.
pub fn model(product_id: u16) -> Option<&'static str> {
    Some(match product_id >> 8 {
        0x40 => "Nano X",
        0x50 => "Nano S Plus",
        0x60 => "Stax",
        0x70 => "Flex",
        0x80 => "Nano Gen5",
        _ => return None,
    })
}

/// A command, before it is framed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apdu {
    pub cla: u8,
    pub ins: u8,
    pub p1: u8,
    pub p2: u8,
    pub data: Vec<u8>,
}

impl Apdu {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.cla, self.ins, self.p1, self.p2, self.data.len() as u8];
        out.extend_from_slice(&self.data);
        out
    }
}

/// The name and version of the app that is open: `BOLOS` on the dashboard.
pub fn get_app_and_version() -> Apdu {
    Apdu {
        cla: 0xb0,
        ins: 0x01,
        p1: 0,
        p2: 0,
        data: Vec::new(),
    }
}

/// Open the Solana app. The device asks the user first.
pub fn open_app() -> Apdu {
    Apdu {
        cla: 0xe0,
        ins: 0xd8,
        p1: 0,
        p2: 0,
        data: b"Solana".to_vec(),
    }
}

/// Quit the app that is open, back to the dashboard.
pub fn quit_app() -> Apdu {
    Apdu {
        cla: 0xb0,
        ins: 0xa7,
        p1: 0,
        p2: 0,
        data: Vec::new(),
    }
}

/// The open app, as `B0 01` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAndVersion {
    pub name: String,
    pub version: String,
}

pub fn parse_app_and_version(data: &[u8]) -> Result<AppAndVersion, LedgerError> {
    let bad = || LedgerError::BadReply;
    // A format byte, then the name and the version, each after its length.
    let (&format, rest) = data.split_first().ok_or_else(bad)?;
    if format != 1 {
        return Err(bad());
    }
    let read = |rest: &mut &[u8]| -> Result<String, LedgerError> {
        let (&len, tail) = rest.split_first().ok_or_else(bad)?;
        let text = tail.get(..usize::from(len)).ok_or_else(bad)?;
        *rest = &tail[usize::from(len)..];
        String::from_utf8(text.to_vec()).map_err(|_| bad())
    };
    let mut rest = rest;
    let name = read(&mut rest)?;
    let version = read(&mut rest)?;
    Ok(AppAndVersion { name, version })
}

pub fn get_configuration() -> Apdu {
    Apdu {
        cla: CLA,
        ins: INS_GET_CONFIGURATION,
        p1: 0,
        p2: 0,
        data: Vec::new(),
    }
}

/// The Solana app's settings and version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppConfiguration {
    /// Blind signing is on in the app's settings.
    pub blind_signing: bool,
    pub version: (u8, u8, u8),
}

pub fn parse_configuration(data: &[u8]) -> Result<AppConfiguration, LedgerError> {
    let [blind, _pubkey_display, major, minor, patch, ..] = data else {
        return Err(LedgerError::BadReply);
    };
    Ok(AppConfiguration {
        blind_signing: *blind != 0,
        version: (*major, *minor, *patch),
    })
}

fn path_bytes(path: &[u32]) -> Vec<u8> {
    let mut out = vec![path.len() as u8];
    for index in path {
        out.extend_from_slice(&index.to_be_bytes());
    }
    out
}

/// The key at `path`. With `confirm`, the device shows the address and
/// the user approves it there.
pub fn get_pubkey(path: &[u32], confirm: bool) -> Apdu {
    Apdu {
        cla: CLA,
        ins: INS_GET_PUBKEY,
        p1: u8::from(confirm),
        p2: 0,
        data: path_bytes(path),
    }
}

pub fn parse_pubkey(data: &[u8]) -> Result<[u8; 32], LedgerError> {
    data.try_into().map_err(|_| LedgerError::BadReply)
}

fn chunked(ins: u8, path: &[u32], payload: &[u8]) -> Vec<Apdu> {
    // One signer, then its path, then the bytes to sign.
    let mut data = vec![1];
    data.extend_from_slice(&path_bytes(path));
    data.extend_from_slice(payload);
    let chunks: Vec<&[u8]> = data.chunks(MAX_CHUNK).collect();
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut p2 = 0;
            if index > 0 {
                p2 |= P2_EXTEND;
            }
            if index < last {
                p2 |= P2_MORE;
            }
            Apdu {
                cla: CLA,
                ins,
                p1: 1,
                p2,
                data: chunk.to_vec(),
            }
        })
        .collect()
}

/// Sign `message`, the bytes a transaction's signatures cover, with the key
/// at `path`. The device answers the last chunk with the signature.
pub fn sign_message(path: &[u32], message: &[u8]) -> Vec<Apdu> {
    chunked(INS_SIGN_MESSAGE, path, message)
}

/// Sign an off-chain message, already in the OCMS format.
pub fn sign_offchain_message(path: &[u32], ocms: &[u8]) -> Vec<Apdu> {
    chunked(INS_SIGN_OFFCHAIN_MESSAGE, path, ocms)
}

pub fn parse_signature(data: &[u8]) -> Result<[u8; 64], LedgerError> {
    data.try_into().map_err(|_| LedgerError::BadReply)
}

/// What a status word other than `9000` means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// The user rejected it on the device.
    Rejected,
    /// Blind signing is off, or the app will not do this.
    NotSupported,
    /// The app does not read this message: a v1 one before app 1.18.
    InvalidMessage,
    /// A bad header, or the key is not one of the message's signers.
    BadHeader,
    BadOffchainMessage,
    TooLong,
    BadPath,
    /// The Solana app is not the one open.
    WrongApp,
    /// No app called Solana is installed.
    AppNotInstalled,
    /// The user refused to open the app.
    OpenRefused,
    Locked,
    NoPin,
    /// A status word this list does not name.
    Status(u16),
    /// A reply that does not have the shape it should.
    BadReply,
}

impl LedgerError {
    pub fn from_status(status: u16) -> Self {
        match status {
            0x6985 => Self::Rejected,
            0x6808 => Self::NotSupported,
            0x6a80 => Self::InvalidMessage,
            0x6a81 => Self::BadHeader,
            0x6a82 => Self::BadOffchainMessage,
            0x6a83 => Self::TooLong,
            0x6a84 => Self::BadPath,
            0x6d00 | 0x6e00 | 0x6e01 => Self::WrongApp,
            0x6807 => Self::AppNotInstalled,
            0x5501 => Self::OpenRefused,
            0x5515 => Self::Locked,
            0x5502 => Self::NoPin,
            other => Self::Status(other),
        }
    }
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected => write!(f, "You rejected it on the Ledger."),
            Self::NotSupported => write!(f, "The Solana app will not sign this. Turn on Blind signing in its settings, then try again."),
            Self::InvalidMessage => write!(f, "The Solana app cannot read this transaction. Update it to 1.18 or later."),
            Self::BadHeader => write!(f, "This Ledger's key is not one this transaction asks to sign."),
            Self::BadOffchainMessage => write!(f, "The Solana app cannot read this message."),
            Self::TooLong => write!(f, "This is too long for the Solana app."),
            Self::BadPath => write!(f, "The Solana app does not derive keys at this path."),
            Self::WrongApp => write!(f, "Open the Solana app on your Ledger."),
            Self::AppNotInstalled => write!(f, "This Ledger has no Solana app. Install it with Ledger Live."),
            Self::OpenRefused => write!(f, "The Solana app was not opened on the Ledger."),
            Self::Locked => write!(f, "Unlock your Ledger."),
            Self::NoPin => write!(f, "This Ledger has no PIN set up yet."),
            Self::Status(status) => write!(f, "The Ledger answered {status:04X}."),
            Self::BadReply => write!(f, "The Ledger's answer could not be read."),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Split a reply into its data and status word, and turn any status but
/// `9000` into an error.
pub fn check_reply(reply: &[u8]) -> Result<&[u8], LedgerError> {
    let split = reply.len().checked_sub(2).ok_or(LedgerError::BadReply)?;
    let status = u16::from_be_bytes([reply[split], reply[split + 1]]);
    if status == 0x9000 {
        Ok(&reply[..split])
    } else {
        Err(LedgerError::from_status(status))
    }
}

/// An APDU in 64-byte HID reports: the channel, the tag, the packet index,
/// and in the first one the APDU's length.
pub fn frame(apdu: &[u8]) -> Vec<[u8; PACKET]> {
    let mut packets = Vec::new();
    let mut rest = apdu;
    let mut index: u16 = 0;
    loop {
        let mut packet = [0u8; PACKET];
        packet[..2].copy_from_slice(&CHANNEL.to_be_bytes());
        packet[2] = TAG_APDU;
        packet[3..5].copy_from_slice(&index.to_be_bytes());
        let mut at = 5;
        if index == 0 {
            packet[5..7].copy_from_slice(&(apdu.len() as u16).to_be_bytes());
            at = 7;
        }
        let take = rest.len().min(PACKET - at);
        packet[at..at + take].copy_from_slice(&rest[..take]);
        rest = &rest[take..];
        packets.push(packet);
        index += 1;
        if rest.is_empty() {
            return packets;
        }
    }
}

/// Puts a reply back together from its HID reports.
#[derive(Default)]
pub struct Unframer {
    expected: Option<usize>,
    next_index: u16,
    data: Vec<u8>,
}

impl Unframer {
    /// Take one report. `Some(reply)` once the reply is whole.
    pub fn push(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, LedgerError> {
        if packet.len() < 7
            || u16::from_be_bytes([packet[0], packet[1]]) != CHANNEL
            || packet[2] != TAG_APDU
            || u16::from_be_bytes([packet[3], packet[4]]) != self.next_index
        {
            return Err(LedgerError::BadReply);
        }
        let body = if self.next_index == 0 {
            self.expected = Some(usize::from(u16::from_be_bytes([packet[5], packet[6]])));
            &packet[7..]
        } else {
            &packet[5..]
        };
        self.next_index += 1;
        let expected = self.expected.expect("set by the first report");
        let take = (expected - self.data.len()).min(body.len());
        self.data.extend_from_slice(&body[..take]);
        if self.data.len() == expected {
            Ok(Some(std::mem::take(&mut self.data)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::parse_path;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn get_pubkey_names_the_path() {
        let path = parse_path("m/44'/501'/0'").unwrap();
        assert_eq!(
            hex(&get_pubkey(&path, true).to_bytes()),
            "e00501000d038000002c800001f580000000"
        );
    }

    #[test]
    fn a_long_message_goes_in_chunks_with_its_flags() {
        let path = parse_path("m/44'/501'/0'/0'").unwrap();
        let message = vec![7u8; 600];
        let chunks = sign_message(&path, &message);
        // 1 + 17 bytes of path + 600: three chunks.
        let flags: Vec<u8> = chunks.iter().map(|apdu| apdu.p2).collect();
        assert_eq!(flags, [0x02, 0x03, 0x01]);
        assert!(chunks
            .iter()
            .all(|apdu| apdu.p1 == 1 && apdu.data.len() <= MAX_CHUNK));
        assert_eq!(&chunks[0].data[..2], &[1, 4]);
        let total: usize = chunks.iter().map(|apdu| apdu.data.len()).sum();
        assert_eq!(total, 1 + 17 + 600);
        assert_eq!(sign_message(&path, &[1, 2])[0].p2, 0);
    }

    #[test]
    fn framing_round_trips() {
        let apdu: Vec<u8> = (0..=200).collect();
        let packets = frame(&apdu);
        assert_eq!(packets.len(), 4);
        // Channel 0101, tag 05, index 0, length 201.
        assert_eq!(hex(&packets[0][..7]), "010105000000c9");
        let mut unframer = Unframer::default();
        let mut whole = None;
        for packet in &packets {
            whole = unframer.push(packet).unwrap();
        }
        assert_eq!(whole, Some(apdu));
        // Out of order is refused.
        assert_eq!(
            Unframer::default().push(&packets[1]),
            Err(LedgerError::BadReply)
        );
    }

    #[test]
    fn replies_are_read() {
        assert_eq!(check_reply(&[1, 2, 0x90, 0x00]), Ok(&[1u8, 2][..]));
        assert_eq!(check_reply(&[0x69, 0x85]), Err(LedgerError::Rejected));
        assert_eq!(check_reply(&[0x6a, 0x80]), Err(LedgerError::InvalidMessage));
        assert_eq!(check_reply(&[0x90]), Err(LedgerError::BadReply));
        let config = parse_configuration(&[1, 0, 1, 18, 0, 0, 0]).unwrap();
        assert!(config.blind_signing);
        assert_eq!(config.version, (1, 18, 0));
        let app = parse_app_and_version(b"\x01\x06Solana\x061.18.0\x01\x00").unwrap();
        assert_eq!(
            (app.name.as_str(), app.version.as_str()),
            ("Solana", "1.18.0")
        );
        assert_eq!(model(0x5011), Some("Nano S Plus"));
    }
}
