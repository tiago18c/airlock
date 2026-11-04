//! Trezor: Codec v1 framing over the USB vendor interface, and the session
//! and Solana messages, by their ids and field numbers in Trezor's
//! `messages*.proto`. The Safe 7 speaks THP instead of Codec v1; the
//! messages inside are the same.

use crate::proto::{ProtoError, Read, Writer};

/// Trezor's USB vendor and product ids outside the bootloader, and in it.
pub const VENDOR_ID: u16 = 0x1209;
pub const PRODUCT_ID: u16 = 0x53c1;
pub const BOOTLOADER_PRODUCT_ID: u16 = 0x53c0;
pub const PACKET: usize = 64;

/// Message type ids, from `messages.proto`.
pub mod id {
    pub const INITIALIZE: u16 = 0;
    pub const SUCCESS: u16 = 2;
    pub const FAILURE: u16 = 3;
    pub const FEATURES: u16 = 17;
    pub const CANCEL: u16 = 20;
    pub const BUTTON_REQUEST: u16 = 26;
    pub const BUTTON_ACK: u16 = 27;
    pub const PASSPHRASE_REQUEST: u16 = 41;
    pub const PASSPHRASE_ACK: u16 = 42;
    pub const GET_FEATURES: u16 = 55;
    pub const END_SESSION: u16 = 83;
    pub const SOLANA_GET_PUBLIC_KEY: u16 = 900;
    pub const SOLANA_PUBLIC_KEY: u16 = 901;
    pub const SOLANA_GET_ADDRESS: u16 = 902;
    pub const SOLANA_ADDRESS: u16 = 903;
    pub const SOLANA_SIGN_TX: u16 = 904;
    pub const SOLANA_TX_SIGNATURE: u16 = 905;
    pub const SOLANA_SIGN_MESSAGE: u16 = 906;
    pub const SOLANA_MESSAGE_SIGNATURE: u16 = 907;
}

/// A message: its type id and its protobuf bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub kind: u16,
    pub body: Vec<u8>,
}

impl Message {
    pub fn new(kind: u16, body: Vec<u8>) -> Self {
        Self { kind, body }
    }
}

/// A message in 64-byte packets: `?##`, the type, the length and the
/// first 55 bytes, then `?` and 63 more bytes per packet, zero-padded.
pub fn frame(message: &Message) -> Vec<[u8; PACKET]> {
    let mut data = Vec::with_capacity(8 + message.body.len());
    data.extend_from_slice(b"##");
    data.extend_from_slice(&message.kind.to_be_bytes());
    data.extend_from_slice(&(message.body.len() as u32).to_be_bytes());
    data.extend_from_slice(&message.body);
    data.chunks(PACKET - 1)
        .map(|chunk| {
            let mut packet = [0u8; PACKET];
            packet[0] = b'?';
            packet[1..=chunk.len()].copy_from_slice(chunk);
            packet
        })
        .collect()
}

/// A message bigger than this is refused when reading.
pub const MAX_MESSAGE: usize = 1 << 20;

/// Puts a message back together from its packets.
#[derive(Default)]
pub struct Unframer {
    head: Option<(u16, usize)>,
    body: Vec<u8>,
}

impl Unframer {
    pub fn push(&mut self, packet: &[u8]) -> Result<Option<Message>, TrezorError> {
        if packet.first() != Some(&b'?') {
            return Err(TrezorError::BadReply);
        }
        let mut rest = &packet[1..];
        if self.head.is_none() {
            if rest.len() < 8 || &rest[..2] != b"##" {
                return Err(TrezorError::BadReply);
            }
            let kind = u16::from_be_bytes([rest[2], rest[3]]);
            let len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
            if len > MAX_MESSAGE {
                return Err(TrezorError::BadReply);
            }
            self.head = Some((kind, len));
            rest = &rest[8..];
        }
        let (kind, len) = self.head.expect("set above");
        let take = (len - self.body.len()).min(rest.len());
        self.body.extend_from_slice(&rest[..take]);
        if self.body.len() == len {
            self.head = None;
            Ok(Some(Message::new(kind, std::mem::take(&mut self.body))))
        } else {
            Ok(None)
        }
    }
}

/// Start a session. An empty session id asks for a new one.
pub fn initialize() -> Message {
    Message::new(id::INITIALIZE, Vec::new())
}

pub fn get_features() -> Message {
    Message::new(id::GET_FEATURES, Vec::new())
}

pub fn button_ack() -> Message {
    Message::new(id::BUTTON_ACK, Vec::new())
}

/// The passphrase is always typed on the Trezor, never in Airlock.
pub fn passphrase_on_device() -> Message {
    Message::new(id::PASSPHRASE_ACK, Writer::default().bool(3, true).finish())
}

pub fn cancel() -> Message {
    Message::new(id::CANCEL, Vec::new())
}

pub fn end_session() -> Message {
    Message::new(id::END_SESSION, Vec::new())
}

pub fn solana_get_public_key(path: &[u32], show: bool) -> Message {
    Message::new(
        id::SOLANA_GET_PUBLIC_KEY,
        Writer::default().uints(1, path).bool(2, show).finish(),
    )
}

pub fn solana_get_address(path: &[u32], show: bool) -> Message {
    Message::new(
        id::SOLANA_GET_ADDRESS,
        Writer::default()
            .uints(1, path)
            .bool(2, show)
            .bool(3, true)
            .finish(),
    )
}

/// A token account the transaction names, so the device can say whose it
/// is. All four are base58 addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenAccountInfo {
    pub base_address: String,
    pub token_program: String,
    pub token_mint: String,
    pub token_account: String,
}

/// Sign `message`, the bytes a transaction's signatures cover.
pub fn solana_sign_tx(
    path: &[u32],
    message: &[u8],
    token_accounts: &[TokenAccountInfo],
) -> Message {
    let mut body = Writer::default();
    body.uints(1, path).bytes(2, message);
    if !token_accounts.is_empty() {
        let mut info = Writer::default();
        for account in token_accounts {
            let entry = Writer::default()
                .bytes(1, account.base_address.as_bytes())
                .bytes(2, account.token_program.as_bytes())
                .bytes(3, account.token_mint.as_bytes())
                .bytes(4, account.token_account.as_bytes())
                .finish();
            info.bytes(1, &entry);
        }
        body.bytes(3, &info.finish());
    }
    body.bool(5, true);
    Message::new(id::SOLANA_SIGN_TX, body.finish())
}

/// Sign `text` as an off-chain message v1 that names `signers`. The device
/// builds the OCMS bytes itself, and says which in its reply.
pub fn solana_sign_message(path: &[u32], text: &str, signers: &[[u8; 32]]) -> Message {
    let mut inner = Writer::default();
    inner.bytes(1, text.as_bytes());
    for signer in signers {
        inner.bytes(2, signer);
    }
    Message::new(
        id::SOLANA_SIGN_MESSAGE,
        Writer::default()
            .uints(1, path)
            .bool(3, true)
            .bytes(4, &inner.finish())
            .finish(),
    )
}

/// What the device said back, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Features(Features),
    /// The device waits for the user; answer with [`button_ack`].
    ButtonRequest {
        name: Option<String>,
    },
    /// Answer with [`passphrase_on_device`].
    PassphraseRequest,
    PublicKey([u8; 32]),
    Address(String),
    Signature([u8; 64]),
    MessageSignature {
        signature: [u8; 64],
        /// The OCMS bytes it signed.
        signed: Option<Vec<u8>>,
    },
    Success,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    pub version: (u32, u32, u32),
    pub device_id: Option<String>,
    pub model: Option<String>,
    pub internal_model: Option<String>,
    pub unlocked: Option<bool>,
    pub passphrase_protection: bool,
    pub bootloader: bool,
}

/// The failure codes Airlock tells apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrezorError {
    /// The user rejected it on the device.
    Cancelled,
    /// The device cannot read what it was sent: a v1 transaction on
    /// firmware that reads none.
    DataError(String),
    /// A PIN would have to be typed on the host: a Model One.
    PinExpected,
    Busy,
    Failure {
        code: u64,
        message: String,
    },
    /// A message Airlock did not expect at this point.
    Unexpected(u16),
    BadReply,
}

impl std::fmt::Display for TrezorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "You rejected it on the Trezor."),
            Self::DataError(message) => write!(
                f,
                "The Trezor cannot read this: {message}. Update its firmware."
            ),
            Self::PinExpected => write!(
                f,
                "This Trezor asks for its PIN on the computer, which Airlock does not do."
            ),
            Self::Busy => write!(
                f,
                "The Trezor is busy. Finish what it shows, then try again."
            ),
            Self::Failure { message, .. } => write!(f, "The Trezor refused: {message}"),
            Self::Unexpected(kind) => {
                write!(f, "The Trezor sent message {kind}, which was not expected.")
            }
            Self::BadReply => write!(f, "The Trezor's answer could not be read."),
        }
    }
}

impl std::error::Error for TrezorError {}

impl From<ProtoError> for TrezorError {
    fn from(_: ProtoError) -> Self {
        Self::BadReply
    }
}

fn fixed<const N: usize>(bytes: Option<&[u8]>) -> Result<[u8; N], TrezorError> {
    bytes
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(TrezorError::BadReply)
}

/// Read a reply. A `Failure` becomes an error.
pub fn parse_reply(message: &Message) -> Result<Reply, TrezorError> {
    let read = Read::new(&message.body)?;
    Ok(match message.kind {
        id::FEATURES => Reply::Features(Features {
            version: (
                read.uint(2).unwrap_or(0) as u32,
                read.uint(3).unwrap_or(0) as u32,
                read.uint(4).unwrap_or(0) as u32,
            ),
            device_id: read.text(6),
            model: read.text(21),
            internal_model: read.text(44),
            unlocked: read.bool(16),
            passphrase_protection: read.bool(8).unwrap_or(false),
            bootloader: read.bool(5).unwrap_or(false),
        }),
        id::BUTTON_REQUEST => Reply::ButtonRequest { name: read.text(4) },
        id::PASSPHRASE_REQUEST => Reply::PassphraseRequest,
        id::SOLANA_PUBLIC_KEY => Reply::PublicKey(fixed(read.bytes(1))?),
        id::SOLANA_ADDRESS => Reply::Address(read.text(1).ok_or(TrezorError::BadReply)?),
        id::SOLANA_TX_SIGNATURE => Reply::Signature(fixed(read.bytes(1))?),
        id::SOLANA_MESSAGE_SIGNATURE => Reply::MessageSignature {
            signature: fixed(read.bytes(1))?,
            signed: read.bytes(2).map(<[u8]>::to_vec),
        },
        id::SUCCESS => Reply::Success,
        id::FAILURE => {
            let code = read.uint(1).unwrap_or(0);
            let text = read.text(2).unwrap_or_default();
            return Err(match code {
                3 => TrezorError::DataError(text),
                4 | 6 => TrezorError::Cancelled,
                5 => TrezorError::PinExpected,
                15 => TrezorError::Busy,
                _ => TrezorError::Failure {
                    code,
                    message: text,
                },
            });
        }
        other => return Err(TrezorError::Unexpected(other)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::parse_path;

    #[test]
    fn framing_round_trips_through_several_packets() {
        let message = solana_sign_tx(&parse_path("m/44'/501'/0'/0'").unwrap(), &[9; 300], &[]);
        let packets = frame(&message);
        assert_eq!(&packets[0][..3], b"?##");
        assert_eq!(&packets[0][3..5], &904u16.to_be_bytes());
        assert!(packets[1..].iter().all(|packet| packet[0] == b'?'));
        let mut unframer = Unframer::default();
        let mut whole = None;
        for packet in &packets {
            whole = unframer.push(packet).unwrap();
        }
        assert_eq!(whole, Some(message));
    }

    #[test]
    fn requests_carry_their_fields() {
        let path = parse_path("m/44'/501'/0'").unwrap();
        let get = solana_get_public_key(&path, false);
        let read = Read::new(&get.body).unwrap();
        let indices: Vec<u64> = read
            .0
            .iter()
            .filter_map(|(field, value)| match value {
                crate::proto::Field::Varint(n) if *field == 1 => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(indices, [0x8000_002c, 0x8000_01f5, 0x8000_0000]);
        assert_eq!(read.bool(2), Some(false));
        assert_eq!(
            Read::new(&passphrase_on_device().body).unwrap().bool(3),
            Some(true)
        );

        let accounts = [TokenAccountInfo {
            base_address: "a".into(),
            token_program: "b".into(),
            token_mint: "c".into(),
            token_account: "d".into(),
        }];
        let sign = solana_sign_tx(&path, &[1, 2, 3], &accounts);
        let read = Read::new(&sign.body).unwrap();
        assert_eq!(read.bytes(2), Some(&[1u8, 2, 3][..]));
        let info = Read::new(read.bytes(3).unwrap()).unwrap();
        let entry = Read::new(info.bytes(1).unwrap()).unwrap();
        assert_eq!(entry.text(3).as_deref(), Some("c"));
    }

    #[test]
    fn replies_are_read() {
        let features = Message::new(
            id::FEATURES,
            Writer::default()
                .bytes(1, b"trezor.io")
                .uint(2, 2)
                .uint(3, 13)
                .uint(4, 1)
                .bytes(6, b"ABC")
                .bool(8, true)
                .bool(16, true)
                .bytes(44, b"T3T1")
                .finish(),
        );
        let Ok(Reply::Features(features)) = parse_reply(&features) else {
            panic!("features");
        };
        assert_eq!(features.version, (2, 13, 1));
        assert_eq!(features.internal_model.as_deref(), Some("T3T1"));
        assert!(features.passphrase_protection);

        let signature = Message::new(
            id::SOLANA_TX_SIGNATURE,
            Writer::default().bytes(1, &[7; 64]).finish(),
        );
        assert_eq!(parse_reply(&signature), Ok(Reply::Signature([7; 64])));

        let cancelled = Message::new(id::FAILURE, Writer::default().uint(1, 4).finish());
        assert_eq!(parse_reply(&cancelled), Err(TrezorError::Cancelled));
        let short = Message::new(
            id::SOLANA_TX_SIGNATURE,
            Writer::default().bytes(1, &[7; 3]).finish(),
        );
        assert_eq!(parse_reply(&short), Err(TrezorError::BadReply));
    }
}
