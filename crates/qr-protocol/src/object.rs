//! The typed objects a transfer carries: a transaction, a signature, a
//! context note, a public key, the keys of wallets, a key rotation or the
//! state of an account.

use super::*;
use wincode::config::{Config, Configuration};
use wincode::len::UseIntLen;
use wincode::{SchemaRead, SchemaWrite};

/// The wincode settings of a payload: one byte per enum tag, two bytes per
/// length, little-endian integers. Nothing in a payload is longer than
/// [`MAX_PAYLOAD`], which two bytes can state.
fn config() -> impl Config {
    Configuration::default()
        .with_length_encoding::<UseIntLen<u16>>()
        .with_tag_encoding::<u8>()
        .with_preallocation_size_limit::<MAX_PAYLOAD>()
}

/// The most objects one payload carries: enough for a transaction, its note
/// and a signature for each of its signers and authorizations.
pub const MAX_OBJECTS: usize = 64;
/// The most Vector rotation nonces one key rotation is signed at.
pub const MAX_ROTATION_NONCES: usize = 8;
/// The most wallet keys in one object.
pub const MAX_WALLET_KEYS: usize = 32;
/// The longest wallet key label, in bytes.
pub const MAX_KEY_LABEL: usize = 64;
/// The most IDL pins in one context note.
pub const MAX_PINS: usize = 64;
/// The most Vector authorizations in one context note.
pub const MAX_AUTHORIZATIONS: usize = 16;
/// The most accounts in one context note.
pub const MAX_ACCOUNTS: usize = 32;

/// Kind 2. A signature, and what it signs.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub enum Signed {
    /// `pubkey` signed the transaction.
    #[wincode(tag = 0)]
    Transaction {
        pubkey: [u8; PUBKEY_BYTES],
        signature: [u8; SIGNATURE_BYTES],
    },
    /// `authority` signed an authorization message the transaction carries.
    #[wincode(tag = 1)]
    Authorization {
        authority: [u8; PUBKEY_BYTES],
        signature: [u8; SIGNATURE_BYTES],
    },
    /// The key that controls `account` signed for it, against `nonce`. The
    /// signature is as long as the account's scheme makes it.
    #[wincode(tag = 2)]
    AccountAuthorization {
        account: [u8; PUBKEY_BYTES],
        nonce: [u8; 32],
        signature: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContextPin {
    pub program_id: [u8; PUBKEY_BYTES],
    pub idl_sha256: [u8; 32],
}

/// What a device needs, beyond the transaction, to sign for an account that
/// authorizes with a key of its own.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContextAuthorization {
    pub account: [u8; PUBKEY_BYTES],
    /// The account's signing scheme, which sets how long its signature is.
    pub scheme: u8,
    /// What the account's next signature has to be made over.
    pub nonce: [u8; 32],
    /// The lowest leaf the chain still accepts, for a key that signs once
    /// per leaf. Zero otherwise.
    pub leaf_floor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContextAccount {
    pub pubkey: [u8; PUBKEY_BYTES],
    pub data: Vec<u8>,
}

/// Kind 3. Matched to `sha256` of the versioned message already on screen.
///
/// A note carries a checksum per program id, what a device needs to sign
/// each account's authorization, and account bytes. It never carries IDL
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct TxContext {
    pub message_sha256: [u8; 32],
    pub pins: Vec<ContextPin>,
    pub authorizations: Vec<ContextAuthorization>,
    pub accounts: Vec<ContextAccount>,
}

impl TxContext {
    /// A note with one pin per program id and no accounts. Pins and
    /// authorizations are never dropped: a device cannot sign without its
    /// authorization, and a note that left one out would not say so. More
    /// than a note holds is refused.
    pub fn new(
        message_sha256: [u8; 32],
        pins: &[ContextPin],
        authorizations: &[ContextAuthorization],
    ) -> Result<Self, ProtocolError> {
        let mut unique: Vec<ContextPin> = Vec::new();
        for pin in pins {
            if !unique.iter().any(|kept| kept.program_id == pin.program_id) {
                unique.push(pin.clone());
            }
        }
        let context = Self {
            message_sha256,
            pins: unique,
            authorizations: authorizations.to_vec(),
            accounts: Vec::new(),
        };
        context.check()?;
        Ok(context)
    }

    fn check(&self) -> Result<(), ProtocolError> {
        at_most(self.pins.len(), MAX_PINS)?;
        at_most(self.authorizations.len(), MAX_AUTHORIZATIONS)?;
        at_most(self.accounts.len(), MAX_ACCOUNTS)?;
        self.accounts
            .iter()
            .try_for_each(|account| bytes(&account.data, true))
    }
}

/// A typed object of a payload.
///
/// On the wire, a tag byte names the kind and the fields follow in order.
/// Arrays are written as they are, and every other run of bytes or entries
/// behind a length (u16 LE).
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub enum QrObject {
    /// Kind 1. A Solana v1 wire (`0x81` …), with every signature it has so far.
    #[wincode(tag = 1)]
    Transaction(Vec<u8>),
    #[wincode(tag = 2)]
    Signature(Signed),
    #[wincode(tag = 3)]
    Context(TxContext),
    /// Kind 4. A key another device holds, under the scheme it signs with.
    #[wincode(tag = 4)]
    PublicKey { scheme: u8, key: Vec<u8> },
    /// Kind 5. Wallets whose keys another device holds, for this one to
    /// add without them.
    #[wincode(tag = 5)]
    WalletKeys(Vec<WalletKey>),
    /// Kind 6. An account moved to its next key, by the device that holds
    /// the key, for another device to send.
    #[wincode(tag = 6)]
    KeyRotation(KeyRotation),
    /// Kind 7. What the chain says of an account, read by another device.
    #[wincode(tag = 7)]
    AccountState(AccountState),
}

/// Kind 7. Where an account is: what its next signature is made over, the
/// lowest leaf it accepts, and the key it holds.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct AccountState {
    pub account: [u8; PUBKEY_BYTES],
    pub scheme: u8,
    pub nonce: [u8; 32],
    pub leaf_floor: u32,
    pub key: Vec<u8>,
}

/// Kind 6. The key of `account` signs its move to `next_key`. The device
/// that signs it cannot read the chain, so it signs once for each nonce the
/// account may be at, and the device that sends it picks the one the
/// account holds.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct KeyRotation {
    pub account: [u8; PUBKEY_BYTES],
    pub scheme: u8,
    pub next_key: Vec<u8>,
    pub signed: Vec<RotationSignature>,
}

/// One signature of a [`KeyRotation`], made at `nonce`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct RotationSignature {
    pub nonce: [u8; 32],
    pub signature: Vec<u8>,
}

/// What a shared wallet key stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub enum KeyRole {
    /// The address of a key: the key is the address.
    #[wincode(tag = 0)]
    Address,
    /// A Vector account, made from this key under `scheme`.
    #[wincode(tag = 1)]
    Vector { scheme: u8 },
}

/// One wallet of a wallet keys object.
#[derive(Debug, Clone, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct WalletKey {
    pub role: KeyRole,
    /// What the other device calls it, at most [`MAX_KEY_LABEL`] bytes.
    pub label: String,
    pub key: Vec<u8>,
}

impl QrObject {
    /// Every limit the layout itself does not enforce.
    fn check(&self) -> Result<(), ProtocolError> {
        match self {
            QrObject::Transaction(wire) => {
                if wire.first().copied() != Some(V1_PREFIX) || wire.len() > MAX_TRANSACTION_SIZE {
                    return Err(ProtocolError::BadObject);
                }
                Ok(())
            }
            QrObject::Signature(Signed::AccountAuthorization { signature, .. }) => {
                bytes(signature, false)
            }
            QrObject::Signature(_) => Ok(()),
            QrObject::Context(context) => context.check(),
            QrObject::PublicKey { key, .. } => bytes(key, false),
            QrObject::WalletKeys(keys) => {
                if keys.is_empty() {
                    return Err(ProtocolError::BadObject);
                }
                at_most(keys.len(), MAX_WALLET_KEYS)?;
                keys.iter().try_for_each(|key| {
                    at_most(key.label.len(), MAX_KEY_LABEL)?;
                    bytes(&key.key, false)?;
                    if key.role == KeyRole::Address && key.key.len() != PUBKEY_BYTES {
                        return Err(ProtocolError::BadObject);
                    }
                    Ok(())
                })
            }
            QrObject::KeyRotation(rotation) => {
                bytes(&rotation.next_key, false)?;
                if rotation.signed.is_empty() {
                    return Err(ProtocolError::BadObject);
                }
                at_most(rotation.signed.len(), MAX_ROTATION_NONCES)?;
                rotation
                    .signed
                    .iter()
                    .try_for_each(|signed| bytes(&signed.signature, false))
            }
            QrObject::AccountState(state) => bytes(&state.key, false),
        }
    }
}

fn at_most(len: usize, max: usize) -> Result<(), ProtocolError> {
    if len > max {
        return Err(ProtocolError::BadObject);
    }
    Ok(())
}

/// A run of bytes no longer than a payload, and not empty unless it may be.
fn bytes(bytes: &[u8], may_be_empty: bool) -> Result<(), ProtocolError> {
    if bytes.is_empty() && !may_be_empty {
        return Err(ProtocolError::BadObject);
    }
    if bytes.len() > MAX_PAYLOAD {
        return Err(ProtocolError::TooLarge { len: bytes.len() });
    }
    Ok(())
}

/// The payload of one transfer: `objects` in order, behind their count.
/// Refused when it is empty, holds more than [`MAX_OBJECTS`], breaks a limit
/// of an object, or is longer than [`MAX_PAYLOAD`].
pub fn encode_payload(objects: &[QrObject]) -> Result<Vec<u8>, ProtocolError> {
    check_payload(objects)?;
    let payload = wincode::config::serialize(&objects.to_vec(), config())
        .map_err(|_| ProtocolError::BadObject)?;
    if payload.len() > MAX_PAYLOAD {
        return Err(ProtocolError::TooLarge { len: payload.len() });
    }
    Ok(payload)
}

/// The payload of a transfer that carries `object` alone.
pub fn encode_object(object: &QrObject) -> Result<Vec<u8>, ProtocolError> {
    encode_payload(std::slice::from_ref(object))
}

/// Every object of a payload, in order. A payload has an exact layout:
/// anything after its last object refuses it.
pub fn decode_payload(payload: &[u8]) -> Result<Vec<QrObject>, ProtocolError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(ProtocolError::TooLarge { len: payload.len() });
    }
    let objects: Vec<QrObject> = wincode::config::deserialize_exact(payload, config())
        .map_err(|_| ProtocolError::BadObject)?;
    check_payload(&objects)?;
    Ok(objects)
}

/// What every payload is, in whatever form: one to [`MAX_OBJECTS`] objects,
/// each within its limits.
pub(crate) fn check_payload(objects: &[QrObject]) -> Result<(), ProtocolError> {
    if objects.is_empty() {
        return Err(ProtocolError::Empty);
    }
    at_most(objects.len(), MAX_OBJECTS)?;
    objects.iter().try_for_each(QrObject::check)
}

/// The one object of a payload. A payload of several is refused.
pub fn decode_object(payload: &[u8]) -> Result<QrObject, ProtocolError> {
    let mut objects = decode_payload(payload)?;
    if objects.len() != 1 {
        return Err(ProtocolError::NotOneObject(objects.len()));
    }
    Ok(objects.remove(0))
}

/// `keys` as wallet keys objects, as few as fit a payload each, in the
/// order given. Labels are cut to [`MAX_KEY_LABEL`] bytes on a character
/// boundary. Empty when `keys` is.
pub fn encode_wallet_keys(keys: &[WalletKey]) -> Result<Vec<QrObject>, ProtocolError> {
    let keys: Vec<WalletKey> = keys
        .iter()
        .map(|key| WalletKey {
            label: short_label(&key.label).to_string(),
            ..key.clone()
        })
        .collect();
    let fits = |part: &[WalletKey]| {
        part.len() <= MAX_WALLET_KEYS && encode_object(&QrObject::WalletKeys(part.to_vec())).is_ok()
    };
    let mut objects = Vec::new();
    let mut part: Vec<WalletKey> = Vec::new();
    for key in keys {
        part.push(key);
        if fits(&part) {
            continue;
        }
        let key = part.pop().expect("just pushed");
        // One key that does not fit on its own is refused.
        if part.is_empty() {
            return Err(ProtocolError::BadObject);
        }
        objects.push(QrObject::WalletKeys(std::mem::take(&mut part)));
        part.push(key);
        if !fits(&part) {
            return Err(ProtocolError::BadObject);
        }
    }
    if !part.is_empty() {
        objects.push(QrObject::WalletKeys(part));
    }
    Ok(objects)
}

/// `label` cut to at most [`MAX_KEY_LABEL`] bytes, on a character boundary.
fn short_label(label: &str) -> &str {
    let mut end = label.len().min(MAX_KEY_LABEL);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    &label[..end]
}
